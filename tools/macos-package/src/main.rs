//! Assemble a relocatable, firmware-free macOS app and ZIP from local builds.
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn command(program: &str, arguments: &[&std::ffi::OsStr]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn dependencies(file: &Path) -> Result<Vec<String>, String> {
    let listing = command("otool", &["-L".as_ref(), file.as_os_str()])?;
    Ok(listing
        .lines()
        .skip(1)
        .filter(|line| line.starts_with('\t'))
        .filter_map(|line| {
            line.trim()
                .split(" (compatibility")
                .next()
                .map(str::to_owned)
        })
        .collect())
}

fn resolve(name: &str, source: &Path) -> Result<PathBuf, String> {
    let path = if let Some(relative) = name.strip_prefix("@loader_path/") {
        source.parent().unwrap().join(relative)
    } else if let Some(relative) = name.strip_prefix("@rpath/") {
        let listing = command("otool", &["-l".as_ref(), source.as_os_str()])?;
        let mut roots = Vec::new();
        let mut rpath = false;
        for line in listing.lines() {
            if line.trim() == "cmd LC_RPATH" {
                rpath = true;
            } else if rpath {
                if let Some(value) = line.trim().strip_prefix("path ") {
                    let value = value.split(" (offset").next().unwrap();
                    roots.push(if let Some(p) = value.strip_prefix("@loader_path/") {
                        source.parent().unwrap().join(p)
                    } else {
                        PathBuf::from(value)
                    });
                    rpath = false;
                }
            }
        }
        roots.push(source.parent().unwrap().to_owned());
        roots
            .into_iter()
            .map(|p| p.join(relative))
            .find(|p| p.is_file())
            .ok_or_else(|| format!("Cannot resolve {name} from {}", source.display()))?
    } else {
        PathBuf::from(name)
    };
    path.canonicalize()
        .map_err(|e| format!("Cannot resolve {name}: {e}"))
}

fn deployment_floor(file: &Path) -> Result<(u32, u32, u32), String> {
    let listing = command("otool", &["-l".as_ref(), file.as_os_str()])?;
    Ok(parse_deployment_floor(&listing))
}

fn parse_deployment_floor(listing: &str) -> (u32, u32, u32) {
    let mut floor = (13, 0, 0);
    let mut version_key = None;
    for line in listing.lines().map(str::trim) {
        if line.starts_with("cmd ") {
            version_key = match line {
                "cmd LC_BUILD_VERSION" => Some("minos "),
                "cmd LC_VERSION_MIN_MACOS" => Some("version "),
                _ => None,
            };
        }
        if let Some(value) = version_key.and_then(|key| line.strip_prefix(key)) {
            let mut parts = value.split('.').map(|s| s.parse::<u32>().unwrap_or(0));
            floor = floor.max((
                parts.next().unwrap_or(0),
                parts.next().unwrap_or(0),
                parts.next().unwrap_or(0),
            ));
        }
    }
    floor
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), destination.join(entry.file_name()))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn app_version(value: &str) -> Result<(&str, &str), String> {
    let version = value.strip_prefix('v').unwrap_or(value);
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        return Err("App version must be MAJOR.MINOR.PATCH with an optional suffix".into());
    }
    Ok((version, core))
}

fn run() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("macos-package must run on macOS".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut args = env::args_os().skip(1);
    let qemu = PathBuf::from(args.next().ok_or(
        "Usage: macos-package QEMU_BINARY [--debug] [--output DIRECTORY] [--version VERSION]",
    )?)
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let mut debug = false;
    let mut out = root.join("emulator/dist");
    let mut version = env!("CARGO_PKG_VERSION").to_owned();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--debug") => debug = true,
            Some("--output") => {
                out = PathBuf::from(args.next().ok_or("--output requires a directory")?)
            }
            Some("--version") => {
                version = args
                    .next()
                    .ok_or("--version requires a version")?
                    .into_string()
                    .map_err(|_| "Non-UTF8 app version")?;
            }
            _ => return Err(format!("Unknown option: {}", arg.to_string_lossy())),
        }
    }
    let (version, bundle_version) = app_version(&version)?;
    let app = out.join("L6max Emulator.app");
    let zip = out.join("L6max-Emulator-macos.zip");
    if app.exists() || zip.exists() {
        return Err("Output app/ZIP already exists; choose a new --output directory".into());
    }
    let machines = command(
        qemu.to_str().ok_or("Non-UTF8 QEMU path")?,
        &["-machine".as_ref(), "help".as_ref()],
    )?;
    if !machines.lines().any(|line| line.starts_with("l6max-dual")) {
        return Err("QEMU binary is missing l6max-dual; build the custom board first".into());
    }
    let mut build = Command::new("cargo");
    build
        .current_dir(root)
        .args(["build", "--locked", "-p", "l6max-gui"]);
    if !debug {
        build.arg("--release");
    }
    if !build.status().map_err(|e| e.to_string())?.success() {
        return Err("GUI build failed".into());
    }
    let gui = root.join(if debug {
        "target/debug/l6max-gui"
    } else {
        "target/release/l6max-gui"
    });
    let contents = app.join("Contents");
    let resources = contents.join("Resources");
    let frameworks = contents.join("Frameworks");
    fs::create_dir_all(contents.join("MacOS")).map_err(|e| e.to_string())?;
    fs::create_dir_all(resources.join("libexec")).map_err(|e| e.to_string())?;
    fs::create_dir_all(&frameworks).map_err(|e| e.to_string())?;
    let gui_copy = contents.join("MacOS/l6max-gui");
    let qemu_copy = resources.join("libexec/qemu-system-arm");
    fs::copy(&gui, &gui_copy).map_err(|e| e.to_string())?;
    fs::copy(&qemu, &qemu_copy).map_err(|e| e.to_string())?;
    let mut files = vec![(gui, gui_copy.clone()), (qemu, qemu_copy.clone())];
    let mut libraries: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut index = 0;
    while index < files.len() {
        let (source, copy) = files[index].clone();
        for name in dependencies(&source)? {
            if name.starts_with("/System/Library/") || name.starts_with("/usr/lib/") {
                continue;
            }
            let original = resolve(&name, &source)?;
            if original == source {
                continue;
            } // LC_ID_DYLIB
            let destination = if let Some(destination) = libraries.get(&original) {
                destination.clone()
            } else {
                let filename = Path::new(&name).file_name().ok_or("Missing library name")?;
                let destination = frameworks.join(filename);
                if destination.exists() {
                    return Err(format!(
                        "Conflicting library name: {}",
                        filename.to_string_lossy()
                    ));
                }
                fs::copy(&original, &destination).map_err(|e| e.to_string())?;
                libraries.insert(original.clone(), destination.clone());
                files.push((original, destination.clone()));
                destination
            };
            let prefix = if copy == gui_copy {
                "@loader_path/../Frameworks/"
            } else if copy == qemu_copy {
                "@loader_path/../../Frameworks/"
            } else {
                "@loader_path/"
            };
            let relocated = format!(
                "{prefix}{}",
                destination.file_name().unwrap().to_string_lossy()
            );
            command(
                "install_name_tool",
                &[
                    "-change".as_ref(),
                    name.as_ref(),
                    relocated.as_ref(),
                    copy.as_os_str(),
                ],
            )?;
        }
        if copy.parent() == Some(frameworks.as_path()) {
            let id = format!(
                "@loader_path/{}",
                copy.file_name().unwrap().to_string_lossy()
            );
            command(
                "install_name_tool",
                &["-id".as_ref(), id.as_ref(), copy.as_os_str()],
            )?;
        }
        index += 1;
    }
    // Include the project's notices and reproducible model sources with the app.
    let licenses = resources.join("Licenses");
    fs::create_dir_all(&licenses).map_err(|e| e.to_string())?;
    for name in ["LICENSE", "NOTICE.md"] {
        fs::copy(root.join(name), licenses.join(name)).map_err(|e| e.to_string())?;
    }
    fs::copy(
        root.join("emulator/qemu/COPYING"),
        licenses.join("QEMU-COPYING"),
    )
    .map_err(|e| e.to_string())?;
    for (folder, label) in [
        ("emulator/gui/assets/icons", "Lucide"),
        ("emulator/gui/assets/icons/phosphor", "Phosphor"),
    ] {
        fs::copy(
            root.join(folder).join("LICENSE"),
            licenses.join(format!("{label}-LICENSE")),
        )
        .map_err(|e| e.to_string())?;
    }
    copy_tree(
        &root.join("emulator/qemu/hw"),
        &resources.join("Source/qemu-model/hw"),
    )?;
    // Retain dependency attribution in the binary distribution, too.
    let metadata_output = Command::new("cargo")
        .current_dir(root)
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .output()
        .map_err(|e| e.to_string())?;
    if !metadata_output.status.success() {
        return Err("Could not collect Cargo dependency notices".into());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata_output.stdout).map_err(|e| e.to_string())?;
    let mut rust_dependencies = Vec::new();
    for package in metadata["packages"]
        .as_array()
        .ok_or("Missing Cargo packages")?
    {
        if package["source"].is_null() {
            continue;
        }
        let name = package["name"].as_str().unwrap();
        let version = package["version"].as_str().unwrap();
        let directory = Path::new(package["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap();
        let destination = licenses.join("Rust").join(format!("{name}-{version}"));
        fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let label = entry.file_name().to_string_lossy().to_uppercase();
            if entry.path().is_file()
                && (label.starts_with("LICENSE")
                    || label.starts_with("COPYING")
                    || label.starts_with("NOTICE")
                    || label.starts_with("COPYRIGHT"))
            {
                fs::copy(entry.path(), destination.join(entry.file_name()))
                    .map_err(|e| e.to_string())?;
            }
        }
        rust_dependencies.push(serde_json::json!({"name":name,"version":version,"license":package["license"],"source":package["source"]}));
    }
    fs::write(
        resources.join("rust-dependencies.json"),
        serde_json::to_vec_pretty(&rust_dependencies).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    // Homebrew installs generally retain license text beside INSTALL_RECEIPT.json.
    let mut manifest = Vec::new();
    for (source, copy) in &libraries {
        let mut origin = None;
        if let Some(cellar) = source
            .ancestors()
            .find(|p| p.join("INSTALL_RECEIPT.json").exists())
        {
            let formula = cellar
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy();
            origin = Some(format!(
                "{formula} {}",
                cellar.file_name().unwrap().to_string_lossy()
            ));
            let target = licenses.join(formula.as_ref());
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            for entry in fs::read_dir(cellar).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let label = entry.file_name().to_string_lossy().to_uppercase();
                if entry.path().is_file()
                    && (label.starts_with("COPYING")
                        || label.starts_with("LICENSE")
                        || label.starts_with("COPYRIGHT"))
                {
                    fs::copy(entry.path(), target.join(entry.file_name()))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        manifest.push(serde_json::json!({"library":copy.file_name().unwrap().to_string_lossy(),"package":origin}));
    }
    fs::write(
        resources.join("native-dependencies.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    fs::write(resources.join("Source/README.txt"),"QEMU upstream version: 11.1.2. Custom model sources are included here.\nTagged releases provide matching QEMU, Rust, and native dependency sources in the companion sources archive. This directory alone is not a complete corresponding-source archive.\n").map_err(|e|e.to_string())?;
    fs::write(resources.join("build-version.txt"), format!("{version}\n"))
        .map_err(|e| e.to_string())?;
    fs::copy(
        root.join("emulator/gui/assets/app-icon/L6max.icns"),
        resources.join("L6max.icns"),
    )
    .map_err(|e| e.to_string())?;
    let mut floor = (13, 0, 0);
    for (_, file) in &files {
        floor = floor.max(deployment_floor(file)?);
    }
    let minimum_os = format!("{}.{}.{}", floor.0, floor.1, floor.2);
    fs::write(contents.join("Info.plist"),format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>L6max Emulator</string>
<key>CFBundleDisplayName</key><string>L6max Emulator</string>
<key>CFBundleIdentifier</key><string>org.l6max.emulator</string>
<key>CFBundleExecutable</key><string>l6max-gui</string>
<key>CFBundleIconFile</key><string>L6max.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{bundle_version}</string>
<key>CFBundleVersion</key><string>{bundle_version}</string>
<key>LSMinimumSystemVersion</key><string>{minimum_os}</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
"#)).map_err(|e|e.to_string())?;
    for (_, copy) in files.iter().rev() {
        command(
            "codesign",
            &[
                "--force".as_ref(),
                "--sign".as_ref(),
                "-".as_ref(),
                copy.as_os_str(),
            ],
        )?;
    }
    command(
        "codesign",
        &[
            "--force".as_ref(),
            "--sign".as_ref(),
            "-".as_ref(),
            app.as_os_str(),
        ],
    )?;
    command(
        "codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            app.as_os_str(),
        ],
    )?;
    // A relocated binary must work without the build machine's loader paths.
    command(
        qemu_copy.to_str().ok_or("Non-UTF8 bundle path")?,
        &["-machine".as_ref(), "help".as_ref()],
    )?;
    command(
        "ditto",
        &[
            "-c".as_ref(),
            "-k".as_ref(),
            "--sequesterRsrc".as_ref(),
            "--keepParent".as_ref(),
            app.as_os_str(),
            zip.as_os_str(),
        ],
    )?;
    println!(
        "App: {}\nZIP: {}\nLocal ad-hoc signature; not notarized. No firmware is included.",
        app.display(),
        zip.display()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("macos-package: {error}");
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_version_is_safe_for_plist_and_retains_prerelease_identity() {
        assert_eq!(app_version("v1.2.3-beta.1"), Ok(("1.2.3-beta.1", "1.2.3")));
        assert_eq!(app_version("0.1.0"), Ok(("0.1.0", "0.1.0")));
        assert!(app_version("1.2").is_err());
        assert!(app_version("1.2.3</string>").is_err());
    }
    #[test]
    fn deployment_target_ignores_compiler_and_sdk_versions() {
        let listing = "cmd LC_BUILD_VERSION\nminos 15.4\nsdk 27.0\nntools 1\ntool 3\nversion 27037.1\ncmd LC_SOURCE_VERSION\nversion 27037.1\ncmd LC_VERSION_MIN_MACOS\nversion 14.2\nsdk 27.0\n";
        assert_eq!(parse_deployment_floor(listing), (15, 4, 0));
        assert_eq!(
            parse_deployment_floor("cmd LC_BUILD_VERSION\nminos 11.0\nversion 27037.1\n"),
            (13, 0, 0)
        );
    }
}
