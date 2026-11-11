//! Exercise the native menu bar, its actions and native pickers without host input.
use crate::NativePanel;
use l6max_host::protocol;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// A failure from a menu operation queued outside the tick.
static NATIVE_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub struct MenuSmoke {
    firmware: PathBuf,
    /// The MAIN image with midi-sync installed in the firmware.
    firmware_patched_main: Vec<u8>,
    stage: u8,
    deadline: Instant,
}

/// A 44.1 kHz stereo 32-bit float WAV, so that assigning it needs a conversion.
fn test_wav(seconds: u32) -> Vec<u8> {
    const RATE: u32 = 44100;
    let samples: Vec<u8> = (0..RATE * seconds)
        .flat_map(|n| {
            let value = (std::f32::consts::TAU * 440.0 * n as f32 / RATE as f32).sin() * 0.01;
            [value, value]
        })
        .flat_map(f32::to_le_bytes)
        .collect();
    let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x03\0\x02\0".to_vec();
    bytes.extend(RATE.to_le_bytes());
    bytes.extend((RATE * 8).to_le_bytes());
    bytes.extend([8, 0, 32, 0]);
    bytes.extend(b"data");
    bytes.extend((samples.len() as u32).to_le_bytes());
    bytes.extend(samples);
    bytes
}

/// The device's saved flash must hold this application.
fn check_installed(state: &std::path::Path, main: &std::path::Path) -> Result<(), String> {
    let nor = std::fs::read(state.join("main-nor.bin")).map_err(|e| e.to_string())?;
    let main = std::fs::read(main).map_err(|e| e.to_string())?;
    if nor[0x50000..0x50000 + main.len()] != main[..] {
        return Err("The device does not hold the selected application".into());
    }
    Ok(())
}

impl MenuSmoke {
    pub fn from_environment() -> Option<Self> {
        std::env::var_os("L6_MENU_SMOKE_FIRMWARE").map(|path| Self {
            firmware: path.into(),
            firmware_patched_main: Vec::new(),
            stage: 0,
            deadline: Instant::now() + Duration::from_secs(30),
        })
    }

    #[cfg(target_os = "macos")]
    fn cancel_picker(directories: bool) -> Result<bool, String> {
        use objc2_app_kit::{NSApplication, NSOpenPanel};
        use objc2_foundation::MainThreadMarker;
        let marker = MainThreadMarker::new().ok_or("Picker check needs main thread")?;
        for window in NSApplication::sharedApplication(marker).windows().iter() {
            if let Some(picker) = window.downcast_ref::<NSOpenPanel>() {
                if !picker.isVisible() {
                    continue;
                }
                if picker.canChooseDirectories() != directories
                    || picker.canChooseFiles() == directories
                {
                    return Err("Native picker has incorrect file/directory mode".into());
                }
                unsafe {
                    picker.cancel(None);
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(not(target_os = "macos"))]
    fn cancel_picker(_: bool) -> Result<bool, String> {
        Err("Native menu smoke requires macOS".into())
    }

    /// Find a menu in the native menu bar by its title.
    #[cfg(target_os = "macos")]
    fn native_menu(menu: &str) -> Result<objc2::rc::Retained<objc2_app_kit::NSMenu>, String> {
        use objc2_app_kit::NSApplication;
        use objc2_foundation::{MainThreadMarker, NSString};
        let marker = MainThreadMarker::new().ok_or("Menu check needs main thread")?;
        let bar = NSApplication::sharedApplication(marker)
            .mainMenu()
            .ok_or("The application has no menu bar")?;
        bar.itemAtIndex(bar.indexOfItemWithTitle(&NSString::from_str(menu)))
            .and_then(|top| top.submenu())
            .ok_or_else(|| format!("The menu bar has no {menu} menu"))
    }

    /// Run `work` on the main queue after this tick returns. Menu validation
    /// and actions call back into GPUI, which this tick is still borrowing.
    #[cfg(target_os = "macos")]
    fn on_main_queue(work: Box<dyn FnOnce() -> Result<(), String>>) {
        use std::ffi::c_void;
        unsafe extern "C" {
            static _dispatch_main_q: c_void;
            fn dispatch_async_f(
                queue: *const c_void,
                context: *mut c_void,
                work: extern "C" fn(*mut c_void),
            );
        }
        extern "C" fn run(context: *mut c_void) {
            let work =
                unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce() -> Result<(), String>>>()) };
            if let Err(error) = work() {
                *NATIVE_ERROR.lock().unwrap() = Some(error);
            }
        }
        unsafe {
            dispatch_async_f(
                &raw const _dispatch_main_q,
                Box::into_raw(Box::new(work)).cast(),
                run,
            );
        }
    }

    /// Validate a menu's items, as AppKit does when the menu opens.
    #[cfg(target_os = "macos")]
    fn validate_menu(menu: &str) -> Result<(), String> {
        let menu = menu.to_owned();
        Self::on_main_queue(Box::new(move || {
            Self::native_menu(&menu)?.update();
            Ok(())
        }));
        Ok(())
    }

    /// Perform a menu item's action as a click does. A disabled item is an error.
    #[cfg(target_os = "macos")]
    fn click(menu: &str, item: &str) -> Result<(), String> {
        let (menu, item) = (menu.to_owned(), item.to_owned());
        Self::on_main_queue(Box::new(move || {
            use objc2_foundation::NSString;
            let submenu = Self::native_menu(&menu)?;
            let index = submenu.indexOfItemWithTitle(&NSString::from_str(&item));
            let entry = submenu
                .itemAtIndex(index)
                .ok_or_else(|| format!("The {menu} menu has no {item:?} item"))?;
            if !entry.isEnabled() {
                return Err(format!("{menu} → {item} is disabled"));
            }
            submenu.performActionForItemAtIndex(index);
            Ok(())
        }));
        Ok(())
    }

    /// Whether the native menu currently lists `item`.
    #[cfg(target_os = "macos")]
    fn has_item(menu: &str, item: &str) -> Result<bool, String> {
        use objc2_foundation::NSString;
        let submenu = Self::native_menu(menu)?;
        Ok(submenu.indexOfItemWithTitle(&NSString::from_str(item)) >= 0)
    }

    #[cfg(not(target_os = "macos"))]
    fn has_item(_: &str, _: &str) -> Result<bool, String> {
        Err("Native menu smoke requires macOS".into())
    }

    #[cfg(not(target_os = "macos"))]
    fn validate_menu(_: &str) -> Result<(), String> {
        Err("Native menu smoke requires macOS".into())
    }

    #[cfg(not(target_os = "macos"))]
    fn click(_: &str, _: &str) -> Result<(), String> {
        Err("Native menu smoke requires macOS".into())
    }

    pub fn tick(
        &mut self,
        panel: &mut NativePanel,
        cx: &mut gpui::Context<NativePanel>,
    ) -> Result<bool, String> {
        if let Some(error) = NATIVE_ERROR.lock().unwrap().take() {
            return Err(error);
        }
        if Instant::now() > self.deadline {
            return Err(format!("Menu smoke timed out at stage {}", self.stage));
        }
        match self.stage {
            0 => {
                if panel.bundle.is_none() {
                    return Err("Run from an app bundle".into());
                }
                let expected_qemu = std::env::var("L6_MENU_SMOKE_QEMU_SHA256")
                    .map_err(|_| "L6_MENU_SMOKE_QEMU_SHA256 is required")?;
                crate::bundle::verify_bundled_qemu(
                    &panel.bundle.as_ref().unwrap().qemu,
                    &expected_qemu,
                )?;
                Self::validate_menu("File")?;
                self.stage = 20;
            }
            20 => {
                Self::click("File", "Open Firmware…")?;
                self.stage = 1;
            }
            1 if panel.choosing_firmware && Self::cancel_picker(false)? => {
                println!("PASS: Open Firmware action presents native file picker");
                self.stage = 2;
            }
            2 if !panel.choosing_firmware => {
                // Import through the existing engine path, then check menu
                // dispatch with a running guest, not only the first-run overlay.
                panel.load_firmware(&self.firmware)?;
                Self::validate_menu("File")?;
                self.stage = 21;
            }
            21 => {
                Self::click("File", "Open Firmware…")?;
                self.stage = 3;
            }
            3 if panel.choosing_firmware && Self::cancel_picker(false)? => {
                println!("PASS: Open Firmware picker also opens with a running guest");
                self.stage = 4;
            }
            4 if !panel.choosing_firmware => {
                if panel.controls.is_none() || panel.firmware_error.is_some() {
                    return Err("Cancelling firmware picker disturbed guest".into());
                }
                Self::validate_menu("File")?;
                self.stage = 24;
            }
            24 => {
                Self::click("File", "Use SD Card Folder…")?;
                self.stage = 5;
            }
            5 if panel.choosing_firmware && Self::cancel_picker(true)? => {
                println!("PASS: SD Card Folder action presents native directory picker");
                self.stage = 6;
            }
            6 if !panel.choosing_firmware && !panel.importing_sd => {
                if panel.controls.is_none() || panel.firmware_error.is_some() {
                    return Err("Cancelling SD picker disturbed guest".into());
                }
                println!("PASS: picker cancellation keeps guest running and actions reusable");
                if panel.options.sd_image.is_none() {
                    return Err("The guest has no SD card to eject".into());
                }
                Self::click("File", "Eject SD Card")?;
                self.stage = 30;
            }
            30 if panel.options.sd_image.is_none() && panel.ejected_card.is_some() => {
                if !panel
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.text == "SD card ejected" && !notice.error)
                {
                    return Err("Ejecting the card showed no notice in the window".into());
                }
                // The menu follows the state on the tick that saw it change.
                Self::validate_menu("File")?;
                self.stage = 31;
            }
            31 => {
                Self::click("File", "Insert SD Card")?;
                self.stage = 32;
            }
            32 if panel.options.sd_image.is_some() && panel.ejected_card.is_none() => {
                println!("PASS: Eject SD Card and Insert SD Card swap in the File menu");
                Self::validate_menu("Audio Input")?;
                self.deadline = Instant::now() + Duration::from_secs(120);
                self.stage = 22;
            }
            22 => {
                Self::click("Audio Input", "Channels and Playback…")?;
                self.stage = 7;
            }
            7 if panel.audio_window.is_some() => {
                // Assigning files must bring the window back by itself.
                let audio = panel.audio_window.take().unwrap();
                audio
                    .update(cx, |_, window, _| window.remove_window())
                    .map_err(|e| e.to_string())?;
                self.stage = 13;
            }
            13 => {
                let wav = panel.options.logs.join("menu-smoke.wav");
                std::fs::write(&wav, test_wav(30)).map_err(|e| e.to_string())?;
                let patch = [(1, wav.clone()), (5, wav)].into_iter().collect();
                panel.assign_audio_patch(patch, cx);
                self.stage = 8;
            }
            8 => {
                if let Some(error) = &panel.audio_error {
                    return Err(format!("Audio assignment failed: {error}"));
                }
                if panel.audio_conversion.is_none() && panel.audio_length_ms > 0 {
                    if panel.audio_window.is_none() {
                        return Err("Conversion did not open the audio window".into());
                    }
                    let stale = l6max_host::audio_input::stale_channels(
                        &panel.options.audio_input_directory(),
                        &panel.audio_patch,
                    )?;
                    if !stale.is_empty() {
                        return Err(format!("Channels {stale:?} were not converted"));
                    }
                    if panel.saved_audio_patch().len() != 2 {
                        return Err("Audio assignment was not saved".into());
                    }
                    println!("PASS: audio window opens and converts assigned files");
                    Self::validate_menu("Audio Input")?;
                    self.stage = 33;
                }
            }
            33 => {
                Self::click("Audio Input", "Play")?;
                self.stage = 9;
            }
            9 if panel.audio_position().1 && panel.audio_position().0 > 0 => {
                println!("PASS: assigned audio plays and reports its position");
                self.stage = 34;
            }
            34 => {
                // The tick that saw playback start rebuilt the menu.
                if !Self::has_item("Audio Input", "Pause")?
                    || Self::has_item("Audio Input", "Play")?
                {
                    return Err("The Audio Input menu did not switch Play to Pause".into());
                }
                println!("PASS: the Audio Input menu offers Pause while playing");
                let controls = panel.controls.as_ref().ok_or("No guest inputs")?;
                controls
                    .main
                    .audio_input_transport(protocol::AUDIO_INPUT_SEEK, 20000)
                    .map_err(|e| e.to_string())?;
                self.stage = 10;
            }
            10 if panel.audio_position().0 >= 20000 => {
                let controls = panel.controls.as_ref().ok_or("No guest inputs")?;
                controls
                    .main
                    .audio_input_transport(protocol::AUDIO_INPUT_STOP, 0)
                    .map_err(|e| e.to_string())?;
                self.stage = 11;
            }
            11 if panel.audio_position() == (0, false) => {
                println!("PASS: audio playback seeks and stops");
                panel.clear_audio_inputs(cx);
                self.stage = 12;
            }
            12 if panel.audio_conversion.is_none() && panel.audio_length_ms == 0 => {
                if !panel.saved_audio_patch().is_empty() {
                    return Err("Cleared audio stayed in the configuration".into());
                }
                println!("PASS: clearing channels removes their audio and saved files");
                Self::validate_menu("File")?;
                self.stage = 23;
            }
            23 => {
                Self::click("File", "Firmware Patches…")?;
                self.stage = 14;
            }
            14 if panel.patches_window.is_some() => {
                let patches = panel.patches_window.unwrap();
                // Applying updates the panel, which this tick is still updating.
                cx.defer(move |cx| {
                    let _ = patches.update(cx, |patches, window, cx| {
                        let selection =
                            crate::patches_window::Selection::new(vec!["midi-sync".into()], vec![]);
                        patches.apply(selection, window, cx)
                    });
                });
                self.stage = 17;
            }
            17 if panel.applying_patches => {
                let closed = panel
                    .patches_window
                    .unwrap()
                    .update(cx, |_, _, _| ())
                    .is_err();
                if !closed {
                    return Err("Applying did not close the patches window".into());
                }
                println!("PASS: applying closes the window and runs in the background");
                self.stage = 15;
            }
            15 if !panel.applying_patches => {
                if let Some(error) = &panel.patch_error {
                    return Err(format!("Applying a patch failed: {error}"));
                }
                let state = panel.options.state_dir.clone().ok_or("No device state")?;
                check_installed(&state, &state.join("patched-firmware/main_firmware.bin"))?;
                if panel.saved_patches() != ["midi-sync"] || panel.controls.is_none() {
                    return Err("Applying a patch did not restart and save it".into());
                }
                println!("PASS: applying a patch installs it, restarts and saves it");
                self.firmware_patched_main =
                    std::fs::read(state.join("patched-firmware/main_firmware.bin"))
                        .map_err(|e| e.to_string())?;
                let selection =
                    crate::patches_window::Selection::new(vec![], vec!["midi-sync".into()]);
                panel.apply_patches(selection, cx);
                self.stage = 18;
            }
            18 if !panel.applying_patches => {
                if let Some(error) = &panel.patch_error {
                    return Err(format!("Loading a patch from the SD card failed: {error}"));
                }
                let state = panel.options.state_dir.clone().ok_or("No device state")?;
                let main = state.join("patched-firmware/main_firmware.bin");
                check_installed(&state, &main)?;
                let stock = std::fs::read(panel.options.firmware_dir.join("main_firmware.bin"))
                    .map_err(|e| e.to_string())?;
                let installed = std::fs::read(&main).map_err(|e| e.to_string())?;
                if installed == stock || installed == self.firmware_patched_main {
                    return Err("SD card delivery did not install the Patch loader".into());
                }
                let card = panel.options.sd_image.clone().ok_or("No SD card")?;
                // Nothing to write means the card already holds exactly the module.
                if l6max_host::sd_patches::install(&card, panel.model, &["midi-sync".into()])? {
                    return Err("The SD card did not hold the midi-sync module".into());
                }
                if !panel.saved_patches().is_empty()
                    || panel.saved_sd_patches() != ["midi-sync"]
                    || panel.controls.is_none()
                {
                    return Err("SD card delivery did not restart and save it".into());
                }
                println!("PASS: loading a patch from the SD card installs the loader and module");
                panel.apply_patches(Default::default(), cx);
                self.stage = 16;
            }
            16 if !panel.applying_patches => {
                if let Some(error) = &panel.patch_error {
                    return Err(format!("Removing patches failed: {error}"));
                }
                let state = panel.options.state_dir.clone().ok_or("No device state")?;
                check_installed(
                    &state,
                    &panel.options.firmware_dir.join("main_firmware.bin"),
                )?;
                if !panel.saved_patches().is_empty()
                    || !panel.saved_sd_patches().is_empty()
                    || panel.controls.is_none()
                    || state.join("patched-firmware").exists()
                {
                    return Err("Removing patches did not restore the stock firmware".into());
                }
                let card = panel.options.sd_image.clone().ok_or("No SD card")?;
                if l6max_host::sd_patches::install(&card, panel.model, &[])? {
                    return Err("Removing patches left the module on the SD card".into());
                }
                println!("PASS: removing patches restores the stock firmware");
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
}
