//! First-run preparation: locate or build QEMU, and make firmware images
//! available from an `L6max.bin` update package.
//!
//! Both steps are skipped when the caller supplied the input explicitly, and by
//! packaged apps (`Options::auto_setup == false`), which carry their own QEMU and
//! import firmware through the UI.
use crate::{engine::Options, paths::repository_root};
use qemu_build::{Request, build_locked, can_build, default_binary, is_stale};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A QEMU already present: the macOS scratch build, then the staged build.
pub fn existing_qemu() -> Option<PathBuf> {
    [
        PathBuf::from("/private/tmp/l6-qemu-src/build/qemu-system-arm"),
        default_binary(repository_root()),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Packages looked for when none is given: `L6max.bin`, then `firmware/L6max.bin`,
/// in `root`.
pub fn package_candidates(root: &Path) -> [PathBuf; 2] {
    [root.join("L6max.bin"), root.join("firmware/L6max.bin")]
}

fn has_images(directory: &Path) -> bool {
    ["main_firmware.bin", "secondary_firmware.bin"]
        .iter()
        .all(|name| directory.join(name).is_file())
}

/// Unpack `package` into `cache/<package SHA-256>` once and return that directory.
pub fn unpack_cached(package: &Path, cache: &Path) -> Result<PathBuf, String> {
    let bytes = fs::read(package).map_err(|e| format!("cannot read {}: {e}", package.display()))?;
    l6fw::check_layout(&bytes).map_err(|e| {
        format!(
            "{} is not a supported L6max update package: {e}",
            package.display()
        )
    })?;
    let directory = cache.join(format!("{:x}", Sha256::digest(&bytes)));
    if has_images(&directory) {
        return Ok(directory);
    }
    eprintln!("Unpacking {}...", package.display());
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    // Unpack beside the target, then rename, so an interrupted run leaves no
    // half-written directory that would be mistaken for a finished one.
    let partial = cache.join(format!(".partial-{}", std::process::id()));
    let _ = fs::remove_dir_all(&partial);
    l6fw::unpack(package, &partial)?;
    if directory.exists() {
        let _ = fs::remove_dir_all(&directory);
    }
    fs::rename(&partial, &directory).map_err(|e| e.to_string())?;
    Ok(directory)
}

const MISSING_FIRMWARE: &str = "no firmware found. Supply your own L6max 1.10 update package \
(not distributed with this project; ZOOM publishes it on the L6max support page) by any of: \
placing L6max.bin in the repository root, `--firmware-package /path/to/L6max.bin`, the \
L6_FIRMWARE environment variable, or `--firmware-dir` with an unpacked package";

impl Options {
    /// Fill in what the user did not provide, building or unpacking as needed.
    /// Safe to call repeatedly.
    pub fn prepare(&mut self) -> Result<(), String> {
        let root = repository_root();
        self.prepare_qemu(root)?;
        self.prepare_firmware(root)
    }

    fn prepare_qemu(&mut self, root: &Path) -> Result<(), String> {
        let staged = default_binary(root);
        match &self.qemu {
            // A rebuilt board model must not silently run against an old binary.
            Some(path) if *path == staged && can_build(root) && is_stale(root, path) => {
                eprintln!("The L6max board model changed; rebuilding QEMU...");
                if let Err(error) = build_locked(&Request {
                    repository: root.to_path_buf(),
                    source: None,
                    minimal: false,
                }) {
                    eprintln!("Rebuilding QEMU failed ({error}); using the existing build.");
                }
            }
            Some(_) => {}
            None if can_build(root) => {
                eprintln!(
                    "Building QEMU with the L6max board (first run only; this takes several minutes)..."
                );
                self.qemu = Some(
                    build_locked(&Request {
                        repository: root.to_path_buf(),
                        source: None,
                        minimal: false,
                    })
                    .map_err(|e| format!("could not build QEMU: {e}"))?,
                );
            }
            None => {
                return Err("QEMU was not found. Pass --qemu PATH or set L6_QEMU".into());
            }
        }
        Ok(())
    }

    fn prepare_firmware(&mut self, root: &Path) -> Result<(), String> {
        if self.firmware_package.is_none() {
            self.firmware_package = package_candidates(root).into_iter().find(|p| p.is_file());
        }
        let default_directory = root.join("unpacked");
        if has_images(&self.firmware_dir) {
            return Ok(());
        }
        // An explicit directory is never replaced behind the user's back.
        if self.firmware_dir != default_directory {
            return Err(format!(
                "{} does not contain main_firmware.bin and secondary_firmware.bin",
                self.firmware_dir.display()
            ));
        }
        let package = self.firmware_package.clone().ok_or(MISSING_FIRMWARE)?;
        self.firmware_dir = unpack_cached(&package, &root.join("emulator/state/firmware"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("l6-setup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_file_that_is_not_a_package_is_rejected_with_its_name() {
        let dir = temp("reject");
        let bogus = dir.join("L6max.bin");
        fs::write(&bogus, b"not firmware").unwrap();
        let error = unpack_cached(&bogus, &dir.join("cache")).unwrap_err();
        assert!(error.contains("L6max.bin") && error.contains("not a supported"));
        assert!(!dir.join("cache").exists(), "nothing is cached on failure");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn package_candidates_are_the_documented_locations() {
        let candidates = package_candidates(Path::new("/repo"));
        assert_eq!(candidates[0], Path::new("/repo/L6max.bin"));
        assert_eq!(candidates[1], Path::new("/repo/firmware/L6max.bin"));
    }

    #[test]
    fn explicit_empty_firmware_directory_is_an_error_not_a_fallback() {
        let dir = temp("explicit");
        let mut options = Options::parse_args(["--no-auto-setup".into()]).unwrap();
        options.firmware_dir = dir.clone();
        let error = options.prepare_firmware(repository_root()).unwrap_err();
        assert!(error.contains("does not contain main_firmware.bin"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn existing_images_need_no_package() {
        let dir = temp("images");
        for name in ["main_firmware.bin", "secondary_firmware.bin"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let mut options = Options::parse_args([]).unwrap();
        options.firmware_dir = dir.clone();
        options.prepare_firmware(repository_root()).unwrap();
        assert_eq!(options.firmware_dir, dir);
        fs::remove_dir_all(dir).unwrap();
    }
}
