//! Build the pinned QEMU with the L6 and L6max board models.
//!
//! The submodule stays pristine: a copy is staged under
//! `emulator/qemu/build-source`, the board sources are installed into it, and it
//! is built there so rebuilds are incremental.
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const MESON_ENTRY: &str = "arm_common_ss.add(files('l6max.c'))";
const BOARD_SOURCES: &[&str] = &[
    "l6max.c",
    "l6-profile.h",
    "l6-panel-gpio.h",
    "l6-panel-timer.h",
    "l6max-display.h",
    "l6max-indicators.h",
    "l6max-usb.h",
    "l6-sai.h",
    "l6-edma.h",
    "l6-ccm-analog.h",
];

fn install_board_sources(repository: &Path, source: &Path) -> Result<(), String> {
    let board_dir = repository.join("emulator/qemu/hw/arm");
    let target_dir = source.join("hw/arm");
    for name in BOARD_SOURCES {
        fs::copy(board_dir.join(name), target_dir.join(name))
            .map_err(|error| format!("cannot install {name}: {error}"))?;
    }
    Ok(())
}

fn apply_patch(
    source: &Path,
    repository: &Path,
    name: &str,
    marker_file: &str,
    markers: &[&str],
) -> Result<(), String> {
    let patch = repository.join("emulator/qemu/patches").join(name);
    let patched = fs::read_to_string(source.join(marker_file))
        .map_err(|error| format!("could not read QEMU {marker_file}: {error}"))?;
    if markers.iter().all(|marker| patched.contains(marker)) {
        return Ok(());
    }

    let check = Command::new("patch")
        .current_dir(source)
        .args(["--dry-run", "--forward", "--batch", "--quiet", "-p1"])
        .arg("--input")
        .arg(&patch)
        .status()
        .map_err(|error| format!("could not check {name}: {error}"))?;
    if !check.success() {
        return Err(format!("{name} does not apply to this QEMU source"));
    }

    let status = Command::new("patch")
        .current_dir(source)
        .args(["--forward", "--batch", "-p1"])
        .arg("--input")
        .arg(&patch)
        .status()
        .map_err(|error| format!("could not apply {name}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("applying {name} exited with {status}"))
    }
}

fn apply_patches(source: &Path, repository: &Path) -> Result<(), String> {
    apply_patch(
        source,
        repository,
        "coreaudio-underrun.patch",
        "audio/coreaudio.m",
        &[
            "kAudioFormatFlagsNativeFloatPacked",
            "core->underruns++",
            "playback had",
            "L6_AUDIO_COREAUDIO_CAPTURE",
        ],
    )?;
    apply_patch(
        source,
        repository,
        "short-poll-timeout.patch",
        "util/qemu-timer.c",
        &["starves timers"],
    )?;
    apply_patch(
        source,
        repository,
        "icount-deadline-notify.patch",
        "accel/tcg/tcg-accel-ops-icount.c",
        &["icount_timers_stay_on_vcpu"],
    )?;
    apply_patch(
        source,
        repository,
        "softfloat-zero-multiply.patch",
        "fpu/softfloat.c",
        &["A zero times a zero or normal"],
    )?;
    apply_patch(
        source,
        repository,
        "sd-multiblock-transfer.patch",
        "hw/sd/sd.c",
        &["Take what fits in the current block"],
    )?;
    apply_patch(
        source,
        repository,
        "sd-write-coalescing.patch",
        "hw/sd/sd.c",
        &["sd_blk_write_multiple"],
    )
}

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

/// The pinned QEMU source is a Git submodule. Fetch it on first use, shallowly,
/// so a fresh clone builds with one command.
fn ensure_submodule(upstream: &Path) -> Result<(), String> {
    if upstream.join("VERSION").is_file() {
        return Ok(());
    }
    let repository = upstream.join("../../..");
    eprintln!("Fetching the pinned QEMU submodule (first run only)...");
    let status = Command::new("git")
        .current_dir(&repository)
        .args(["submodule", "update", "--init", "--depth", "1", "emulator/qemu/upstream"])
        .status()
        .map_err(|error| {
            format!(
                "could not run git to fetch the QEMU submodule ({error}); run `git submodule update --init emulator/qemu/upstream` or pass a QEMU 11.1.2 source path"
            )
        })?;
    if !status.success() {
        return Err(format!(
            "fetching the QEMU submodule failed ({status}); run `git submodule update --init emulator/qemu/upstream` or pass a QEMU 11.1.2 source path"
        ));
    }
    Ok(())
}

/// What to build. `source` overrides the pinned submodule with a QEMU 11.1.2 tree.
pub struct Request {
    pub repository: PathBuf,
    pub source: Option<PathBuf>,
    pub minimal: bool,
    /// Keep TCG plugin support, which costs CPU even with no plugin loaded.
    pub plugins: bool,
    pub optimization: Optimization,
}

/// How QEMU is compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Optimization {
    /// The platform compiler without LTO, for quick local rebuilds.
    Standard,
    /// LTO with this platform's checked-in profile, built with [`PGO_CLANG_VERSION`].
    Pgo,
    /// LTO with profile instrumentation, in its own build directory, for training.
    Instrumented,
}

/// The Clang that trained the checked-in profiles. Profile matching depends on
/// the compiler, so optimized builds require exactly this version.
pub const PGO_CLANG_VERSION: &str = "21.1.8";

/// Where an instrumented build leaves the binary.
pub const INSTRUMENTED_BUILD_DIR: &str = "build-pgo-instrumented";

/// The checked-in profile name for this host, when one is maintained.
pub fn pgo_platform() -> Option<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-arm64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        _ => None,
    }
}

/// The compressed profile for this host, whether or not it exists yet.
pub fn pgo_profile(repository: &Path) -> Option<PathBuf> {
    pgo_platform().map(|platform| {
        repository
            .join("emulator/qemu/pgo")
            .join(format!("{platform}.profdata.gz"))
    })
}

/// C and C++ compilers. Linux always uses Clang; macOS uses Homebrew's
/// `llvm@21` for optimized builds so both platforms match the profiles.
fn clang(optimizing: bool) -> Option<(String, String)> {
    let default = if cfg!(target_os = "linux") {
        Some(("clang", "clang++"))
    } else if cfg!(target_os = "macos") && optimizing {
        Some((
            "/opt/homebrew/opt/llvm@21/bin/clang",
            "/opt/homebrew/opt/llvm@21/bin/clang++",
        ))
    } else {
        None
    };
    default.map(|(cc, cxx)| {
        (
            env::var("CC").unwrap_or_else(|_| cc.into()),
            env::var("CXX").unwrap_or_else(|_| cxx.into()),
        )
    })
}

/// The LLD linker for LTO builds, overridable with `LLD`.
fn lld() -> String {
    env::var("LLD").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/opt/homebrew/opt/lld@21/bin/ld64.lld".into()
        } else {
            "lld".into()
        }
    })
}

/// `llvm-profdata` from the same LLVM, overridable with `LLVM_PROFDATA`.
pub fn llvm_profdata() -> String {
    env::var("LLVM_PROFDATA").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/opt/homebrew/opt/llvm@21/bin/llvm-profdata".into()
        } else {
            "llvm-profdata".into()
        }
    })
}

fn require_pgo_clang(cc: &str) -> Result<(), String> {
    let output = Command::new(cc)
        .arg("--version")
        .output()
        .map_err(|error| {
            format!("could not run {cc} ({error}); optimized builds need Clang {PGO_CLANG_VERSION}")
        })?;
    let version = String::from_utf8_lossy(&output.stdout);
    if version.contains(&format!("clang version {PGO_CLANG_VERSION}")) {
        Ok(())
    } else {
        Err(format!(
            "optimized builds need Clang {PGO_CLANG_VERSION}, which trained the profiles; {cc} reports {}",
            version.lines().next().unwrap_or("no version")
        ))
    }
}

/// Where a default build leaves the binary.
pub fn default_binary(repository: &Path) -> PathBuf {
    repository.join("emulator/qemu/build-source/build/qemu-system-arm")
}

/// A checkout (not a packaged app) has the board sources needed to build.
pub fn can_build(repository: &Path) -> bool {
    repository.join("emulator/qemu/hw/arm/l6max.c").is_file()
}

/// True when any board or model source is newer than `binary`.
pub fn is_stale(repository: &Path, binary: &Path) -> bool {
    let Ok(built) = fs::metadata(binary).and_then(|m| m.modified()) else {
        return true;
    };
    fn newest(directory: &Path) -> Option<SystemTime> {
        fs::read_dir(directory)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    newest(&path)
                } else {
                    entry.metadata().ok()?.modified().ok()
                }
            })
            .max()
    }
    [
        repository.join("emulator/qemu/hw"),
        repository.join("emulator/qemu/patches"),
    ]
    .into_iter()
    .filter_map(|path| newest(&path))
    .any(|source| source > built)
}

/// Build under an exclusive lock, so concurrent callers (for example two smoke
/// tools started together) build once and the rest reuse the result.
pub fn build_locked(request: &Request) -> Result<PathBuf, String> {
    let lock_path = request.repository.join("emulator/qemu/.build.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| format!("cannot open {}: {e}", lock_path.display()))?;
    lock.lock()
        .map_err(|e| format!("cannot lock the QEMU build: {e}"))?;
    let result = build(request);
    let _ = lock.unlock();
    result
}

pub fn build(request: &Request) -> Result<PathBuf, String> {
    let upstream = request.repository.join("emulator/qemu/upstream");
    let source = request.source.clone();
    let minimal = request.minimal;
    if source.is_none() {
        ensure_submodule(&upstream)?;
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
        let staged = request.repository.join("emulator/qemu/build-source");
        copy_source(&source, &staged)?;
        staged.canonicalize().map_err(|e| e.to_string())?
    } else {
        source
    };
    apply_patches(&source, &request.repository)?;
    install_board_sources(&request.repository, &source)?;
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

    let optimizing = request.optimization != Optimization::Standard;
    let compilers = clang(optimizing);
    if optimizing {
        let (cc, _) = compilers.as_ref().ok_or("optimized builds need Clang")?;
        require_pgo_clang(cc)?;
    }
    // An instrumented build keeps its own directory so the normal binary stays usable.
    let build_dir = if request.optimization == Optimization::Instrumented {
        let dir = source.join(INSTRUMENTED_BUILD_DIR);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        dir
    } else {
        source.join("build")
    };
    let mut cflags = Vec::new();
    let mut ldflags = Vec::new();
    match request.optimization {
        Optimization::Standard => {}
        Optimization::Pgo => match pgo_profile(&request.repository).filter(|p| p.is_file()) {
            Some(profile) => {
                // Configure recreates the build directory, so unpack beside it.
                let unpacked = source.join("qemu-pgo.profdata");
                let data = Command::new("gzip")
                    .arg("-dc")
                    .arg(&profile)
                    .output()
                    .map_err(|error| format!("could not run gzip: {error}"))?;
                if !data.status.success() {
                    return Err(format!("could not unpack {}", profile.display()));
                }
                fs::write(&unpacked, data.stdout).map_err(|e| e.to_string())?;
                cflags.push(format!("-fprofile-use={}", unpacked.display()));
                // Configure's compiler checks run with -Werror, and their test
                // programs' main() never matches the profile. Stale functions
                // only lose guidance, so keep the mismatch warnings quiet.
                cflags.push("-Wno-backend-plugin".into());
                cflags.push("-Wno-profile-instr-out-of-date".into());
            }
            None => eprintln!("qemu-build: no profile for this platform; building with LTO only"),
        },
        Optimization::Instrumented => {
            cflags.push("-fprofile-generate".into());
            ldflags.push("-fprofile-generate".into());
        }
    }
    if optimizing {
        ldflags.push(format!("-fuse-ld={}", lld()));
    }

    let mut configure = Command::new(source.join("configure"));
    let configure_dir = if request.optimization == Optimization::Instrumented {
        &build_dir
    } else {
        &source
    };
    configure.current_dir(configure_dir).args([
        "--target-list=arm-softmmu",
        "--disable-docs",
        "--disable-gtk",
        "--disable-sdl",
        "--disable-cocoa",
        "--disable-vnc",
        "--disable-tools",
        "--disable-werror",
        "--enable-fdt=auto",
    ]);
    if !request.plugins {
        configure.arg("--disable-plugins");
    }
    // CI bundles need TCG and the board's peripherals, without unrelated
    // optional host services discovered from the runner's installed libraries.
    if minimal {
        configure.args(["--without-default-features", "--enable-tcg"]);
        if cfg!(target_os = "linux") {
            configure.arg("--enable-pa");
        }
    }
    // Linux builds use Clang like macOS; CC and CXX select a versioned one.
    if let Some((cc, cxx)) = &compilers {
        configure
            .arg(format!("--cc={cc}"))
            .arg(format!("--cxx={cxx}"));
        if cfg!(target_os = "macos") {
            configure.arg(format!("--objcc={cc}"));
        }
    }
    // Homebrew's Clang does not find Xcode's SDK on its own.
    let sdkroot =
        if cfg!(target_os = "macos") && compilers.is_some() && env::var_os("SDKROOT").is_none() {
            let sdk = Command::new("xcrun")
                .arg("--show-sdk-path")
                .output()
                .map_err(|error| format!("could not run xcrun: {error}"))?;
            Some(String::from_utf8_lossy(&sdk.stdout).trim().to_owned())
        } else {
            None
        };
    if let Some(sdk) = &sdkroot {
        configure.env("SDKROOT", sdk);
    }
    if optimizing {
        configure.arg("--enable-lto");
        // LTO archives hold LLVM bitcode. An older Xcode's ar cannot index
        // it, leaving lld unable to find archive members, so use llvm@21's.
        if cfg!(target_os = "macos") && env::var_os("AR").is_none() {
            configure.env("AR", "/opt/homebrew/opt/llvm@21/bin/llvm-ar");
        }
    }
    if !cflags.is_empty() {
        configure
            .arg(format!("--extra-cflags={}", cflags.join(" ")))
            .arg(format!("--extra-cxxflags={}", cflags.join(" ")));
    }
    if !ldflags.is_empty() {
        configure.arg(format!("--extra-ldflags={}", ldflags.join(" ")));
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
    let mut ninja = Command::new("ninja");
    ninja.arg("-C").arg(&build_dir).arg("qemu-system-arm");
    if let Some(sdk) = &sdkroot {
        ninja.env("SDKROOT", sdk);
    }
    let status = ninja
        .status()
        .map_err(|error| format!("could not run Ninja ({error}); install it, for example `brew install ninja` or `apt install ninja-build`"))?;
    if !status.success() {
        return Err(format!("QEMU build exited with {status}"));
    }
    Ok(build_dir.join("qemu-system-arm"))
}

#[cfg(test)]
mod tests {
    use super::{BOARD_SOURCES, install_board_sources};
    use std::{fs, time::SystemTime};

    #[test]
    fn stages_all_shared_board_sources_including_profile_header() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qemu-build-board-sources-{}-{unique}",
            std::process::id()
        ));
        let board_dir = root.join("repository/emulator/qemu/hw/arm");
        let source_dir = root.join("qemu/hw/arm");
        fs::create_dir_all(&board_dir).unwrap();
        fs::create_dir_all(&source_dir).unwrap();
        for name in BOARD_SOURCES {
            fs::write(board_dir.join(name), format!("source {name}")).unwrap();
        }

        install_board_sources(&root.join("repository"), &root.join("qemu")).unwrap();

        assert_eq!(
            fs::read_to_string(source_dir.join("l6-profile.h")).unwrap(),
            "source l6-profile.h"
        );
        for name in BOARD_SOURCES {
            assert!(source_dir.join(name).is_file(), "missing staged {name}");
        }
        fs::remove_dir_all(root).unwrap();
    }
}
