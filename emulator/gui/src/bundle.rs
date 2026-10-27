//! Firmware import and writable paths for a relocatable application: a macOS
//! `.app` bundle or a Linux AppDir (as mounted from an AppImage).
use l6max_host::engine::Options;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Bundle {
    pub resources: PathBuf,
    pub data: PathBuf,
}

impl Bundle {
    pub fn detect() -> Result<Option<Self>, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let Some(resources) = resources_for(&executable) else {
            return Ok(None);
        };
        let data = match std::env::var_os("L6_APP_DATA") {
            Some(path) => PathBuf::from(path),
            None => default_data_directory()?,
        };
        fs::create_dir_all(&data).map_err(|e| e.to_string())?;
        Ok(Some(Self { resources, data }))
    }

    pub fn restore(&self, options: &mut Options) -> Result<bool, String> {
        options.qemu = None;
        options.logs = self.data.join("logs");
        let marker = self.data.join("selected-firmware.txt");
        if !marker.exists() {
            return Ok(false);
        }
        let hash = fs::read_to_string(marker).map_err(|e| e.to_string())?;
        self.configure(&hash, options)?;
        Ok(true)
    }

    fn configure(&self, hash: &str, options: &mut Options) -> Result<(), String> {
        if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("Invalid saved firmware selection; select the package again".into());
        }
        let firmware = self.data.join("firmware").join(hash);
        for name in ["main_firmware.bin", "secondary_firmware.bin"] {
            if !firmware.join(name).is_file() {
                return Err("Saved firmware is incomplete; select the package again".into());
            }
        }
        if !firmware.join("L6.bin").is_file() && !firmware.join("L6max.bin").is_file() {
            return Err("Saved firmware package is missing; select it again".into());
        }
        let qemu = self.resources.join("libexec/qemu-system-arm");
        if !qemu.is_file() {
            return Err("The application bundle is missing QEMU".into());
        }
        options.qemu = Some(qemu);
        options.firmware_dir = firmware;
        options.state_dir = Some(self.data.join("devices").join(hash));
        options.logs = self.data.join("logs").join(hash);
        options.sd_image = None;
        options.no_sd = false;
        Ok(())
    }

    /// Validate before changing the running guest. Only complete imports are remembered.
    pub fn import(&self, package: &Path, options: &mut Options) -> Result<(), String> {
        let mut file = fs::File::open(package).map_err(|e| format!("Cannot read firmware: {e}"))?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        if !matches!(size, 0x1a8200 | 0x395200) {
            return Err("Select a supported L6 or L6max update package".into());
        }
        use std::io::Read;
        let mut bytes = Vec::with_capacity(size as usize);
        file.by_ref()
            .take(size + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let model = l6fw::model_from_package(&bytes)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let parent = self.data.join("firmware");
        fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        let destination = parent.join(&hash);
        if !destination.exists() {
            let temporary = parent.join(format!(".import-{}", std::process::id()));
            if temporary.exists() {
                fs::remove_dir_all(&temporary).map_err(|e| e.to_string())?
            }
            // Snapshot the selected file so extraction and hashing use the same bytes.
            let snapshot = parent.join(format!(".package-{}.bin", std::process::id()));
            fs::write(&snapshot, &bytes).map_err(|e| e.to_string())?;
            let result = l6fw::unpack(&snapshot, &temporary).and_then(|()| {
                let name = match model {
                    l6fw::Model::L6 => "L6.bin",
                    l6fw::Model::L6max => "L6max.bin",
                };
                fs::rename(&snapshot, temporary.join(name)).map_err(|e| e.to_string())?;
                fs::rename(&temporary, &destination).map_err(|e| e.to_string())
            });
            if result.is_err() {
                let _ = fs::remove_file(&snapshot);
                let _ = fs::remove_dir_all(&temporary);
            }
            result?;
        }
        self.configure(&hash, options)
    }

    pub fn remember(&self, options: &Options) -> Result<(), String> {
        let hash = options
            .firmware_dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("Missing firmware identity")?;
        let temporary = self.data.join("selected-firmware.tmp");
        fs::write(&temporary, hash).map_err(|e| e.to_string())?;
        fs::rename(temporary, self.data.join("selected-firmware.txt")).map_err(|e| e.to_string())
    }
}

/// Return the packaged update file matching the extracted MAIN image.
pub fn selected_package(firmware_dir: &Path) -> Result<PathBuf, String> {
    let main = fs::read(firmware_dir.join("main_firmware.bin"))
        .map_err(|error| format!("Cannot read selected MAIN firmware: {error}"))?;
    let filename = match l6fw::model_from_main(&main)? {
        l6fw::Model::L6 => "L6.bin",
        l6fw::Model::L6max => "L6max.bin",
    };
    let package = firmware_dir.join(filename);
    if !package.is_file() {
        return Err(format!("Selected firmware package is missing: {filename}"));
    }
    Ok(package)
}

pub fn verify_bundled_qemu(resources: &Path, expected_sha256: &str) -> Result<PathBuf, String> {
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("A 64-character bundled QEMU SHA-256 is required".into());
    }
    let qemu = resources.join("libexec/qemu-system-arm");
    let bytes = fs::read(&qemu).map_err(|error| format!("Cannot read bundled QEMU: {error}"))?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(format!(
            "Bundled QEMU hash mismatch: expected {expected_sha256}, got {actual}"
        ));
    }
    Ok(qemu)
}

/// Writable per-user storage: Application Support on macOS, XDG data elsewhere.
fn default_data_directory() -> Result<PathBuf, String> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Ok(home()
            .ok_or("Cannot locate Application Support")?
            .join("Library/Application Support/L6max Emulator"));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home().map(|home| home.join(".local/share")))
        .ok_or("Cannot locate the user data directory")?;
    Ok(base.join("l6max-emulator"))
}

/// Read-only resources next to the executable, or `None` for a development run.
fn resources_for(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    let parent = directory.parent()?;
    // macOS: L6max Emulator.app/Contents/MacOS/l6max-gui
    if directory.file_name()? == "MacOS" && parent.file_name()? == "Contents" {
        return Some(parent.join("Resources"));
    }
    // Linux AppDir: <root>/usr/bin/l6max-gui, resources in <root>/usr/share/l6max-emulator.
    // Requiring the bundled QEMU keeps a system-installed `/usr/bin` from matching.
    if directory.file_name()? == "bin" && parent.file_name()? == "usr" {
        let resources = parent.join("share/l6max-emulator");
        if resources.join("libexec/qemu-system-arm").is_file() {
            return Some(resources);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_bundle_without_checkout_paths() {
        assert_eq!(
            resources_for(Path::new(
                "/Applications/L6max Emulator.app/Contents/MacOS/l6max-gui"
            )),
            Some(PathBuf::from(
                "/Applications/L6max Emulator.app/Contents/Resources"
            ))
        );
        assert_eq!(
            resources_for(Path::new("/tmp/target/debug/l6max-gui")),
            None
        );
    }

    #[test]
    fn selected_package_follows_the_main_reset_vector_when_both_files_exist() {
        let dir = std::env::temp_dir().join(format!("l6-bundle-model-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (reset, expected) in [(0x8000_1415u32, "L6.bin"), (0x8000_0415, "L6max.bin")] {
            let mut main = vec![0; 8];
            main[..4].copy_from_slice(&0x2001_0000u32.to_le_bytes());
            main[4..].copy_from_slice(&reset.to_le_bytes());
            fs::write(dir.join("main_firmware.bin"), main).unwrap();
            fs::write(dir.join("L6.bin"), b"L6 package").unwrap();
            fs::write(dir.join("L6max.bin"), b"L6max package").unwrap();
            assert_eq!(selected_package(&dir).unwrap(), dir.join(expected));
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn recognizes_an_appdir_only_with_bundled_qemu() {
        let root = std::env::temp_dir().join(format!("l6-appdir-{}", std::process::id()));
        let resources = root.join("usr/share/l6max-emulator");
        let executable = root.join("usr/bin/l6max-gui");
        let _ = fs::remove_dir_all(&root);
        assert_eq!(resources_for(&executable), None);
        fs::create_dir_all(resources.join("libexec")).unwrap();
        fs::write(resources.join("libexec/qemu-system-arm"), b"").unwrap();
        assert_eq!(resources_for(&executable), Some(resources));
        assert_eq!(resources_for(Path::new("/usr/bin/l6max-gui")), None);
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn bundled_qemu_verification_requires_the_exact_pinned_binary() {
        let root = std::env::temp_dir().join(format!("l6-bundle-qemu-{}", std::process::id()));
        let resources = root.join("Resources");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(resources.join("libexec")).unwrap();
        let qemu = resources.join("libexec/qemu-system-arm");
        fs::write(&qemu, b"pinned bundled qemu").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"pinned bundled qemu"));
        assert_eq!(verify_bundled_qemu(&resources, &expected).unwrap(), qemu);
        assert!(verify_bundled_qemu(&resources, &"0".repeat(64)).is_err());
        assert!(verify_bundled_qemu(&resources, "not-a-sha").is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rejects_invalid_package_without_changing_selection() {
        let root = std::env::temp_dir().join(format!("l6-bundle-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let bundle = Bundle {
            resources: root.join("Resources"),
            data: root.clone(),
        };
        let bad = root.join("invalid.bin");
        fs::write(&bad, b"not firmware").unwrap();
        let mut options = Options::parse_args([]).unwrap();
        let before = options.firmware_dir.clone();
        assert!(bundle.import(&bad, &mut options).is_err());
        assert_eq!(options.firmware_dir, before);
        assert!(!root.join("selected-firmware.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
