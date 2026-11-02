//! Exercise the real action dispatcher and native pickers without host input.
use crate::{NativePanel, OpenAudioWindow, OpenFirmware, OpenSdFolder};
use l6max_host::protocol;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct MenuSmoke {
    firmware: PathBuf,
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
        cx: &mut gpui::Context<NativePanel>,
    ) -> Result<bool, String> {
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
                window.dispatch_action(Box::new(OpenAudioWindow), cx);
                self.deadline = Instant::now() + Duration::from_secs(120);
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
                    let controls = panel.controls.as_ref().ok_or("No guest inputs")?;
                    controls
                        .main
                        .audio_input_transport(protocol::AUDIO_INPUT_PLAY, 0)
                        .map_err(|e| e.to_string())?;
                    self.stage = 9;
                }
            }
            9 if panel.audio_position().1 && panel.audio_position().0 > 0 => {
                println!("PASS: assigned audio plays and reports its position");
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
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
}
