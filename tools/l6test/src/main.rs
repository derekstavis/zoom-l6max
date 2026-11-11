//! Reproducible suite driver. All guest runners are supervised child processes.
mod gui_capture;
mod gui_probe;
mod screen_gif;

use l6max_host::engine::Options;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use smoke_core::{CaseResult, Scenario, model_name};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{Read, Seek, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// What the driver runs with: the configuration's firmware packages, and the QEMU
/// and packaged app chosen for this run.
struct Config {
    firmware: BTreeMap<String, PathBuf>,
    qemu: PathBuf,
    qemu_sha256: String,
    app: Option<App>,
}
/// A packaged application, for the cases that test one.
struct App {
    executable: PathBuf,
    qemu_sha256: String,
}
/// What one case runs from: the configured firmware and its scenario's declarations.
#[derive(Clone)]
struct Fixture {
    name: String,
    model: String,
    firmware_package: Option<PathBuf>,
    patches: Vec<String>,
    installed_patches: Vec<String>,
    sd_patches: Option<Vec<String>>,
    sd_bundle_mode: Option<String>,
    sd_bundle: Option<PathBuf>,
    update_package: Option<PathBuf>,
    state_spec: Option<PathBuf>,
    card_spec: Option<PathBuf>,
    audio_device: Option<String>,
    audio_input: Vec<smoke_core::AudioInput>,
    gdb: bool,
}
static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn interrupt(_: libc::c_int) {
    CANCELLED.store(true, std::sync::atomic::Ordering::Relaxed);
}
fn cancelled() -> bool {
    CANCELLED.load(std::sync::atomic::Ordering::Relaxed)
}
fn audio_device_spec(fixture: &Fixture, logs: &Path) -> String {
    fixture
        .audio_device
        .as_deref()
        .unwrap_or("driver=none,id=l6-audio")
        .replace("{logs}", &logs.display().to_string())
}
fn gdb_socket_path() -> PathBuf {
    // Cases may run concurrently; each fixture copy needs its own socket.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    env::temp_dir().join(format!("l6gdb-{}-{index}.sock", std::process::id()))
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFailure {
    model: String,
    step: usize,
    contains: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    version: u32,
    case: Vec<Case>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    /// Known failures at an exact step and error. A pass becomes XPASS.
    #[serde(default)]
    expected_failure: Vec<ExpectedFailure>,
    id: String,
    suites: Vec<String>,
    models: Vec<String>,
    scenario: Option<PathBuf>,
    package: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    #[serde(default)]
    needs_configuration: bool,
    #[serde(default)]
    gui: bool,
    #[serde(default)]
    volatile: bool,
    #[serde(default)]
    no_sd: bool,
    blocked: Option<String>,
    #[serde(default = "case_timeout")]
    wall_timeout_s: u64,
}
fn case_timeout() -> u64 {
    900
}
struct Args {
    command: String,
    config: PathBuf,
    suite: String,
    case: Option<String>,
    backend: String,
    model: String,
    output: Option<PathBuf>,
    build: bool,
    jobs: usize,
    qemu: Option<PathBuf>,
    app: Option<PathBuf>,
}
/// Each guest runs one single-threaded TCG engine plus its scenario process.
/// Leave headroom so concurrent guests keep up with real time.
fn default_jobs() -> usize {
    thread::available_parallelism().map_or(1, |cores| (cores.get() / 4).clamp(1, 4))
}
fn parse_args() -> Result<Args, String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let mut result = Args {
        command,
        config: PathBuf::from(l6max_host::config::FILE_NAME),
        suite: "quick".into(),
        case: None,
        backend: "headless".into(),
        model: "all".into(),
        output: None,
        build: true,
        jobs: default_jobs(),
        qemu: None,
        app: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                result.config = PathBuf::from(args.next().ok_or("--config needs a file")?)
            }
            "--suite" => result.suite = args.next().ok_or("--suite needs a name")?,
            "--case" => result.case = Some(args.next().ok_or("--case needs a name")?),
            "--backend" => {
                result.backend = args.next().ok_or("--backend needs headless, gui or both")?
            }
            "--model" => result.model = args.next().ok_or("--model needs l6, l6max or all")?,
            "--output" => {
                result.output = Some(PathBuf::from(
                    args.next().ok_or("--output needs a directory")?,
                ))
            }
            "--no-build" => result.build = false,
            "--qemu" => {
                result.qemu = Some(PathBuf::from(args.next().ok_or("--qemu needs a binary")?))
            }
            "--app" => {
                result.app = Some(PathBuf::from(
                    args.next().ok_or("--app needs a packaged application")?,
                ))
            }
            "--jobs" => {
                result.jobs = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .filter(|jobs| *jobs > 0)
                    .ok_or("--jobs needs a positive number")?
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if !["headless", "gui", "both"].contains(&result.backend.as_str())
        || !["l6", "l6max", "all"].contains(&result.model.as_str())
    {
        return Err("invalid backend or model".into());
    }
    Ok(result)
}
fn hash(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut sha = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
    }
    Ok(format!("{:x}", sha.finalize()))
}
fn hash_patch_directory(path: &Path) -> Result<String, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .map(|entry| entry.map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut sha = Sha256::new();
    for entry in entries {
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        sha.update((name.len() as u32).to_le_bytes());
        sha.update(name.as_bytes());
        let mut file = fs::File::open(entry.path()).map_err(|e| e.to_string())?;
        let mut buffer = [0; 65536];
        loop {
            let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            sha.update(&buffer[..count]);
        }
    }
    Ok(format!("{:x}", sha.finalize()))
}
/// Hash a sparse card image by its allocated extents and total length, so a
/// large image costs only what it stores. Compare only hashes from this function.
fn hash_sparse(path: &Path) -> Result<String, String> {
    use std::os::fd::AsRawFd;
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let fd = file.as_raw_fd();
    let mut sha = Sha256::new();
    sha.update(length.to_le_bytes());
    let mut buffer = vec![0; 1024 * 1024];
    let mut position = 0;
    while position < length {
        let data = unsafe { libc::lseek(fd, position as libc::off_t, libc::SEEK_DATA) };
        if data < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENXIO) {
                break;
            }
            return Err(format!("{}: {error}", path.display()));
        }
        let hole = unsafe { libc::lseek(fd, data, libc::SEEK_HOLE) };
        if hole < 0 {
            return Err(format!(
                "{}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        file.seek(std::io::SeekFrom::Start(data as u64))
            .map_err(|e| e.to_string())?;
        sha.update((data as u64).to_le_bytes());
        let mut remaining = (hole - data) as u64;
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..count])
                .map_err(|e| e.to_string())?;
            sha.update(&buffer[..count]);
            remaining -= count as u64;
        }
        position = hole as u64;
    }
    Ok(format!("{:x}", sha.finalize()))
}
fn files(root: &Path) -> Result<BTreeMap<String, String>, String> {
    fn visit(
        root: &Path,
        path: &Path,
        output: &mut BTreeMap<String, String>,
    ) -> Result<(), String> {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_symlink() {
                return Err(format!(
                    "fixture/source tree contains a symlink: {}",
                    path.display()
                ));
            }
            if path.is_dir() {
                visit(root, &path, output)?;
            } else {
                output.insert(
                    path.strip_prefix(root).unwrap().to_string_lossy().into(),
                    hash(&path)?,
                );
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result)?;
    Ok(result)
}
fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let target = target.join(entry.file_name());
        if path.is_symlink() {
            return Err("state templates may not contain symlinks".into());
        }
        if path.is_dir() {
            copy_tree(&path, &target)?;
        } else {
            fs::copy(path, target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn git(root: &Path, args: &[&str]) -> String {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
        .unwrap_or_default()
}
/// The fixture for a case, from the configured firmware and the case's scenario.
/// Cases without a scenario boot from the completed-setup state and an empty card.
fn case_fixture(
    config: &Config,
    catalog_dir: &Path,
    model: &str,
    case_id: &str,
    scenario: Option<&Scenario>,
) -> Result<Fixture, String> {
    let firmware = config
        .firmware
        .get(model)
        .ok_or_else(|| format!("no firmware package configured for {model}"))?;
    let (state, card) = match scenario {
        Some(scenario) => (scenario.state.as_deref(), scenario.card.as_deref()),
        None => (Some("setup-complete"), Some("empty")),
    };
    let spec = |directory: &str, name: &str| {
        catalog_dir
            .join(directory)
            .join(format!("{model}-{name}.toml"))
    };
    Ok(Fixture {
        name: case_id.into(),
        model: model.into(),
        firmware_package: Some(firmware.clone()),
        patches: scenario.map_or_else(Vec::new, |s| s.patches.clone()),
        installed_patches: scenario.map_or_else(Vec::new, |s| s.installed_patches.clone()),
        sd_patches: scenario.and_then(|s| s.sd_patches.clone()),
        sd_bundle_mode: scenario.and_then(|s| s.sd_bundle_mode.clone()),
        sd_bundle: None,
        update_package: None,
        state_spec: state.map(|name| spec("states", name)),
        card_spec: card.map(|name| spec("cards", name)),
        audio_device: scenario.and_then(|s| s.audio_device.clone()),
        audio_input: scenario.map_or_else(Vec::new, |s| s.audio_input.clone()),
        gdb: scenario.is_some_and(|s| s.gdb),
    })
}

/// Use the requested QEMU, or the repository's own, building it if `build` is set
/// and it is missing or older than its sources.
fn resolve_qemu(root: &Path, build: bool, requested: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(requested) = requested {
        return fs::canonicalize(requested).map_err(|e| format!("{}: {e}", requested.display()));
    }
    let binary = qemu_build::default_binary(root);
    if build && qemu_build::is_stale(root, &binary) {
        qemu_build::build_locked(&qemu_build::Request {
            repository: root.to_owned(),
            source: None,
            minimal: false,
            plugins: false,
            optimization: qemu_build::Optimization::Standard,
        })?;
    }
    if !binary.is_file() {
        return Err(format!(
            "{} is missing; run without --no-build or pass --qemu",
            binary.display()
        ));
    }
    Ok(binary)
}

/// Find the executable and QEMU inside a macOS `.app` or a Linux AppDir.
fn resolve_app(path: &Path) -> Result<App, String> {
    let (executable, qemu) = if path.join("Contents").is_dir() {
        (
            path.join("Contents/MacOS/l6max-gui"),
            path.join("Contents/Resources/libexec/qemu-system-arm"),
        )
    } else {
        (
            path.join("usr/bin/l6max-gui"),
            path.join("usr/share/l6max-emulator/libexec/qemu-system-arm"),
        )
    };
    if !executable.is_file() || !qemu.is_file() {
        return Err(format!("{} is not a packaged application", path.display()));
    }
    Ok(App {
        executable: fs::canonicalize(executable).map_err(|e| e.to_string())?,
        qemu_sha256: hash(&qemu)?,
    })
}

fn load_config(
    path: &Path,
    root: &Path,
    build: bool,
    qemu: Option<&Path>,
    app: Option<&Path>,
) -> Result<Config, String> {
    let path = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let firmware = l6max_host::config::Config::load(&path)?.firmware;
    for (model, firmware) in &firmware {
        if !firmware.is_file() {
            return Err(format!(
                "firmware for {model} must be an update package: {}",
                firmware.display()
            ));
        }
    }
    let qemu = resolve_qemu(root, build, qemu)?;
    Ok(Config {
        firmware,
        qemu_sha256: hash(&qemu)?,
        qemu,
        app: app.map(resolve_app).transpose()?,
    })
}

/// Construct the experimental loader and card bundle from the configured
/// stock package. Both outputs are per-case and recorded with the fixture.
fn prepare_sd_patch(root: &Path, fixture: &mut Fixture, dir: &Path) -> Result<(), String> {
    let Some(patches) = fixture.sd_patches.clone() else {
        return Ok(());
    };
    if !fixture.installed_patches.is_empty() {
        return Err("SD patch fixtures cannot combine installed_patches".into());
    }
    if !patches.is_empty() && fixture.card_spec.is_none() {
        return Err("SD patches need a card spec".into());
    }
    let package = fixture
        .firmware_package
        .as_ref()
        .ok_or("SD patches need firmware")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let source = dir.join("sd-stock-firmware");
    l6fw::unpack(package, &source)?;
    let build = |output: &Path, option: &str, selected: &[String]| -> Result<(), String> {
        let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.current_dir(root)
            .args(["run", "--offline", "--locked", "-p", "l6patch", "--"])
            .arg(&source)
            .arg(output)
            .arg(option);
        for patch in selected {
            cmd.arg("--patch").arg(patch);
        }
        let result = cmd
            .output()
            .map_err(|e| format!("could not start l6patch: {e}"))?;
        let mut log = result.stdout;
        log.extend_from_slice(&result.stderr);
        let name = if option == "--experimental-patch-loader" {
            "patch-loader-build.log"
        } else {
            "sd-patches-build.log"
        };
        fs::write(dir.join(name), &log).map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(format!("l6patch failed: {}", String::from_utf8_lossy(&log)));
        }
        Ok(())
    };
    let loader = dir.join("patch-loader-firmware");
    build(&loader, "--experimental-patch-loader", &fixture.patches)?;
    fixture.firmware_package = Some(loader.join(if fixture.model == "l6" {
        "L6.BIN"
    } else {
        "L6max.bin"
    }));
    fixture.patches.clear();
    if !patches.is_empty() {
        let bundle = dir.join("sd-patches-build");
        build(&bundle, "--experimental-patch-bundle", &patches)?;
        fixture.sd_bundle = Some(bundle.join("PATCHES"));
    }
    Ok(())
}

fn fixture_options(
    config: &Config,
    fixture: &Fixture,
    dir: &Path,
) -> Result<(Options, Value), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut selected_package = fixture.firmware_package.clone();
    let firmware = if !fixture.patches.is_empty() {
        let package = fixture
            .firmware_package
            .as_ref()
            .ok_or("patches require firmware_package")?;
        let bytes = fs::read(package).map_err(|e| e.to_string())?;
        let names: Vec<&str> = fixture.patches.iter().map(String::as_str).collect();
        let (patched, report) = firmware_patch_wasm::apply_patches(&bytes, &names)?;
        let patched_package = dir.join("patched-firmware.bin");
        fs::write(&patched_package, patched).map_err(|e| e.to_string())?;
        fs::write(
            dir.join("patch-report.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        let output = dir.join("firmware");
        l6fw::unpack(&patched_package, &output)?;
        fs::write(
            output.join("patch.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        selected_package = Some(patched_package);
        output
    } else {
        l6max_host::setup::unpack_cached(
            fixture.firmware_package.as_ref().unwrap(),
            &l6max_host::paths::repository_root().join("emulator/state/firmware"),
        )?
    };
    let main = fs::read(firmware.join("main_firmware.bin")).map_err(|e| e.to_string())?;
    let model = l6fw::model_from_main(&main)?;
    if model_name(model) != fixture.model {
        return Err("fixture model differs from selected firmware".into());
    }
    if let Some(package) = &fixture.update_package {
        let bytes = fs::read(package).map_err(|e| e.to_string())?;
        if l6fw::model_from_package(&bytes).map_err(|e| e.to_string())? != model {
            return Err("update package belongs to a different model than the fixture".into());
        }
    }
    let state = dir.join("state");
    fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    // A spec edits the disposable copy: seed missing NOR from the selected
    // firmware, commit settings with the firmware's bank protocol, and write
    // the panel RTC/flash it declares.
    let state_spec = fixture
        .state_spec
        .as_ref()
        .map(|path| -> Result<Value, String> {
            let report =
                l6nor::state::apply(&l6nor::StateSpec::load(path)?, Some(&firmware), &state)?;
            Ok(json!({"sha256": hash(path)?, "applied": report}))
        })
        .transpose()?;
    let audio_device = audio_device_spec(fixture, dir);
    let mut args = vec![
        "--no-auto-setup".into(),
        "--qemu".into(),
        config.qemu.clone().into_os_string(),
        "--firmware-dir".into(),
        firmware.clone().into_os_string(),
        "--state-dir".into(),
        state.into_os_string(),
        "--logs".into(),
        dir.as_os_str().to_owned(),
        "--audio-device".into(),
        audio_device.clone().into(),
    ];
    if fixture.gdb {
        args.extend(["--gdb-socket".into(), gdb_socket_path().into_os_string()]);
    }
    let sd_hash = if let Some(spec) = &fixture.card_spec {
        let target = dir.join("card.img");
        let mut card = sd_image::spec::CardSpec::load(spec)?;
        if let Some(bundle_dir) = &fixture.sd_bundle
            && fixture.sd_bundle_mode.as_deref() != Some("absent")
        {
            let mut files = fs::read_dir(bundle_dir)
                .map_err(|e| format!("could not enumerate patch output directory: {e}"))?
                .map(|entry| entry.map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            files.sort_by_key(|entry| entry.file_name());
            if fixture.sd_bundle_mode.as_deref() == Some("ordered-chain") {
                let report: serde_json::Value = serde_json::from_slice(
                    &fs::read(bundle_dir.parent().unwrap().join("patch.json"))
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                let filename = report["patches"][0]["file"]
                    .as_str()
                    .and_then(|path| Path::new(path).file_name())
                    .ok_or("dynamic module report has no output filename")?;
                let original = fs::read(bundle_dir.join(filename)).map_err(|e| e.to_string())?;
                let pass_through = report["patches"][0]["pass_through_entry_offset"]
                    .as_u64()
                    .or_else(|| report["patches"][0]["pass_through_entry_offsets"][0].as_u64())
                    .ok_or("dynamic module report has no pass-through entry")?
                    as u32;
                let code_len = u32::from_le_bytes(original[8..12].try_into().unwrap()) as usize;
                let version = u16::from_le_bytes(original[4..6].try_into().unwrap());
                let header_len = patch_loader_core::dynamic::HEADER_LEN_V1;
                let hook_count = u16::from_le_bytes(original[16..18].try_into().unwrap()) as usize;
                if hook_count != 1 || version != patch_loader_core::dynamic::FORMAT_VERSION {
                    return Err("ordered-chain fixture expects one dynamic hook".into());
                }
                let mut pass_through_module = original.clone();
                let entry_offset = header_len + code_len + 4;
                pass_through_module[entry_offset..entry_offset + 4]
                    .copy_from_slice(&pass_through.to_le_bytes());
                let checksum = patch_loader_core::dynamic::checksum(&pass_through_module)
                    .ok_or("invalid dynamic fixture checksum")?;
                pass_through_module[24..28].copy_from_slice(&checksum.to_le_bytes());
                let first_path = dir.join("sd-chain-first.BIN");
                let second_path = dir.join("sd-chain-second.BIN");
                fs::write(&first_path, pass_through_module).map_err(|e| e.to_string())?;
                fs::write(&second_path, original).map_err(|e| e.to_string())?;
                // Insert in reverse order so the firmware must sort by numeric prefix.
                card.file.push(sd_image::spec::File {
                    path: "PATCHES/10-SECOND.BIN".into(),
                    content: sd_image::spec::Content::Copy(second_path),
                });
                card.file.push(sd_image::spec::File {
                    path: "PATCHES/2-FIRST.BIN".into(),
                    content: sd_image::spec::Content::Copy(first_path),
                });
            } else if fixture.sd_bundle_mode.as_deref() == Some("corrupt-first") {
                let first = files.first().ok_or("empty SD patch directory")?;
                let mut bytes = fs::read(first.path()).map_err(|e| e.to_string())?;
                *bytes.last_mut().ok_or("empty SD patch")? ^= 1;
                let corrupt = dir.join("sd-corrupt-first.BIN");
                fs::write(&corrupt, bytes).map_err(|e| e.to_string())?;
                card.file.push(sd_image::spec::File {
                    path: "PATCHES/AAA.BIN".into(),
                    content: sd_image::spec::Content::Copy(corrupt),
                });
            }
            for entry in files {
                if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                    continue;
                }
                if fixture.sd_bundle_mode.as_deref() == Some("ordered-chain") {
                    continue;
                }
                let filename = entry.file_name().to_string_lossy().into_owned();
                let source = if fixture.sd_bundle_mode.as_deref() == Some("corrupt") {
                    let mut bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
                    *bytes.last_mut().ok_or("empty SD patch")? ^= 1;
                    let corrupt = dir.join(format!("sd-corrupt-{filename}"));
                    fs::write(&corrupt, bytes).map_err(|e| e.to_string())?;
                    corrupt
                } else {
                    entry.path()
                };
                card.file.push(sd_image::spec::File {
                    path: format!("PATCHES/{filename}"),
                    content: sd_image::spec::Content::Copy(source),
                });
            }
        }
        card.build(&target, spec.parent().unwrap_or(Path::new(".")))
            .map_err(|e| e.to_string())?;
        args.extend(["--sd-image".into(), target.clone().into_os_string()]);
        Some(hash_sparse(&target)?)
    } else {
        if fixture.sd_bundle.is_some() {
            return Err("SD patch bundle requires a card spec".into());
        }
        args.push("--no-sd".into());
        None
    };
    if let Some(package) = &selected_package {
        args.extend([
            "--firmware-package".into(),
            package.clone().into_os_string(),
        ]);
    }
    if let Some(package) = &fixture.update_package {
        args.extend(["--update-package".into(), package.clone().into_os_string()]);
    }
    for patch in &fixture.installed_patches {
        args.extend(["--patch".into(), patch.into()]);
    }
    let options = Options::parse_args(args)?;
    let manifest = json!({"fixture":fixture.name,"model":fixture.model,"firmware":files(&firmware)?,"package_sha256":selected_package.as_ref().map(|p|hash(p)).transpose()?,"patches":fixture.patches,"sd_patches":fixture.sd_patches,"sd_bundle_mode":fixture.sd_bundle_mode,"sd_patch_set_sha256":fixture.sd_bundle.as_ref().map(|p|hash_patch_directory(p)).transpose()?,"update_package_sha256":fixture.update_package.as_ref().map(|p|hash(p)).transpose()? ,"state_spec":state_spec,"sd_sha256":sd_hash,"audio_device":audio_device,"qemu_sha256":config.qemu_sha256});
    Ok((options, manifest))
}
fn engine_args(options: &Options) -> Vec<String> {
    let mut result = vec![
        "--no-auto-setup".into(),
        "--qemu".into(),
        options.qemu.as_ref().unwrap().display().to_string(),
        "--firmware-dir".into(),
        options.firmware_dir.display().to_string(),
        "--logs".into(),
        options.logs.display().to_string(),
        "--audio-device".into(),
        options.audio_device.clone().unwrap(),
    ];
    if let Some(socket) = &options.gdb_socket {
        result.extend(["--gdb-socket".into(), socket.display().to_string()]);
    }
    if let Some(state) = &options.state_dir {
        result.extend(["--state-dir".into(), state.display().to_string()]);
    } else {
        result.push("--volatile".into());
    }
    if let Some(card) = &options.sd_image {
        result.extend(["--sd-image".into(), card.display().to_string()]);
    } else {
        result.push("--no-sd".into());
    }
    if let Some(package) = &options.firmware_package {
        result.extend(["--firmware-package".into(), package.display().to_string()]);
    }
    if let Some(package) = &options.update_package {
        result.extend(["--update-package".into(), package.display().to_string()]);
    }
    for patch in options.patches.iter().flatten() {
        result.extend(["--patch".into(), patch.clone()]);
    }
    if options.update_bootloader {
        result.push("--update-bootloader".into());
    }
    if options.usb_enabled {
        result.push("--usb".into());
    }
    if options.usb_cable_at_start {
        result.push("--usb-cable-at-start".into());
    }
    if options.midi_enabled {
        result.push("--midi".into());
    }
    if options.trace_sd {
        result.push("--trace-sd".into());
    }
    if options.start_paused {
        result.push("--start-paused".into());
    }
    result
}
struct ChildGroup(Child);
impl Drop for ChildGroup {
    fn drop(&mut self) {
        // The driver creates this group. Always retire descendants, even after
        // an unexpectedly successful parent exits without reaping QEMU.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGTERM);
        }
        if matches!(self.0.try_wait(), Ok(None)) {
            let end = Instant::now() + Duration::from_secs(3);
            while Instant::now() < end && matches!(self.0.try_wait(), Ok(None)) {
                thread::sleep(Duration::from_millis(20));
            }
        }
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}
fn supervise(mut command: Command, dir: &Path, timeout: u64) -> Result<bool, String> {
    let stdout = fs::File::create(dir.join("stdout.log")).map_err(|e| e.to_string())?;
    let stderr = fs::File::create(dir.join("stderr.log")).map_err(|e| e.to_string())?;
    command
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);
    let mut child = ChildGroup(command.spawn().map_err(|e| e.to_string())?);
    let started = Instant::now();
    let mut next = Instant::now() + Duration::from_secs(15);
    loop {
        if cancelled() {
            return Err("run interrupted; child process group stopped".into());
        }
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            return Ok(status.success());
        }
        if started.elapsed() > Duration::from_secs(timeout) {
            return Err("child exceeded wall-clock limit; its process group was stopped".into());
        }
        if Instant::now() >= next {
            tracing::info!(event="child_progress",wall_seconds=started.elapsed().as_secs_f64(),artifacts=%dir.display(),"child running");
            next = Instant::now() + Duration::from_secs(15);
        }
        thread::sleep(Duration::from_millis(25));
    }
}
fn child_scenario() -> Result<(), String> {
    let mut args = env::args_os().skip(2);
    let path = PathBuf::from(args.next().ok_or("missing scenario")?);
    let options = Options::parse_args(args)?;
    smoke_core::run(Scenario::load_for_case(&path)?, options)
}
/// Build persistent state by running a semantic headless scenario on an isolated device.
fn expand(value: &str, options: &Options) -> String {
    let mut text = value
        .replace(
            "{firmware_dir}",
            &options.firmware_dir.display().to_string(),
        )
        .replace("{logs}", &options.logs.display().to_string())
        .replace(
            "{qemu}",
            &options.qemu.as_ref().unwrap().display().to_string(),
        );
    if let Some(package) = &options.firmware_package {
        text = text.replace("{package}", &package.display().to_string());
    }
    text
}
fn run_case(
    root: &Path,
    catalog_dir: &Path,
    config: &Config,
    fixture: &Fixture,
    case: &Case,
    backend: &str,
    dir: &Path,
) -> CaseResult {
    let mut result = CaseResult {
        case: case.id.clone(),
        model: fixture.model.clone(),
        backend: backend.into(),
        outcome: "BLOCKED".into(),
        detail: String::new(),
        artifacts: dir.into(),
    };
    let run = || -> Result<(bool, Value), String> {
        if cancelled() {
            return Err("not run: suite interrupted".into());
        }
        if let Some(reason) = &case.blocked {
            return Err(reason.clone());
        }
        if let Some(path) = &case.scenario {
            let scenario = Scenario::load(&catalog_dir.join(path))?;
            if !scenario.backends.iter().any(|b| b == backend) {
                return Err(format!("scenario does not support {backend} backend"));
            }
        }
        if case.needs_configuration && case.scenario.is_none() && config.app.is_none() {
            return Err("requires a packaged application; pass --app PATH".into());
        }
        if hash(&config.qemu)? != config.qemu_sha256 {
            return Err("QEMU changed since preflight".into());
        }
        let mut selected_fixture = fixture.clone();
        prepare_sd_patch(root, &mut selected_fixture, dir)?;
        if let Some(path) = &case.scenario {
            let patches = Scenario::load(&catalog_dir.join(path))?
                .update_patches
                .remove(&fixture.model)
                .unwrap_or_default();
            if !patches.is_empty() {
                let package = fixture
                    .firmware_package
                    .as_ref()
                    .ok_or("update_patches require firmware_package")?;
                let bytes = fs::read(package).map_err(|e| e.to_string())?;
                let names: Vec<&str> = patches.iter().map(String::as_str).collect();
                let (patched, report) = firmware_patch_wasm::apply_patches(&bytes, &names)?;
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                let update = dir.join("patched-update.bin");
                fs::write(&update, patched).map_err(|e| e.to_string())?;
                // The post-update checks locate the installed hooks from the
                // report that sits beside the package.
                fs::write(
                    update.with_extension("json"),
                    serde_json::to_vec_pretty(&report).unwrap(),
                )
                .map_err(|e| e.to_string())?;
                selected_fixture.update_package = Some(update);
            }
        }
        if case.no_sd {
            selected_fixture.card_spec = None;
        }
        let (mut options, mut manifest) = fixture_options(config, &selected_fixture, dir)?;
        if case.volatile && selected_fixture.state_spec.is_none() {
            options.state_dir = None;
        }
        if case.no_sd {
            options.sd_image = None;
            options.no_sd = true;
        }
        // Setup-input steps are skipped when the spec already built their result.
        let spec_state = selected_fixture.state_spec.is_some();
        manifest["execution_spec_state"] = json!(spec_state);
        if let Some(path) = &case.scenario {
            let mut scenario = Scenario::load(&catalog_dir.join(path))?;
            if spec_state {
                scenario = scenario.without_setup_input();
            }
            smoke_core::devices::prepare(&scenario, &mut options)?;
        }
        manifest["execution_volatile"] = json!(case.volatile);
        manifest["execution_no_sd"] = json!(case.no_sd);
        manifest["execution_sd_sha256"] = options
            .sd_image
            .as_ref()
            .map(|path| hash_sparse(path))
            .transpose()?
            .map_or(Value::Null, |value| json!(value));
        manifest["execution_state_sha256"] = options
            .state_dir
            .as_ref()
            .map(|path| files(path))
            .transpose()?
            .map_or(Value::Null, |value| json!(value));
        fs::write(
            dir.join("inputs.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        let packaged = case.needs_configuration && case.scenario.is_none();
        let binary = if let Some(app) = config.app.as_ref().filter(|_| packaged) {
            app.executable.clone()
        } else {
            let name = if case.scenario.is_some() && backend == "headless" {
                "l6test"
            } else if case.scenario.is_some() {
                "l6max-gui"
            } else {
                case.package.as_deref().ok_or("case has no executable")?
            };
            dir.parent().unwrap().join("bin").join(name)
        };
        let binary_sha256 = hash(&binary)?;
        let mut command = Command::new(&binary);
        command.current_dir(root);
        for (key, _) in env::vars_os() {
            if key.to_string_lossy().starts_with("L6_")
                || key.to_string_lossy().starts_with("L6MAX_")
            {
                command.env_remove(key);
            }
        }
        let log_format = env::var("L6_LOG_FORMAT").unwrap_or_else(|_| "json".into());
        command.env("L6_LOG_FORMAT", log_format);
        command
            .env("SMOKE_CASE", &case.id)
            .env("SMOKE_MODEL", &fixture.model)
            .env("SMOKE_BACKEND", backend)
            .env(
                smoke_core::STATE_SPEC_ENV,
                if spec_state { "1" } else { "0" },
            );
        command.env(
            "RUST_LOG",
            env::var("RUST_LOG").unwrap_or_else(|_| l6max_host::logging::DEFAULT_FILTER.into()),
        );
        let arguments: Vec<String> = case.arguments.iter().map(|a| expand(a, &options)).collect();
        if arguments.iter().any(|a| a.contains('{')) {
            return Err("unresolved legacy argument placeholder; supply the required package/path explicitly".into());
        }
        if let Some(path) = &case.scenario {
            let scenario = catalog_dir.join(path);
            if backend == "headless" {
                command.arg("__scenario").arg(&scenario);
            } else {
                command.arg("--smoke-scenario").arg(&scenario);
            }
            command.args(engine_args(&options));
        } else {
            command.args(&arguments);
            command.args(engine_args(&options));
        }
        if !selected_fixture.audio_input.is_empty() {
            let directory = dir.join("audio-input");
            for input in &selected_fixture.audio_input {
                let bytes = input
                    .content
                    .bytes(catalog_dir)
                    .map_err(|e| e.to_string())?;
                l6max_host::audio_input::prepare_bytes(
                    &directory,
                    u32::from(input.channel),
                    &bytes,
                )
                .map_err(|e| e.to_string())?;
            }
            command.env("L6_AUDIO_INPUT_DIR", directory);
        }
        for (key, value) in &case.environment {
            let mut value = expand(value, &options);
            if let Some(package) = config.firmware.get("l6") {
                value = value.replace("{l6_package}", &package.display().to_string());
            }
            if let Some(app) = &config.app {
                value = value.replace("{app_qemu_sha256}", &app.qemu_sha256);
            }
            if value.contains('{') {
                return Err(format!("unresolved placeholder in {key}: {value}"));
            }
            command.env(key, value);
        }
        let invocation = json!({"binary":binary,"binary_sha256":hash(&binary)?,"arguments":command.get_args().map(|s|s.to_string_lossy().into_owned()).collect::<Vec<_>>(),"environment":command.get_envs().filter_map(|(k,v)|v.map(|v|(k.to_string_lossy().into_owned(),v.to_string_lossy().into_owned()))).collect::<BTreeMap<_,_>>()});
        fs::write(
            dir.join("invocation.json"),
            serde_json::to_vec_pretty(&invocation).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        let success = supervise(command, dir, case.wall_timeout_s)?;
        if hash(&binary)? != binary_sha256 {
            return Err("legacy executable changed during the case; result is invalid".into());
        }
        if hash(&config.qemu)? != config.qemu_sha256 {
            return Err("QEMU changed during the case; result is invalid".into());
        }
        let scenario = fs::read(dir.join("scenario.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        Ok((success, scenario))
    };
    match run() {
        Ok((success, scenario)) => {
            result.outcome =
                if success && (case.scenario.is_none() || scenario["outcome"] == "PASS") {
                    "PASS"
                } else {
                    "FAIL"
                }
                .into();
            result.detail = scenario["error"]
                .as_str()
                .unwrap_or(if success {
                    "completed"
                } else {
                    "child failed; inspect stdout.log and stderr.log"
                })
                .into();
            apply_expected(&mut result, &scenario, &case.expected_failure);
        }
        Err(error) => {
            result.detail = error;
            if dir.join("invocation.json").exists() {
                result.outcome = "FAIL".into();
            }
        }
    }
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(
        dir.join("result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    );
    result
}

fn apply_expected(result: &mut CaseResult, scenario: &Value, expected: &[ExpectedFailure]) {
    if let Some(expected) = expected.iter().find(|e| e.model == result.model) {
        if result.outcome == "PASS" {
            result.outcome = "XPASS".into();
            result.detail = format!("unexpected pass: {}", expected.reason);
        } else if result.outcome == "FAIL"
            && scenario["failed_step"].as_u64() == Some(expected.step as u64)
            && result.detail.contains(&expected.contains)
        {
            result.outcome = "XFAIL".into();
            result.detail = format!("{}: {}", expected.reason, result.detail);
        }
    }
}

struct RunJournal(PathBuf);
impl Drop for RunJournal {
    fn drop(&mut self) {
        let path = self.0.join("report.json");
        let mut report = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or_else(|| json!({"schema":1}));
        if report.get("outcome").is_none() || report["outcome"] == "RUNNING" {
            report["outcome"] = json!("BLOCKED");
            report["error"] = json!(
                "driver exited before suite completion; inspect invocation/build logs and the terminal error"
            );
            let _ = fs::write(path, serde_json::to_vec_pretty(&report).unwrap());
        }
    }
}
fn execute() -> Result<i32, String> {
    let args = parse_args()?;
    let root = l6max_host::paths::repository_root();
    let catalog_dir = root.join("tests/smoke");
    let catalog: Catalog = toml::from_str(
        &fs::read_to_string(catalog_dir.join("catalog.toml")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if catalog.version != 1 {
        return Err("unsupported catalog schema".into());
    }
    for case in &catalog.case {
        if let Some(path) = &case.scenario {
            Scenario::load(&catalog_dir.join(path))?;
        }
    }
    if args.command == "list" {
        for case in &catalog.case {
            println!(
                "{:<24} {:<12} {}{}",
                case.id,
                case.models.join(","),
                case.suites.join(","),
                if case.needs_configuration {
                    " [explicit configuration required]"
                } else {
                    ""
                }
            );
        }
        return Ok(0);
    }
    if !["run", "check"].contains(&args.command.as_str()) {
        println!(
            "Usage: cargo run -p l6test -- list | check | run [--config config.toml] [--suite NAME | --case ID] [--model l6|l6max|all] [--backend headless|gui|both] [--output NEW_DIR] [--no-build] [--jobs N] [--qemu BINARY] [--app PACKAGED_APP]\n       cargo run -p l6test -- probe gui --keys TARGET,... --volatile [engine options]\n       cargo run -p l6test -- capture screen --out FILE.gif [capture options] --volatile [engine options]"
        );
        return Ok(0);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/l6smk-{stamp:x}")));
    if output.exists() {
        return Err(
            "output directory already exists; choose a new one to preserve evidence".into(),
        );
    }
    fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let output = fs::canonicalize(output).map_err(|e| e.to_string())?;
    tracing::info!(event="run_started", artifacts=%output.display(), suite=%args.suite, backend=%args.backend, model=%args.model,"smoke run started");
    let _journal = RunJournal(output.clone());
    copy_tree(&catalog_dir, &output.join("spec"))?;
    let catalog_dir = output.join("spec");
    let preflight = || -> Result<(Config, Value), String> {
        let config = load_config(
            &args.config,
            root,
            args.build,
            args.qemu.as_deref(),
            args.app.as_deref(),
        )?;
        let _shared = l6max_host::shared_memory::SharedRam::new(2048).map_err(|e| {
            format!("shared memory unavailable: {e}; rerun with the required sandbox permissions")
        })?;
        let mut untracked = BTreeMap::new();
        for name in git(root, &["ls-files", "--others", "--exclude-standard"]).lines() {
            let path = root.join(name);
            if path.is_file() {
                untracked.insert(name.to_owned(), hash(&path)?);
            }
        }
        let manifest = json!({"schema":1,"tooling_commit":git(root,&["rev-parse","HEAD"]),"working_tree":git(root,&["status","--short"]),"tracked_diff_sha256":format!("{:x}",Sha256::digest(git(root,&["diff","--binary","HEAD"]).as_bytes())),"untracked":untracked,"qemu":config.qemu,"qemu_sha256":config.qemu_sha256,"qemu_provenance":"explicitly pinned external binary; build source not attested","scenario_files":files(&catalog_dir)?,"board_sources":files(&root.join("emulator/qemu/hw/arm"))?,"qemu_patches":files(&root.join("emulator/qemu/patches"))?,"logging":{"format":env::var("L6_LOG_FORMAT").unwrap_or_else(|_|"json".into()),"filter":env::var("RUST_LOG").unwrap_or_else(|_|l6max_host::logging::DEFAULT_FILTER.into())},"driver_sha256":hash(&env::current_exe().map_err(|e|e.to_string())?)?,"suite":args.suite,"case":args.case,"backend":args.backend,"model":args.model});
        Ok((config, manifest))
    };
    let (config, manifest) = match preflight() {
        Ok(v) => v,
        Err(error) => {
            fs::write(
                output.join("report.json"),
                serde_json::to_vec_pretty(&json!({"schema":1,"outcome":"BLOCKED","error":error}))
                    .unwrap(),
            )
            .map_err(|e| e.to_string())?;
            return Err(error);
        }
    };
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let cases: Vec<_> = catalog
        .case
        .iter()
        .filter(|c| {
            args.case
                .as_ref()
                .map_or_else(|| c.suites.contains(&args.suite), |id| &c.id == id)
        })
        .collect();
    if cases.is_empty() {
        return Err("no cases selected".into());
    }
    if args.command == "check" {
        fs::write(
            output.join("report.json"),
            serde_json::to_vec_pretty(&json!({"schema":1,"outcome":"PASS","preflight_only":true}))
                .unwrap(),
        )
        .map_err(|e| e.to_string())?;
        println!("Preflight passed. QEMU is pinned by binary hash for this run.");
        return Ok(0);
    }
    let backends: Vec<&str> = if args.backend == "both" {
        vec!["headless", "gui"]
    } else {
        vec![&args.backend]
    };
    let mut packages = BTreeSet::new();
    for case in &cases {
        if case.blocked.is_some()
            || case.needs_configuration && case.scenario.is_none() && config.app.is_none()
        {
            continue;
        }
        if let Some(package) = &case.package {
            packages.insert(package.as_str());
        }
        if case.scenario.as_ref().is_some_and(|p| {
            Scenario::load(&catalog_dir.join(p))
                .is_ok_and(|s| s.backends.iter().any(|b| b == "gui"))
        }) && backends.contains(&"gui")
        {
            packages.insert("l6max-gui");
        }
    }
    if args.build && !packages.is_empty() {
        let mut command = Command::new("cargo");
        command
            .current_dir(root)
            .args(["build", "--offline", "--locked"]);
        for package in &packages {
            command.arg("-p").arg(package);
        }
        let built = supervise(command, &output, 600);
        if !matches!(built, Ok(true)) {
            let error = built
                .err()
                .unwrap_or_else(|| "build failed; inspect stdout.log/stderr.log".into());
            fs::write(
                output.join("report.json"),
                serde_json::to_vec_pretty(&json!({"schema":1,"outcome":"BLOCKED","error":error}))
                    .unwrap(),
            )
            .map_err(|e| e.to_string())?;
            return Err(error);
        }
    }
    // Resolve Cargo's actual output directory; CARGO_TARGET_DIR or workspace
    // configuration must not make us copy an older binary from root/target.
    let metadata = Command::new("cargo")
        .current_dir(root)
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !metadata.status.success() {
        return Err("cargo metadata failed while resolving built executables".into());
    }
    let metadata: Value = serde_json::from_slice(&metadata.stdout).map_err(|e| e.to_string())?;
    let built_dir = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .ok_or("Cargo metadata omitted target_directory")?,
    )
    .join("debug");
    fs::create_dir_all(output.join("bin")).map_err(|e| e.to_string())?;
    fs::copy(
        env::current_exe().map_err(|e| e.to_string())?,
        output.join("bin/l6test"),
    )
    .map_err(|e| e.to_string())?;
    for package in packages {
        fs::copy(built_dir.join(package), output.join("bin").join(package))
            .map_err(|e| e.to_string())?;
    }
    let mut jobs = vec![];
    for case in cases {
        for model in case
            .models
            .iter()
            .filter(|m| args.model == "all" || *m == &args.model)
        {
            let scenario = case
                .scenario
                .as_ref()
                .map(|p| Scenario::load(&catalog_dir.join(p)))
                .transpose()?;
            let selected: Vec<&str> = match &scenario {
                Some(_) if case.gui => vec!["gui"],
                Some(scenario) => backends
                    .iter()
                    .copied()
                    .filter(|b| scenario.backends.iter().any(|s| s == b))
                    .collect(),
                None => vec![if case.gui { "gui" } else { "headless" }],
            };
            for backend in selected {
                // GUI windows, native host MIDI ports and external executables
                // are host-wide resources; such cases run alone.
                let exclusive =
                    backend == "gui" || scenario.as_ref().is_none_or(Scenario::uses_native_midi);
                jobs.push(Job {
                    case,
                    model: model.clone(),
                    backend,
                    exclusive,
                    dir: output.join(format!("{}-{model}-{backend}", case.id.replace('/', "-"))),
                });
            }
        }
    }
    let run_job = |job: &Job| -> CaseResult {
        let case_span = tracing::info_span!("smoke_case",case=%job.case.id,model=%job.model,backend=job.backend,artifacts=%job.dir.display());
        let _entered = case_span.enter();
        tracing::info!(event = "case_started", "case started");
        let scenario = job
            .case
            .scenario
            .as_ref()
            .map(|path| Scenario::load(&catalog_dir.join(path)))
            .transpose();
        let fixture = scenario.and_then(|scenario| {
            case_fixture(
                &config,
                &catalog_dir,
                &job.model,
                &job.case.id,
                scenario.as_ref(),
            )
        });
        let result = match fixture {
            Ok(fixture) => run_case(
                root,
                &catalog_dir,
                &config,
                &fixture,
                job.case,
                job.backend,
                &job.dir,
            ),
            Err(detail) => CaseResult {
                case: job.case.id.clone(),
                model: job.model.clone(),
                backend: job.backend.into(),
                outcome: "BLOCKED".into(),
                detail,
                artifacts: job.dir.clone(),
            },
        };
        match result.outcome.as_str() {
            "PASS" => {
                tracing::info!(event="case_finished",outcome=%result.outcome,detail=%result.detail,"case completed")
            }
            "BLOCKED" | "XFAIL" => {
                tracing::warn!(event="case_finished",outcome=%result.outcome,detail=%result.detail,"case incomplete")
            }
            _ => {
                tracing::error!(event="case_finished",outcome=%result.outcome,detail=%result.detail,"case failed")
            }
        }
        result
    };
    // Queue shareable cases first so an exclusive one never idles the pool.
    jobs.sort_by_key(|job| job.exclusive);
    let results = schedule(
        &jobs,
        args.jobs,
        |job| job.exclusive,
        run_job,
        |finished| {
            fs::write(
                output.join("report.json"),
                serde_json::to_vec_pretty(
                    &json!({"schema":1,"outcome":"RUNNING","results":finished}),
                )
                .unwrap(),
            )
            .map_err(|e| e.to_string())
        },
    )?;
    if results.is_empty() {
        return Err("no model/case combinations selected".into());
    }
    let failed = results
        .iter()
        .any(|r| r.outcome == "FAIL" || r.outcome == "XPASS");
    let blocked = results.iter().any(|r| r.outcome == "BLOCKED");
    let outcome = if failed {
        "FAIL"
    } else if blocked {
        "BLOCKED"
    } else if results.iter().any(|r| r.outcome == "XFAIL") {
        "XFAIL"
    } else {
        "PASS"
    };
    let report = json!({"schema":1,"outcome":outcome,"results":results});
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let mut summary = fs::File::create(output.join("summary.md")).map_err(|e| e.to_string())?;
    writeln!(
        summary,
        "Run: {outcome}\n\n| Case | Model | Backend | Result |\n| --- | --- | --- | --- |"
    )
    .map_err(|e| e.to_string())?;
    for result in &results {
        writeln!(
            summary,
            "| {} | {} | {} | {} |",
            result.case, result.model, result.backend, result.outcome
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(if failed {
        1
    } else if blocked {
        2
    } else {
        0
    })
}
struct Job<'a> {
    case: &'a Case,
    model: String,
    backend: &'a str,
    exclusive: bool,
    dir: PathBuf,
}
/// Run jobs on up to `limit` workers. An exclusive job starts only when no
/// other job is running and holds back later jobs until it finishes. Results
/// keep job order; `progress` receives the completed results so far.
fn schedule<J: Sync, R: Send>(
    jobs: &[J],
    limit: usize,
    exclusive: impl Fn(&J) -> bool + Sync,
    run: impl Fn(&J) -> R + Sync,
    progress: impl Fn(&[&R]) -> Result<(), String> + Sync,
) -> Result<Vec<R>, String> {
    use std::sync::{Condvar, Mutex};
    struct State<R> {
        next: usize,
        running: usize,
        exclusive: bool,
        results: Vec<Option<R>>,
        error: Option<String>,
    }
    let state = Mutex::new(State {
        next: 0,
        running: 0,
        exclusive: false,
        results: jobs.iter().map(|_| None).collect(),
        error: None,
    });
    let changed = Condvar::new();
    thread::scope(|scope| {
        for _ in 0..limit.clamp(1, jobs.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let mut guard = state.lock().unwrap();
                    let index = loop {
                        if guard.next == jobs.len() || guard.error.is_some() {
                            return;
                        }
                        let exclusive = exclusive(&jobs[guard.next]);
                        if !guard.exclusive && (guard.running == 0 || !exclusive) {
                            break guard.next;
                        }
                        guard = changed.wait(guard).unwrap();
                    };
                    let exclusive = exclusive(&jobs[index]);
                    guard.next += 1;
                    guard.running += 1;
                    guard.exclusive = exclusive;
                    drop(guard);
                    let result = run(&jobs[index]);
                    let mut guard = state.lock().unwrap();
                    guard.results[index] = Some(result);
                    guard.running -= 1;
                    guard.exclusive &= !exclusive;
                    let finished: Vec<&R> = guard.results.iter().flatten().collect();
                    if let Err(error) = progress(&finished) {
                        guard.error = Some(error);
                    }
                    drop(guard);
                    changed.notify_all();
                }
            });
        }
    });
    let state = state.into_inner().unwrap();
    if let Some(error) = state.error {
        return Err(error);
    }
    Ok(state.results.into_iter().flatten().collect())
}
fn main() {
    let cli_args = env::args_os().skip(1).collect::<Vec<_>>();
    if let Err(error) = l6max_host::logging::init(l6max_host::logging::Format::Json) {
        eprintln!("{error}");
        std::process::exit(2);
    }
    let context = l6max_host::logging::context("driver");
    let _entered = context.enter();
    if cli_args.first().is_some_and(|arg| arg == "__scenario") {
        if let Err(error) = child_scenario() {
            tracing::error!(event="scenario_failed",%error,"scenario failed");
            std::process::exit(1);
        }
        return;
    }
    if cli_args.len() >= 2 && cli_args[0] == "probe" && cli_args[1] == "gui" {
        if let Err(error) = gui_probe::run(cli_args[2..].to_vec()) {
            tracing::error!(event="gui_probe_failed",%error,"GUI screen probe failed");
            std::process::exit(1);
        }
        return;
    }
    if cli_args.len() >= 2 && cli_args[0] == "capture" && cli_args[1] == "screen" {
        if let Err(error) = screen_gif::run(cli_args[2..].to_vec()) {
            tracing::error!(event="screen_capture_failed",%error,"L6max screen GIF capture failed");
            std::process::exit(1);
        }
        return;
    }
    if cli_args.len() >= 2 && cli_args[0] == "capture" && cli_args[1] == "gui" {
        if let Err(error) = gui_capture::run(cli_args[2..].to_vec()) {
            tracing::error!(event="gui_capture_failed",%error,"GPUI GUI capture failed");
            std::process::exit(1);
        }
        return;
    }
    unsafe {
        libc::signal(libc::SIGINT, interrupt as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, interrupt as *const () as libc::sighandler_t);
    }
    match execute() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            tracing::error!(event="run_blocked",%error,"smoke run blocked");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn temp() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "l6test-unit-{}-{stamp}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    #[ignore = "requires L6_SD_TEST_PACKAGE and L6_SD_TEST_MODEL"]
    fn builds_sd_fixture_from_local_firmware() {
        let model = env::var("L6_SD_TEST_MODEL").expect("set L6_SD_TEST_MODEL");
        let package =
            PathBuf::from(env::var_os("L6_SD_TEST_PACKAGE").expect("set L6_SD_TEST_PACKAGE"));
        let selected = env::var("L6_SD_TEST_PATCHES")
            .unwrap_or_else(|_| "midi-sync".into())
            .split(',')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let static_patches = env::var("L6_SD_TEST_STATIC")
            .ok()
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default();
        let root = l6max_host::paths::repository_root();
        let dir = temp();
        let mut fixture = Fixture {
            name: "sd-fixture".into(),
            model: model.clone(),
            firmware_package: Some(package),
            patches: static_patches,
            installed_patches: vec![],
            sd_patches: Some(selected.clone()),
            sd_bundle_mode: None,
            sd_bundle: None,
            update_package: None,
            state_spec: None,
            card_spec: Some(root.join(format!("tests/smoke/cards/{model}-empty.toml"))),
            audio_device: None,
            audio_input: vec![],
            gdb: false,
        };
        prepare_sd_patch(&root, &mut fixture, &dir).unwrap();
        let patch_dir = fixture.sd_bundle.as_ref().unwrap();
        let report: serde_json::Value = serde_json::from_slice(
            &fs::read(patch_dir.parent().unwrap().join("patch.json")).unwrap(),
        )
        .unwrap();
        let filename = report["patches"][0]["file"]
            .as_str()
            .and_then(|path| Path::new(path).file_name())
            .unwrap();
        let patch_file = patch_dir.join(filename);
        let bytes = fs::read(&patch_file).unwrap();
        assert_eq!(&bytes[..4], b"L6SB");
        assert_eq!(
            u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
            patch_loader_core::dynamic::FORMAT_VERSION
        );
        let checksum_offset = if bytes[4..6] == 9u16.to_le_bytes() {
            24..28
        } else {
            20..24
        };
        assert_eq!(
            patch_loader_core::dynamic::checksum(&bytes),
            Some(u32::from_le_bytes(
                bytes[checksum_offset].try_into().unwrap()
            ))
        );
        let package = fixture.firmware_package.as_ref().unwrap();
        let unpacked = dir.join("verify-loader");
        l6fw::unpack(package, &unpacked).unwrap();
        let spec = fixture.card_spec.as_ref().unwrap();
        let mut card = sd_image::spec::CardSpec::load(spec).unwrap();
        let patch_path = format!("PATCHES/{}", filename.to_string_lossy());
        card.file.push(sd_image::spec::File {
            path: patch_path.clone(),
            content: sd_image::spec::Content::Copy(patch_file),
        });
        let image = dir.join("card.img");
        card.build(&image, spec.parent().unwrap()).unwrap();
        assert!(sd_image::entry_exists(&image, &patch_path).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn fixture_audio_backend_resolves_the_case_artifact_directory() {
        let fixture = Fixture {
            name: "l6-recorder-playback".into(),
            model: "l6".into(),
            firmware_package: None,
            patches: vec![],
            installed_patches: vec![],
            sd_patches: None,
            sd_bundle_mode: None,
            sd_bundle: None,
            update_package: None,
            state_spec: None,
            card_spec: None,
            audio_device: Some("driver=wav,id=l6-audio,path={logs}/audio.wav".into()),
            audio_input: Vec::new(),
            gdb: true,
        };
        assert_eq!(
            audio_device_spec(&fixture, Path::new("/tmp/case-logs")),
            "driver=wav,id=l6-audio,path=/tmp/case-logs/audio.wav"
        );
        let default_fixture = Fixture {
            audio_device: None,
            audio_input: Vec::new(),
            gdb: false,
            ..fixture
        };
        assert_eq!(
            audio_device_spec(&default_fixture, Path::new("/tmp/case-logs")),
            "driver=none,id=l6-audio"
        );
    }
    #[test]
    fn gdb_socket_path_fits_qemus_unix_socket_limit() {
        assert!(gdb_socket_path().as_os_str().as_encoded_bytes().len() < 100);
    }
    #[test]
    fn fixture_copy_does_not_write_through_to_the_template() {
        let dir = temp();
        let source = dir.join("template");
        let copy = dir.join("copy");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("flash"), b"original").unwrap();
        copy_tree(&source, &copy).unwrap();
        assert_eq!(files(&source).unwrap(), files(&copy).unwrap());
        fs::write(copy.join("flash"), b"changed").unwrap();
        assert_eq!(fs::read(source.join("flash")).unwrap(), b"original");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn firmware_must_be_a_package_and_fixtures_are_not_local() {
        let root = l6max_host::paths::repository_root();
        let dir = temp();
        let qemu = dir.join("qemu");
        fs::write(&qemu, b"qemu").unwrap();
        fs::write(dir.join("L6max.bin"), b"").unwrap();
        fs::create_dir(dir.join("unpacked")).unwrap();
        let config = dir.join("local.toml");
        let write = |body: &str| fs::write(&config, format!("version=1\n{body}")).unwrap();
        let load = || load_config(&config, &root, false, Some(&qemu), None);
        write("[firmware]\nl6max='L6max.bin'\n");
        assert!(load().is_ok());
        write("[firmware]\nl6max='L6max.bin'\n[emulator.l6max.input_patch]\n1='/missing.wav'\n");
        assert!(load().is_ok(), "the driver ignores input patches");
        write("[firmware]\nl6max='unpacked'\n");
        assert!(load().err().unwrap().contains("update package"));
        write("[firmware]\nother='L6max.bin'\n");
        assert!(load().err().unwrap().contains("unknown model"));
        write("[[fixture]]\nname='prepared'\nmodel='l6max'\n");
        assert!(load().is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_packaged_app_needs_its_executable_and_qemu() {
        let dir = temp();
        assert!(resolve_app(&dir).is_err());
        fs::create_dir_all(dir.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(dir.join("Contents/Resources/libexec")).unwrap();
        fs::write(dir.join("Contents/MacOS/l6max-gui"), b"gui").unwrap();
        fs::write(
            dir.join("Contents/Resources/libexec/qemu-system-arm"),
            b"qemu",
        )
        .unwrap();
        let app = resolve_app(&dir).unwrap();
        assert!(app.executable.ends_with("Contents/MacOS/l6max-gui"));
        assert_eq!(app.qemu_sha256, format!("{:x}", Sha256::digest(b"qemu")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_case_fixture_comes_from_its_scenario() {
        let root = l6max_host::paths::repository_root();
        let catalog = root.join("tests/smoke");
        let config = Config {
            firmware: BTreeMap::from([("l6max".to_string(), PathBuf::from("L6max.bin"))]),
            qemu: PathBuf::new(),
            qemu_sha256: String::new(),
            app: None,
        };
        let scenario = Scenario::load(&catalog.join("scenarios/host-midi-sync.toml")).unwrap();
        let fixture = case_fixture(
            &config,
            &catalog,
            "l6max",
            "host-midi-sync",
            Some(&scenario),
        )
        .unwrap();
        assert_eq!(fixture.patches, ["midi-sync"]);
        assert!(fixture.state_spec.unwrap().is_file());
        assert!(
            case_fixture(&config, &catalog, "l6", "host-midi-sync", Some(&scenario))
                .err()
                .unwrap()
                .contains("no firmware package")
        );
    }

    #[test]
    fn every_scenario_card_has_a_valid_spec_for_each_model() {
        let catalog_dir = l6max_host::paths::repository_root().join("tests/smoke");
        let mut cards = 0;
        for entry in fs::read_dir(catalog_dir.join("scenarios")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "toml") {
                continue;
            }
            let scenario = Scenario::load(&path).unwrap();
            for model in &scenario.models {
                let fixture = Config {
                    firmware: BTreeMap::from([(model.clone(), PathBuf::from("firmware.bin"))]),
                    qemu: PathBuf::new(),
                    qemu_sha256: String::new(),
                    app: None,
                };
                let fixture = case_fixture(
                    &fixture,
                    &catalog_dir,
                    model,
                    &scenario.name,
                    Some(&scenario),
                )
                .unwrap();
                if let Some(spec_path) = &fixture.card_spec {
                    let spec = sd_image::spec::CardSpec::load(spec_path).unwrap();
                    assert_eq!(&spec.model, model, "{}", spec_path.display());
                    cards += 1;
                }
                if let Some(spec_path) = &fixture.state_spec {
                    let spec = l6nor::StateSpec::load(spec_path).unwrap();
                    assert_eq!(&spec.model, model, "{}", spec_path.display());
                }
            }
        }
        assert!(cards > 0);
    }

    #[test]
    fn package_placeholder_uses_the_selected_generated_package() {
        let dir = temp();
        let args = vec![
            "--qemu".into(),
            dir.join("qemu").into_os_string(),
            "--firmware-dir".into(),
            dir.join("firmware").into_os_string(),
            "--firmware-package".into(),
            dir.join("patched.bin").into_os_string(),
        ];
        let options = Options::parse_args(args).unwrap();
        assert_eq!(
            expand("{package}", &options),
            dir.join("patched.bin").display().to_string()
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn expected_failures_match_the_exact_step_and_error_and_expose_unexpected_passes() {
        let failures = [ExpectedFailure {
            model: "l6".into(),
            step: 10,
            contains: "ExpectRing".into(),
            reason: "Link to the tracked investigation".into(),
        }];
        for (step, detail, initial, expected) in [
            (10, "ExpectRing timed out", "FAIL", "XFAIL"),
            (9, "ExpectRing timed out", "FAIL", "FAIL"),
            (10, "input disconnected", "FAIL", "FAIL"),
            (10, "completed", "PASS", "XPASS"),
        ] {
            let mut result = CaseResult {
                case: "controls".into(),
                model: "l6".into(),
                backend: "headless".into(),
                outcome: initial.into(),
                detail: detail.into(),
                artifacts: PathBuf::new(),
            };
            apply_expected(&mut result, &json!({"failed_step":step}), &failures);
            assert_eq!(result.outcome, expected);
        }
    }
    #[test]
    fn scheduler_bounds_concurrency_and_isolates_exclusive_jobs() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let exclusive_overlap = AtomicUsize::new(0);
        let jobs: Vec<(usize, bool)> = (0..12).map(|i| (i, i % 5 == 4)).collect();
        let results = schedule(
            &jobs,
            3,
            |job| job.1,
            |job| {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                if job.1 && now != 1 {
                    exclusive_overlap.fetch_add(1, Ordering::SeqCst);
                }
                thread::sleep(Duration::from_millis(20));
                if job.1 && running.load(Ordering::SeqCst) != 1 {
                    exclusive_overlap.fetch_add(1, Ordering::SeqCst);
                }
                running.fetch_sub(1, Ordering::SeqCst);
                job.0
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(results, (0..12).collect::<Vec<_>>());
        assert_eq!(peak.load(Ordering::SeqCst), 3);
        assert_eq!(exclusive_overlap.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn timeout_stops_descendants_as_well_as_the_parent() {
        let dir = temp();
        let marker = dir.join("leaked");
        let mut command = Command::new("sh");
        command
            .args(["-c", "(sleep 2; printf leaked > \"$1\") & wait", "sh"])
            .arg(&marker);
        assert!(
            supervise(command, &dir, 1)
                .unwrap_err()
                .contains("wall-clock")
        );
        thread::sleep(Duration::from_millis(1600));
        assert!(!marker.exists(), "descendant escaped process-group cleanup");
        fs::remove_dir_all(dir).unwrap();
    }
}
