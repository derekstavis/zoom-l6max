//! `cargo run -p qemu-build -- [--minimal] [QEMU_SOURCE]`: print the built binary path.
use qemu_build::{Request, build_locked};
use std::{env, path::PathBuf, process::ExitCode};

fn run() -> Result<(), String> {
    let mut source = None;
    let mut minimal = false;
    for arg in env::args_os().skip(1) {
        if arg == "--minimal" {
            minimal = true;
        } else if !arg.to_string_lossy().starts_with('-') && source.is_none() {
            source = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected argument: {}", arg.to_string_lossy()));
        }
    }
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = build_locked(&Request {
        repository: repository.canonicalize().map_err(|e| e.to_string())?,
        source,
        minimal,
    })?;
    println!("{}", binary.display());
    Ok(())
}
fn main() -> ExitCode {
    if let Err(error) = run() {
        eprintln!("qemu-build: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
