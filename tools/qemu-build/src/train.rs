//! Refresh this platform's checked-in QEMU profile.
//!
//! Builds an instrumented QEMU, runs a fixed headless workload against the
//! firmware named in a local config, merges the counts and writes
//! `emulator/qemu/pgo/<platform>.profdata.gz` with its manifest.
use qemu_build::{
    Optimization, PGO_CLANG_VERSION, Request, build_locked, llvm_profdata, pgo_platform,
    pgo_profile,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Audio-heavy cases: eight input tracks, an EQ change, and L6 input.
const TRAINING_CASES: &[&str] = &[
    "l6max-audio-input",
    "l6max-eq-audio-update",
    "l6-audio-input",
];
/// Guest seconds of idle running for each model.
const IDLE_SECONDS: &str = "60";

pub fn run(repository: &Path, config: &Path) -> Result<PathBuf, String> {
    let platform = pgo_platform().ok_or("no profile is maintained for this platform")?;
    let config = config
        .canonicalize()
        .map_err(|error| format!("{}: {error}", config.display()))?;
    let qemu = build_locked(&Request {
        repository: repository.to_owned(),
        source: None,
        // Shipped builds use the minimal configuration.
        minimal: true,
        plugins: false,
        optimization: Optimization::Instrumented,
    })?;
    let work = qemu.parent().unwrap().join("training");
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|e| format!("cannot clear {}: {e}", work.display()))?;
    }
    let raw = work.join("raw");
    fs::create_dir_all(&raw).map_err(|e| e.to_string())?;
    // macOS needs continuous mode: test cases stop QEMU before its exit
    // handlers run. Linux writes complete profiles at exit, and its
    // continuous mode needs extra instrumentation flags.
    let pattern = if cfg!(target_os = "macos") {
        "qemu-%c%p.profraw"
    } else {
        "qemu-%p.profraw"
    };

    let cargo = |package: &str| {
        let mut command = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        command
            .current_dir(repository)
            .args(["run", "--locked", "--release", "-q", "-p", package, "--"])
            .env("LLVM_PROFILE_FILE", raw.join(pattern));
        command
    };
    for case in TRAINING_CASES {
        eprintln!("qemu-build: training on {case}");
        let status = cargo("l6test")
            .args(["run", "--config"])
            .arg(&config)
            .arg("--qemu")
            .arg(&qemu)
            .args([
                "--case",
                case,
                "--backend",
                "headless",
                "--no-build",
                "--jobs",
                "1",
            ])
            .arg("--output")
            .arg(work.join(case))
            .status()
            .map_err(|error| format!("could not run l6test: {error}"))?;
        if !status.success() {
            return Err(format!(
                "training case {case} failed; see {}",
                work.join(case).display()
            ));
        }
    }
    for model in ["l6max", "l6"] {
        eprintln!("qemu-build: training on {model} idle");
        let status = cargo("l6max-host")
            .args(["--seconds", IDLE_SECONDS, "--config"])
            .arg(&config)
            .args(["--model", model, "--qemu"])
            .arg(&qemu)
            .args(["--volatile", "--no-sd", "--logs"])
            .arg(work.join(format!("idle-{model}")))
            .status()
            .map_err(|error| format!("could not run l6max-host: {error}"))?;
        if !status.success() {
            return Err(format!("idle training for {model} failed"));
        }
    }

    let profiles: Vec<PathBuf> = fs::read_dir(&raw)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "profraw"))
        .collect();
    if profiles.is_empty() {
        return Err("training wrote no profiles".into());
    }
    let merged = work.join("qemu.profdata");
    let status = Command::new(llvm_profdata())
        .arg("merge")
        .arg("-o")
        .arg(&merged)
        .args(&profiles)
        .status()
        .map_err(|error| format!("could not run llvm-profdata: {error}"))?;
    if !status.success() {
        return Err("llvm-profdata merge failed".into());
    }

    let profile = pgo_profile(repository).unwrap();
    fs::create_dir_all(profile.parent().unwrap()).map_err(|e| e.to_string())?;
    let compressed = Command::new("gzip")
        .args(["-9", "-n", "-c"])
        .arg(&merged)
        .output()
        .map_err(|error| format!("could not run gzip: {error}"))?;
    if !compressed.status.success() {
        return Err("gzip failed".into());
    }
    fs::write(&profile, compressed.stdout).map_err(|e| e.to_string())?;

    let commit = Command::new("git")
        .current_dir(repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    let cases = TRAINING_CASES
        .iter()
        .map(|case| format!("\"{case}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        profile.with_file_name(format!("{platform}.toml")),
        format!(
            "# Written by `qemu-build --train-pgo`.\nclang = \"{PGO_CLANG_VERSION}\"\nsource = \"{commit}\"\ncases = [{cases}]\nidle_seconds = {IDLE_SECONDS}\nprofiles = {}\n",
            profiles.len()
        ),
    )
    .map_err(|e| e.to_string())?;
    Ok(profile)
}
