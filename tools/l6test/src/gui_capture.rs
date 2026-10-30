//! Capture the live emulator panel through GPUI's offscreen renderer.
use l6max_host::{engine::Options, headless::Headless};
use serde_json::json;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

pub fn run(arguments: Vec<OsString>) -> Result<(), String> {
    let mut args = arguments.into_iter();
    let mut output = None;
    let mut wait_guest_ms = 45_000u32;
    let mut isolated_state = false;
    let mut has_logs = false;
    let mut engine_args = Vec::new();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--out") => {
                output = Some(PathBuf::from(
                    args.next().ok_or("--out requires a PNG path")?,
                ));
            }
            Some("--wait-guest-ms") => {
                wait_guest_ms = args
                    .next()
                    .ok_or("--wait-guest-ms requires a value")?
                    .to_str()
                    .ok_or("wait duration must be UTF-8")?
                    .parse()
                    .map_err(|_| "invalid --wait-guest-ms value")?;
            }
            Some("--help" | "-h") => {
                println!(
                    "Usage: l6test capture gui --out FILE.png --volatile|--state-dir DIR [--wait-guest-ms N] [engine options]\n\nBoots an isolated L6max guest, samples its live panel and LCD, then renders the emulator UI with GPUI's offscreen renderer. No display server is required."
                );
                return Ok(());
            }
            _ => {
                isolated_state |= argument == "--volatile" || argument == "--state-dir";
                has_logs |= argument == "--logs";
                engine_args.push(argument);
            }
        }
    }
    let output = output.ok_or("--out FILE.png is required")?;
    if output.extension().and_then(|ext| ext.to_str()) != Some("png") {
        return Err("--out path must end in .png".into());
    }
    if output.exists() {
        return Err(format!("refusing to overwrite {}", output.display()));
    }
    if !isolated_state {
        return Err("specify --volatile or a disposable --state-dir".into());
    }
    if wait_guest_ms == 0 || wait_guest_ms > 600_000 {
        return Err("--wait-guest-ms must be 1..600000".into());
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if !has_logs {
        let logs = output.with_extension("logs");
        if logs.exists() {
            return Err(format!(
                "refusing to overwrite log directory {}",
                logs.display()
            ));
        }
        engine_args.push(OsString::from("--logs"));
        engine_args.push(logs.into_os_string());
    }
    let options = Options::parse_args(engine_args)?;
    let guest = Headless::start(&options)?;
    if guest.engine.model() != l6fw::Model::L6max {
        return Err("GUI capture requires L6max firmware".into());
    }
    let deadline = Instant::now() + Duration::from_secs(180);
    while guest.guest_ms() < wait_guest_ms {
        guest.sample_display().map_err(|error| error.to_string())?;
        if Instant::now() > deadline {
            return Err("guest clock stalled during GUI capture".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    guest.sample_display().map_err(|error| error.to_string())?;
    // Let the indicator sampler publish one complete frame at the selected state.
    let stable_until = guest.guest_ms().saturating_add(1000);
    while guest.guest_ms() < stable_until {
        guest.sample_display().map_err(|error| error.to_string())?;
        if Instant::now() > deadline {
            return Err("guest clock stalled while settling the panel".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let inputs = guest
        .engine
        .inputs
        .as_ref()
        .ok_or("QEMU input unavailable")?;
    let panel = inputs.panel.snapshot();
    let display = guest
        .sample_display()
        .map_err(|error| error.to_string())?
        .unwrap_or([0; 1024]);
    let data_path = output.with_extension("panel.json");
    if data_path.exists() {
        return Err(format!("refusing to overwrite {}", data_path.display()));
    }
    fs::write(
        &data_path,
        serde_json::to_vec_pretty(&json!({
            "indicators": panel.indicators,
            "indicator_levels": panel.indicator_levels,
            "switches": vec![false; 54],
            "analog": inputs.analog_positions(),
            "powered": true,
            "display": display.to_vec(),
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    let root = l6max_host::paths::repository_root();
    let build = Command::new("cargo")
        .current_dir(&root)
        .args([
            "build",
            "--offline",
            "--locked",
            "-p",
            "l6max-gui",
            "--features",
            "headless-screenshots",
        ])
        .output()
        .map_err(|error| format!("build GPUI screenshot renderer: {error}"))?;
    if !build.status.success() {
        return Err(format!(
            "build GPUI screenshot renderer failed: {}",
            String::from_utf8_lossy(&build.stderr)
        ));
    }
    let metadata = Command::new("cargo")
        .current_dir(&root)
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .output()
        .map_err(|error| error.to_string())?;
    if !metadata.status.success() {
        return Err("cargo metadata failed while locating l6max-gui".into());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata.stdout).map_err(|error| error.to_string())?;
    let binary = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .ok_or("cargo metadata omitted target directory")?,
    )
    .join("debug/l6max-gui");
    let rendered = Command::new(binary)
        .arg("--render-panel")
        .arg("l6max")
        .arg("--data")
        .arg(&data_path)
        .arg("--output")
        .arg(&output)
        .output()
        .map_err(|error| format!("start offscreen GPUI renderer: {error}"))?;
    if !rendered.status.success() {
        return Err(format!(
            "offscreen GPUI renderer failed: {}",
            String::from_utf8_lossy(&rendered.stderr)
        ));
    }
    println!(
        "{}; guest_ms={}, ring_masks={:?}; panel data: {}",
        String::from_utf8_lossy(&rendered.stdout).trim(),
        guest.guest_ms(),
        l6max_host::indicators::rings(panel.indicators),
        data_path.display()
    );
    Ok(())
}
