//! Assemble a firmware-free Linux AppDir, and optionally an x86_64 AppImage,
//! from local builds of the GUI and the custom QEMU.
//!
//! Libraries are bundled except those every Linux system provides or that must
//! match the host (C library, graphics, audio, display servers). Each bundled
//! library must belong to a Debian package so its source can be published.
use std::{
    collections::BTreeSet,
    env, fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

/// Libraries left to the host system, after the AppImage project's exclude list.
const HOST_LIBRARIES: &[&str] = &[
    "linux-vdso.so.1",
    "ld-linux-x86-64.so.2",
    "libc.so.6",
    "libm.so.6",
    "libdl.so.2",
    "libpthread.so.0",
    "librt.so.1",
    "libutil.so.1",
    "libresolv.so.2",
    "libnsl.so.1",
    "libcrypt.so.1",
    "libgcc_s.so.1",
    "libstdc++.so.6",
    "libz.so.1",
    "libasound.so.2",
    "libfontconfig.so.1",
    "libfreetype.so.6",
    "libexpat.so.1",
    "libGL.so.1",
    "libEGL.so.1",
    "libGLX.so.0",
    "libGLdispatch.so.0",
    "libOpenGL.so.0",
    "libvulkan.so.1",
    "libdrm.so.2",
    "libgbm.so.1",
    "libX11.so.6",
    "libxcb.so.1",
    "libwayland-client.so.0",
];
const DESKTOP: &str = "[Desktop Entry]
Type=Application
Name=L6max Emulator
Comment=Emulator for the ZOOM LiveTrak L6max mixer
Exec=l6max-gui
Icon=l6max-emulator
Categories=AudioVideo;Audio;Midi;Mixer;
Terminal=false
";
const APP_RUN: &str = "#!/bin/sh
HERE=\"$(dirname \"$(readlink -f \"$0\")\")\"
export LD_LIBRARY_PATH=\"$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}\"
exec \"$HERE/usr/bin/l6max-gui\" \"$@\"
";

fn run(program: &str, arguments: &[&std::ffi::OsStr], directory: &Path) -> Result<String, String> {
    let output = Command::new(program)
        .current_dir(directory)
        .args(arguments)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("copy {}: {e}", from.display()))
}
fn executable(path: &Path) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

/// Shared libraries `binary` needs, as `(soname, resolved path)`.
fn dependencies(binary: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let listing = run("ldd", &[binary.as_os_str()], Path::new("."))?;
    let mut libraries = Vec::new();
    for line in listing.lines().map(str::trim) {
        let Some((name, rest)) = line.split_once(" => ") else {
            continue; // the loader itself, or the vDSO
        };
        if rest.starts_with("not found") {
            return Err(format!(
                "{} needs {name}, which is not installed",
                binary.display()
            ));
        }
        let path = rest.split(" (").next().unwrap_or(rest);
        libraries.push((name.to_owned(), PathBuf::from(path)));
    }
    Ok(libraries)
}
/// Debian package and version owning `path`, trying the resolved file too.
fn package_of(path: &Path) -> Result<(String, String), String> {
    let real = fs::canonicalize(path).map_err(|e| e.to_string())?;
    for candidate in [path, real.as_path()] {
        if let Ok(owner) = run(
            "dpkg",
            &["-S".as_ref(), candidate.as_os_str()],
            Path::new("."),
        ) {
            let package = owner
                .split(':')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
            let version = run(
                "dpkg-query",
                &["-W".as_ref(), "-f=${Version}".as_ref(), package.as_ref()],
                Path::new("."),
            )?;
            return Ok((package, version.trim().to_owned()));
        }
    }
    Err(format!(
        "{} belongs to no Debian package, so its source cannot be published",
        path.display()
    ))
}

fn main() {
    if let Err(error) = package() {
        eprintln!("linux-package: {error}");
        std::process::exit(1);
    }
}

fn package() -> Result<(), String> {
    let mut qemu = None;
    let mut version = "0.1.0".to_owned();
    let mut release = true;
    let mut appimagetool = env::var_os("APPIMAGETOOL").map(PathBuf::from);
    let mut runtime = env::var_os("APPIMAGE_RUNTIME").map(PathBuf::from);
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--debug" => release = false,
            "--version" => version = args.next().ok_or("--version requires a value")?,
            "--appimagetool" => {
                appimagetool = Some(args.next().ok_or("--appimagetool requires a path")?.into())
            }
            "--runtime" => runtime = Some(args.next().ok_or("--runtime requires a path")?.into()),
            value if !value.starts_with('-') && qemu.is_none() => qemu = Some(PathBuf::from(value)),
            other => return Err(format!("unexpected argument: {other}")),
        }
    }
    let usage = "Usage: linux-package PATH_TO_qemu-system-arm [--version X.Y.Z] [--debug] [--appimagetool PATH] [--runtime PATH]";
    let qemu = qemu
        .ok_or(usage)?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let version = version.trim_start_matches('v').to_owned();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|e| e.to_string())?;

    let mut build = vec!["build", "--locked", "-p", "l6max-gui"];
    if release {
        build.push("--release");
    }
    let build: Vec<&std::ffi::OsStr> = build.into_iter().map(std::ffi::OsStr::new).collect();
    run("cargo", &build, &root)?;
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let gui = target
        .join(if release { "release" } else { "debug" })
        .join("l6max-gui");

    let dist = root.join("emulator/dist");
    let app = dist.join("L6max-Emulator.AppDir");
    let _ = fs::remove_dir_all(&app);
    let resources = app.join("usr/share/l6max-emulator");
    copy(&gui, &app.join("usr/bin/l6max-gui"))?;
    copy(&qemu, &resources.join("libexec/qemu-system-arm"))?;
    for path in [
        app.join("usr/bin/l6max-gui"),
        resources.join("libexec/qemu-system-arm"),
    ] {
        executable(&path)?;
    }

    // Bundle every non-host library either executable needs.
    let mut bundled = BTreeSet::new();
    let mut manifest = Vec::new();
    for binary in [&gui, &qemu] {
        for (name, path) in dependencies(binary)? {
            if HOST_LIBRARIES.contains(&name.as_str()) || !bundled.insert(name.clone()) {
                continue;
            }
            let (package, package_version) = package_of(&path)?;
            copy(
                &fs::canonicalize(&path).map_err(|e| e.to_string())?,
                &app.join("usr/lib").join(&name),
            )?;
            manifest.push(serde_json::json!({"library": name, "package": format!("{package} {package_version}")}));
        }
    }
    fs::write(
        resources.join("native-dependencies.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Notices: this project, QEMU, icons, Debian copyright files, Cargo crates.
    let licenses = resources.join("Licenses");
    for name in ["LICENSE", "NOTICE.md"] {
        copy(&root.join(name), &licenses.join(name))?;
    }
    copy(
        &root.join("emulator/qemu/COPYING"),
        &licenses.join("QEMU-COPYING"),
    )?;
    for (folder, label) in [
        ("emulator/gui/assets/icons", "Lucide"),
        ("emulator/gui/assets/icons/phosphor", "Phosphor"),
    ] {
        copy(
            &root.join(folder).join("LICENSE"),
            &licenses.join(format!("{label}-LICENSE")),
        )?;
    }
    for package in manifest
        .iter()
        .filter_map(|m| m["package"].as_str()?.split(' ').next())
        .collect::<BTreeSet<_>>()
    {
        let notice = Path::new("/usr/share/doc").join(package).join("copyright");
        if notice.is_file() {
            copy(
                &notice,
                &licenses.join("Debian").join(format!("{package}-copyright")),
            )?;
        }
    }
    let metadata = run(
        "cargo",
        &[
            "metadata".as_ref(),
            "--locked".as_ref(),
            "--format-version".as_ref(),
            "1".as_ref(),
        ],
        &root,
    )?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata).map_err(|e| e.to_string())?;
    let mut rust = Vec::new();
    for package in metadata["packages"]
        .as_array()
        .ok_or("missing Cargo packages")?
    {
        if package["source"].is_null() {
            continue;
        }
        let (name, number) = (
            package["name"].as_str().unwrap(),
            package["version"].as_str().unwrap(),
        );
        let directory = Path::new(package["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap();
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let label = entry.file_name().to_string_lossy().to_uppercase();
            if entry.path().is_file()
                && ["LICENSE", "COPYING", "NOTICE", "COPYRIGHT"]
                    .iter()
                    .any(|p| label.starts_with(p))
            {
                copy(
                    &entry.path(),
                    &licenses
                        .join("Rust")
                        .join(format!("{name}-{number}"))
                        .join(entry.file_name()),
                )?;
            }
        }
        rust.push(serde_json::json!({"name": name, "version": number, "license": package["license"], "source": package["source"]}));
    }
    fs::write(
        resources.join("rust-dependencies.json"),
        serde_json::to_vec_pretty(&rust).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Launcher, desktop entry and icon.
    fs::write(app.join("AppRun"), APP_RUN).map_err(|e| e.to_string())?;
    executable(&app.join("AppRun"))?;
    fs::write(app.join("l6max-emulator.desktop"), DESKTOP).map_err(|e| e.to_string())?;
    copy(
        &app.join("l6max-emulator.desktop"),
        &app.join("usr/share/applications/l6max-emulator.desktop"),
    )?;
    let icon = root.join("emulator/gui/assets/app-icon/icon.svg");
    copy(&icon, &app.join("l6max-emulator.svg"))?;
    copy(
        &icon,
        &app.join("usr/share/icons/hicolor/scalable/apps/l6max-emulator.svg"),
    )?;
    symlink("l6max-emulator.svg", app.join(".DirIcon")).map_err(|e| e.to_string())?;

    verify(&app)?;
    println!("AppDir: {}", app.display());
    let Some(tool) = appimagetool else {
        println!("No --appimagetool given; AppDir only.");
        return Ok(());
    };
    let output = dist.join("L6max-Emulator-x86_64.AppImage");
    let _ = fs::remove_file(&output);
    let mut command = Command::new(&tool);
    command.args(["--appimage-extract-and-run", "--no-appstream"]);
    // Without a runtime file appimagetool downloads a moving "continuous" build.
    if let Some(runtime) = &runtime {
        command.arg("--runtime-file").arg(runtime);
    }
    let status = command
        .arg(&app)
        .arg(&output)
        .env("ARCH", "x86_64")
        .env("VERSION", &version)
        .status()
        .map_err(|e| format!("appimagetool: {e}"))?;
    if !status.success() {
        return Err(format!("appimagetool exited with {status}"));
    }
    verify_appimage(&output, &dist)?;
    println!("AppImage: {}", output.display());
    Ok(())
}

/// The bundled QEMU must start with only the bundled libraries and know the board.
fn verify(app: &Path) -> Result<(), String> {
    let qemu = app.join("usr/share/l6max-emulator/libexec/qemu-system-arm");
    let libraries = app.join("usr/lib");
    let ldd = Command::new("ldd")
        .arg(&qemu)
        .env("LD_LIBRARY_PATH", &libraries)
        .output()
        .map_err(|e| e.to_string())?;
    let listing = String::from_utf8_lossy(&ldd.stdout);
    if listing.contains("not found") {
        return Err(format!("bundled QEMU has unresolved libraries:\n{listing}"));
    }
    // Only host-provided libraries may resolve outside the AppDir.
    for line in listing.lines().filter_map(|l| l.trim().split_once(" => ")) {
        let path = line.1.split(" (").next().unwrap_or("");
        if !path.starts_with(app.to_str().unwrap()) && !HOST_LIBRARIES.contains(&line.0) {
            return Err(format!("{} resolved outside the AppDir: {path}", line.0));
        }
    }
    let machines = Command::new(&qemu)
        .args(["-machine", "help"])
        .env("LD_LIBRARY_PATH", &libraries)
        .output()
        .map_err(|e| e.to_string())?;
    if !String::from_utf8_lossy(&machines.stdout).contains("l6max-dual") {
        return Err("bundled QEMU does not provide the l6max-dual machine".into());
    }
    Ok(())
}
/// Extract the finished image and repeat the check on what users will run.
fn verify_appimage(image: &Path, scratch: &Path) -> Result<(), String> {
    let work = scratch.join("appimage-verify");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    executable(image)?;
    let status = Command::new(image)
        .arg("--appimage-extract")
        .current_dir(&work)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("the AppImage could not be extracted".into());
    }
    verify(&work.join("squashfs-root"))?;
    fs::remove_dir_all(&work).map_err(|e| e.to_string())
}
