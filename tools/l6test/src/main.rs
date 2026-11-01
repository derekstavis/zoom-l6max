//! Reproducible suite driver. All guest runners are supervised child processes.
mod gui_capture;
mod gui_probe;
mod pad_state;
mod screen_gif;

use l6max_host::engine::Options;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use smoke_core::{CaseResult, Scenario, model_name};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u32,
    qemu: PathBuf,
    qemu_sha256: String,
    fixture: Vec<Fixture>,
    #[serde(default)]
    legacy: BTreeMap<String, Legacy>,
    #[serde(default)]
    expected_failure: Vec<ExpectedFailure>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    name: String,
    model: String,
    firmware_dir: Option<PathBuf>,
    firmware_package: Option<PathBuf>,
    #[serde(default)]
    patches: Vec<String>,
    update_package: Option<PathBuf>,
    state_template: Option<PathBuf>,
    state_recipe: Option<PathBuf>,
    state_spec: Option<PathBuf>,
    sd_image: Option<PathBuf>,
    #[serde(default)]
    sound_pad: Vec<pad_state::SoundPad>,
    audio_device: Option<String>,
    #[serde(default)]
    gdb: bool,
    #[serde(default = "paced")]
    timing: String,
    #[serde(default)]
    update_bootloader: bool,
}
static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn interrupt(_: libc::c_int) {
    CANCELLED.store(true, std::sync::atomic::Ordering::Relaxed);
}
fn cancelled() -> bool {
    CANCELLED.load(std::sync::atomic::Ordering::Relaxed)
}
fn paced() -> String {
    "paced".into()
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
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {
    fixture: Option<String>,
    executable: Option<PathBuf>,
    executable_sha256: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFailure {
    case: String,
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
    fixture: Option<String>,
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
}
/// Each guest runs one single-threaded TCG engine plus its scenario process.
/// Leave headroom so concurrent guests keep their paced clocks.
fn default_jobs() -> usize {
    thread::available_parallelism().map_or(1, |cores| (cores.get() / 4).clamp(1, 4))
}
fn parse_args() -> Result<Args, String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let mut result = Args {
        command,
        config: PathBuf::from("smoke.local.toml"),
        suite: "quick".into(),
        case: None,
        backend: "headless".into(),
        model: "all".into(),
        output: None,
        build: true,
        jobs: default_jobs(),
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
fn resolve(base: &Path, path: &mut PathBuf) {
    if path.is_relative() {
        *path = base.join(&*path);
    }
}
fn load_config(path: &Path) -> Result<Config, String> {
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let mut config: Config = toml::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if config.version != 1 || config.qemu_sha256.len() != 64 {
        return Err(
            "configuration requires version=1 and a pinned 64-character qemu_sha256".into(),
        );
    }
    let base = path.parent().unwrap();
    resolve(base, &mut config.qemu);
    if hash(&config.qemu)? != config.qemu_sha256 {
        return Err("QEMU binary does not match the configured SHA-256".into());
    }
    let mut keys = BTreeSet::new();
    for fixture in &mut config.fixture {
        if !["l6", "l6max"].contains(&fixture.model.as_str())
            || ![
                "paced",
                "adaptive",
                "host-clock",
                "host-clock-multi",
                "split",
            ]
            .contains(&fixture.timing.as_str())
            || !keys.insert((fixture.name.clone(), fixture.model.clone()))
        {
            return Err("invalid or duplicate fixture/model/timing".into());
        }
        if fixture.firmware_dir.is_some() == fixture.firmware_package.is_some() {
            return Err("each fixture needs exactly one firmware_dir or firmware_package".into());
        }
        if !fixture.patches.is_empty() && fixture.firmware_package.is_none() {
            return Err("patches require a stock firmware_package".into());
        }
        pad_state::validate(&fixture.sound_pad)?;
        if !fixture.sound_pad.is_empty() && fixture.sd_image.is_none() {
            return Err("sound_pad requires an sd_image fixture".into());
        }
        if fixture.state_template.is_some() && fixture.state_recipe.is_some() {
            return Err("choose state_template or state_recipe, not both".into());
        }
        if fixture.state_recipe.is_some() && fixture.state_spec.is_some() {
            return Err("choose state_recipe or state_spec, not both".into());
        }
        for path in [
            &mut fixture.firmware_dir,
            &mut fixture.firmware_package,
            &mut fixture.update_package,
            &mut fixture.state_template,
            &mut fixture.state_recipe,
            &mut fixture.state_spec,
            &mut fixture.sd_image,
        ]
        .into_iter()
        .flatten()
        {
            resolve(base, path);
        }
        if let Some(path) = &fixture.state_spec {
            let spec = l6nor::StateSpec::load(path)?;
            if spec.model != fixture.model {
                return Err(format!(
                    "state spec {} is for {}, not the {} fixture",
                    path.display(),
                    spec.model,
                    fixture.model
                ));
            }
        }
        if let Some(path) = &fixture.state_recipe {
            let recipe = Scenario::load(path)?;
            if !recipe.models.contains(&fixture.model)
                || !recipe.backends.iter().any(|b| b == "headless")
            {
                return Err(
                    "state recipe must support the fixture model and headless backend".into(),
                );
            }
        }
    }
    for legacy in config.legacy.values_mut() {
        if let Some(executable) = &mut legacy.executable {
            resolve(base, executable);
            if legacy
                .executable_sha256
                .as_deref()
                .is_none_or(|sha| sha.len() != 64)
                || hash(executable)? != legacy.executable_sha256.as_deref().unwrap()
            {
                return Err(format!(
                    "configured legacy executable does not match its pinned SHA-256: {}",
                    executable.display()
                ));
            }
        } else if legacy.executable_sha256.is_some() {
            return Err("legacy executable_sha256 requires executable".into());
        }
    }
    for expected in &config.expected_failure {
        if expected.reason.is_empty() || expected.contains.is_empty() || expected.step == 0 {
            return Err("expected failures need an exact step, error substring and reason".into());
        }
    }
    Ok(config)
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
    } else if let Some(path) = &fixture.firmware_dir {
        path.clone()
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
    let state_hashes = if let Some(template) = &fixture.state_template {
        let hashes = files(template)?;
        if template.join("main-nor.bin").exists() {
            let nor = fs::read(template.join("main-nor.bin")).map_err(|e| e.to_string())?;
            if l6fw::model_from_main_nor(&nor)? != model {
                return Err("saved state belongs to a different model".into());
            }
            if !fixture.patches.is_empty()
                && nor.get(0x50000..0x50000 + main.len()) != Some(main.as_slice())
            {
                return Err("saved state MAIN differs from generated patched firmware; use a matching state or a state recipe".into());
            }
        }
        copy_tree(template, &state)?;
        if files(&state)? != hashes {
            return Err("state template changed while copying".into());
        }
        hashes
    } else {
        fs::create_dir_all(&state).map_err(|e| e.to_string())?;
        BTreeMap::new()
    };
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
        "--timing".into(),
        fixture.timing.clone().into(),
    ];
    if fixture.gdb {
        args.extend(["--gdb-socket".into(), gdb_socket_path().into_os_string()]);
    }
    let sd_hash = if let Some(card) = &fixture.sd_image {
        let target = dir.join("card.img");
        fs::copy(card, &target).map_err(|e| e.to_string())?;
        if !fixture.sound_pad.is_empty() {
            pad_state::apply(&target, &fixture.model, &fixture.sound_pad)?;
        }
        args.extend(["--sd-image".into(), target.clone().into_os_string()]);
        Some(hash(&target)?)
    } else {
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
    if fixture.update_bootloader {
        args.push("--update-bootloader".into());
    }
    let options = Options::parse_args(args)?;
    let manifest = json!({"fixture":fixture.name,"model":fixture.model,"firmware":files(&firmware)?,"package_sha256":selected_package.as_ref().map(|p|hash(p)).transpose()?,"patches":fixture.patches,"state_recipe":fixture.state_recipe.as_ref().map(|p|hash(p)).transpose()?,"update_package_sha256":fixture.update_package.as_ref().map(|p|hash(p)).transpose()? ,"state_template":state_hashes,"state_spec":state_spec,"sd_sha256":sd_hash,"sound_pad":if fixture.sd_image.is_some() { json!(fixture.sound_pad) } else { json!([]) },"audio_device":audio_device,"timing":fixture.timing,"update_bootloader":fixture.update_bootloader,"qemu_sha256":config.qemu_sha256});
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
        "--timing".into(),
        match options.timing {
            l6max_host::engine::Timing::Paced => "paced",
            l6max_host::engine::Timing::Adaptive => "adaptive",
            l6max_host::engine::Timing::HostClock => "host-clock",
            l6max_host::engine::Timing::HostClockMulti => "host-clock-multi",
            l6max_host::engine::Timing::Split => "split",
        }
        .into(),
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
fn copy_recipe(source: &Path, destination: &Path) -> Result<PathBuf, String> {
    fn visit(
        root: &Path,
        path: &Path,
        destination: &Path,
        seen: &mut BTreeSet<PathBuf>,
    ) -> Result<(), String> {
        let source = fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !source.starts_with(root) || !seen.insert(source.clone()) {
            return Err("state recipe include escapes its directory or repeats a file".into());
        }
        let text = fs::read_to_string(&source).map_err(|e| e.to_string())?;
        let value: toml::Value = toml::from_str(&text).map_err(|e| e.to_string())?;
        let target = destination.join(source.strip_prefix(root).map_err(|e| e.to_string())?);
        fs::create_dir_all(target.parent().ok_or("invalid recipe path")?)
            .map_err(|e| e.to_string())?;
        fs::write(&target, &text).map_err(|e| e.to_string())?;
        if let Some(steps) = value.get("step").and_then(toml::Value::as_array) {
            for step in steps {
                if step.get("action").and_then(toml::Value::as_str) == Some("include") {
                    let include = step
                        .get("path")
                        .and_then(toml::Value::as_str)
                        .ok_or("recipe include needs a path")?;
                    let child = source
                        .parent()
                        .ok_or("invalid recipe parent")?
                        .join(include);
                    let canonical = fs::canonicalize(&child).map_err(|e| e.to_string())?;
                    if !canonical.starts_with(root) {
                        return Err("state recipe include escapes its directory".into());
                    }
                    if !seen.contains(&canonical) {
                        visit(root, &canonical, destination, seen)?;
                    }
                }
            }
        }
        Ok(())
    }
    let source = fs::canonicalize(source).map_err(|e| e.to_string())?;
    let root = source.parent().ok_or("invalid state recipe path")?;
    visit(root, &source, destination, &mut BTreeSet::new())?;
    Ok(destination.join(source.file_name().ok_or("invalid state recipe name")?))
}
fn prepare_saved_state(
    root: &Path,
    recipe: &Path,
    options: &Options,
    dir: &Path,
    timeout: u64,
) -> Result<(), String> {
    if options.state_dir.is_none() {
        return Err("state recipe requires persistent state; the selected case is volatile".into());
    }
    let recipe_dir = dir.join("state-setup");
    fs::create_dir_all(&recipe_dir).map_err(|e| e.to_string())?;
    let recipe = copy_recipe(recipe, &recipe_dir.join("recipe"))?;
    let scenario = Scenario::load(&recipe)?;
    let mut setup = options.clone();
    setup.logs = recipe_dir.clone();
    smoke_core::devices::prepare(&scenario, &mut setup)?;
    let binary = dir
        .parent()
        .ok_or("missing run directory")?
        .join("bin/l6test");
    let mut command = Command::new(&binary);
    command
        .current_dir(root)
        .arg("__scenario")
        .arg(&recipe)
        .args(engine_args(&setup));
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("L6_") || key.to_string_lossy().starts_with("L6MAX_") {
            command.env_remove(key);
        }
    }
    command
        .env("SMOKE_CASE", "state-setup")
        .env("SMOKE_BACKEND", "headless");
    if !supervise(command, &recipe_dir, timeout)? {
        return Err("state recipe failed; inspect state-setup logs".into());
    }
    let report: Value = serde_json::from_slice(
        &fs::read(recipe_dir.join("scenario.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if report["outcome"] != "PASS" || files(setup.state_dir.as_ref().unwrap())?.is_empty() {
        return Err("state recipe did not produce a passing persistent state".into());
    }
    Ok(())
}
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
        if case.needs_configuration
            && case.scenario.is_none()
            && !config.legacy.contains_key(&case.id)
        {
            return Err(
                "requires an explicit [legacy.\"case-id\"] entry; see the case catalog and testing guide"
                    .into(),
            );
        }
        if case.scenario.is_none()
            && case.package.is_none()
            && config
                .legacy
                .get(&case.id)
                .and_then(|legacy| legacy.executable.as_ref())
                .is_none()
        {
            return Err(format!(
                "requires a hash-pinned executable in [legacy.\"{}\"]",
                case.id
            ));
        }
        if hash(&config.qemu)? != config.qemu_sha256 {
            return Err("QEMU changed since preflight".into());
        }
        let mut selected_fixture = fixture.clone();
        if case.volatile {
            if selected_fixture.state_recipe.is_some() {
                return Err("volatile cases cannot use a state recipe".into());
            }
            // A spec is rebuilt for every case, so it carries no state between
            // cases; volatile cases keep it and run on that disposable state.
            selected_fixture.state_template = None;
        }
        if case.no_sd {
            selected_fixture.sd_image = None;
        }
        let (mut options, mut manifest) = fixture_options(config, &selected_fixture, dir)?;
        if case.volatile && selected_fixture.state_spec.is_none() {
            options.state_dir = None;
        }
        if case.no_sd {
            options.sd_image = None;
            options.no_sd = true;
        }
        if let Some(recipe) = &selected_fixture.state_recipe {
            prepare_saved_state(root, recipe, &options, dir, case.wall_timeout_s)?;
            manifest["state_recipe_files"] = json!(files(&dir.join("state-setup/recipe"))?);
            if hash(&config.qemu)? != config.qemu_sha256 {
                return Err("QEMU changed during state preparation".into());
            }
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
            .map(|path| hash(path))
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
        let local = config.legacy.get(&case.id);
        let binary = if let Some(executable) = local.and_then(|legacy| legacy.executable.as_ref()) {
            executable.clone()
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
        if local
            .and_then(|legacy| legacy.executable_sha256.as_deref())
            .is_some_and(|expected| expected != binary_sha256)
        {
            return Err("legacy executable changed since preflight".into());
        }
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
        let arguments: Vec<String> = case
            .arguments
            .iter()
            .chain(local.into_iter().flat_map(|s| s.arguments.iter()))
            .map(|a| expand(a, &options))
            .collect();
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
        for (key, value) in case
            .environment
            .iter()
            .chain(local.into_iter().flat_map(|s| s.environment.iter()))
        {
            command.env(key, expand(value, &options));
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
            apply_expected(&mut result, &scenario, config);
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

fn apply_expected(result: &mut CaseResult, scenario: &Value, config: &Config) {
    if let Some(expected) = config
        .expected_failure
        .iter()
        .find(|e| e.case == result.case && e.model == result.model)
    {
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
            "Usage: cargo run -p l6test -- list | check | run [--config smoke.local.toml] [--suite NAME | --case ID] [--model l6|l6max|all] [--backend headless|gui|both] [--output NEW_DIR] [--no-build] [--jobs N]\n       cargo run -p l6test -- probe gui --keys TARGET,... --volatile [engine options]\n       cargo run -p l6test -- capture screen --out FILE.gif [capture options] --volatile [engine options]"
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
        let config = load_config(&args.config)?;
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
        for fixture in config
            .fixture
            .iter()
            .filter(|f| args.model == "all" || f.model == args.model)
        {
            fixture_options(
                &config,
                fixture,
                &output.join(format!("check-{}-{}", fixture.name, fixture.model)),
            )?;
        }
        fs::write(
            output.join("report.json"),
            serde_json::to_vec_pretty(&json!({"schema":1,"outcome":"PASS","preflight_only":true}))
                .unwrap(),
        )
        .map_err(|e| e.to_string())?;
        println!(
            "Preflight passed. QEMU is pinned by binary hash; source provenance is explicitly unattested."
        );
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
            || case.needs_configuration
                && case.scenario.is_none()
                && !config.legacy.contains_key(&case.id)
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
            let fixture_name = scenario.as_ref().map_or_else(
                || {
                    config
                        .legacy
                        .get(&case.id)
                        .and_then(|local| local.fixture.clone())
                        .or_else(|| case.fixture.clone())
                        .unwrap_or_else(|| "prepared".into())
                },
                |s| s.fixture.clone(),
            );
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
                    fixture_name: fixture_name.clone(),
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
        let result = if let Some(fixture) = config
            .fixture
            .iter()
            .find(|f| f.model == job.model && f.name == job.fixture_name)
        {
            run_case(
                root,
                &catalog_dir,
                &config,
                fixture,
                job.case,
                job.backend,
                &job.dir,
            )
        } else {
            CaseResult {
                case: job.case.id.clone(),
                model: job.model.clone(),
                backend: job.backend.into(),
                outcome: "BLOCKED".into(),
                detail: format!("missing fixture {}/{}", job.fixture_name, job.model),
                artifacts: job.dir.clone(),
            }
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
    fixture_name: String,
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
    fn fixture_audio_backend_resolves_the_case_artifact_directory() {
        let fixture = Fixture {
            name: "l6-recorder-playback".into(),
            model: "l6".into(),
            firmware_dir: None,
            firmware_package: None,
            patches: vec![],
            update_package: None,
            state_template: None,
            state_recipe: None,
            state_spec: None,
            sd_image: None,
            sound_pad: vec![],
            audio_device: Some("driver=wav,id=l6-audio,path={logs}/audio.wav".into()),
            gdb: true,
            timing: "paced".into(),
            update_bootloader: false,
        };
        assert_eq!(
            audio_device_spec(&fixture, Path::new("/tmp/case-logs")),
            "driver=wav,id=l6-audio,path=/tmp/case-logs/audio.wav"
        );
        let default_fixture = Fixture {
            audio_device: None,
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
    fn state_recipe_snapshot_preserves_includes_and_rejects_escape() {
        let dir = temp();
        let source = dir.join("source");
        fs::create_dir_all(source.join("fragments")).unwrap();
        let recipe = source.join("recipe.toml");
        fs::write(&recipe, "version=1\nname='setup'\nmodels=['l6']\nfixture='fresh'\n[[step]]\naction='include'\npath='fragments/wait.toml'\n").unwrap();
        fs::write(
            source.join("fragments/wait.toml"),
            "[[step]]\naction='delay'\nguest_ms=50\n",
        )
        .unwrap();
        let copy = copy_recipe(&recipe, &dir.join("copy")).unwrap();
        fs::write(source.join("fragments/wait.toml"), "invalid").unwrap();
        assert!(Scenario::load(&copy).is_ok());
        fs::write(
            dir.join("outside.toml"),
            "[[step]]\naction='delay'\nguest_ms=50\n",
        )
        .unwrap();
        fs::write(
            &recipe,
            "[[step]]\naction='include'\npath='../outside.toml'\n",
        )
        .unwrap();
        assert!(
            copy_recipe(&recipe, &dir.join("unsafe-copy"))
                .unwrap_err()
                .contains("escapes")
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn fixture_rejects_conflicting_state_and_patch_inputs() {
        let dir = temp();
        fs::write(dir.join("qemu"), b"qemu").unwrap();
        let qemu_hash = format!("{:x}", Sha256::digest(b"qemu"));
        let config = dir.join("fixtures.toml");
        let header = format!("version=1\nqemu='qemu'\nqemu_sha256='{qemu_hash}'\n");
        fs::write(&config, format!("{header}[[fixture]]\nname='prepared'\nmodel='l6'\nfirmware_dir='firmware'\npatches=['midi-sync']\n")).unwrap();
        assert!(
            load_config(&config)
                .err()
                .unwrap()
                .contains("firmware_package")
        );
        fs::write(&config, format!("{header}[[fixture]]\nname='prepared'\nmodel='l6'\nfirmware_package='firmware.bin'\nstate_template='state'\nstate_recipe='recipe.toml'\n")).unwrap();
        assert!(
            load_config(&config)
                .err()
                .unwrap()
                .contains("state_template or state_recipe")
        );
        fs::write(&config, format!("{header}[[fixture]]\nname='prepared'\nmodel='l6'\nfirmware_package='firmware.bin'\nstate_spec='spec.toml'\nstate_recipe='recipe.toml'\n")).unwrap();
        assert!(
            load_config(&config)
                .err()
                .unwrap()
                .contains("state_recipe or state_spec")
        );
        fs::write(dir.join("spec.toml"), "version=1\nmodel='l6max'\n").unwrap();
        fs::write(&config, format!("{header}[[fixture]]\nname='prepared'\nmodel='l6'\nfirmware_package='firmware.bin'\nstate_spec='spec.toml'\n")).unwrap();
        assert!(load_config(&config).err().unwrap().contains("is for l6max"));
        fs::remove_dir_all(dir).unwrap();
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
    fn wrong_binary_hash_is_blocked_before_any_guest_execution() {
        let dir = temp();
        fs::write(dir.join("qemu"), b"wrong binary").unwrap();
        fs::write(
            dir.join("fixtures.toml"),
            format!(
                "version=1\nqemu='qemu'\nqemu_sha256='{}'\nfixture=[]\n",
                "0".repeat(64)
            ),
        )
        .unwrap();
        assert!(
            load_config(&dir.join("fixtures.toml"))
                .err()
                .unwrap()
                .contains("SHA-256")
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn external_legacy_executable_must_be_hash_pinned_and_resolves_from_config() {
        let dir = temp();
        fs::write(dir.join("qemu"), b"qemu").unwrap();
        fs::write(dir.join("bundle-gui"), b"gui executable").unwrap();
        let executable_hash = format!("{:x}", Sha256::digest(b"gui executable"));
        fs::write(
            dir.join("fixtures.toml"),
            format!(
                "version=1\nqemu='qemu'\nqemu_sha256='{}'\nfixture=[]\n[legacy.\"gui-menu\"]\nexecutable='bundle-gui'\nexecutable_sha256='{}'\n",
                format!("{:x}", Sha256::digest(b"qemu")),
                executable_hash
            ),
        )
        .unwrap();
        let config = load_config(&dir.join("fixtures.toml")).unwrap();
        let legacy = &config.legacy["gui-menu"];
        assert_eq!(
            legacy.executable.as_deref(),
            Some(fs::canonicalize(dir.join("bundle-gui")).unwrap().as_path())
        );
        assert_eq!(
            legacy.executable_sha256.as_deref(),
            Some(executable_hash.as_str())
        );

        fs::write(
            dir.join("fixtures.toml"),
            format!(
                "version=1\nqemu='qemu'\nqemu_sha256='{}'\nfixture=[]\n[legacy.\"gui-menu\"]\nexecutable='bundle-gui'\nexecutable_sha256='{}'\n",
                format!("{:x}", Sha256::digest(b"qemu")),
                "0".repeat(64)
            ),
        )
        .unwrap();
        assert!(load_config(&dir.join("fixtures.toml")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn expected_failures_match_the_exact_step_and_error_and_expose_unexpected_passes() {
        let config:Config=toml::from_str("version=1\nqemu='unused'\nqemu_sha256='unused'\nfixture=[]\n[[expected_failure]]\ncase='controls'\nmodel='l6'\nstep=10\ncontains='ExpectRing'\nreason='tracked issue'\n").unwrap();
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
            apply_expected(&mut result, &json!({"failed_step":step}), &config);
            assert_eq!(result.outcome, expected);
        }
    }
    #[test]
    fn a_saved_state_for_the_other_model_is_rejected_before_copying() {
        let dir = temp();
        let firmware = dir.join("firmware");
        let template = dir.join("template");
        fs::create_dir_all(&firmware).unwrap();
        fs::create_dir_all(&template).unwrap();
        let mut main = Vec::new();
        main.extend(0x20010000u32.to_le_bytes());
        main.extend(0x80001415u32.to_le_bytes());
        fs::write(firmware.join("main_firmware.bin"), &main).unwrap();
        let mut nor = vec![0xff; 0x50008];
        nor[0x50000..0x50004].copy_from_slice(&0x20010000u32.to_le_bytes());
        nor[0x50004..].copy_from_slice(&0x80000415u32.to_le_bytes());
        fs::write(template.join("main-nor.bin"), nor).unwrap();
        let config: Config =
            toml::from_str("version=1\nqemu='unused'\nqemu_sha256='unused'\nfixture=[]\n").unwrap();
        let fixture = Fixture {
            name: "prepared".into(),
            model: "l6".into(),
            firmware_dir: Some(firmware),
            firmware_package: None,
            patches: vec![],
            update_package: None,
            state_template: Some(template),
            state_recipe: None,
            state_spec: None,
            sd_image: None,
            sound_pad: vec![],
            audio_device: None,
            gdb: false,
            timing: "paced".into(),
            update_bootloader: false,
        };
        let error = fixture_options(&config, &fixture, &dir.join("run"))
            .err()
            .unwrap();
        assert!(error.contains("different model"));
        assert!(!dir.join("run/state/main-nor.bin").exists());
        fs::remove_dir_all(dir).unwrap();
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
