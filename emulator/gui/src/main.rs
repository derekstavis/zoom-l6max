mod bundle;
mod bundle_smoke;
mod components;
mod configuration;
#[cfg(feature = "headless-screenshots")]
mod headless_screenshot;
mod knob_input;
mod menu_smoke;
mod scenario_smoke;

use components::layout::Layout;
use gpui::{
    App, Bounds, Context, CursorStyle, ImageSource, KeyBinding, Menu, MenuItem, MouseButton,
    PathPromptOptions, Render, RenderImage, SharedString, Window, WindowBounds, WindowOptions,
    actions, div, img, prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use image::{Frame, ImageBuffer, Rgba};
use knob_input::KnobInput;
use l6max_host::{
    engine::{Engine, Options},
    input::InputHub,
    panel::PanelFeed,
};
use smallvec::SmallVec;
use std::{
    sync::{Arc, Mutex, atomic::AtomicU32},
    thread,
    time::{Duration, Instant},
};

const LCD_BUFFER_WIDTH: u32 = 128;
const LCD_BUFFER_HEIGHT: u32 = 64;
const DEFAULT_BUTTON_PRESS_TICKS: u32 = 50;

fn startup_key_targets(
    model: l6fw::Model,
    layout: &Layout,
    pressed: &[Arc<std::sync::atomic::AtomicBool>],
) -> Vec<u32> {
    if model != l6fw::Model::L6 {
        return Vec::new();
    }
    layout
        .buttons
        .iter()
        .filter_map(|(control, _)| {
            pressed[control.id as usize]
                .load(std::sync::atomic::Ordering::Relaxed)
                .then_some(control.id)
        })
        .collect()
}

fn queue_startup_keys(engine: &Engine, controls: &InputHub, targets: &[u32]) -> Result<(), String> {
    let main_before = controls.main.snapshot().applied;
    let panel_before = controls.panel.snapshot().applied;
    let main_expected = targets
        .iter()
        .filter(|target| matches!(target, 0 | 4 | 5 | 48..=53))
        .count() as u64;
    let panel_expected = targets.len() as u64 - main_expected;
    for target in targets {
        // TAP acknowledgements wait for the guest virtual clock, which is
        // paused until startup keys have been queued. DOWN is acknowledged as
        // soon as QEMU records the physical level and stays held into boot.
        controls
            .down(*target, 10_000)
            .map_err(|error| error.to_string())?;
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let main = controls.main.snapshot();
        let panel = controls.panel.snapshot();
        if !main.connected || !panel.connected {
            return Err(format!(
                "startup key input disconnected: MAIN={main:?}, panel={panel:?}"
            ));
        }
        if main.applied >= main_before + main_expected
            && panel.applied >= panel_before + panel_expected
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "startup key input was not applied: MAIN={main:?}, panel={panel:?}"
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
    for client in &engine.management {
        client.resume().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn model_name(model: l6fw::Model) -> &'static str {
    match model {
        l6fw::Model::L6 => "L6",
        l6fw::Model::L6max => "L6max",
    }
}

actions!(
    l6max,
    [
        OpenFirmware,
        OpenSdFolder,
        Quit,
        ConnectUsb,
        DisconnectUsb,
        ConnectMidi,
        DisconnectMidi,
        EjectSdCard
    ]
);

struct NativePanel {
    layout: Layout,
    model: l6fw::Model,
    bundle: Option<bundle::Bundle>,
    firmware_error: Option<String>,
    choosing_firmware: bool,
    importing_sd: bool,
    display: Option<PanelFeed>,
    frame: Arc<RenderImage>,
    last_frame: [u8; 1024],
    last_control_revision: Option<(
        u64,
        [u16; 5],
        bool,
        [bool; l6max_host::controls::INPUT_COUNT],
        [bool; 13],
    )>,
    status: SharedString,
    controls: Option<Arc<InputHub>>,
    button_pressed: [Arc<std::sync::atomic::AtomicBool>; l6max_host::controls::INPUT_COUNT],
    scenario_press_duration: Arc<AtomicU32>,
    knob_drags: [Arc<Mutex<KnobInput>>; 13],
    _qemu: Option<Engine>,
    options: Options,
    powered_off: bool,
    powered_off_signal: Arc<std::sync::atomic::AtomicBool>,
    power_on_requested: Arc<std::sync::atomic::AtomicBool>,
    physical_analog: [u16; 5],
    scenario_smoke: Option<smoke_core::Runner>,
    bundle_smoke: Option<bundle_smoke::BundleSmoke>,
    menu_smoke: Option<menu_smoke::MenuSmoke>,
}

impl NativePanel {
    fn panel_data(&self) -> components::panel::PanelData {
        let switches = std::array::from_fn(|id| {
            self.button_pressed[id].load(std::sync::atomic::Ordering::Relaxed)
        });
        self.display
            .as_ref()
            .map(|feed| feed.data(switches, !self.powered_off))
            .unwrap_or(components::panel::PanelData {
                switches,
                analog: self.physical_analog,
                powered: !self.powered_off,
                ..Default::default()
            })
    }

    fn panel_interactions(&self, scale: f32) -> components::panel::PanelInteractions {
        use std::sync::atomic::Ordering;

        let model = self.model;
        let pressed: Arc<Vec<_>> = Arc::new(self.button_pressed.iter().cloned().collect());
        let controls = self.controls.clone();
        let powered_off = self.powered_off_signal.clone();
        let power_on_requested = self.power_on_requested.clone();
        let scenario_press_duration = self.scenario_press_duration.clone();
        let down_pressed = pressed.clone();
        let down_controls = controls.clone();
        let down_powered_off = powered_off.clone();
        let down_power_on_requested = power_on_requested.clone();
        let button_down = Arc::new(move |id: u32, is_switch: bool| {
            let id = id as usize;
            if down_powered_off.load(Ordering::Relaxed) {
                if id == 53 {
                    down_power_on_requested.store(true, Ordering::Relaxed);
                } else if model == l6fw::Model::L6 {
                    down_pressed[id].fetch_xor(true, Ordering::Relaxed);
                }
                return;
            }
            if is_switch {
                let was_down = down_pressed[id].fetch_xor(true, Ordering::Relaxed);
                if let Some(controls) = &down_controls {
                    let result = if was_down {
                        controls.up(id as u32)
                    } else {
                        controls.down(id as u32, 0)
                    };
                    if let Err(error) = result {
                        tracing::error!(%error, "Switch failed");
                    }
                }
                return;
            }
            down_pressed[id].store(true, Ordering::Relaxed);
            if let Some(controls) = &down_controls {
                let requested = scenario_press_duration.swap(0, Ordering::Relaxed);
                let duration = if requested != 0 {
                    requested
                } else {
                    match (model, id) {
                        (l6fw::Model::L6, 53) => 500,
                        (l6fw::Model::L6max, 53) => 4_000,
                        _ => DEFAULT_BUTTON_PRESS_TICKS,
                    }
                };
                if let Err(error) = controls.down(id as u32, duration) {
                    tracing::error!(%error, "Button down failed");
                }
            }
        });

        let up_pressed = pressed;
        let up_controls = controls;
        let up_powered_off = powered_off;
        let button_up = Arc::new(move |id: u32| {
            let id = id as usize;
            if up_powered_off.load(Ordering::Relaxed)
                || !up_pressed[id].swap(false, Ordering::Relaxed)
            {
                return;
            }
            if let Some(controls) = &up_controls {
                if let Err(error) = controls.up(id as u32) {
                    tracing::error!(%error, "Button up failed");
                }
            }
        });

        let switch_down = button_down.clone();
        let switch_toggle = Arc::new(move |id: u32| switch_down(id, true));

        let drag_states = Arc::new(self.knob_drags.clone());
        let drag_controls = self.controls.clone();
        let drag_start_states = drag_states.clone();
        let encoder_drag_start = Arc::new(move |channel: usize, x: f32, y: f32| {
            drag_start_states[channel].lock().unwrap().start(x, y);
        });
        let drag_move_states = drag_states.clone();
        let move_controls = drag_controls.clone();
        let encoder_drag_move = Arc::new(move |channel: usize, dragging: bool, x: f32, y: f32| {
            let mut drag = drag_move_states[channel].lock().unwrap();
            if !dragging {
                drag.stop();
                return;
            }
            let steps = drag.drag(x, y, 8.0 * scale);
            if steps != 0 {
                if let Some(controls) = &move_controls {
                    if let Err(error) = controls.knob(channel, steps) {
                        tracing::error!(%error, "Encoder drag failed");
                    }
                }
            }
        });
        let scroll_states = drag_states.clone();
        let scroll_controls = drag_controls.clone();
        let encoder_scroll = Arc::new(move |channel: usize, delta: f32| {
            let steps = scroll_states[channel].lock().unwrap().scroll(delta);
            if steps != 0 {
                if let Some(controls) = &scroll_controls {
                    if let Err(error) = controls.knob(channel, steps) {
                        tracing::error!(%error, "Encoder scroll failed");
                    }
                }
            }
        });
        let end_states = drag_states;
        let encoder_drag_end =
            Arc::new(move |channel: usize| end_states[channel].lock().unwrap().stop());

        components::panel::PanelInteractions {
            button_down,
            button_up,
            switch_toggle,
            encoder_drag_start,
            encoder_drag_move,
            encoder_scroll,
            encoder_drag_end,
        }
    }

    fn choose_firmware(&mut self, cx: &mut Context<Self>) {
        if self.bundle.is_none() || self.choosing_firmware || self.importing_sd {
            return;
        }
        self.choosing_firmware = true;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select L6 or L6max firmware".into()),
        });
        cx.spawn(async move |this, cx| {
            let selection = prompt.await;
            let _ = this.update(cx, |panel, cx| {
                panel.choosing_firmware = false;
                match selection {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            if let Err(error) = panel.load_firmware(path) {
                                panel.firmware_error = Some(error);
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => panel.firmware_error = Some(error.to_string()),
                    Err(error) => panel.firmware_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_sd_folder(&mut self, cx: &mut Context<Self>) {
        if self.bundle.is_none() || self.choosing_firmware || self.importing_sd {
            return;
        }
        if self.controls.is_none() {
            self.firmware_error = Some("Select firmware before adding an SD card.".into());
            cx.notify();
            return;
        }
        self.choosing_firmware = true;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use as SD card".into()),
        });
        cx.spawn(async move |this, cx| {
            let selected = prompt.await;
            let path = match selected {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                result => {
                    let _ = this.update(cx, |panel, cx| {
                        panel.firmware_error =
                            Some(format!("Could not select SD folder: {result:?}"));
                        cx.notify();
                    });
                    None
                }
            };
            let image = this
                .update(cx, |panel, cx| {
                    panel.choosing_firmware = false;
                    if path.is_none() {
                        return None;
                    }
                    panel.importing_sd = true;
                    panel.status = "Importing SD card folder…".into();
                    cx.notify();
                    Some(panel.new_sd_image())
                })
                .ok()
                .flatten();
            if let (Some(folder), Some(image)) = (path, image) {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        l6max_host::demo_sd::from_directory(&image, &folder)
                            .map(|()| image)
                            .map_err(|error| error.to_string())
                    })
                    .await;
                let _ = this.update(cx, |panel, cx| {
                    panel.importing_sd = false;
                    let result = result.and_then(|image| panel.use_sd_image(image));
                    if let Err(error) = result {
                        panel.firmware_error = Some(format!("SD card folder: {error}"));
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Like plugging in the USB cable: neither needs the engine to restart,
    /// mirroring how the real hardware's USB and MIDI jacks can be connected
    /// or disconnected at any time.
    fn connect_usb(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self._qemu.as_mut() else {
            return;
        };
        self.status = match engine.connect_usb() {
            Ok(()) => "USB connected".into(),
            Err(error) => format!("USB connect failed: {error}").into(),
        };
        cx.notify();
    }
    fn disconnect_usb(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self._qemu.as_mut() else {
            return;
        };
        engine.disconnect_usb();
        self.status = "USB disconnected".into();
        cx.notify();
    }
    fn connect_midi(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self._qemu.as_ref() else {
            return;
        };
        engine.connect_midi();
        self.status = "MIDI IN/OUT jacks connected".into();
        cx.notify();
    }
    fn disconnect_midi(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self._qemu.as_ref() else {
            return;
        };
        engine.disconnect_midi();
        self.status = "MIDI IN/OUT jacks disconnected".into();
        cx.notify();
    }

    /// Like pulling the SD card out of the slot: no engine restart needed.
    fn eject_sd_card(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self._qemu.as_ref() else {
            return;
        };
        match engine.eject_sd() {
            Ok(()) => {
                self.options.sd_image = None;
                self.status = "SD card ejected".into();
            }
            Err(error) => self.status = format!("Eject SD card failed: {error}").into(),
        }
        cx.notify();
    }

    fn new_sd_image(&self) -> std::path::PathBuf {
        let root = self.bundle.as_ref().unwrap().data.join("cards");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        root.join(format!("card-{stamp}-{}.img", std::process::id()))
    }

    fn use_sd_image(&mut self, image: std::path::PathBuf) -> Result<(), String> {
        let engine = self._qemu.as_ref().ok_or("no running engine")?;
        engine.insert_sd(&image)?;
        self.options.sd_image = Some(image.clone());
        self.options.no_sd = false;
        let state = self
            .options
            .state_dir
            .as_ref()
            .ok_or("SD selection requires persistent state")?;
        let temporary = state.join("sd-card-selection.tmp");
        std::fs::write(&temporary, image.to_str().ok_or("Invalid SD card path")?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(temporary, state.join("sd-card-selection.txt"))
            .map_err(|e| e.to_string())?;
        self.firmware_error = None;
        Ok(())
    }

    fn load_firmware(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bundle = self
            .bundle
            .as_ref()
            .ok_or("Not running from an app bundle")?;
        let mut options = self.options.clone();
        bundle.import(path, &mut options)?;
        let package = bundle::selected_package(&options.firmware_dir)?;
        configuration::configure_with_package(&mut options, Some(&package))
            .map_err(|e| e.to_string())?;
        self.physical_analog = [512; 5];
        self.restart_with_options(options)?;
        self.bundle.as_ref().unwrap().remember(&self.options)?;
        self.firmware_error = None;
        Ok(())
    }

    fn restart_with_options(&mut self, options: Options) -> Result<(), String> {
        self.display.take();
        self.controls.take();
        drop(self._qemu.take());
        self.options = options;
        for button in &self.button_pressed {
            button.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        for drag in &self.knob_drags {
            drag.lock().unwrap().stop();
        }
        self.frame = Arc::new(RenderImage::new(SmallVec::new()));
        self.last_frame = [0; 1024];
        self.power_on_requested
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.power_on()
    }

    fn power_on(&mut self) -> Result<(), String> {
        let startup_keys = startup_key_targets(self.model, &self.layout, &self.button_pressed);
        let mut options = self.options.clone();
        if !startup_keys.is_empty() {
            options.start_paused = true;
        }
        let engine = Engine::start(&options)?;
        self.model = engine.model();
        let controls = engine.inputs.clone().ok_or("QEMU unavailable")?;
        let display = PanelFeed::new(&engine)?;
        controls
            .restore_analog_positions(self.physical_analog)
            .map_err(|e| e.to_string())?;
        if !startup_keys.is_empty() {
            queue_startup_keys(&engine, &controls, &startup_keys)?;
            for target in &startup_keys {
                self.button_pressed[*target as usize]
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.controls = Some(controls);
        self.display = Some(display);
        self._qemu = Some(engine);
        self.powered_off = false;
        self.powered_off_signal
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.status = "Starting firmware".into();
        Ok(())
    }

    fn refresh_power(&mut self) {
        if self.powered_off
            && self
                .power_on_requested
                .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            if let Err(error) = self.power_on() {
                self.status = format!("Power-on failed: {error}").into();
            }
            return;
        }
        if !self
            .controls
            .as_ref()
            .is_some_and(|c| c.main.snapshot().powered_off)
        {
            return;
        }
        self.physical_analog = self.controls.as_ref().unwrap().analog_positions();
        self.display.take();
        self.controls.take();
        drop(self._qemu.take());
        for (control, _) in &self.layout.buttons {
            if !control.switch {
                self.button_pressed[control.id as usize]
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        for drag in &self.knob_drags {
            drag.lock().unwrap().stop();
        }
        self.frame = Arc::new(RenderImage::new(SmallVec::new()));
        self.last_frame = [0; 1024];
        self.powered_off = true;
        self.powered_off_signal
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.status = if self.model == l6fw::Model::L6 {
            "Powered off · select startup buttons, then press Power".into()
        } else {
            "Powered off · press Power to start".into()
        };
    }
    fn refresh_frame(&mut self) -> bool {
        let Some(display) = &self.display else {
            return false;
        };
        let snapshot = match display.sample() {
            Ok(Some(frame)) => frame,
            Ok(None) => return false,
            Err(error) => {
                self.status = format!("Display disconnected: {error}").into();
                return true;
            }
        };
        if snapshot == self.last_frame {
            return false;
        }
        let image = ImageBuffer::from_fn(LCD_BUFFER_WIDTH, LCD_BUFFER_HEIGHT, |x, y| {
            let index = ((63 - y as usize) / 8) * 128 + x as usize;
            let bit = (snapshot[index] >> ((63 - y as usize) % 8)) & 1;
            let [r, g, b] = if bit != 0 {
                [172, 207, 224]
            } else {
                [8, 12, 15]
            };
            // GPUI uploads RenderImage frames as BGRA, including this buffer.
            Rgba([b, g, r, 255])
        });
        let frame = Frame::new(image);
        self.last_frame.copy_from_slice(&snapshot);
        self.frame = Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1)));
        self.status = if self.model == l6fw::Model::L6 {
            "Live L6 panel · verified controls".into()
        } else {
            "Live display · completed SPI transfers".into()
        };
        true
    }
}

impl Render for NativePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let scale = (viewport.width.as_f32() / self.layout.width)
            .min(viewport.height.as_f32() / self.layout.height)
            .max(0.001);
        let panel_data = self.panel_data();
        let lcd = self.layout.lcd;
        let lcd_scale =
            (lcd.width / LCD_BUFFER_WIDTH as f32).min(lcd.height / LCD_BUFFER_HEIGHT as f32);
        let lcd_width = LCD_BUFFER_WIDTH as f32 * lcd_scale;
        let lcd_height = LCD_BUFFER_HEIGHT as f32 * lcd_scale;
        let interactions = self.panel_interactions(scale);
        let show_selection = self.bundle.is_some()
            && (self.controls.is_none() && !self.powered_off || self.firmware_error.is_some());
        let drag_move = interactions.encoder_drag_move.clone();
        let channels = components::layout::interactive_knob_indices(self.model);
        div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .bg(rgb(0x000000))
            .on_mouse_move(move |event, _, _| {
                let position = event.position;
                for channel in &channels {
                    drag_move(
                        *channel,
                        event.dragging(),
                        position.x.as_f32(),
                        position.y.as_f32(),
                    );
                }
            })
            .child(
                div()
                    .relative()
                    .w(px(self.layout.width * scale))
                    .h(px(self.layout.height * scale))
                    .flex_shrink_0()
                    .child(components::panel::panel(
                        &self.layout,
                        scale,
                        panel_data,
                        self.model,
                        &interactions,
                    ))
                    .child(if self.model == l6fw::Model::L6max {
                        div()
                            .absolute()
                            .left(px(lcd.x * scale))
                            .top(px(lcd.y * scale))
                            .w(px(lcd.width * scale))
                            .h(px(lcd.height * scale))
                            .overflow_hidden()
                            .bg(rgb(0x050708))
                            .child(
                                img(ImageSource::Render(self.frame.clone()))
                                    .absolute()
                                    .left(px((lcd.width - lcd_width) * 0.5 * scale))
                                    .top(px((lcd.height - lcd_height) * 0.5 * scale))
                                    .w(px(lcd_width * scale))
                                    .h(px(lcd_height * scale))
                                    .object_fit(gpui::ObjectFit::Contain),
                            )
                    } else {
                        div()
                    })
            )
            .when(show_selection, |element| element.child(
                div().absolute().size_full().occlude().bg(rgba(0x080c18dd))
                    .flex().items_center().justify_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().id("firmware-dialog").w(px(420.0)).max_h(px((viewport.height.as_f32() - 24.0).max(120.0))).overflow_y_scroll().p_8().rounded_xl().bg(rgb(0x171f30))
                        .border_1().border_color(rgb(0x34405c)).text_color(rgb(0xe8eef8))
                        .flex().flex_col().gap_4()
                        .child(div().text_2xl().child(if self.controls.is_some() { "Unable to open selection" } else { "Select firmware" }))
                        .child(if self.controls.is_some() { "Use the File menu to choose another firmware or SD card folder." } else { "Choose your L6 or L6max update file to start the emulator. Firmware is not included." })
                        .when(self.firmware_error.is_some(), |element| element.child(
                            div().text_color(rgb(0xffb3a7)).child(self.firmware_error.clone().unwrap_or_default())
                        ))
                        .child(div().id("select-firmware").rounded_md().px_4().py_3()
                            .bg(rgb(0x5367df)).cursor(CursorStyle::PointingHand)
                            .child(if self.choosing_firmware { "Choosing…" } else { "Select firmware…" })
                            .on_mouse_down(MouseButton::Left, cx.listener(|panel, _, _, cx| panel.choose_firmware(cx))))
                        .when(self.controls.is_some(), |element| element.child(
                            div().id("dismiss-firmware-error").cursor(CursorStyle::PointingHand)
                                .child("Keep current firmware")
                                .on_mouse_down(MouseButton::Left, cx.listener(|panel, _, _, cx| {
                                    panel.firmware_error = None;
                                    cx.notify();
                                }))
                        ))
                    )
            ))
    }
}

#[cfg(target_os = "macos")]
fn configure_window(window: &mut Window, device_width: f32, device_height: f32) {
    use objc2_app_kit::NSView;
    use objc2_foundation::NSSize;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).expect("native window handle");
    if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
        // SAFETY: GPUI supplies a live NSView handle, and this callback runs on
        // the AppKit main thread. The borrow does not outlive the window.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        if let Some(window) = view.window() {
            window.setContentAspectRatio(NSSize::new(device_width as f64, device_height as f64));
            window.setContentSize(NSSize::new(device_width as f64, device_height as f64));
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_window(_window: &mut Window, _device_width: f32, _device_height: f32) {}

/// Linux builds use GPUI's Wayland backend only. Without a session the window
/// system connection never completes, so fail early with the reason.
#[cfg(target_os = "linux")]
fn require_wayland_session() -> Result<(), String> {
    let socket = std::env::var_os("WAYLAND_DISPLAY").map(std::path::PathBuf::from);
    let default = std::env::var_os("XDG_RUNTIME_DIR")
        .map(|runtime| std::path::PathBuf::from(runtime).join("wayland-0"));
    match (socket, default) {
        (Some(_), _) => Ok(()),
        (None, Some(path)) if path.exists() => Ok(()),
        _ => Err("no Wayland session found: WAYLAND_DISPLAY is not set and \
$XDG_RUNTIME_DIR/wayland-0 does not exist. Run this from a Wayland desktop session; \
X11-only sessions are not supported by this build."
            .into()),
    }
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let format = if args.iter().any(|s| s == "--smoke-scenario") {
        l6max_host::logging::Format::Json
    } else {
        l6max_host::logging::Format::Text
    };
    if let Err(error) = l6max_host::logging::init(format) {
        eprintln!("{error}");
        std::process::exit(2);
    }
    let context = l6max_host::logging::context("gui");
    let _entered = context.enter();
    if args.first().is_some_and(|arg| arg == "--render-panel") {
        #[cfg(feature = "headless-screenshots")]
        if let Err(error) = headless_screenshot::render(&args[1..]) {
            tracing::error!(%error, "Could not render panel screenshot");
            std::process::exit(1);
        }
        #[cfg(not(feature = "headless-screenshots"))]
        {
            eprintln!("Rebuild with --features headless-screenshots to render a panel screenshot");
            std::process::exit(2);
        }
        #[cfg(feature = "headless-screenshots")]
        return;
    }
    #[cfg(target_os = "linux")]
    if let Err(error) = require_wayland_session() {
        tracing::error!(%error, "l6max-gui");
        std::process::exit(1);
    }
    let mut options = Options::parse().unwrap_or_else(|error| {
        tracing::error!(%error, "GUI operation failed");
        std::process::exit(2);
    });
    #[cfg(target_os = "macos")]
    if options.audio_device.is_none() {
        // Match the firmware's stream so QEMU's mixer does not resample it.
        options.audio_device = Some(
            "driver=coreaudio,id=l6-audio,out.frequency=48000,out.channels=2,out.format=f32,out.buffer-count=12"
                .into(),
        );
    }
    if options.smoke_scenario.is_none() && !args.iter().any(|arg| arg == "--timing") {
        options.timing = l6max_host::engine::Timing::Split;
    }
    let bundle = bundle::Bundle::detect().unwrap_or_else(|error| {
        tracing::error!(%error, "Could not open app data");
        std::process::exit(1);
    });
    // A packaged app ships QEMU and imports firmware through its UI.
    if bundle.is_some() {
        options.auto_setup = false;
    }
    let bundled = bundle.is_some();
    let bundle_testing = std::env::var_os("L6_BUNDLE_SMOKE_FIRST").is_some();
    let menu_testing = std::env::var_os("L6_MENU_SMOKE_FIRMWARE").is_some();
    let mut firmware_error = None;
    if let Some(bundle) = &bundle {
        if let Err(error) = bundle.restore(&mut options) {
            options.qemu = None;
            firmware_error = Some(error);
        }
        if options.qemu.is_some() {
            match bundle::selected_package(&options.firmware_dir) {
                Ok(package) => {
                    if let Err(error) =
                        configuration::configure_with_package(&mut options, Some(&package))
                    {
                        options.qemu = None;
                        firmware_error = Some(error.to_string());
                    }
                }
                Err(error) => {
                    options.qemu = None;
                    firmware_error = Some(error);
                }
            }
        }
    } else {
        // Build QEMU and unpack firmware on first run, so one `cargo run` works.
        if options.auto_setup {
            options.prepare().unwrap_or_else(|error| {
                tracing::error!(%error, "Could not start QEMU");
                std::process::exit(1);
            });
        }
        if options.smoke_scenario.is_none() {
            configuration::configure(&mut options).unwrap_or_else(|error| {
                tracing::error!(%error, "Could not prepare demo SD card");
                std::process::exit(1);
            });
        }
    }
    let selected_scenario = options
        .smoke_scenario
        .as_ref()
        .map(|path| {
            smoke_core::Scenario::load_for_case(path).and_then(|scenario| {
                if !scenario.backends.iter().any(|b| b == "gui") {
                    return Err("scenario requires the headless backend".into());
                }
                Ok(scenario)
            })
        })
        .transpose()
        .unwrap_or_else(|error| {
            tracing::error!(%error, "Invalid smoke scenario");
            std::process::exit(2);
        });
    let qemu_processes = Engine::start(&options).unwrap_or_else(|error| {
        if bundle.is_some() {
            firmware_error = Some(error);
            options.qemu = None;
            Engine::start(&options).expect("empty engine")
        } else {
            tracing::error!(%error, "Could not start QEMU");
            std::process::exit(1);
        }
    });
    let model = qemu_processes.model();
    let controls = qemu_processes.inputs.clone();
    let display = qemu_processes
        .inputs
        .as_ref()
        .map(|_| PanelFeed::new(&qemu_processes).expect("valid panel feed"));
    let run_tick = display.is_some() || bundle.is_some();
    let testing = options.smoke_scenario.is_some();
    let scenario_smoke = selected_scenario
        .map(|scenario| {
            smoke_core::Runner::new(scenario, model, options.logs.clone()).map(|mut runner| {
                runner.set_separate_clocks(options.timing == l6max_host::engine::Timing::Split);
                runner
            })
        })
        .transpose()
        .unwrap_or_else(|error| {
            tracing::error!(%error, "Invalid smoke scenario");
            std::process::exit(2)
        });
    let passed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let result = passed.clone();
    application().run(move |cx: &mut App| {
        let layout = Layout::for_model(model);
        let device_width = components::layout::window_width(model);
        let device_height = device_width * layout.height / layout.width;
        let bounds = Bounds::centered(None, size(px(device_width), px(device_height)), cx);
        let view = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    is_resizable: true,
                    window_min_size: Some(size(px(600.0), px(device_height / 2.0))),
                    ..Default::default()
                },
                |window, cx| {
                    configure_window(window, device_width, device_height);
                    window.set_window_title(&format!("{} Emulator", model_name(model)));
                    cx.new(|_| NativePanel {
                        layout,
                        model,
                        bundle,
                        firmware_error,
                        choosing_firmware: false,
                        importing_sd: false,
                        display,
                        frame: Arc::new(RenderImage::new(SmallVec::new())),
                        last_frame: [0; 1024],
                        last_control_revision: None,
                        status: "Waiting for firmware".into(),
                        controls: controls.clone(),
                        button_pressed: std::array::from_fn(|_| {
                            Arc::new(std::sync::atomic::AtomicBool::new(false))
                        }),
                        scenario_press_duration: Arc::new(AtomicU32::new(0)),
                        knob_drags: std::array::from_fn(|_| {
                            Arc::new(Mutex::new(KnobInput::default()))
                        }),
                        _qemu: Some(qemu_processes),
                        powered_off: false,
                        powered_off_signal: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        power_on_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        physical_analog: [512; 5],
                        scenario_smoke,
                        bundle_smoke: bundle_smoke::BundleSmoke::from_environment(),
                        menu_smoke: menu_smoke::MenuSmoke::from_environment(),
                        options,
                    })
                },
            )
            .expect("open L6 family native window");
        cx.on_action(move |_: &OpenFirmware, cx| {
            // Global actions bubble while GPUI holds the active window. Updating
            // it here fails with "window not found"; defer until dispatch ends.
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.choose_firmware(cx)) {
                    tracing::error!(%error, "Open Firmware action failed");
                }
            });
        });
        cx.on_action(move |_: &OpenSdFolder, cx| {
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.choose_sd_folder(cx)) {
                    tracing::error!(%error, "SD Card Folder action failed");
                }
            });
        });
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(move |_: &ConnectUsb, cx| {
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.connect_usb(cx)) {
                    tracing::error!(%error, "Connect USB action failed");
                }
            });
        });
        cx.on_action(move |_: &DisconnectUsb, cx| {
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.disconnect_usb(cx)) {
                    tracing::error!(%error, "Disconnect USB action failed");
                }
            });
        });
        cx.on_action(move |_: &ConnectMidi, cx| {
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.connect_midi(cx)) {
                    tracing::error!(%error, "Connect MIDI action failed");
                }
            });
        });
        cx.on_action(move |_: &DisconnectMidi, cx| {
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.disconnect_midi(cx)) {
                    tracing::error!(%error, "Disconnect MIDI action failed");
                }
            });
        });
        cx.on_action(move |_: &EjectSdCard, cx| {
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.eject_sd_card(cx)) {
                    tracing::error!(%error, "Eject SD card action failed");
                }
            });
        });
        cx.bind_keys([
            KeyBinding::new("cmd-o", OpenFirmware, None),
            KeyBinding::new("cmd-shift-o", OpenSdFolder, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.set_menus([
            Menu::new("L6 Emulator").items([MenuItem::action("Quit L6 Emulator", Quit)]),
            Menu::new("File").items([
                MenuItem::action("Open Firmware…", OpenFirmware).disabled(!bundled),
                MenuItem::action("Use SD Card Folder…", OpenSdFolder).disabled(!bundled),
                MenuItem::action("Eject SD Card", EjectSdCard),
            ]),
            Menu::new("Connections").items([
                MenuItem::action("Connect USB", ConnectUsb),
                MenuItem::action("Disconnect USB", DisconnectUsb),
                MenuItem::action("Connect MIDI", ConnectMidi),
                MenuItem::action("Disconnect MIDI", DisconnectMidi),
            ]),
        ]);
        // macOS normally keeps apps alive after their last window closes.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        view.update(cx, |_, _, cx| {
            cx.on_app_quit(|panel, _| {
                // Platform termination does not necessarily unwind Rust.
                // Stop and reap QEMU before permitting the app to exit.
                drop(panel._qemu.take());
                std::future::ready(())
            })
            .detach();
        })
        .expect("register emulator shutdown");
        cx.activate(true);
        if run_tick {
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_nanos(16_666_667))
                        .await;
                    if view
                        .update(cx, |panel, window, cx| {
                            let previous = panel.status.clone();
                            let previous_powered_off = panel.powered_off;
                            panel.refresh_power();
                            let guest_restarted = previous_powered_off && !panel.powered_off;
                            let frame_changed = panel.refresh_frame();
                            let indicator_updates = panel
                                .controls
                                .as_ref()
                                .map_or(0, |controls| controls.panel.snapshot().indicator_updates);
                            let analog = panel
                                .controls
                                .as_ref()
                                .map_or(panel.physical_analog, |controls| {
                                    controls.analog_positions()
                                });
                            let button_pressed = std::array::from_fn(|id| {
                                panel.button_pressed[id].load(std::sync::atomic::Ordering::Relaxed)
                            });
                            let knob_dragging = std::array::from_fn(|id| {
                                panel.knob_drags[id].lock().unwrap().dragging()
                            });
                            let control_revision = Some((
                                indicator_updates,
                                analog,
                                panel.powered_off,
                                button_pressed,
                                knob_dragging,
                            ));
                            let controls_changed = control_revision != panel.last_control_revision;
                            panel.last_control_revision = control_revision;
                            if let Some(mut test) = panel.menu_smoke.take() {
                                match test.tick(panel, window, cx) {
                                    Ok(true) => {
                                        passed.store(true, std::sync::atomic::Ordering::Relaxed);
                                        cx.quit();
                                    }
                                    Ok(false) => panel.menu_smoke = Some(test),
                                    Err(error) => {
                                        tracing::error!(%error, "Menu smoke failed");
                                        panel._qemu.take();
                                        std::process::exit(1);
                                    }
                                }
                            }
                            if let Some(mut test) = panel.bundle_smoke.take() {
                                match test.tick(panel, cx) {
                                    Ok(true) => {
                                        passed.store(true, std::sync::atomic::Ordering::Relaxed);
                                        cx.quit();
                                    }
                                    Ok(false) => panel.bundle_smoke = Some(test),
                                    Err(error) => {
                                        tracing::error!(%error, "Bundle smoke failed");
                                        panel._qemu.take();
                                        std::process::exit(1);
                                    }
                                }
                            }
                            let panel_data = panel.panel_data();
                            if guest_restarted {
                                if let Some(test) = &mut panel.scenario_smoke {
                                    test.guest_restarted(&panel.options.logs);
                                }
                            }
                            if let Some(test) = &mut panel.scenario_smoke {
                                match scenario_smoke::tick(
                                    test,
                                    &panel.layout,
                                    panel.controls.as_deref(),
                                    panel_data,
                                    &panel.scenario_press_duration,
                                    window,
                                    cx,
                                ) {
                                    Ok(true) => {
                                        passed.store(true, std::sync::atomic::Ordering::Relaxed);
                                        cx.quit();
                                    }
                                    Ok(false) => {}
                                    Err(error) => {
                                        tracing::error!(%error, "Scenario failed");
                                        panel._qemu.take();
                                        std::process::exit(1);
                                    }
                                }
                            }
                            if panel.status != previous {
                                window.set_window_title(&format!(
                                    "{} Emulator — {}",
                                    model_name(panel.model),
                                    panel.status
                                ));
                            }
                            if panel.status != previous
                                || panel.powered_off != previous_powered_off
                                || frame_changed
                                || controls_changed
                            {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
    });
    if (testing || bundle_testing || menu_testing)
        && !result.load(std::sync::atomic::Ordering::Relaxed)
    {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod startup_key_tests {
    use super::*;

    #[test]
    fn armed_l6_controls_become_startup_taps_only_for_l6() {
        let pressed: [Arc<std::sync::atomic::AtomicBool>; l6max_host::controls::INPUT_COUNT] =
            std::array::from_fn(|_| Arc::new(std::sync::atomic::AtomicBool::new(false)));
        pressed[8].store(true, std::sync::atomic::Ordering::Relaxed);
        pressed[24].store(true, std::sync::atomic::Ordering::Relaxed);
        let l6_layout = Layout::for_model(l6fw::Model::L6);
        let l6_targets = startup_key_targets(l6fw::Model::L6, &l6_layout, &pressed);
        assert_eq!(l6_targets.len(), 2);
        assert!(l6_targets.contains(&8));
        assert!(l6_targets.contains(&24));

        let l6max_layout = Layout::for_model(l6fw::Model::L6max);
        assert!(startup_key_targets(l6fw::Model::L6max, &l6max_layout, &pressed).is_empty());
    }
}
