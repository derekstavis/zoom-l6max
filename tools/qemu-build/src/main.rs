use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Command, ExitCode},
};

const MESON_ENTRY: &str = "arm_common_ss.add(files('l6max.c'))";

fn copy_source(source: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == ".git" || entry.file_name() == "build" {
            continue;
        }
        let destination = target.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            // QEMU has directory links and links into generated build files;
            // preserve them without following or trying to copy their targets.
            let link = fs::read_link(entry.path()).map_err(|e| e.to_string())?;
            if fs::read_link(&destination).ok().as_ref() == Some(&link) {
                continue;
            }
            if fs::symlink_metadata(&destination).is_ok() {
                fs::remove_file(&destination).map_err(|e| e.to_string())?;
            }
            std::os::unix::fs::symlink(link, destination).map_err(|e| e.to_string())?;
        } else if kind.is_dir() {
            copy_source(&entry.path(), &destination)?;
        } else {
            if destination.is_file()
                && fs::read(entry.path()).map_err(|e| e.to_string())?
                    == fs::read(&destination).map_err(|e| e.to_string())?
            {
                continue;
            }
            fs::copy(entry.path(), destination).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    if let Err(error) = run() {
        eprintln!("qemu-build: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run() -> Result<(), String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let upstream = manifest.join("../../emulator/qemu/upstream");
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

    let source = source
        .unwrap_or_else(|| upstream.clone())
        .canonicalize()
        .map_err(|error| format!("cannot resolve QEMU source: {error}"))?;
    let version = fs::read_to_string(source.join("VERSION"))
        .map_err(|error| format!("cannot read QEMU VERSION: {error}"))?;
    if version.trim() != "11.1.2" {
        return Err(format!("expected QEMU 11.1.2, found {}", version.trim()));
    }

    // Keep the pinned submodule pristine. Patch an ignored staging tree and
    // retain its build directory for incremental rebuilds.
    let source = if upstream.canonicalize().ok().as_ref() == Some(&source) {
        let staged = manifest.join("../../emulator/qemu/build-source");
        copy_source(&source, &staged)?;
        staged.canonicalize().map_err(|e| e.to_string())?
    } else {
        source
    };
    let board = manifest.join("../../emulator/qemu/hw/arm/l6max.c");
    fs::copy(&board, source.join("hw/arm/l6max.c"))
        .map_err(|error| format!("cannot install board source: {error}"))?;
    fs::copy(
        manifest.join("../../emulator/qemu/hw/arm/l6max-display.h"),
        source.join("hw/arm/l6max-display.h"),
    )
    .map_err(|error| format!("cannot install display model: {error}"))?;
    fs::copy(
        manifest.join("../../emulator/qemu/hw/arm/l6max-indicators.h"),
        source.join("hw/arm/l6max-indicators.h"),
    )
    .map_err(|error| format!("cannot install indicator model: {error}"))?;
    fs::copy(
        manifest.join("../../emulator/qemu/hw/arm/l6max-usb.h"),
        source.join("hw/arm/l6max-usb.h"),
    )
    .map_err(|error| format!("cannot install USB model: {error}"))?;
    let meson_path = source.join("hw/arm/meson.build");
    let meson = fs::read_to_string(&meson_path)
        .map_err(|error| format!("cannot read ARM Meson file: {error}"))?;
    if !meson.lines().any(|line| line.trim() == MESON_ENTRY) {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&meson_path)
            .map_err(|error| format!("cannot update ARM Meson file: {error}"))?;
        writeln!(file, "\n{MESON_ENTRY}")
            .map_err(|error| format!("cannot append ARM Meson entry: {error}"))?;
    }

    let mut configure = Command::new(source.join("configure"));
    configure.current_dir(&source).args([
        "--target-list=arm-softmmu",
        "--disable-docs",
        "--disable-gtk",
        "--disable-sdl",
        "--disable-cocoa",
        "--disable-vnc",
        "--disable-tools",
        "--disable-werror",
        "--enable-fdt=system",
    ]);
    // CI bundles need TCG and the board's peripherals, without unrelated
    // optional host services discovered from the runner's installed libraries.
    if minimal {
        configure.args(["--without-default-features", "--enable-tcg"]);
    }
    if cfg!(target_os = "macos") {
        let mut cflags = env::var("CFLAGS").unwrap_or_default();
        cflags.push_str(" -I/opt/homebrew/include");
        let mut ldflags = env::var("LDFLAGS").unwrap_or_default();
        ldflags.push_str(" -L/opt/homebrew/lib");
        configure.env("CFLAGS", cflags).env("LDFLAGS", ldflags);
    }
    let status = configure
        .status()
        .map_err(|error| format!("could not run QEMU configure: {error}"))?;
    if !status.success() {
        return Err(format!("QEMU configure exited with {status}"));
    }
    let status = Command::new("ninja")
        .current_dir(&source)
        .args(["-C", "build", "qemu-system-arm"])
        .status()
        .map_err(|error| format!("could not run Ninja: {error}"))?;
    if !status.success() {
        return Err(format!("QEMU build exited with {status}"));
    }
    println!("{}", source.join("build/qemu-system-arm").display());
    Ok(())
}
