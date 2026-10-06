mod bundle;
mod bundle_smoke;
mod components;
mod configuration;
mod knob_input;
mod menu_smoke;
mod ui_power_smoke;
mod ui_smoke;

use components::layout::{Layout, Region};
use gpui::{
    App, Bounds, Context, CursorStyle, ImageSource, KeyBinding, Menu, MenuItem, MouseButton,
    PathPromptOptions, Render, RenderImage, ScrollDelta, SharedString, Window, WindowBounds,
    WindowOptions, actions, div, img, prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use image::{Frame, ImageBuffer, Rgba};
use knob_input::KnobInput;
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
    input::InputHub,
};
use smallvec::SmallVec;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const DEVICE_WIDTH: f32 = 1200.0;
const DEVICE_HEIGHT: f32 =
    DEVICE_WIDTH * components::layout::BODY.height / components::layout::BODY.width;
const LCD_BUFFER_WIDTH: u32 = 128;
const LCD_BUFFER_HEIGHT: u32 = 64;
const DEFAULT_BUTTON_PRESS_TICKS: u32 = 50;

fn button_hit(
    event_id: usize,
    is_switch: bool,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    controls: Option<Arc<InputHub>>,
    pressed: Arc<std::sync::atomic::AtomicBool>,
) -> impl IntoElement {
    let down_pressed = pressed.clone();
    let up_pressed = pressed.clone();
    let out_pressed = pressed;
    let down_controls = controls.clone();
    let up_controls = controls;
    let out_controls = up_controls.clone();
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .h(px(height))
        .rounded(px(4.0))
        .hover(|style| style.bg(rgba(0xffe6a455)))
        .on_mouse_down(MouseButton::Left, move |_, _, _| {
            if is_switch {
                let was_down = down_pressed.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(controls) = &down_controls {
                    let result = if was_down {
                        controls.up(event_id as u32)
                    } else {
                        controls.down(event_id as u32, 0)
                    };
                    if let Err(error) = result {
                        eprintln!("Switch failed: {error}");
                    }
                }
                return;
            }
            down_pressed.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(controls) = &down_controls {
                if let Err(error) = controls.down(event_id as u32, DEFAULT_BUTTON_PRESS_TICKS) {
                    eprintln!("Button down failed: {error}");
                }
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, _| {
            if is_switch {
                return;
            }
            if !up_pressed.swap(false, std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            if let Some(controls) = &up_controls {
                if let Err(error) = controls.up(event_id as u32) {
                    eprintln!("Button up failed: {error}");
                }
            }
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, _| {
            if is_switch {
                return;
            }
            if !out_pressed.swap(false, std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            if let Some(controls) = &out_controls {
                if let Err(error) = controls.up(event_id as u32) {
                    eprintln!("Button up failed: {error}");
                }
            }
        })
}

fn knob_hit(
    channel: usize,
    region: Region,
    scale: f32,
    drag: Arc<Mutex<KnobInput>>,
    controls: Option<Arc<InputHub>>,
) -> impl IntoElement {
    let down_drag = drag.clone();
    let scroll_drag = drag.clone();
    let up_drag = drag.clone();
    let out_drag = drag.clone();
    let scroll_controls = controls.clone();
    let cursor = if drag.lock().unwrap().dragging() {
        CursorStyle::ClosedHand
    } else {
        CursorStyle::OpenHand
    };
    div()
        .absolute()
        .left(px(region.x * scale))
        .top(px(region.y * scale))
        .w(px(region.width * scale))
        .h(px(region.height * scale))
        .rounded_full()
        .cursor(cursor)
        .on_mouse_down(MouseButton::Left, move |event, _, _| {
            down_drag
                .lock()
                .unwrap()
                .start(event.position.x.as_f32(), event.position.y.as_f32());
        })
        .on_scroll_wheel(move |event, _, cx| {
            let delta = match event.delta {
                ScrollDelta::Lines(p) => {
                    if p.y.abs() >= p.x.abs() {
                        p.y
                    } else {
                        p.x
                    }
                }
                ScrollDelta::Pixels(p) => {
                    if p.y.abs() >= p.x.abs() {
                        p.y.as_f32() / 24.0
                    } else {
                        p.x.as_f32() / 24.0
                    }
                }
            };
            let steps = scroll_drag.lock().unwrap().scroll(delta);
            if steps != 0 {
                if let Some(controls) = &scroll_controls {
                    if let Err(error) = controls.knob(channel, steps) {
                        eprintln!("Encoder scroll failed: {error}");
                    }
                }
            }
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, move |_, _, _| {
            up_drag.lock().unwrap().stop()
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, _| {
            out_drag.lock().unwrap().stop()
        })
}

actions!(l6max, [OpenFirmware, OpenSdFolder, Quit]);

struct NativePanel {
    layout: Layout,
    bundle: Option<bundle::Bundle>,
    firmware_error: Option<String>,
    choosing_firmware: bool,
    importing_sd: bool,
    display: Option<PublishedDisplay>,
    frame: Arc<RenderImage>,
    last_frame: [u8; 1024],
    status: SharedString,
    controls: Option<Arc<InputHub>>,
    button_pressed: [Arc<std::sync::atomic::AtomicBool>; l6max_host::controls::INPUT_COUNT],
    knob_drags: [Arc<Mutex<KnobInput>>; 13],
    _qemu: Option<Engine>,
    options: Options,
    powered_off: bool,
    power_on_requested: Arc<std::sync::atomic::AtomicBool>,
    physical_analog: [u16; 5],
    ui_smoke: Option<ui_smoke::UiSmoke>,
    power_smoke: Option<ui_power_smoke::PowerSmoke>,
    bundle_smoke: Option<bundle_smoke::BundleSmoke>,
    menu_smoke: Option<menu_smoke::MenuSmoke>,
}

impl NativePanel {
    fn choose_firmware(&mut self, cx: &mut Context<Self>) {
        if self.bundle.is_none() || self.choosing_firmware || self.importing_sd {
            return;
        }
        self.choosing_firmware = true;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select L6max firmware".into()),
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

    fn new_sd_image(&self) -> std::path::PathBuf {
        let root = self.bundle.as_ref().unwrap().data.join("cards");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        root.join(format!("card-{stamp}-{}.img", std::process::id()))
    }

    fn use_sd_image(&mut self, image: std::path::PathBuf) -> Result<(), String> {
        let mut options = self.options.clone();
        options.sd_image = Some(image.clone());
        options.no_sd = false;
        if let Some(controls) = &self.controls {
            self.physical_analog = controls.analog_positions();
        }
        self.restart_with_options(options)?;
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
        let package = options.firmware_dir.join("L6max.bin");
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
        let engine = Engine::start(&self.options)?;
        let controls = engine.inputs.clone().ok_or("QEMU unavailable")?;
        let display = PublishedDisplay::new(
            engine.display_memory.as_ref().unwrap().map(),
            controls.main.clone(),
        )
        .map_err(|e| e.to_string())?;
        controls
            .restore_analog_positions(self.physical_analog)
            .map_err(|e| e.to_string())?;
        for (control, _) in &self.layout.buttons {
            if control.switch
                && self.button_pressed[control.id as usize]
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                controls.down(control.id, 0).map_err(|e| e.to_string())?;
            }
        }
        self.controls = Some(controls);
        self.display = Some(display);
        self._qemu = Some(engine);
        self.powered_off = false;
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
        self.status = "Powered off · press Power to start".into();
    }
    fn refresh_frame(&mut self) {
        let Some(display) = &self.display else {
            return;
        };
        let snapshot = match display.sample() {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(error) => {
                self.status = format!("Display disconnected: {error}").into();
                return;
            }
        };
        if snapshot == self.last_frame {
            return;
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
        self.status = "Live display · completed SPI transfers".into();
    }
}

impl Render for NativePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let scale = (viewport.width.as_f32() / self.layout.width)
            .min(viewport.height.as_f32() / self.layout.height)
            .max(0.001);
        let rows = self
            .controls
            .as_ref()
            .map(|c| c.panel.snapshot().indicators)
            .unwrap_or([0; 8]);
        let switches = std::array::from_fn(|id| {
            self.button_pressed[id].load(std::sync::atomic::Ordering::Relaxed)
        });
        let analog = self
            .controls
            .as_ref()
            .map(|c| c.analog_positions())
            .unwrap_or(self.physical_analog);
        let lcd = self.layout.lcd;
        let lcd_scale =
            (lcd.width / LCD_BUFFER_WIDTH as f32).min(lcd.height / LCD_BUFFER_HEIGHT as f32);
        let lcd_width = LCD_BUFFER_WIDTH as f32 * lcd_scale;
        let lcd_height = LCD_BUFFER_HEIGHT as f32 * lcd_scale;
        let move_drags = self.knob_drags.clone();
        let move_controls = self.controls.clone();
        let active_drag = move_drags
            .iter()
            .any(|drag| drag.lock().unwrap().dragging());
        let show_selection = self.bundle.is_some()
            && (self.controls.is_none() && !self.powered_off || self.firmware_error.is_some());
        div()
            .relative()
            .cursor(if active_drag {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::Arrow
            })
            .on_mouse_move(move |event, _, _| {
                for (channel, drag) in move_drags.iter().enumerate() {
                    let mut drag = drag.lock().unwrap();
                    if !event.dragging() {
                        drag.stop();
                        continue;
                    }
                    let steps = drag.drag(
                        event.position.x.as_f32(),
                        event.position.y.as_f32(),
                        8.0 * scale,
                    );
                    if steps != 0 {
                        if let Some(controls) = &move_controls {
                            if let Err(error) = controls.knob(channel, steps) {
                                eprintln!("Encoder drag failed: {error}");
                            }
                        }
                    }
                }
            })
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .bg(rgb(0x000000))
            .child(
                div()
                    .relative()
                    .w(px(self.layout.width * scale))
                    .h(px(self.layout.height * scale))
                    .flex_shrink_0()
                    .child(components::panel::panel(
                        &self.layout,
                        scale,
                        rows,
                        switches,
                        analog,
                    ))
                    .child(
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
                            ),
                    )
                    .children(
                        self.layout
                            .buttons
                            .iter()
                            .copied()
                            .map(|(control, region)| {
                                let id = control.id as usize;
                                if id == 53 && self.powered_off {
                                    let request = self.power_on_requested.clone();
                                    return div()
                                        .absolute()
                                        .left(px(region.x * scale))
                                        .top(px(region.y * scale))
                                        .w(px(region.width * scale))
                                        .h(px(region.height * scale))
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                            request
                                                .store(true, std::sync::atomic::Ordering::Relaxed);
                                        })
                                        .into_any_element();
                                }
                                button_hit(
                                    id,
                                    control.switch,
                                    region.x * scale,
                                    region.y * scale,
                                    region.width * scale,
                                    region.height * scale,
                                    self.controls.clone(),
                                    self.button_pressed[id].clone(),
                                )
                                .into_any_element()
                            }),
                    )
                    .children((0..13).map(|channel| {
                        knob_hit(
                            channel,
                            self.layout.knobs[channel],
                            scale,
                            self.knob_drags[channel].clone(),
                            self.controls.clone(),
                        )
                    })),
            )
            .when(show_selection, |element| element.child(
                div().absolute().size_full().occlude().bg(rgba(0x080c18dd))
                    .flex().items_center().justify_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().id("firmware-dialog").w(px(420.0)).max_h(px((viewport.height.as_f32() - 24.0).max(120.0))).overflow_y_scroll().p_8().rounded_xl().bg(rgb(0x171f30))
                        .border_1().border_color(rgb(0x34405c)).text_color(rgb(0xe8eef8))
                        .flex().flex_col().gap_4()
                        .child(div().text_2xl().child(if self.controls.is_some() { "Unable to open selection" } else { "Select firmware" }))
                        .child(if self.controls.is_some() { "Use the File menu to choose another firmware or SD card folder." } else { "Choose your L6max update file to start the emulator. Firmware is not included." })
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
fn configure_window(window: &mut Window) {
    use objc2_app_kit::NSView;
    use objc2_foundation::NSSize;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).expect("native window handle");
    if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
        // SAFETY: GPUI supplies a live NSView handle, and this callback runs on
        // the AppKit main thread. The borrow does not outlive the window.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        if let Some(window) = view.window() {
            window.setContentAspectRatio(NSSize::new(DEVICE_WIDTH as f64, DEVICE_HEIGHT as f64));
            window.setContentSize(NSSize::new(DEVICE_WIDTH as f64, DEVICE_HEIGHT as f64));
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_window(_window: &mut Window) {}

fn main() {
    let mut options = Options::parse().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    if std::env::var_os("L6_UI_POWER_SMOKE").is_some() && options.state_dir.is_some() {
        eprintln!("L6_UI_POWER_SMOKE requires --volatile to protect device state");
        std::process::exit(2);
    }
    let bundle = bundle::Bundle::detect().unwrap_or_else(|error| {
        eprintln!("Could not open app data: {error}");
        std::process::exit(1);
    });
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
            let package = options.firmware_dir.join("L6max.bin");
            if let Err(error) = configuration::configure_with_package(&mut options, Some(&package))
            {
                options.qemu = None;
                firmware_error = Some(error.to_string());
            }
        }
    } else {
        configuration::configure(&mut options).unwrap_or_else(|error| {
            eprintln!("Could not prepare demo SD card: {error}");
            std::process::exit(1);
        });
    }
    let qemu_processes = Engine::start(&options).unwrap_or_else(|error| {
        if bundle.is_some() {
            firmware_error = Some(error);
            options.qemu = None;
            Engine::start(&options).expect("empty engine")
        } else {
            eprintln!("Could not start QEMU: {error}");
            std::process::exit(1);
        }
    });
    let controls = qemu_processes.inputs.clone();
    let display = qemu_processes
        .display_memory
        .as_ref()
        .zip(controls.as_ref())
        .map(|(memory, controls)| {
            PublishedDisplay::new(memory.map(), controls.main.clone())
                .expect("valid display segment")
        });
    let run_tick = display.is_some() || bundle.is_some();
    let testing = options.ui_input_smoke;
    let power_testing = std::env::var_os("L6_UI_POWER_SMOKE").is_some();
    let passed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let result = passed.clone();
    application().run(move |cx: &mut App| {
        let layout = Layout::new();
        let bounds = Bounds::centered(None, size(px(DEVICE_WIDTH), px(DEVICE_HEIGHT)), cx);
        let view = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    is_resizable: true,
                    window_min_size: Some(size(px(600.0), px(DEVICE_HEIGHT / 2.0))),
                    ..Default::default()
                },
                |window, cx| {
                    configure_window(window);
                    window.set_window_title("L6max Emulator");
                    cx.new(|_| NativePanel {
                        layout,
                        bundle,
                        firmware_error,
                        choosing_firmware: false,
                        importing_sd: false,
                        display,
                        frame: Arc::new(RenderImage::new(SmallVec::new())),
                        last_frame: [0; 1024],
                        status: "Waiting for firmware".into(),
                        controls: controls.clone(),
                        button_pressed: std::array::from_fn(|_| {
                            Arc::new(std::sync::atomic::AtomicBool::new(false))
                        }),
                        knob_drags: std::array::from_fn(|_| {
                            Arc::new(Mutex::new(KnobInput::default()))
                        }),
                        _qemu: Some(qemu_processes),
                        powered_off: false,
                        power_on_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        physical_analog: [512; 5],
                        ui_smoke: testing.then(|| ui_smoke::UiSmoke::new(options.logs.clone())),
                        power_smoke: power_testing.then(ui_power_smoke::PowerSmoke::new),
                        bundle_smoke: bundle_smoke::BundleSmoke::from_environment(),
                        menu_smoke: menu_smoke::MenuSmoke::from_environment(),
                        options,
                    })
                },
            )
            .expect("open L6max native window");
        cx.on_action(move |_: &OpenFirmware, cx| {
            // Global actions bubble while GPUI holds the active window. Updating
            // it here fails with "window not found"; defer until dispatch ends.
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.choose_firmware(cx)) {
                    eprintln!("Open Firmware action failed: {error}");
                }
            });
        });
        cx.on_action(move |_: &OpenSdFolder, cx| {
            cx.defer(move |cx| {
                if let Err(error) = view.update(cx, |panel, _, cx| panel.choose_sd_folder(cx)) {
                    eprintln!("SD Card Folder action failed: {error}");
                }
            });
        });
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-o", OpenFirmware, None),
            KeyBinding::new("cmd-shift-o", OpenSdFolder, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.set_menus([
            Menu::new("L6max Emulator").items([MenuItem::action("Quit L6max Emulator", Quit)]),
            Menu::new("File").items([
                MenuItem::action("Open Firmware…", OpenFirmware).disabled(!bundled),
                MenuItem::action("Use SD Card Folder…", OpenSdFolder).disabled(!bundled),
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
                            panel.refresh_power();
                            panel.refresh_frame();
                            if let Some(mut test) = panel.menu_smoke.take() {
                                match test.tick(panel, window, cx) {
                                    Ok(true) => {
                                        passed.store(true, std::sync::atomic::Ordering::Relaxed);
                                        cx.quit();
                                    }
                                    Ok(false) => panel.menu_smoke = Some(test),
                                    Err(error) => {
                                        eprintln!("Menu smoke failed: {error}");
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
                                        eprintln!("Bundle smoke failed: {error}");
                                        panel._qemu.take();
                                        std::process::exit(1);
                                    }
                                }
                            }
                            if let Some(mut test) = panel.power_smoke.take() {
                                match test.tick(panel, window, cx) {
                                    Ok(true) => {
                                        passed.store(true, std::sync::atomic::Ordering::Relaxed);
                                        cx.quit();
                                    }
                                    Err(error) => {
                                        eprintln!("UI Power failed: {error}");
                                        panel._qemu.take();
                                        std::process::exit(1);
                                    }
                                    Ok(false) => panel.power_smoke = Some(test),
                                }
                            }
                            if let (Some(test), Some(inputs)) =
                                (&mut panel.ui_smoke, &panel.controls)
                            {
                                match test.tick(&panel.layout, inputs, window, cx) {
                                    Ok(true) => {
                                        passed.store(true, std::sync::atomic::Ordering::Relaxed);
                                        cx.quit();
                                    }
                                    Err(error) => {
                                        eprintln!("UI input failed: {error}");
                                        panel._qemu.take();
                                        std::process::exit(1);
                                    }
                                    Ok(false) => {}
                                }
                            }
                            if panel.status != previous {
                                window.set_window_title(&format!(
                                    "L6max Emulator — {}",
                                    panel.status
                                ));
                            }
                            cx.notify();
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
    if (testing || power_testing || bundle_testing || menu_testing)
        && !result.load(std::sync::atomic::Ordering::Relaxed)
    {
        std::process::exit(1);
    }
}
