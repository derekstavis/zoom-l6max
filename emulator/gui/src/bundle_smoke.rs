//! Opt-in packaged-app check; exercises import/relaunch without the host pointer.
use crate::NativePanel;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct BundleSmoke {
    first: PathBuf,
    second: PathBuf,
    sd_folder: Option<PathBuf>,
    card_only: bool,
    stage: u8,
    deadline: Instant,
}

impl BundleSmoke {
    pub fn from_environment() -> Option<Self> {
        let first = std::env::var_os("L6_BUNDLE_SMOKE_FIRST")?;
        let second =
            std::env::var_os("L6_BUNDLE_SMOKE_SECOND").expect("L6_BUNDLE_SMOKE_SECOND is required");
        Some(Self {
            first: first.into(),
            second: second.into(),
            sd_folder: std::env::var_os("L6_BUNDLE_SMOKE_SD_FOLDER").map(PathBuf::from),
            card_only: std::env::var_os("L6_BUNDLE_SMOKE_CARD_ONLY").is_some(),
            stage: 0,
            deadline: Instant::now() + Duration::from_secs(360),
        })
    }

    fn select_sd_folder(&mut self, panel: &mut NativePanel) -> Result<(), String> {
        let folder = self
            .sd_folder
            .as_ref()
            .ok_or("SD folder fixture is required")?;
        let image = panel.new_sd_image();
        l6max_host::demo_sd::from_directory(&image, folder).map_err(|e| e.to_string())?;
        panel.options.trace_sd = true;
        let previous = panel
            ._qemu
            .as_ref()
            .and_then(|engine| engine.process_id())
            .ok_or("Start with selected firmware")?;
        panel.use_sd_image(image)?;
        #[cfg(target_os = "macos")]
        if unsafe { libc::kill(previous as i32, 0) } == 0 {
            return Err("Old QEMU survived SD card restart".into());
        }
        println!("PASS: imported SD folder and restarted guest without changing firmware state");
        self.stage = 3;
        Ok(())
    }

    pub fn tick(&mut self, panel: &mut NativePanel, cx: &mut gpui::App) -> Result<bool, String> {
        if Instant::now() > self.deadline {
            return Err("Packaged firmware smoke timed out".into());
        }
        match self.stage {
            0 => {
                if panel.bundle.is_none() || (!self.card_only && panel.controls.is_some()) {
                    return Err(
                        "Run this check from a bundle with a fresh L6_APP_DATA directory".into(),
                    );
                }
                let menus = cx.get_menus().ok_or("No native menu")?;
                let has_open = menus.iter().any(|menu| menu.name.as_ref() == "File" && menu.items.iter().any(|item| {
                    matches!(item, gpui::OwnedMenuItem::Action { name, disabled: false, .. } if name == "Open Firmware…")
                }));
                let has_sd = menus.iter().any(|menu| menu.name.as_ref() == "File" && menu.items.iter().any(|item| {
                    matches!(item, gpui::OwnedMenuItem::Action { name, disabled: false, .. } if name == "Use SD Card Folder…")
                }));
                if !has_sd {
                    return Err("Native File menu is missing SD folder selection".into());
                }
                if !has_open {
                    return Err("Native File menu is missing enabled Open Firmware".into());
                }
                if self.card_only {
                    self.select_sd_folder(panel)?;
                    return Ok(false);
                }
                panel.load_firmware(&self.first)?;
                println!("PASS: empty bundle imported first firmware and started QEMU");
                self.stage = 1;
            }
            1 | 2 | 3 => {
                let controls = panel.controls.as_ref().ok_or("No guest inputs")?;
                if controls.panel.snapshot().guest_ms < 40000 {
                    return Ok(false);
                }
                if panel.last_frame.iter().all(|b| *b == 0) {
                    return Err("No firmware display after boot".into());
                }
                let bundle = panel.bundle.as_ref().unwrap();
                let nor = std::fs::read(
                    panel
                        .options
                        .state_dir
                        .as_ref()
                        .unwrap()
                        .join("main-nor.bin"),
                )
                .map_err(|e| e.to_string())?;
                let expected = std::fs::read(panel.options.firmware_dir.join("main_firmware.bin"))
                    .map_err(|e| e.to_string())?;
                if nor[0x50000..0x50000 + expected.len()] != expected {
                    return Err("Persisted NOR does not contain selected firmware".into());
                }
                let mut restored = panel.options.clone();
                bundle.restore(&mut restored)?;
                if restored.firmware_dir != panel.options.firmware_dir {
                    return Err("Selection was not remembered".into());
                }
                println!(
                    "PASS: firmware boots, displays pixels, persists matching NOR and selection"
                );
                if self.stage == 3 {
                    let mut configured = panel.options.clone();
                    configured.sd_image = None;
                    crate::configuration::configure_with_package(
                        &mut configured,
                        Some(&panel.options.firmware_dir.join("L6max.bin")),
                    )
                    .map_err(|e| e.to_string())?;
                    if configured.sd_image != panel.options.sd_image {
                        return Err("SD folder selection not restored".into());
                    }
                    let trace = std::fs::read_to_string(panel.options.logs.join("engine.trace"))
                        .map_err(|e| e.to_string())?;
                    for command in ["CMD00", "CMD08", "CMD17"] {
                        if !trace.contains(command) {
                            return Err(format!("Folder card did not receive firmware {command}"));
                        }
                    }
                    println!(
                        "PASS: SD folder snapshot mounted through guest SD commands and selection persists"
                    );
                    return Ok(true);
                }
                if self.stage == 2 {
                    if self.sd_folder.is_some() {
                        self.select_sd_folder(panel)?;
                        return Ok(false);
                    }
                    return Ok(true);
                }
                let original = controls.clone();
                let options = panel.options.clone();
                let bad = bundle.data.join("invalid-test.bin");
                std::fs::write(&bad, b"invalid firmware").map_err(|e| e.to_string())?;
                if panel.load_firmware(&bad).is_ok() {
                    return Err("Invalid firmware accepted".into());
                }
                if !Arc::ptr_eq(&original, panel.controls.as_ref().unwrap())
                    || options.firmware_dir != panel.options.firmware_dir
                {
                    return Err("Rejected import disturbed running guest".into());
                }
                println!("PASS: invalid selection leaves running firmware untouched");
                let previous_pid = panel
                    ._qemu
                    .as_ref()
                    .and_then(|engine| engine.process_id())
                    .ok_or("Missing first QEMU process")?;
                panel.load_firmware(&self.second)?;
                #[cfg(target_os = "macos")]
                if unsafe { libc::kill(previous_pid as i32, 0) } == 0 {
                    return Err("Old QEMU process survived restart".into());
                }
                if Arc::ptr_eq(&original, panel.controls.as_ref().unwrap())
                    || options.state_dir == panel.options.state_dir
                {
                    return Err(
                        "Second selection did not restart into separate device state".into(),
                    );
                }
                println!(
                    "PASS: second firmware selection reaped and restarted the guest with isolated state"
                );
                self.stage = 2;
            }
            _ => unreachable!(),
        }
        Ok(false)
    }
}
