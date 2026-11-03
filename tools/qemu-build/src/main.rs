//! `cargo run -p qemu-build -- [--minimal] [--plugins] [--optimized] [QEMU_SOURCE]`:
//! print the built binary path.
//! `cargo run -p qemu-build -- --train-pgo --config FILE`: refresh this platform's profile.
mod train;

use qemu_build::{Optimization, Request, build_locked};
use std::{env, path::PathBuf, process::ExitCode};

fn run() -> Result<(), String> {
    let mut source = None;
    let mut minimal = false;
    let mut plugins = false;
    let mut optimization = Optimization::Standard;
    let mut train_config = None;
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--minimal" {
            minimal = true;
        } else if arg == "--plugins" {
            plugins = true;
        } else if arg == "--optimized" {
            optimization = Optimization::Pgo;
        } else if arg == "--train-pgo" {
            optimization = Optimization::Instrumented;
        } else if arg == "--config" {
            train_config = Some(PathBuf::from(
                args.next().ok_or("--config requires a file")?,
            ));
        } else if !arg.to_string_lossy().starts_with('-') && source.is_none() {
            source = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected argument: {}", arg.to_string_lossy()));
        }
    }
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if optimization == Optimization::Instrumented {
        let config =
            train_config.ok_or("--train-pgo requires --config FILE with local firmware")?;
        if plugins || minimal || source.is_some() {
            return Err("--train-pgo trains the default pinned build only".into());
        }
        let profile = train::run(&repository, &config)?;
        println!("{}", profile.display());
        return Ok(());
    }
    if train_config.is_some() {
        return Err("--config is only used with --train-pgo".into());
    }
    let binary = build_locked(&Request {
        repository,
        source,
        minimal,
        plugins,
        optimization,
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
