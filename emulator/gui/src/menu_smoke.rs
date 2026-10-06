//! Exercise the real action dispatcher and native pickers without host input.
use crate::{NativePanel, OpenFirmware, OpenSdFolder};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct MenuSmoke {
    firmware: PathBuf,
    stage: u8,
    deadline: Instant,
}

impl MenuSmoke {
    pub fn from_environment() -> Option<Self> {
        std::env::var_os("L6_MENU_SMOKE_FIRMWARE").map(|path| Self {
            firmware: path.into(),
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

    pub fn tick(
        &mut self,
        panel: &mut NativePanel,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<bool, String> {
        if Instant::now() > self.deadline {
            return Err(format!("Menu smoke timed out at stage {}", self.stage));
        }
        match self.stage {
            0 => {
                if panel.bundle.is_none() {
                    return Err("Run from an app bundle".into());
                }
                window.dispatch_action(Box::new(OpenFirmware), cx);
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
                window.dispatch_action(Box::new(OpenFirmware), cx);
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
                window.dispatch_action(Box::new(OpenSdFolder), cx);
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
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
}
