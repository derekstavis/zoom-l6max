//! Development defaults anchored to the host crate, independent of the caller's cwd.
use std::path::Path;

pub fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate resides in emulator/host")
}
