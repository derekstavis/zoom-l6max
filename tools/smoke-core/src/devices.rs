//! Typed peripheral operations. Sequencing, timing and artifacts stay in the runner.
use crate::{Scenario, Session, scenario::PadAssignmentSource, waveform};
use l6max_host::{engine::Options, headless::Headless};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum Operation {
    SdCard {
        inserted: bool,
    },
    UsbDescriptors,
    UsbStorage,
    UsbMidi {
        value: u8,
    },
    HostMidiRoundTrip,
    DirectUsbMidiRoundTrip {
        cable: u8,
        expect_ring_change: bool,
    },
    L6maxDinMixerControl {
        expect_din_ring_change: bool,
    },
    L6DinUsbForwarding,
    HostMidiSync,
    L6MidiSync,
    L6MidiRingPinSweep {
        cc: u8,
    },
    L6MidiMuteIndicators,
    UsbSysex {
        requests: Vec<crate::scenario::UsbSysexRequest>,
    },
    EditorPadAssignments,
    EditorPadPlayback {
        pad: u8,
    },
    L6maxUiPadAssignment {
        pad: u8,
    },
    SoundPadPlayback {
        pad: u8,
        target: u32,
        assignment: PadAssignmentSource,
        require_pad_led_change: bool,
        playback_window_ms: u32,
        require_stereo_meter: bool,
        expect_source: Option<String>,
        max_residual_db: f64,
    },
    L6RecorderPlayback {
        target: u32,
    },
    L6maxRecorderPlayback {
        target: u32,
    },
    ExpectMemory {
        address: u32,
        bytes: Vec<u8>,
    },
    ExpectSdPatch {
        loaded: bool,
        chain_count: Option<u8>,
    },
    ExpectHeap {
        minimum_free: u32,
    },
    HoldChord {
        targets: Vec<u32>,
        duration_ms: u32,
    },
    AudioCallbacks,
    KnobDialog,
    FirmwareUpdate,
    WasmPatch,
    WarmReset,
    Restart,
    ExpectRecorder,
    PowerPress {
        hold_ms: u32,
        powered: bool,
    },
    ExpectSettings {
        restored: bool,
    },
    PanelEvent {
        target: u32,
        column: u8,
        row: u8,
        event: u8,
    },
    L6MainGpioEvent {
        target: u32,
        event: u8,
    },
    L6UsbStartup,
    L6DinEventDispatch {
        group: u8,
        cc: u8,
        value: u8,
    },
    L6maxTapHandler,
    AudioInputTransport {
        command: String,
        position_ms: i32,
    },
    AudioInputLoad {
        channel: u32,
        loaded: bool,
    },
    ExpectAudioInput {
        playing: bool,
        min_ms: u32,
        max_ms: u32,
    },
    ExpectTone {
        file: Option<String>,
        hz: f64,
        levels: [Option<f64>; 2],
        tolerance_db: f64,
    },
    ExpectCardEntry {
        path: String,
    },
    L6InputSweep,
    L6DinSweep,
    L6maxInputTiming,
    L6maxPanelKeyMap,
    L6maxChannelSignalPwm,
    L6LevelMeterPwm,
}

/// Configure only the hardware instrumentation explicitly requested by a scenario.
pub fn prepare(scenario: &Scenario, options: &mut Options) -> Result<(), String> {
    if scenario.step.iter().any(|step| {
        matches!(
            step.action,
            crate::Action::L6maxChannelSignalPwm | crate::Action::L6LevelMeterPwm
        )
    }) {
        options.usb_enabled = false;
        options.midi_enabled = false;
        options.gdb_socket =
            Some(std::env::temp_dir().join(format!("l6sp-{}.sock", std::process::id())));
    }
    if scenario.name == "factory-protocol" {
        prepare_factory_fixture(options)?;
    }
    if (scenario.main_sha256.is_some() || !scenario.main_sha256_any.is_empty())
        && scenario.sd_patches.as_ref().is_none_or(Vec::is_empty)
    {
        use sha2::{Digest, Sha256};
        let main = selected_main_image(options)?;
        let actual = format!("{:x}", Sha256::digest(main));
        let accepted: Vec<_> = scenario
            .main_sha256
            .iter()
            .chain(&scenario.main_sha256_any)
            .collect();
        if !accepted
            .iter()
            .any(|expected| actual.eq_ignore_ascii_case(expected))
        {
            return Err(format!(
                "firmware MAIN hash {actual} does not match scenario guard {accepted:?}"
            ));
        }
    }
    if scenario.step.iter().any(|s| {
        matches!(
            s.action,
            crate::Action::AudioCallbacks
                | crate::Action::ExpectSettings { .. }
                | crate::Action::ExpectRecorder
        )
    }) {
        use sha2::{Digest, Sha256};
        let main =
            fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
        let hash = format!("{:x}", Sha256::digest(&main));
        if ![
            "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799",
            "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461",
        ]
        .contains(&hash.as_str())
        {
            return Err("unsupported firmware revision for callback/settings addresses".into());
        }
        if let Some(path) = options
            .state_dir
            .as_ref()
            .map(|p| p.join("main-nor.bin"))
            .filter(|p| p.exists())
        {
            let nor = fs::read(path).map_err(|e| e.to_string())?;
            let offset = 0x50000; // l6fw::model_from_main_nor and the modeled installer.
            if nor.get(offset..offset + main.len()) != Some(main.as_slice()) {
                return Err("installed MAIN differs from selected firmware revision".into());
            }
        }
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::AudioCallbacks))
    {
        if !matches!(
            scenario.step.first().map(|s| &s.action),
            Some(crate::Action::AudioCallbacks)
        ) {
            return Err(
                "audio-callbacks must be the first step so callbacks cannot be missed".into(),
            );
        }
        options.start_paused = true;
        options.gdb_socket = Some(options.logs.join("audio.gdb.sock"));
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::PanelEvent { .. }))
    {
        let main =
            fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(main));
        if hash != "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799" {
            return Err("panel-event requires the hash-verified stock L6 MAIN image".into());
        }
        options.gdb_socket = Some(options.logs.join("panel-event-map.gdb.sock"));
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::L6MainGpioEvent { .. }))
    {
        use sha2::{Digest, Sha256};
        let main =
            fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
        let hash = format!("{:x}", Sha256::digest(main));
        if hash != "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799" {
            return Err("l6-main-gpio-event requires the hash-verified stock L6 MAIN image".into());
        }
        options.gdb_socket = Some(options.logs.join("l6-main-gpio-event.gdb.sock"));
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::L6maxTapHandler))
    {
        use sha2::{Digest, Sha256};
        let main =
            fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
        let hash = format!("{:x}", Sha256::digest(main));
        if hash != "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461" {
            return Err(
                "l6max-tap-handler requires the hash-verified stock L6max MAIN image".into(),
            );
        }
        options.gdb_socket = Some(options.logs.join("l6max-tap-handler.gdb.sock"));
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::L6UsbStartup))
    {
        if !matches!(
            scenario.step.first().map(|step| &step.action),
            Some(crate::Action::L6UsbStartup)
        ) {
            return Err("l6-usb-startup must be the first scenario action".into());
        }
        if scenario.main_sha256.is_none() {
            return Err("l6-usb-startup requires an exact MAIN firmware hash guard".into());
        }
        options.start_paused = true;
        options.usb_enabled = false;
        options.midi_enabled = false;
        options.gdb_socket = Some(options.logs.join("l6-usb-startup.gdb.sock"));
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::L6DinEventDispatch { .. }))
    {
        use sha2::{Digest, Sha256};
        let main =
            fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
        let hash = format!("{:x}", Sha256::digest(main));
        if hash != "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799" {
            return Err(
                "l6-din-event-dispatch requires the hash-verified stock L6 MAIN image".into(),
            );
        }
        options.start_paused = true;
        options.usb_enabled = false;
        options.midi_enabled = false;
        options.gdb_socket =
            Some(std::env::temp_dir().join(format!("l6ed-{}.sock", std::process::id())));
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::L6DinSweep))
    {
        use sha2::{Digest, Sha256};
        let main =
            fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
        let hash = format!("{:x}", Sha256::digest(main));
        if hash != "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799" {
            return Err("l6-din-sweep requires the hash-verified stock L6 MAIN image".into());
        }
        options.usb_enabled = false;
        options.midi_enabled = true;
        options.gdb_socket = Some(options.logs.join("l6-din-sweep.gdb.sock"));
    }
    if scenario.step.iter().any(|s| {
        matches!(
            s.action,
            crate::Action::HostMidiRoundTrip | crate::Action::DirectUsbMidiRoundTrip { .. }
        )
    }) {
        options.usb_enabled = scenario
            .step
            .iter()
            .any(|s| matches!(s.action, crate::Action::HostMidiRoundTrip));
        options.midi_enabled = false;
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::L6maxDinMixerControl { .. }))
    {
        options.usb_enabled = false;
        options.midi_enabled = true;
    }
    if scenario
        .step
        .iter()
        .any(|s| matches!(s.action, crate::Action::L6MidiSync))
    {
        options.usb_enabled = false;
        options.midi_enabled = false;
        options.gdb_socket = Some(options.logs.join("l6-midi-sync.gdb.sock"));
    }
    options.trace_sd |= scenario.trace_sd
        || scenario
            .step
            .iter()
            .any(|s| matches!(s.action, crate::Action::SdCard { .. }));
    // Typed USB operations own the host tokens; present VBUS from boot so the
    // firmware initializes its USB class tasks, without starting the native
    // MIDI bridge which would compete for those same endpoints.
    if scenario.step.iter().any(|s| {
        matches!(
            s.action,
            crate::Action::UsbDescriptors
                | crate::Action::UsbStorage
                | crate::Action::UsbMidi { .. }
                | crate::Action::UsbSysex { .. }
                | crate::Action::EditorPadAssignments
                | crate::Action::EditorPadPlayback { .. }
                | crate::Action::L6maxUiPadAssignment { .. }
        )
    }) {
        options.usb_enabled = false;
        options.usb_cable_at_start = true;
        if !scenario
            .step
            .iter()
            .any(|step| matches!(step.action, crate::Action::L6maxDinMixerControl { .. }))
        {
            options.midi_enabled = false;
        }
    }
    Ok(())
}

fn prepare_factory_fixture(options: &mut Options) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    const FACTORY_MAIN: &str = "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461";
    let state = options.logs.join("factory-state");
    let card = options.logs.join("factory-card.img");
    if options.state_dir.as_ref() == Some(&state)
        && options.sd_image.as_ref() == Some(&card)
        && state.is_dir()
        && card.is_file()
    {
        return Ok(());
    }
    if options.state_dir.is_some() || options.sd_image.is_some() {
        return Err("factory-protocol owns its disposable state and SD fixture".into());
    }
    let main = fs::read(options.firmware_dir.join("main_firmware.bin"))
        .map_err(|error| error.to_string())?;
    if format!("{:x}", Sha256::digest(main)) != FACTORY_MAIN {
        return Err("factory-protocol requires the hash-verified L6max MAIN image".into());
    }
    fs::create_dir_all(&options.logs).map_err(|error| error.to_string())?;
    fs::create_dir(&state).map_err(|error| error.to_string())?;
    create_factory_card(&card)?;
    options.sd_image = Some(card);
    options.state_dir = Some(state);
    options.no_sd = false;
    Ok(())
}

fn selected_main_image(options: &Options) -> Result<Vec<u8>, String> {
    let source = fs::read(options.firmware_dir.join("main_firmware.bin"))
        .map_err(|error| error.to_string())?;
    let Some(nor_path) = options
        .state_dir
        .as_ref()
        .map(|state| state.join("main-nor.bin"))
        .filter(|path| path.is_file())
    else {
        return Ok(source);
    };
    let nor = fs::read(nor_path).map_err(|error| error.to_string())?;
    let start = 0x50000usize;
    let end = start
        .checked_add(source.len())
        .ok_or_else(|| "installed MAIN range overflow".to_string())?;
    let installed = nor
        .get(start..end)
        .ok_or_else(|| "persistent NOR does not contain a complete MAIN image".to_string())?;
    Ok(installed.to_vec())
}

/// Run the firmware updater's full disposable-state lifecycle. This action owns
/// its guest restarts because each boot changes the NOR and SD fixture.
pub fn run_firmware_update(options: &Options) -> Result<(), String> {
    let main =
        fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
    let model = l6fw::model_from_main(&main).map_err(|e| e.to_string())?;
    if model == l6fw::Model::L6 {
        run_l6_update(options, &main)
    } else {
        run_l6max_update(options, &main)
    }
}

/// Compare the browser patcher's full model/profile matrix with the native
/// l6patch CLI. The CLI is the reference implementation under test;
/// each supported output must match byte-for-byte, and unsupported L6 profiles
/// must be rejected by both implementations.
pub fn run_wasm_patch(options: &Options) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::process::Command;

    let main = fs::read(options.firmware_dir.join("main_firmware.bin"))
        .map_err(|error| error.to_string())?;
    let model = l6fw::model_from_main(&main).map_err(|error| error.to_string())?;
    let label = crate::model_name(model);
    let root = options.logs.join("wasm-patch-lifecycle");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let stock_path = root.join("stock.bin");
    l6fw::repack(&options.firmware_dir, &stock_path, false).map_err(|error| error.to_string())?;
    let stock = fs::read(&stock_path).map_err(|error| error.to_string())?;
    if l6fw::model_from_package(&stock).map_err(|error| error.to_string())? != model {
        return Err(format!(
            "repacked firmware package does not detect as {label}"
        ));
    }
    let wasm_model = firmware_patch_wasm::detect_firmware_model(&stock)
        .map_err(|error| format!("WASM firmware model detection failed: {error:?}"))?;
    let expected_wasm_model = match model {
        l6fw::Model::L6 => "L6",
        l6fw::Model::L6max => "L6max",
    };
    if wasm_model != expected_wasm_model {
        return Err(format!(
            "WASM detected {wasm_model}, expected {expected_wasm_model}"
        ));
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = workspace.join("tools/l6patch/Cargo.toml");
    let mut results = Vec::new();
    let profiles: Vec<Vec<&str>> = match model {
        l6fw::Model::L6max => (1u16..(1u16 << firmware_patch_core::PATCHES.len()))
            .map(|mask| {
                firmware_patch_core::PATCHES
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| mask & (1u16 << index) != 0)
                    .map(|(_, patch)| *patch)
                    .collect()
            })
            .collect(),
        l6fw::Model::L6 => vec![vec!["midi-sync"]],
    };

    for (index, selected) in profiles.iter().enumerate() {
        let native_out = root.join(format!("native-{index}"));
        let mut command = Command::new("cargo");
        command
            .current_dir(&workspace)
            .args(["run", "--offline", "--locked", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .arg("--")
            .arg(&options.firmware_dir)
            .arg(&native_out)
            .args(["--update-version", "0111"]);
        for patch in selected {
            command.args(["--patch", patch]);
        }
        let output = command.output().map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "native patch build failed for {selected:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let filename = match model {
            l6fw::Model::L6 => "L6.BIN",
            l6fw::Model::L6max => "L6max.bin",
        };
        let native = fs::read(native_out.join(filename)).map_err(|error| error.to_string())?;
        let (wasm, report) = firmware_patch_wasm::apply_patches(&stock, selected)
            .map_err(|message| format!("WASM patching failed for {selected:?}: {message}"))?;
        if wasm != native {
            return Err(format!(
                "{label} {selected:?} mismatch: WASM {} bytes SHA-256 {:x}, native {} bytes SHA-256 {:x}",
                wasm.len(),
                Sha256::digest(&wasm),
                native.len(),
                Sha256::digest(&native)
            ));
        }
        results.push(json!({
            "patches": selected,
            "bytes": wasm.len(),
            "sha256": format!("{:x}", Sha256::digest(&wasm)),
            "usage": report["usage"].as_str().unwrap_or("")
        }));
    }

    // The persistent loader can coexist with any static L6max patch subset.
    // Include the loader-only profile: users may install the loader first and
    // add SD patches later without rebuilding the firmware update.
    let mut loader_results = Vec::new();
    if model == l6fw::Model::L6max {
        for mask in 0u16..(1u16 << firmware_patch_core::PATCHES.len()) {
            let selected: Vec<&str> = firmware_patch_core::PATCHES
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1u16 << index) != 0)
                .map(|(_, patch)| *patch)
                .collect();
            let native_out = root.join(format!("native-loader-{mask:03}"));
            let mut command = Command::new("cargo");
            command
                .current_dir(&workspace)
                .args(["run", "--offline", "--locked", "--quiet", "--manifest-path"])
                .arg(&manifest)
                .arg("--")
                .arg(&options.firmware_dir)
                .arg(&native_out)
                .arg("--experimental-patch-loader");
            for patch in &selected {
                command.args(["--patch", patch]);
            }
            let output = command.output().map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "native patch-loader build failed for {selected:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let native =
                fs::read(native_out.join("L6max.bin")).map_err(|error| error.to_string())?;
            let (wasm, report) =
                firmware_patch_wasm::apply_patches_with_loader(&stock, &selected, true).map_err(
                    |message| format!("WASM patch-loader build failed for {selected:?}: {message}"),
                )?;
            if wasm != native {
                fs::write(root.join(format!("wasm-loader-{mask:03}.bin")), &wasm)
                    .map_err(|error| error.to_string())?;
                let differences = wasm
                    .iter()
                    .zip(&native)
                    .enumerate()
                    .filter_map(|(offset, (left, right))| {
                        (left != right).then_some((offset, *left, *right))
                    })
                    .take(16)
                    .collect::<Vec<_>>();
                return Err(format!(
                    "L6max loader {selected:?} mismatch at {differences:?}: WASM {} bytes SHA-256 {:x}, native {} bytes SHA-256 {:x}",
                    wasm.len(),
                    Sha256::digest(&wasm),
                    native.len(),
                    Sha256::digest(&native)
                ));
            }
            loader_results.push(json!({
                "patches": selected,
                "bytes": wasm.len(),
                "sha256": format!("{:x}", Sha256::digest(&wasm)),
                "usage": report["usage"].as_str().unwrap_or("")
            }));
        }
    }

    if model == l6fw::Model::L6 {
        for unsupported in ["knob-dialog", "din-mixer-control"] {
            match firmware_patch_wasm::apply_patches(&stock, &[unsupported]) {
                Err(message) if message.contains("only the midi-sync patch is supported") => {}
                _ => {
                    return Err(format!(
                        "WASM accepted unsupported L6 profile {unsupported}"
                    ));
                }
            }
            let native_out = root.join(format!("native-unsupported-{unsupported}"));
            let output = Command::new("cargo")
                .current_dir(&workspace)
                .args(["run", "--offline", "--locked", "--quiet", "--manifest-path"])
                .arg(&manifest)
                .arg("--")
                .arg(&options.firmware_dir)
                .arg(&native_out)
                .args(["--patch", unsupported])
                .output()
                .map_err(|error| error.to_string())?;
            if output.status.success() {
                return Err(format!(
                    "native patcher accepted unsupported L6 profile {unsupported}"
                ));
            }
        }
    }

    fs::write(
        options.logs.join("wasm-patch.json"),
        serde_json::to_vec_pretty(&json!({"model":label,"stock_sha256":format!("{:x}", Sha256::digest(&stock)),"profiles":results,"patch_loader_profiles":loader_results}))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn update_word(guest: &Headless, address: u32) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        guest.engine.management[0]
            .read_memory(0, address, 4)
            .map_err(|e| e.to_string())?
            .try_into()
            .unwrap(),
    ))
}

fn run_l6_update(options: &Options, main_image: &[u8]) -> Result<(), String> {
    let package_path = options
        .update_package
        .clone()
        .or_else(|| options.firmware_package.clone())
        .unwrap_or_else(|| options.logs.join("L6-repacked.BIN"));
    if options.update_package.is_none() && options.firmware_package.is_none() {
        l6fw::repack(&options.firmware_dir, &package_path, false).map_err(|e| e.to_string())?;
    }
    let package_path = package_path.canonicalize().map_err(|e| e.to_string())?;
    let package = fs::read(&package_path).map_err(|e| e.to_string())?;
    if l6fw::model_from_package(&package).map_err(|e| e.to_string())? != l6fw::Model::L6 {
        return Err("L6 firmware-update requires an L6 package".into());
    }
    let (_, segments) = l6fw::unpack_bytes(&package).map_err(|e| e.to_string())?;
    let packaged_main = segments
        .get("main_firmware.bin")
        .ok_or("L6 package missing MAIN segment")?;
    if options.update_package.is_none() {
        if packaged_main.as_slice() != main_image {
            return Err("L6 update package MAIN differs from the selected firmware".into());
        }
    } else {
        use sha2::{Digest, Sha256};
        if format!("{:x}", Sha256::digest(main_image))
            != "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799"
            || packaged_main.as_slice() == main_image
        {
            return Err(
                "patched L6 update requires stock 1.10 source and a changed MAIN image".into(),
            );
        }
    }
    fs::create_dir_all(&options.logs).map_err(|e| e.to_string())?;
    let root = options.logs.join("l6-update-fixture");
    fs::create_dir(&root).map_err(|e| e.to_string())?;
    let state = root.join("state");
    fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    let mut initial_options = options.clone();
    initial_options.state_dir = Some(state.clone());
    initial_options.sd_image = None;
    initial_options.no_sd = true;
    initial_options.update_bootloader = false;
    initial_options.logs = root.join("before-update");
    let initial = Headless::start(&initial_options).map_err(|e| e.to_string())?;
    if initial.engine.model() != l6fw::Model::L6 {
        return Err("L6 firmware directory booted as another model".into());
    }
    initial.until(40_000).map_err(|e| e.to_string())?;
    drop(initial);
    let initial_nor = fs::read(state.join("main-nor.bin")).map_err(|e| e.to_string())?;
    let card = root.join("update-card.img");
    l6max_host::demo_sd::create(&card, &package_path).map_err(|e| e.to_string())?;
    let nor_path = state.join("main-nor.bin");
    let mut nor = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&nor_path)
        .map_err(|e| e.to_string())?;
    nor.seek(SeekFrom::Start(0x1ff000))
        .map_err(|e| e.to_string())?;
    nor.write_all(b"FW UPDT\0").map_err(|e| e.to_string())?;
    nor.sync_all().map_err(|e| e.to_string())?;
    drop(nor);
    let mut update_options = initial_options;
    update_options.sd_image = Some(card);
    update_options.no_sd = false;
    update_options.update_bootloader = true;
    update_options.logs = root.join("after-update");
    let installed = Headless::start(&update_options).map_err(|e| e.to_string())?;
    if installed.engine.model() != l6fw::Model::L6
        || fs::read_to_string(update_options.logs.join("update-bootloader.log"))
            .map_err(|e| e.to_string())?
            != "Installed\n"
    {
        return Err("L6 replacement installer did not report Installed".into());
    }
    let installed_nor = fs::read(&nor_path).map_err(|e| e.to_string())?;
    for (name, start) in [
        ("main_firmware.bin", 0x50000),
        ("main_trailer.bin", 0x1f2ff8),
        ("secondary_firmware.bin", 0x1f3000),
        ("secondary_trailer.bin", 0x1f6ff8),
        ("package_footer.bin", 0x1f7fc0),
    ] {
        let image = segments
            .get(name)
            .ok_or_else(|| format!("L6 package missing segment {name}"))?;
        if installed_nor.get(start..start + image.len()) != Some(image.as_slice()) {
            return Err(format!("installed L6 segment differs: {name}"));
        }
    }
    if installed_nor.get(0x1ff000..0x1ff008) == Some(b"FW UPDT\0")
        || installed_nor.get(..0x50000) != initial_nor.get(..0x50000)
        || installed_nor.get(0x1f8000..0x1ff000) != initial_nor.get(0x1f8000..0x1ff000)
    {
        return Err("L6 update changed protected NOR or left pending marker".into());
    }
    let resource = segments
        .get("resource_region.bin")
        .ok_or("L6 package has no resource region")?;
    let mut expected_resource = vec![0xff; 0x400000];
    expected_resource[..resource.len()].copy_from_slice(resource);
    if fs::read(state.join("resource-nor.bin")).map_err(|e| e.to_string())? != expected_resource {
        return Err("installed L6 resource NOR differs from package".into());
    }
    let reset_vector = u32::from_le_bytes(packaged_main[4..8].try_into().unwrap());
    if update_word(&installed, 0x80001004)? != reset_vector {
        return Err("installed L6 MAIN reset vector does not match package".into());
    }
    installed.until(40_000).map_err(|e| e.to_string())?;
    drop(installed);
    if options.update_package.is_some() {
        let mut verify = options.clone();
        verify.sd_image = None;
        verify.no_sd = true;
        verify.update_bootloader = false;
        let package = options.update_package.as_ref().unwrap();
        let report = package.with_extension("json");
        verify.firmware_dir = if report.is_file() {
            // A patched package keeps its patch report beside it; the MIDI
            // probe reads the installed hook targets from that report.
            let dir = options.logs.join("post-update-firmware");
            let _ = fs::remove_dir_all(&dir);
            l6fw::unpack(package, &dir)?;
            fs::copy(&report, dir.join("patch.json")).map_err(|e| e.to_string())?;
            dir
        } else {
            l6max_host::setup::unpack_cached(
                package,
                &l6max_host::paths::repository_root().join("emulator/state/firmware"),
            )?
        };
        verify.gdb_socket = Some(
            std::env::temp_dir().join(format!("l6-post-update-midi-{}.sock", std::process::id())),
        );
        let session = crate::Session::start(&verify)?;
        let start = session
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .panel
            .snapshot()
            .guest_ms;
        let deadline = Instant::now() + Duration::from_secs(180);
        while session
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .panel
            .snapshot()
            .guest_ms
            .wrapping_sub(start)
            < 5_000
        {
            session.poll()?;
            if Instant::now() >= deadline {
                return Err("patched L6 did not reach its post-update MIDI probe point".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        State::default()
            .execute(&session, &verify, Operation::L6MidiSync, 120_000, 300)
            .map_err(|error| format!("installed L6 midi-sync behavior failed: {error}"))?;
    }
    fs::write(
        options.logs.join("firmware-update.json"),
        serde_json::to_vec_pretty(&json!({"model":"l6","outcome":"PASS","segments_installed":5,"resource_region":true,"pending_marker_cleared":true,"protected_regions_preserved":true,"installed_firmware_booted":true})).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn update_checksum(package: &[u8]) -> u32 {
    package[0x200..0x1a8200]
        .iter()
        .fold(0u32, |sum, &byte| sum.wrapping_add(u32::from(byte)))
}

fn run_l6max_update(options: &Options, main: &[u8]) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    const ORIGINAL_PACKAGE_SHA256: &str =
        "4e20f92c9c5131ed082c0feec2b2d7d0445fb576296f11f6dace56a4fc0c9b47";
    let source = options
        .firmware_package
        .as_ref()
        .ok_or("L6max firmware-update requires its original package in the fixture")?;
    let target_package = options
        .update_package
        .as_ref()
        .map(fs::read)
        .transpose()
        .map_err(|e| e.to_string())?;
    let original = fs::read(source).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&original)) != ORIGINAL_PACKAGE_SHA256
        || format!("{:x}", Sha256::digest(main))
            != "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461"
    {
        return Err(
            "L6max firmware-update requires the hash-pinned stock 1.10 package and MAIN".into(),
        );
    }
    if update_checksum(&original) != u32::from_be_bytes(original[0x1fc..0x200].try_into().unwrap())
    {
        return Err("original L6max package checksum mismatch".into());
    }
    fs::create_dir_all(&options.logs).map_err(|e| e.to_string())?;
    let root = options.logs.join("l6max-update-fixtures");
    fs::create_dir(&root).map_err(|e| e.to_string())?;
    for valid in [false, true] {
        let label = if valid { "valid" } else { "bad-checksum" };
        let directory = root.join(label);
        fs::create_dir(&directory).map_err(|e| e.to_string())?;
        let mut package = if valid {
            target_package.clone().unwrap_or_else(|| original.clone())
        } else {
            original.clone()
        };
        if target_package.is_none() {
            for offset in [0x1a31fc, 0x1a81fc] {
                package[offset..offset + 4].copy_from_slice(b"0111");
            }
            package[0x1a71fc..0x1a7200].copy_from_slice(b"0101");
            package[0x21c..0x220].copy_from_slice(&0x13579bdfu32.to_le_bytes());
            package[0x1a321c..0x1a3220].copy_from_slice(&0x2468ace0u32.to_le_bytes());
            if valid {
                let checksum = update_checksum(&package).to_be_bytes();
                package[0x1fc..0x200].copy_from_slice(&checksum);
            }
        } else if valid {
            l6max_host::package::check_layout(&package).map_err(|e| e.to_string())?;
            l6max_host::update_bootloader::validate(&package).map_err(|e| e.to_string())?;
            if package[0x1a31fc..0x1a3200].iter().cmp(b"0110".iter()) != std::cmp::Ordering::Greater
            {
                return Err(
                    "patched L6max update package must carry a version newer than 1.10".into(),
                );
            }
        } else {
            package[0x1fc] ^= 1;
        }
        let card = directory.join("card.img");
        let mut volume = sd_image::create_empty(&card, "L6MAX").map_err(|e| e.to_string())?;
        volume
            .write("L6max.BIN", &package)
            .and_then(|()| volume.flush())
            .map_err(|e| e.to_string())?;
        drop(volume);
        let mut local = options.clone();
        local.sd_image = Some(card);
        local.no_sd = false;
        local.state_dir = Some(directory.join("state"));
        local.update_bootloader = false;
        local.logs = directory.join("boot");
        let guest = Headless::start(&local).map_err(|e| e.to_string())?;
        guest.until(40_000).map_err(|e| e.to_string())?;
        let mode = update_word(&guest, 0x80462d9c)?;
        let flags = guest.engine.management[0]
            .read_memory(0, 0x8020a5ca, 1)
            .map_err(|e| e.to_string())?[0];
        if flags & 9 != 0 || mode != if valid { 6 } else { 1 } {
            return Err(format!(
                "{label}: firmware did not gate package as expected; mode={mode}, flags={flags:#x}"
            ));
        }
        let nor_path = directory.join("state/main-nor.bin");
        if valid {
            if update_word(&guest, 0x80202610)? != 0x802003b4 {
                return Err("valid L6max package did not open the update dialog".into());
            }
            guest.tap(2, 50, 1000).map_err(|e| e.to_string())?;
            guest.tap(3, 50, 1000).map_err(|e| e.to_string())?;
            if update_word(&guest, 0x80202610)? != 0x8020029c
                || update_word(&guest, 0x8020029c)? & 0x100 == 0
            {
                return Err("valid L6max update preparation did not complete".into());
            }
            let before = fs::read(&nor_path).map_err(|e| e.to_string())?;
            if before.get(0x1ff000..0x1ff008) == Some(b"FW UPDT\0") {
                return Err("update marker set before power handoff".into());
            }
            guest
                .engine
                .inputs
                .as_ref()
                .unwrap()
                .tap(53, 3500)
                .map_err(|e| e.to_string())?;
            let deadline = Instant::now() + Duration::from_secs(30);
            while !guest
                .engine
                .inputs
                .as_ref()
                .unwrap()
                .main
                .snapshot()
                .powered_off
            {
                guest.display.sample().map_err(|e| e.to_string())?;
                if Instant::now() > deadline {
                    return Err("L6max firmware did not cut power".into());
                }
                thread::sleep(Duration::from_millis(5));
            }
            drop(guest);
            let pending = fs::read(&nor_path).map_err(|e| e.to_string())?;
            if pending.get(0x1ff000..0x1ff008) != Some(b"FW UPDT\0") {
                return Err("power handoff did not save update marker".into());
            }
            local.update_bootloader = true;
            local.logs = directory.join("installed");
            let installed = Headless::start(&local).map_err(|e| e.to_string())?;
            installed.until(40_000).map_err(|e| e.to_string())?;
            if fs::read_to_string(local.logs.join("update-bootloader.log"))
                .map_err(|e| e.to_string())?
                != "Installed\n"
                || update_word(&installed, 0x80462d9c)? != 4
                || (target_package.is_none() && update_word(&installed, 0x8000001c)? != 0x13579bdf)
            {
                return Err("L6max update install or application handoff failed".into());
            }
            let programmed =
                fs::read_to_string(local.logs.join("engine.trace")).map_err(|e| e.to_string())?;
            if target_package.is_none()
                && (!programmed.contains("l6 panel ROM 16 KiB image match: yes")
                    || programmed
                        .lines()
                        .filter(|line| line.starts_with("l6 panel ROM write 080"))
                        .count()
                        != 64)
            {
                return Err("L6max panel ROM programming did not verify 64 writes".into());
            }
            drop(installed);
            let after = fs::read(&nor_path).map_err(|e| e.to_string())?;
            if after.get(..0x50000) != pending.get(..0x50000)
                || after.get(0x50000..0x1f8000) != package.get(0x200..)
                || after.get(0x1f8000..0x1ff000) != pending.get(0x1f8000..0x1ff000)
                || after.get(0x1ff000..0x1ff008) == Some(b"FW UPDT\0")
            {
                return Err("L6max update changed protected NOR or left update marker".into());
            }
            let panel =
                fs::read(directory.join("state/panel-flash.bin")).map_err(|e| e.to_string())?;
            if panel.get(..0x3ffc) != package.get(0x1a3200..0x1a71fc)
                || panel.get(0x3ffc..0x4000) != package.get(0x1a71fc..0x1a7200)
            {
                return Err("installed L6max panel firmware differs from update package".into());
            }
            local.sd_image = None;
            local.no_sd = true;
            local.logs = directory.join("restart");
            let restarted = Headless::start(&local).map_err(|e| e.to_string())?;
            restarted.until(40_000).map_err(|e| e.to_string())?;
            if (target_package.is_none() && update_word(&restarted, 0x8000001c)? != 0x13579bdf)
                || fs::read_to_string(local.logs.join("update-bootloader.log"))
                    .map_err(|e| e.to_string())?
                    != "NoPending\n"
                || fs::read(directory.join("state/panel-flash.bin")).map_err(|e| e.to_string())?
                    != panel
            {
                return Err("L6max installed image did not survive clean restart".into());
            }
            let trace =
                fs::read_to_string(local.logs.join("engine.trace")).map_err(|e| e.to_string())?;
            if trace.contains("l6 panel ROM erase") || trace.contains("l6 panel ROM mass erase") {
                return Err("L6max unexpectedly reinstalled panel firmware on restart".into());
            }
            if target_package.is_some() {
                drop(restarted);
                let mut verify = local.clone();
                verify.sd_image = None;
                verify.no_sd = true;
                verify.update_bootloader = false;
                verify.firmware_dir = l6max_host::setup::unpack_cached(
                    options.update_package.as_ref().unwrap(),
                    &l6max_host::paths::repository_root().join("emulator/state/firmware"),
                )?;
                verify.gdb_socket = Some(std::env::temp_dir().join(format!(
                    "l6max-post-update-knob-{}.sock",
                    std::process::id()
                )));
                let session = crate::Session::start(&verify)?;
                let start = session
                    .engine
                    .inputs
                    .as_ref()
                    .unwrap()
                    .panel
                    .snapshot()
                    .guest_ms;
                let deadline = Instant::now() + Duration::from_secs(180);
                while session
                    .engine
                    .inputs
                    .as_ref()
                    .unwrap()
                    .panel
                    .snapshot()
                    .guest_ms
                    .wrapping_sub(start)
                    < 40_000
                {
                    session.poll()?;
                    if Instant::now() >= deadline {
                        return Err("patched L6max did not reach its post-update UI state".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                State::default()
                    .execute(&session, &verify, Operation::KnobDialog, 180_000, 300)
                    .map_err(|error| {
                        format!("installed L6max knob-dialog behavior failed: {error}")
                    })?;
            }
        } else {
            drop(guest);
            let nor = fs::read(&nor_path).map_err(|e| e.to_string())?;
            if nor.get(0x1ff000..0x1ff008) == Some(b"FW UPDT\0") {
                return Err("corrupt L6max package triggered an update".into());
            }
        }
    }
    fs::write(
        options.logs.join("firmware-update.json"),
        serde_json::to_vec_pretty(&json!({"model":"l6max","outcome":"PASS","valid_package_installed":true,"corrupt_package_rejected":true,"panel_flash_verified":true,"protected_storage_preserved":true,"clean_restart_verified":true})).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn thumb_bl_target(address: u32, instruction: [u8; 4]) -> Option<u32> {
    let first = u16::from_le_bytes([instruction[0], instruction[1]]);
    let second = u16::from_le_bytes([instruction[2], instruction[3]]);
    if first & 0xf800 != 0xf000 || second & 0xd000 != 0xd000 {
        return None;
    }
    let sign = ((first >> 10) & 1) as u32;
    let j1 = ((second >> 13) & 1) as u32;
    let j2 = ((second >> 11) & 1) as u32;
    let i1 = (!(j1 ^ sign)) & 1;
    let i2 = (!(j2 ^ sign)) & 1;
    let immediate = (sign << 24)
        | (i1 << 23)
        | (i2 << 22)
        | (((first & 0x03ff) as u32) << 12)
        | (((second & 0x07ff) as u32) << 1);
    let displacement = ((immediate as i32) << 7) >> 7;
    Some(address.wrapping_add(4).wrapping_add(displacement as u32))
}

fn create_factory_card(card: &std::path::Path) -> Result<(), String> {
    let mut volume = sd_image::create_empty(card, "L6MAX").map_err(|e| e.to_string())?;
    volume
        .write(
            "D445_JIG_000.txt",
            b"emulator fixture; no calibration parameters\n",
        )
        .and_then(|()| volume.flush())
        .map_err(|error| error.to_string())
}

#[derive(Default)]
pub struct State {
    saved: Option<(Vec<u8>, usize)>,
    midi: Option<l6max_host::usb::Endpoints>,
    gdb: Option<l6max_diagnostics::gdb::Gdb>,
    host_midi: Option<Box<dyn crate::host_midi::Client>>,
    encoder_turned_up: bool,
}

impl State {
    pub fn take_host_midi(&mut self) -> Option<Box<dyn crate::host_midi::Client>> {
        self.host_midi.take()
    }
}

fn wait_l6_input(
    session: &Session,
    mut ready: impl FnMut() -> bool,
    what: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        session.poll()?;
        if ready() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for L6 {what}"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn guest_now(session: &Session) -> u32 {
    session
        .engine
        .inputs
        .as_ref()
        .map_or(0, |inputs| inputs.panel.snapshot().guest_ms)
}

/// Attach the cable and wait, under the step's deadlines, until firmware runs
/// its device controller. Firmware starts it after boot (about 22 guest s on
/// L6; about 4 s on L6max, or 42 s with the cable attached at power-on), and
/// the device model refuses tokens with ENODEV until then.
fn wait_usb_controller(
    session: &Session,
    usb: &l6max_host::usb::UsbHost,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<(), String> {
    usb.connect(true).map_err(|error| error.to_string())?;
    let start = guest_now(session);
    let ceiling = Instant::now() + Duration::from_secs(wall_timeout_s);
    while !usb
        .controller_running()
        .map_err(|error| error.to_string())?
    {
        if Instant::now() >= ceiling || guest_now(session).wrapping_sub(start) > guest_timeout_ms {
            return Err("USB device controller did not start".into());
        }
        session.poll()?;
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

fn wait_guest_duration(
    session: &Session,
    duration_ms: u32,
    deadline: Instant,
) -> Result<(), String> {
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("guest wait needs inputs")?;
    let start = inputs.panel.snapshot().guest_ms;
    while inputs.panel.snapshot().guest_ms.wrapping_sub(start) < duration_ms {
        session.poll()?;
        if Instant::now() >= deadline {
            return Err(format!(
                "guest did not advance {duration_ms} ms before deadline"
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn run_l6_usb_startup(
    session: &Session,
    options: &Options,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, String> {
    use l6max_diagnostics::gdb;
    const BREAKPOINTS: [(u32, &str); 3] = [
        (0x8007_3b84, "usb_start_wrapper"),
        (0x8004_1190, "usb_start_initializer"),
        (0x8007_2e88, "usb_phy_gate"),
    ];
    let qmp = &session.engine.management[0];
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("input hub unavailable")?;
    let socket = options
        .gdb_socket
        .as_ref()
        .ok_or("l6-usb-startup requires GDB")?;
    let mut debugger = gdb::Gdb::connect(socket).map_err(|error| error.to_string())?;
    for (address, _) in BREAKPOINTS {
        debugger
            .breakpoint(address, true)
            .map_err(|error| error.to_string())?;
    }
    qmp.resume().map_err(|error| error.to_string())?;

    let mut hits = Vec::new();
    let mut targeted_inputs = 0usize;
    let mut panel_sweep = 0usize;
    let mut panel_ids = vec![1, 2, 3, 6];
    panel_ids.extend(7..=47);
    let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    while inputs.panel.snapshot().guest_ms < 60_000 {
        session.poll()?;
        let guest_ms = inputs.panel.snapshot().guest_ms;
        if guest_ms > guest_timeout_ms {
            return Err(format!(
                "L6 USB startup exceeded its guest deadline at {guest_ms} ms"
            ));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "60-second L6 USB probe timed out at {guest_ms} guest ms"
            ));
        }
        if !qmp
            .query_status()
            .map_err(|error| error.to_string())?
            .running
        {
            let pc = debugger
                .read_register(15)
                .map_err(|error| error.to_string())?
                & !1;
            let Some((address, name)) = BREAKPOINTS.iter().find(|(address, _)| *address == pc)
            else {
                return Err(format!(
                    "L6 USB startup stopped at unexpected PC {pc:#010x}"
                ));
            };
            let registers = (0..4)
                .map(|register| debugger.read_register(register))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            hits.push(json!({
                "function": name,
                "address": format!("{address:#010x}"),
                "r0_r3": registers,
                "guest_ms": inputs.panel.snapshot().guest_ms,
            }));
            debugger
                .breakpoint(*address, false)
                .map_err(|error| error.to_string())?;
            qmp.resume().map_err(|error| error.to_string())?;
        } else {
            let guest_ms = inputs.panel.snapshot().guest_ms;
            let target_times = [40_000, 42_000, 44_000, 46_000];
            if targeted_inputs < target_times.len() && guest_ms >= target_times[targeted_inputs] {
                if targeted_inputs < 2 {
                    inputs.panel.tap(3, 50).map_err(|error| error.to_string())?;
                } else {
                    inputs.main.tap(49, 50).map_err(|error| error.to_string())?;
                }
                targeted_inputs += 1;
            } else if targeted_inputs == target_times.len()
                && panel_sweep < panel_ids.len()
                && guest_ms >= 48_000 + panel_sweep as u32 * 250
            {
                inputs
                    .panel
                    .tap(panel_ids[panel_sweep], 50)
                    .map_err(|error| error.to_string())?;
                panel_sweep += 1;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "60-second L6 USB probe timed out at {} guest ms",
                    inputs.panel.snapshot().guest_ms
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    qmp.stop().map_err(|error| error.to_string())?;
    session.poll()?;
    let trace =
        fs::read_to_string(options.logs.join("engine.trace")).map_err(|error| error.to_string())?;
    for (address, name) in BREAKPOINTS {
        if !hits
            .iter()
            .any(|hit| hit["address"] == format!("{address:#010x}"))
        {
            return Err(format!(
                "L6 USB startup did not reach {name} at {address:#010x}"
            ));
        }
    }
    let usb_controller_writes = trace.matches("l6_usb_write offset=").count();
    if usb_controller_writes == 0 {
        return Err(
            "L6 USB startup reached its functions but wrote no controller registers".into(),
        );
    }
    if targeted_inputs != 4 || panel_sweep != panel_ids.len() {
        return Err(format!(
            "input replay incomplete: targeted {targeted_inputs}/4, panel {panel_sweep}/{}",
            panel_ids.len()
        ));
    }
    Ok(json!({
        "model": "L6",
        "guest_ms": inputs.panel.snapshot().guest_ms,
        "targeted_inputs": targeted_inputs,
        "panel_input_ids": panel_ids,
        "breakpoint_hits": hits,
        "usb_controller_writes": usb_controller_writes,
    }))
}

fn run_l6max_channel_signal_pwm(
    session: &Session,
    options: &Options,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, String> {
    use l6max_diagnostics::gdb::Gdb;
    if session.engine.model() != l6fw::Model::L6max {
        return Err("l6max-channel-signal-pwm requires L6max firmware".into());
    }
    let socket = options.gdb_socket.as_ref().ok_or("GDB is required")?;
    let qmp = &session.engine.management[0];
    let mut debugger = Gdb::connect(socket).map_err(|error| error.to_string())?;
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("headless panel input missing")?;
    // SIGNAL 1-4 and 5-8 respond on separate scales, each in five brightness
    // steps. Each stimulus sits well inside one observed step, away from the
    // levels where a lamp changes step.
    let sweeps: [(
        &str,
        std::ops::Range<usize>,
        std::ops::Range<usize>,
        [f32; 5],
    ); 2] = [
        (
            "signal-1-to-4",
            0..4,
            0..4,
            [
                1_600_000.0,
                2_250_000.0,
                3_200_000.0,
                4_550_000.0,
                8_000_000.0,
            ],
        ),
        (
            "signal-5-to-8",
            4..12,
            4..8,
            [
                9_000_000.0,
                12_800_000.0,
                18_000_000.0,
                25_500_000.0,
                40_000_000.0,
            ],
        ),
    ];
    let started_guest = inputs.panel.snapshot().guest_ms;
    let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    let mut sweep_evidence = Vec::new();
    let mut failures = Vec::new();

    // The SIGNAL lamps start following these values a second or two after
    // the mixer is ready. Hold the first band high until SIGNAL 1 lights, so
    // the sweep cannot start before the lamps respond.
    let mut warm_up = Vec::with_capacity(12 * 4);
    for channel in 0..12 {
        let value: f32 = if channel < 4 { 8_000_000.0 } else { 0.0 };
        warm_up.extend_from_slice(&value.to_le_bytes());
    }
    let warm_up_start = inputs.panel.snapshot().guest_ms;
    let mut last_seed = None;
    loop {
        let panel = session.poll().map_err(|error| error.to_string())?;
        let guest_ms = inputs.panel.snapshot().guest_ms;
        let (row, column) = l6max_host::indicators::LED_PINS[52];
        if panel.indicator_levels[row][(column ^ 7) as usize] != 0 {
            break;
        }
        if guest_ms.wrapping_sub(warm_up_start) >= 10_000 || Instant::now() >= deadline {
            return Err("SIGNAL 1 did not light within 10 guest seconds".into());
        }
        if last_seed.is_none_or(|seed: u32| guest_ms.wrapping_sub(seed) >= 40) {
            qmp.stop().map_err(|error| error.to_string())?;
            debugger
                .write(0x2001_50e0, &warm_up)
                .map_err(|error| error.to_string())?;
            qmp.resume().map_err(|error| error.to_string())?;
            last_seed = Some(guest_ms);
        }
        thread::sleep(Duration::from_millis(1));
    }

    for (name, accumulator_range, output_range, levels) in sweeps {
        {
            // A lit band takes a moment to fall after its values drop. Drain
            // the first band, lit by the warm-up or the previous sweep, before
            // measuring, so samples cannot contain earlier LEDs.
            let zeroes = vec![0_u8; 12 * 4];
            qmp.stop().map_err(|error| error.to_string())?;
            debugger
                .write(0x2001_50e0, &zeroes)
                .map_err(|error| error.to_string())?;
            qmp.resume().map_err(|error| error.to_string())?;
            let drain_start = inputs.panel.snapshot().guest_ms;
            let mut last_seed = drain_start;
            let mut zero_samples = 0_u8;
            loop {
                let panel = session.poll().map_err(|error| error.to_string())?;
                let guest_ms = inputs.panel.snapshot().guest_ms;
                let prior_band_off = (0..4).all(|channel| {
                    let (row, column) = l6max_host::indicators::LED_PINS[52 + channel];
                    panel.indicator_levels[row][(column ^ 7) as usize] == 0
                });
                zero_samples = if prior_band_off { zero_samples + 1 } else { 0 };
                if zero_samples >= 3 {
                    break;
                }
                let elapsed = guest_ms.wrapping_sub(drain_start);
                if elapsed >= 1_000 {
                    return Err(format!(
                        "previous L6max SIGNAL band did not clear within {elapsed} guest ms"
                    ));
                }
                if guest_ms.wrapping_sub(last_seed) >= 40 {
                    qmp.stop().map_err(|error| error.to_string())?;
                    debugger
                        .write(0x2001_50e0, &zeroes)
                        .map_err(|error| error.to_string())?;
                    qmp.resume().map_err(|error| error.to_string())?;
                    last_seed = guest_ms;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        let mut stage_evidence = Vec::new();
        for value in levels {
            let mut bytes = Vec::with_capacity(12 * 4);
            for channel in 0..12 {
                let channel_value = if accumulator_range.contains(&channel) {
                    value
                } else {
                    0.0
                };
                bytes.extend_from_slice(&channel_value.to_le_bytes());
            }
            let mut seed = || -> Result<(), String> {
                qmp.stop().map_err(|error| error.to_string())?;
                debugger
                    .write(0x2001_50e0, &bytes)
                    .map_err(|error| error.to_string())?;
                qmp.resume().map_err(|error| error.to_string())?;
                Ok(())
            };
            seed()?;
            let stage_start = inputs.panel.snapshot().guest_ms;
            let mut last_seed = stage_start;
            let mut samples: Vec<[u8; 8]> = Vec::new();
            loop {
                let panel = session.poll().map_err(|error| error.to_string())?;
                let guest_ms = inputs.panel.snapshot().guest_ms;
                if guest_ms.wrapping_sub(last_seed) >= 40 {
                    seed()?;
                    last_seed = guest_ms;
                }
                let sample = std::array::from_fn(|channel| {
                    let (row, column) = l6max_host::indicators::LED_PINS[52 + channel];
                    panel.indicator_levels[row][(column ^ 7) as usize]
                });
                samples.push(sample);
                if guest_ms.wrapping_sub(stage_start) >= 1_000 {
                    break;
                }
                if Instant::now() >= deadline
                    || guest_ms.wrapping_sub(started_guest) > guest_timeout_ms
                {
                    return Err(format!("timed out sampling {name} PWM"));
                }
                thread::sleep(Duration::from_millis(5));
            }
            let mean: [f64; 8] = std::array::from_fn(|channel| {
                samples
                    .iter()
                    .map(|sample| sample[channel] as u64)
                    .sum::<u64>() as f64
                    / samples.len() as f64
            });
            stage_evidence.push(json!({"input_level":value,"sample_count":samples.len(),"signal_led_mean_levels":mean,"samples":samples}));
        }
        let medians = stage_evidence
            .iter()
            .map(|entry| {
                entry["signal_led_mean_levels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_f64().unwrap())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for channel in output_range.clone() {
            for stage in 0..medians.len() {
                if medians[stage][channel] == 0.0
                    || (stage > 0 && medians[stage][channel] <= medians[stage - 1][channel])
                {
                    let details = medians.iter().map(|row| row[channel]).collect::<Vec<_>>();
                    failures.push(format!("SIGNAL LED {} did not produce five distinct increasing sampled PWM levels in {name}: {details:?}", channel + 1));
                }
            }
        }
        sweep_evidence.push(json!({"group":name,"accumulator_indices":accumulator_range,"output_channels":output_range,"levels":stage_evidence}));
    }
    let evidence =
        json!({"stimulus":"firmware_shared_channel_level_accumulators","sweeps":sweep_evidence});
    fs::write(
        options.logs.join("channel-signal-pwm.json"),
        serde_json::to_vec_pretty(&evidence).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(evidence)
}

fn run_l6_level_meter_pwm(
    session: &Session,
    options: &Options,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, String> {
    use l6max_diagnostics::gdb::Gdb;
    if session.engine.model() != l6fw::Model::L6 {
        return Err("l6-level-meter-pwm requires L6 firmware".into());
    }
    let socket = options.gdb_socket.as_ref().ok_or("GDB is required")?;
    let qmp = &session.engine.management[0];
    let mut debugger = Gdb::connect(socket).map_err(|error| error.to_string())?;
    // QEMU can report a paused VM after the GDB client attaches even though
    // the scenario was running. Resume before measuring guest-time progress.
    if !qmp
        .query_status()
        .map_err(|error| error.to_string())?
        .running
    {
        qmp.resume().map_err(|error| error.to_string())?;
    }
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("headless panel input missing")?;

    // Skip the long boot-time panel output sequence; the PWM probe exercises
    // the task's normal meter loop and panel updates.
    const TASK_ENTRY: u32 = 0x8002_b140;
    const STARTUP_CALL: u32 = 0x8002_b150;
    const THRESHOLD_BUILDER: u32 = 0x8002_6f88;
    const TASK_PHASE: u32 = 0x801f_8e84;
    const ACTIVE_GATES: u32 = 0x801f_8e74;
    const CONVERTED_ACCUMULATORS: u32 = 0x801f_90b8;
    const THRESHOLD_STAGES: [u32; 6] = [0, 1_500_000, 2_000_000, 3_000_000, 4_000_000, 6_000_000];
    const STAGE_WINDOW_MS: u32 = 500;
    const SETTLE_MS: u32 = 100;

    let started_guest = inputs.panel.snapshot().guest_ms;
    let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    let initial_gates = qmp
        .read_memory(0, ACTIVE_GATES, 3)
        .map_err(|error| error.to_string())?;
    let initial_phase = u32::from_le_bytes(
        qmp.read_memory(0, TASK_PHASE, 4)
            .map_err(|error| error.to_string())?
            .try_into()
            .unwrap(),
    );
    qmp.stop().map_err(|error| error.to_string())?;
    debugger
        .breakpoint(TASK_ENTRY, true)
        .map_err(|error| format!("setting LevelMeterLed task-entry breakpoint: {error}"))?;
    if let Err(error) = debugger.continue_until_stop(Duration::from_secs(wall_timeout_s.min(30))) {
        let status = qmp
            .query_status()
            .map(|status| status.status)
            .unwrap_or_else(|read| format!("unavailable: {read}"));
        qmp.resume().map_err(|resume| resume.to_string())?;
        return Err(format!(
            "LevelMeterLed task entry {TASK_ENTRY:#010x} was not reached within {}s (phase {initial_phase}, gates {initial_gates:02x?}, QEMU {status}): {error}; PWM probe was not run",
            wall_timeout_s.min(30)
        ));
    }
    let entry_pc = debugger
        .read_register(15)
        .map_err(|error| error.to_string())?
        & !1;
    if entry_pc != TASK_ENTRY {
        qmp.resume().map_err(|error| error.to_string())?;
        return Err(format!(
            "LevelMeterLed entry breakpoint stopped at {entry_pc:#010x}, expected {TASK_ENTRY:#010x}"
        ));
    }
    debugger
        .breakpoint(TASK_ENTRY, false)
        .map_err(|error| error.to_string())?;
    qmp.stop().map_err(|error| error.to_string())?;
    const NOP_PAIR: [u8; 4] = [0x00, 0xbf, 0x00, 0xbf];
    debugger
        .write(STARTUP_CALL, &NOP_PAIR)
        .map_err(|error| format!("skipping the LevelMeterLed startup output sequence: {error}"))?;
    let patched_call = qmp
        .read_memory(0, STARTUP_CALL, NOP_PAIR.len())
        .map_err(|error| error.to_string())?;
    if patched_call != NOP_PAIR {
        qmp.resume().map_err(|error| error.to_string())?;
        return Err("could not verify the focused LevelMeterLed startup skip".into());
    }
    debugger
        .write(ACTIVE_GATES, &[1, 1])
        .map_err(|error| error.to_string())?;
    debugger
        .breakpoint(THRESHOLD_BUILDER, true)
        .map_err(|error| format!("setting L6 meter threshold-builder breakpoint: {error}"))?;
    if let Err(error) = debugger.continue_until_stop(Duration::from_secs(5)) {
        let interrupt = debugger
            .interrupt()
            .unwrap_or_else(|interrupt| format!("interrupt failed: {interrupt}"));
        let pc = debugger
            .read_register(15)
            .map(|register| format!("{:#x}", register & !1))
            .unwrap_or_else(|read| format!("unavailable: {read}"));
        let gates = qmp
            .read_memory(0, ACTIVE_GATES, 2)
            .map(|bytes| format!("{bytes:02x?}"))
            .unwrap_or_else(|read| format!("unavailable: {read}"));
        let phase = qmp
            .read_memory(0, 0x801f_8e84, 4)
            .map(|bytes| format!("{bytes:02x?}"))
            .unwrap_or_else(|read| format!("unavailable: {read}"));
        qmp.resume().map_err(|resume| resume.to_string())?;
        return Err(format!(
            "waiting for L6 meter threshold builder failed: {error}; stop={interrupt}; PC={pc}; gates={gates}; phase={phase}"
        ));
    }
    if debugger
        .read_register(15)
        .map_err(|error| error.to_string())?
        & !1
        != THRESHOLD_BUILDER
    {
        return Err("L6 meter threshold-builder breakpoint was missed".into());
    }
    let mut stage_evidence = Vec::new();
    for value in THRESHOLD_STAGES {
        let bytes: Vec<u8> = (0..10).flat_map(|_| value.to_le_bytes()).collect();
        let stage_start = inputs.panel.snapshot().guest_ms;
        let mut sums = [[0_u64; 24]; 8];
        let mut sample_count = 0_u64;
        loop {
            debugger
                .write(ACTIVE_GATES, &[1, 1])
                .map_err(|error| error.to_string())?;
            debugger
                .write(CONVERTED_ACCUMULATORS, &bytes)
                .map_err(|error| error.to_string())?;
            let injected = qmp
                .read_memory(0, CONVERTED_ACCUMULATORS, bytes.len())
                .map_err(|error| error.to_string())?;
            if injected != bytes {
                return Err(format!(
                    "L6 meter accumulator injection failed at {value}: wrote {bytes:02x?}, read {injected:02x?}"
                ));
            }
            debugger
                .continue_until_stop(Duration::from_secs(10))
                .map_err(|error| format!("advancing L6 meter task at {value}: {error}"))?;
            if debugger
                .read_register(15)
                .map_err(|error| error.to_string())?
                & !1
                != THRESHOLD_BUILDER
            {
                return Err("L6 meter threshold-builder breakpoint was missed".into());
            }
            let panel = session.poll().map_err(|error| error.to_string())?;
            let guest_ms = inputs.panel.snapshot().guest_ms;
            let elapsed = guest_ms.wrapping_sub(stage_start);
            if elapsed >= SETTLE_MS {
                for (row, values) in panel.indicator_levels.iter().enumerate() {
                    for (column, level) in values.iter().enumerate() {
                        sums[row][column] += u64::from(*level);
                    }
                }
                sample_count += 1;
            }
            if elapsed >= STAGE_WINDOW_MS {
                break;
            }
            if Instant::now() >= deadline || guest_ms.wrapping_sub(started_guest) > guest_timeout_ms
            {
                return Err(format!("timed out sampling L6 level-meter PWM at {value}"));
            }
        }
        if sample_count == 0 {
            return Err("L6 level-meter PWM captured no panel samples".into());
        }
        let group_a_records = qmp
            .read_memory(0, 0x8062_93dc, 12)
            .map_err(|error| error.to_string())?;
        let group_b_records = qmp
            .read_memory(0, 0x8062_93e8, 12)
            .map_err(|error| error.to_string())?;
        let threshold_masks_and_codes = qmp
            .read_memory(0, 0x801f_8e88, 0x32)
            .map_err(|error| error.to_string())?;
        let mean_levels: [[f64; 24]; 8] = std::array::from_fn(|row| {
            std::array::from_fn(|column| sums[row][column] as f64 / sample_count as f64)
        });
        stage_evidence.push(json!({
            "input_level": value,
            "sample_count": sample_count,
            "group_a_records": group_a_records,
            "group_b_records": group_b_records,
            "threshold_masks_and_codes": threshold_masks_and_codes,
            "mean_levels": mean_levels,
        }));
    }
    // Treat this as a diagnostic only when the controlled input reaches a
    // panel output. A stopped task or an unmapped/uninitialized output must
    // never report a passing PWM result.
    let stage_means = stage_evidence
        .iter()
        .map(|stage| {
            stage["mean_levels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    row.as_array()
                        .unwrap()
                        .iter()
                        .map(|level| level.as_f64().unwrap())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let responsive_cells = (0..8)
        .flat_map(|row| (0..24).map(move |column| (row, column)))
        .filter(|&(row, column)| {
            let levels = stage_means
                .iter()
                .map(|stage| stage[row][column])
                .collect::<Vec<_>>();
            levels.first() == Some(&0.0)
                && levels[1..].iter().all(|level| *level > 0.0)
                && levels.windows(2).all(|pair| pair[1] > pair[0])
        })
        .count();
    debugger
        .breakpoint(THRESHOLD_BUILDER, false)
        .map_err(|error| error.to_string())?;
    qmp.resume().map_err(|error| error.to_string())?;

    let evidence = json!({
        "stimulus": "l6_level_meter_threshold_builder_inputs",
        "setup": "boot_time_panel_output_sequence_skipped",
        "responsive_cells": responsive_cells,
        "samples_per_stage": stage_evidence,
    });
    fs::write(
        options.logs.join("l6-level-meter-pwm.json"),
        serde_json::to_vec_pretty(&evidence).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if responsive_cells == 0 {
        return Err("L6 meter stimulus produced no panel cell with five distinct increasing nonzero duty levels; measurements saved to l6-level-meter-pwm.json".into());
    }
    Ok(evidence)
}

fn run_l6max_panel_key_map(session: &Session, wall_timeout_s: u64) -> Result<Value, String> {
    if session.engine.model() != l6fw::Model::L6max {
        return Err("l6max-panel-key-map requires L6max firmware".into());
    }
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("panel inputs unavailable")?;
    let qmp = &session.engine.management[0];
    const KEY_TABLE: u32 = 0x800c_48c4;
    const KEY_STATE: u32 = 0x8088_3554;
    const KEYS: [(u32, u32); 4] = [(15, 0x19), (20, 0x1a), (25, 0x1b), (30, 0x1c)];

    let read_words = || -> Result<[u32; 6], String> {
        let bytes = qmp
            .read_memory(0, KEY_STATE, 24)
            .map_err(|error| error.to_string())?;
        Ok(std::array::from_fn(|index| {
            u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
        }))
    };
    let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    let mut observations = Vec::with_capacity(KEYS.len());
    for (target, code) in KEYS {
        let lookup = qmp
            .read_memory(0, KEY_TABLE + code * 8, 8)
            .map_err(|error| error.to_string())?;
        // The table stores the key's bit number within its state word.
        let bit = u32::from_le_bytes(lookup[..4].try_into().unwrap());
        let word_index = u32::from_le_bytes(lookup[4..].try_into().unwrap()) as usize;
        let mask = 1u32.checked_shl(bit).unwrap_or(0);
        let expected_mask = 1u32 << (code - 0x18);
        if mask != expected_mask || word_index != 4 {
            return Err(format!(
                "L6max panel key {code:#04x} maps to mask={mask:#010x}, word={word_index}; expected mask={expected_mask:#010x}, word=4"
            ));
        }
        let before = read_words()?;
        if before[word_index] & mask != 0 {
            return Err(format!(
                "L6max panel key target {target} starts active: word {}={:#010x}, mask={mask:#010x}",
                word_index, before[word_index]
            ));
        }

        let applied = inputs.panel.snapshot().applied;
        inputs.down(target, 0).map_err(|error| error.to_string())?;
        wait_l6_input(
            session,
            || inputs.panel.snapshot().applied > applied,
            "sound-pad key press acknowledgement",
        )?;
        wait_guest_duration(session, 100, deadline)?;
        let held = read_words()?;
        if held[word_index] & mask == 0 {
            return Err(format!(
                "L6max panel key target {target} did not set key {code:#04x}: word {} before={:#010x} held={:#010x} mask={mask:#010x}",
                word_index, before[word_index], held[word_index]
            ));
        }

        let applied = inputs.panel.snapshot().applied;
        inputs.up(target).map_err(|error| error.to_string())?;
        wait_l6_input(
            session,
            || inputs.panel.snapshot().applied > applied,
            "sound-pad key release acknowledgement",
        )?;
        wait_guest_duration(session, 100, deadline)?;
        let released = read_words()?;
        if released[word_index] & mask != 0 {
            return Err(format!(
                "L6max panel key target {target} remained active after release: word {}={:#010x}, mask={mask:#010x}",
                word_index, released[word_index]
            ));
        }
        observations.push(json!({
            "target": target,
            "key_code": code,
            "word_index": word_index,
            "mask": format!("{mask:#010x}"),
            "before": format!("{:#010x}", before[word_index]),
            "held": format!("{:#010x}", held[word_index]),
            "released": format!("{:#010x}", released[word_index]),
        }));
    }
    Ok(json!({"sound_pad_keys": observations}))
}

fn run_l6_input_sweep(session: &Session, options: &Options) -> Result<Value, String> {
    use std::time::Duration;
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("L6 input sweep requires connected inputs")?;
    let trace_path = options.logs.join("engine.trace");
    let count = |pattern: &str| -> Result<usize, String> {
        let trace = fs::read_to_string(&trace_path).map_err(|error| error.to_string())?;
        Ok(trace.matches(pattern).count())
    };
    for target in [0, 5] {
        let before = inputs.main.snapshot().rejected;
        inputs
            .main
            .tap(target, 120)
            .map_err(|error| error.to_string())?;
        wait_l6_input(
            session,
            || inputs.main.snapshot().rejected > before,
            "unsupported MAIN target rejection",
        )?;
    }

    let mut panel_targets: Vec<u32> = (7..31).collect();
    panel_targets.extend([1, 2, 3, 6, 47]);
    let mut verified = Vec::with_capacity(panel_targets.len());
    for target in panel_targets {
        let press_pattern = "l6 panel USART1 TX 91\n";
        let release_pattern = "l6 panel USART1 TX 90\n";
        let presses_before = count(press_pattern)?;
        let releases_before = count(release_pattern)?;
        let panel = inputs.panel.snapshot();
        inputs
            .panel
            .tap(target, 120)
            .map_err(|error| error.to_string())?;
        wait_l6_input(
            session,
            || {
                let now = inputs.panel.snapshot();
                now.applied > panel.applied && now.released > panel.released
            },
            "panel tap application and release",
        )?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while count(release_pattern)? < releases_before + 1 {
            session.poll()?;
            if Instant::now() >= deadline {
                return Err(format!(
                    "timed out waiting for L6 panel input {target} release report"
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
        let presses = count(press_pattern)?.saturating_sub(presses_before);
        let releases = count(release_pattern)?.saturating_sub(releases_before);
        if (presses, releases) != (1, 1) {
            return Err(format!(
                "L6 panel input {target} emitted {presses} press and {releases} release reports"
            ));
        }
        verified.push(target);
    }

    let encoder_pattern = "l6 panel USART1 TX a1\n";
    let encoder_before = count(encoder_pattern)?;
    let mut encoder_targets = Vec::with_capacity(6);
    for encoder in 0..6 {
        let applied = inputs.panel.snapshot().applied;
        inputs
            .encoder(encoder, 1)
            .map_err(|error| error.to_string())?;
        wait_l6_input(
            session,
            || inputs.panel.snapshot().applied > applied,
            "encoder input application",
        )?;
        encoder_targets.push(encoder);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while count(encoder_pattern)?.saturating_sub(encoder_before) < 6 {
        session.poll()?;
        if Instant::now() >= deadline {
            return Err("timed out waiting for six L6 encoder reports".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let encoder_reports = count(encoder_pattern)?.saturating_sub(encoder_before);
    if encoder_reports != 6 {
        return Err(format!(
            "expected six L6 encoder reports, got {encoder_reports}"
        ));
    }

    for target in [31] {
        let before = inputs.panel.snapshot().rejected;
        inputs
            .panel
            .down(target, 50)
            .map_err(|error| error.to_string())?;
        wait_l6_input(
            session,
            || inputs.panel.snapshot().rejected > before,
            "unsupported panel target rejection",
        )?;
    }
    let before = inputs.panel.snapshot().rejected;
    inputs.encoder(6, 1).map_err(|error| error.to_string())?;
    wait_l6_input(
        session,
        || inputs.panel.snapshot().rejected > before,
        "unsupported encoder rejection",
    )?;

    let rejected_main = inputs.main.snapshot().rejected;
    let rejected_panel = inputs.panel.snapshot().rejected;
    if (rejected_main, rejected_panel) != (2, 2) {
        return Err(format!(
            "L6 invalid-input checks observed {rejected_main} MAIN and {rejected_panel} panel rejections; expected 2 each"
        ));
    }

    Ok(
        json!({"panel_targets":verified,"encoders":encoder_targets,"encoder_reports":encoder_reports,"unsupported_main_targets_rejected":[0,5],"unsupported_panel_target_rejected":31,"unsupported_encoder_rejected":6,"expected_rejections":{"main":rejected_main,"panel":rejected_panel}}),
    )
}

/// Retries connecting to a just-enabled native MIDI endpoint. The host MIDI
/// bridge opens its CoreMIDI/ALSA port on its own worker thread sometime
/// after the engine starts, so a caller that connects right away can lose
/// that race; `crate::host_midi::connect` makes exactly one attempt and
/// returns "missing native MIDI ..." if the port isn't there yet. Polls the
/// session between attempts so the guest keeps running while this waits.
fn connect_native_midi(
    session: &Session,
    endpoint: &str,
) -> Result<Box<dyn crate::host_midi::Client>, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match crate::host_midi::connect(endpoint) {
            Ok(client) => return Ok(client),
            Err(error) if error.contains("missing native MIDI") && Instant::now() < deadline => {
                session.poll()?;
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_l6_din_sweep(session: &Session, options: &Options) -> Result<Value, String> {
    use l6max_host::host_midi::{din_port_name_for_model, endpoint_name};
    let endpoint = endpoint_name(din_port_name_for_model(l6fw::Model::L6));
    let mut client = connect_native_midi(session, &endpoint)?;
    run_l6_din_sweep_inner(session, options, client.as_mut())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_l6_din_dispatch_monitor(
    socket: &std::path::Path,
) -> Result<
    (
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        [std::sync::Arc<std::sync::atomic::AtomicUsize>; 4],
        thread::JoinHandle<()>,
    ),
    String,
> {
    use l6max_diagnostics::gdb::Gdb;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    const BREAKPOINTS: [u32; 6] = [
        0x8002f490, // DIN MIDI CC handler
        0x80020ca0, // Main event submitter
        0x80020690, // Main event queue reader
        0x8003d790, // mixer setter
        0x8003e0c8, // mixer setter
        0x8003e408, // mixer setter
    ];
    let mut gdb = Gdb::connect(socket).map_err(|error| error.to_string())?;
    for address in BREAKPOINTS {
        gdb.breakpoint(address, true)
            .map_err(|error| error.to_string())?;
    }
    let running = Arc::new(AtomicBool::new(true));
    let counts: [Arc<AtomicUsize>; 4] = std::array::from_fn(|_| Arc::new(AtomicUsize::new(0)));
    let worker_running = running.clone();
    let worker_counts = counts.clone();
    let worker = thread::spawn(move || {
        while worker_running.load(Ordering::Relaxed) {
            if gdb.continue_until_stop(Duration::from_secs(120)).is_err() {
                break;
            }
            if !worker_running.load(Ordering::Relaxed) {
                break;
            }
            let Ok(pc) = gdb.read_register(15) else {
                break;
            };
            let address = pc & !1;
            let counter = match address {
                0x8002f490 => Some(&worker_counts[0]),
                0x80020ca0 => Some(&worker_counts[1]),
                0x80020690 => Some(&worker_counts[2]),
                0x8003d790 | 0x8003e0c8 | 0x8003e408 => Some(&worker_counts[3]),
                _ => None,
            };
            if let Some(counter) = counter {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            if gdb.breakpoint(address, false).is_err()
                || gdb.request("s").is_err()
                || gdb.breakpoint(address, true).is_err()
            {
                break;
            }
        }
    });
    Ok((running, counts, worker))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn run_l6_din_sweep(_session: &Session, _options: &Options) -> Result<Value, String> {
    Err("l6-din-sweep requires CoreMIDI or ALSA".into())
}

fn run_l6_din_sweep_inner(
    session: &Session,
    options: &Options,
    client: &mut dyn crate::host_midi::Client,
) -> Result<Value, String> {
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("L6 DIN sweep has no inputs")?;
    let mixer_snapshot = || -> Result<Vec<u8>, String> {
        let qmp = &session.engine.management[0];
        let mut snapshot = qmp
            .read_memory(0, 0x80440dca, 0x7e)
            .map_err(|error| error.to_string())?;
        snapshot.extend(
            qmp.read_memory(0, 0x80440e9a, 6)
                .map_err(|error| error.to_string())?,
        );
        Ok(snapshot)
    };
    let trace_path = options.logs.join("engine.trace");
    let ready_deadline = Instant::now() + Duration::from_secs(120);
    let main_queue_pending = || -> Result<u32, String> {
        let bytes = session.engine.management[0]
            .read_memory(0, 0x806b2f20, 4)
            .map_err(|e| e.to_string())?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    };
    // Main must be draining its queue, or the sweep only fills it.
    loop {
        session.poll()?;
        let trace = fs::read_to_string(&trace_path).map_err(|e| e.to_string())?;
        if trace.contains("LPUART2 (MIDI jacks) CTRL 002c0000") && main_queue_pending()? == 0 {
            break;
        }
        if Instant::now() >= ready_deadline {
            return Err("timed out waiting for L6 DIN UART receive and interrupt enable".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let socket = options
        .gdb_socket
        .as_deref()
        .ok_or("L6 DIN sweep has no GDB socket")?;
    let (dispatch_running, dispatch_counts, dispatch_monitor) =
        start_l6_din_dispatch_monitor(socket)?;
    let before = l6max_host::indicators::rings(inputs.panel.snapshot().indicators)[0];
    let queue_before = session.engine.management[0]
        .read_memory(0, 0x806b2f20, 4)
        .map_err(|e| e.to_string())?;
    let queue_before = u32::from_le_bytes(queue_before.try_into().unwrap());
    let mixer_before = mixer_snapshot()?;
    let mut previous = before;
    let mut changes = Vec::new();
    for cc in 0..=127u8 {
        client.send(&[0xb0, cc, 0x7f])?;
        let deadline = Instant::now() + Duration::from_millis(50);
        while Instant::now() < deadline {
            session.poll()?;
            thread::sleep(Duration::from_millis(2));
        }
        let ring = l6max_host::indicators::rings(inputs.panel.snapshot().indicators)[0];
        if ring != previous {
            changes.push((cc, previous, ring));
            previous = ring;
        }
    }
    let trace = fs::read_to_string(&trace_path).map_err(|e| e.to_string())?;
    let accepted_bytes = trace
        .lines()
        .filter(|line| {
            line.contains("LPUART2 (MIDI jacks) host RX ") && line.contains(" RE 1 RIE 1")
        })
        .filter_map(|line| {
            line.split("host RX ")
                .nth(1)?
                .split(" bytes CTRL")
                .next()?
                .parse::<usize>()
                .ok()
        })
        .sum::<usize>();
    let expected = (0..=127u8)
        .flat_map(|cc| [0xb0, cc, 0x7f])
        .collect::<Vec<_>>();
    let drain_deadline = Instant::now() + Duration::from_secs(10);
    let (din_ring, ring_state) = loop {
        session.poll()?;
        let state = session.engine.management[0]
            .read_memory(0, 0x8062eda8, 0x20)
            .map_err(|e| e.to_string())?;
        let write_index = u32::from_le_bytes(state[4..8].try_into().unwrap());
        let read_index = u32::from_le_bytes(state[12..16].try_into().unwrap());
        if write_index >= expected.len() as u32 && read_index >= expected.len() as u32 {
            let bytes = session.engine.management[0]
                .read_memory(0, 0x8062e5a8, expected.len())
                .map_err(|e| e.to_string())?;
            break (bytes, state);
        }
        if Instant::now() >= drain_deadline {
            return Err(format!(
                "L6 DIN ring did not drain: write index {write_index}, read index {read_index}"
            ));
        }
        thread::sleep(Duration::from_millis(5));
    };
    session.engine.management[0]
        .stop()
        .map_err(|e| format!("cannot stop L6 after DIN dispatch sweep: {e}"))?;
    dispatch_running.store(false, std::sync::atomic::Ordering::Relaxed);
    dispatch_monitor
        .join()
        .map_err(|_| "L6 DIN dispatch monitor thread panicked".to_owned())?;
    let dispatch_hits = dispatch_counts
        .each_ref()
        .map(|count| count.load(std::sync::atomic::Ordering::Relaxed));
    let matching = din_ring
        .iter()
        .zip(&expected)
        .filter(|(a, b)| a == b)
        .count();
    let write_index = u32::from_le_bytes(ring_state[4..8].try_into().unwrap());
    let read_index = u32::from_le_bytes(ring_state[12..16].try_into().unwrap());
    let queue_after = session.engine.management[0]
        .read_memory(0, 0x806b2f20, 4)
        .map_err(|e| e.to_string())?;
    let queue_after = u32::from_le_bytes(queue_after.try_into().unwrap());
    let mixer_after = mixer_snapshot()?;
    let mixer_changed_bytes = mixer_before
        .iter()
        .zip(&mixer_after)
        .filter(|(a, b)| a != b)
        .count();
    let dispatch_map = session.engine.management[0]
        .read_memory(0, 0x80629bb8, 128)
        .map_err(|e| e.to_string())?;
    let mapped_cc_count = dispatch_map.iter().filter(|cc| **cc != 0xff).count();
    fs::write(
        options.logs.join("l6-din-sweep.json"),
        serde_json::to_vec_pretty(&json!({"sent_messages":128,"uart_bytes":accepted_bytes,"ring_bytes":matching,"write_index":write_index,"read_index":read_index,"indicator_before":before,"indicator_after":previous,"indicator_changes":changes,"main_queue_items_before":queue_before,"main_queue_items_after":queue_after,"mapped_cc_count":mapped_cc_count,"mixer_state_changed_bytes":mixer_changed_bytes})).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    if accepted_bytes != expected.len()
        || matching != expected.len()
        || write_index != expected.len() as u32
        || read_index != write_index
        || changes.is_empty()
        || dispatch_hits[0] != 128
        || dispatch_hits[1] < mapped_cc_count
        || dispatch_hits[2] < dispatch_hits[1]
        || dispatch_hits[3] == 0
        || mixer_changed_bytes == 0
    {
        return Err(format!(
            "L6 DIN sweep failed: UART accepted {accepted_bytes}/{} bytes, ring {matching}/{}, indices {write_index}/{read_index}, indicator changes {}, Main queue {queue_before}->{queue_after}, mapped CCs {mapped_cc_count}, dispatch hits handler/submit/reader/setter={dispatch_hits:?}, mixer bytes changed {mixer_changed_bytes}",
            expected.len(),
            expected.len(),
            changes.len()
        ));
    }
    Ok(
        json!({"sent_messages":128,"uart_bytes":accepted_bytes,"ring_bytes":matching,"write_index":write_index,"read_index":read_index,"indicator_changes":changes.len(),"dispatch_hits":{"midi_handler":dispatch_hits[0],"event_submitter":dispatch_hits[1],"main_queue_reader":dispatch_hits[2],"mixer_setters":dispatch_hits[3]},"mixer_changed_bytes":mixer_changed_bytes}),
    )
}

fn run_l6max_input_timing(session: &Session, options: &Options) -> Result<Value, String> {
    use std::time::Duration;
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("L6max input timing check requires connected inputs")?;
    let report_count = |pattern: &str| -> Result<usize, String> {
        let trace = fs::read_to_string(options.logs.join("engine.trace"))
            .map_err(|error| error.to_string())?;
        Ok(trace.matches(pattern).count())
    };
    let current = session.poll()?;
    let qmp_clients = &session.engine.management;
    for client in qmp_clients {
        client.stop().map_err(|error| error.to_string())?;
    }
    // Take the baseline once paused: a heartbeat sent just before the pause can
    // still arrive after it, and would read as the clock advancing while paused.
    thread::sleep(Duration::from_millis(100));
    session.poll()?;
    let input_before = inputs.panel.snapshot();
    let presses_before = report_count("l6 panel USART1 TX 91\n")?;
    let releases_before = report_count("l6 panel USART1 TX 90\n")?;
    let send_result = (|| -> Result<(), String> {
        inputs
            .panel
            .down(1, 50)
            .map_err(|error| error.to_string())?;
        inputs.panel.up(1).map_err(|error| error.to_string())?;
        inputs
            .panel
            .down(1, 50)
            .map_err(|error| error.to_string())?;
        inputs.panel.up(1).map_err(|error| error.to_string())?;
        wait_l6_input(
            session,
            || inputs.panel.snapshot().applied >= input_before.applied + 1,
            "first paused L6max input acknowledgement",
        )?;
        thread::sleep(Duration::from_millis(700));
        let paused = inputs.panel.snapshot();
        if paused.applied != input_before.applied + 1 || paused.guest_ms != input_before.guest_ms {
            return Err(format!(
                "L6max input progressed while QEMU was paused: before={input_before:?}, paused={paused:?}"
            ));
        }
        Ok(())
    })();
    let mut resume_error = None;
    for client in qmp_clients {
        if let Err(error) = client.resume() {
            resume_error.get_or_insert_with(|| error.to_string());
        }
    }
    send_result?;
    if let Some(error) = resume_error {
        return Err(format!("could not resume QEMU after paused input: {error}"));
    }
    wait_l6_input(
        session,
        || inputs.panel.snapshot().applied >= input_before.applied + 4,
        "queued L6max input delivery after resume",
    )?;
    let report_deadline = Instant::now() + Duration::from_secs(10);
    while report_count("l6 panel USART1 TX 91\n")? < presses_before + 2
        || report_count("l6 panel USART1 TX 90\n")? < releases_before + 2
    {
        session.poll()?;
        if Instant::now() >= report_deadline {
            return Err("timed out waiting for two L6max press/release reports".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let presses = report_count("l6 panel USART1 TX 91\n")? - presses_before;
    let releases = report_count("l6 panel USART1 TX 90\n")? - releases_before;
    if (presses, releases) != (2, 2) {
        return Err(format!(
            "L6max rapid input emitted {presses} press and {releases} release reports"
        ));
    }
    let resumed_guest = inputs.panel.snapshot().guest_ms;
    wait_l6_input(
        session,
        || inputs.panel.snapshot().guest_ms.wrapping_sub(resumed_guest) >= 1500,
        "L6max display settle after rapid input",
    )?;
    let after = session.poll()?;
    if current.display == after.display {
        return Err("L6max display did not change after the two Up inputs".into());
    }
    let encoder_before = report_count("l6 panel USART1 TX a1\n")?;
    let applied = inputs.panel.snapshot().applied;
    inputs.encoder(2, 1).map_err(|error| error.to_string())?;
    wait_l6_input(
        session,
        || inputs.panel.snapshot().applied > applied,
        "L6max encoder input acknowledgement",
    )?;
    let encoder_deadline = Instant::now() + Duration::from_secs(10);
    while report_count("l6 panel USART1 TX a1\n")? < encoder_before + 1 {
        session.poll()?;
        if Instant::now() >= encoder_deadline {
            return Err("timed out waiting for the L6max encoder report".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let encoder_reports = report_count("l6 panel USART1 TX a1\n")? - encoder_before;
    if encoder_reports != 1 {
        return Err(format!(
            "expected one L6max encoder report, got {encoder_reports}"
        ));
    }
    Ok(
        json!({"paused_guest_clock":true,"rapid_press_reports":presses,"rapid_release_reports":releases,"display_changed":true,"encoder_channel":3,"encoder_reports":encoder_reports}),
    )
}

impl State {
    pub fn restarted(&mut self) {
        self.midi = None;
    }
    pub fn execute(
        &mut self,
        session: &Session,
        options: &Options,
        operation: Operation,
        guest_timeout_ms: u32,
        wall_timeout_s: u64,
    ) -> Result<Value, String> {
        let started = Instant::now();
        let guest_start = session
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .panel
            .snapshot()
            .guest_ms;
        let result = self.execute_inner(
            session,
            options,
            operation,
            guest_timeout_ms,
            wall_timeout_s,
        );
        let result = result.map_err(|e| e.to_string())?;
        let elapsed = session
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .panel
            .snapshot()
            .guest_ms
            .wrapping_sub(guest_start);
        if started.elapsed() > Duration::from_secs(wall_timeout_s) || elapsed > guest_timeout_ms {
            return Err("device operation exceeded its step deadline".into());
        }
        Ok(result)
    }

    fn run_host_midi_round_trip(
        &mut self,
        session: &Session,
        guest_timeout_ms: u32,
        wall_timeout_s: u64,
        direct_cable: Option<u8>,
        expect_ring_change: bool,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let model = session.engine.model();
        let endpoint = l6max_host::host_midi::endpoint_name(
            l6max_host::host_midi::port_names_for_model(model)[1],
        );
        let mut client: Box<dyn crate::host_midi::Client> = if let Some(cable) = direct_cable {
            Box::new(crate::host_midi::DirectUsbClient::connect(
                session.engine.usb.clone().ok_or("USB host unavailable")?,
                cable,
            )?)
        } else {
            let endpoint_deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match crate::host_midi::connect(&endpoint) {
                    Ok(client) => break client,
                    Err(error)
                        if error.contains("missing native MIDI")
                            && Instant::now() < endpoint_deadline =>
                    {
                        session.poll()?;
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => {
                        let bridge = session
                            .engine
                            .midi
                            .as_ref()
                            .map(|midi| midi.status.lock().unwrap().clone())
                            .unwrap_or_else(|| "USB MIDI bridge unavailable".into());
                        return Err(std::io::Error::other(format!(
                            "{error}; bridge status: {bridge}"
                        ))
                        .into());
                    }
                }
            }
        };
        let inputs = session
            .engine
            .inputs
            .as_ref()
            .ok_or("MIDI test needs inputs")?;
        let expected_cc = if model == l6fw::Model::L6 {
            [0xb0, 0x53]
        } else {
            [0xb0, 0x51]
        };
        let start_guest = inputs.panel.snapshot().guest_ms;
        let deadline = Instant::now() + Duration::from_secs(wall_timeout_s.min(30));
        // Alternate direction: repeated turns one way saturate and send no CC.
        self.encoder_turned_up = !self.encoder_turned_up;
        inputs.encoder(0, if self.encoder_turned_up { -4 } else { 4 })?;
        let mut knob_message = None;
        while Instant::now() < deadline {
            session.poll()?;
            if let Some(message) = client.poll().map_err(std::io::Error::other)? {
                if message[..2] == expected_cc {
                    knob_message = Some(message);
                    break;
                }
            }
            let elapsed = inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest);
            if elapsed > guest_timeout_ms {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let knob_message = knob_message.ok_or_else(|| {
            std::io::Error::other(format!(
                "model-specific encoder CC {expected_cc:02x?} did not reach the host MIDI endpoint"
            ))
        })?;
        // Let the encoder's remaining reports land before taking the baseline.
        wait_guest_duration(session, 1000, deadline)?;
        let before = inputs.panel.snapshot().indicators;
        let mixer_cc = if model == l6fw::Model::L6 { 0x53 } else { 0x51 };
        client
            .send(&[0xb0, mixer_cc, 100])
            .map_err(std::io::Error::other)?;
        let indicator_deadline =
            Instant::now() + Duration::from_secs(3).min(Duration::from_secs(wall_timeout_s));
        let after = loop {
            session.poll()?;
            let after = inputs.panel.snapshot().indicators;
            let changed =
                l6max_host::indicators::rings(after)[0] != l6max_host::indicators::rings(before)[0];
            if changed {
                if expect_ring_change {
                    break after;
                }
                return Err(std::io::Error::other(format!(
                    "direct USB MIDI cable {} unexpectedly changed the channel-1 ring",
                    direct_cable.unwrap_or(1)
                ))
                .into());
            }
            if Instant::now() >= indicator_deadline {
                if !expect_ring_change {
                    break before;
                }
                return Err(std::io::Error::other(format!(
                    "host MIDI CC {mixer_cc:#04x}=100 did not change the model's firmware indicator"
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(5));
        };
        self.host_midi = Some(client);
        let endpoint_description = if let Some(cable) = direct_cable {
            format!("direct-usb-cable-{cable}")
        } else {
            endpoint
        };
        Ok(
            json!({"endpoint":endpoint_description,"encoder_cc":knob_message,"mixer_cc":[0xb0,mixer_cc,100],"expect_ring_change":expect_ring_change,"indicator_before":before,"indicator_after":after}),
        )
    }

    fn run_l6_din_usb_forwarding(
        &mut self,
        session: &Session,
        wall_timeout_s: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        use l6max_host::host_midi::{din_port_name_for_model, endpoint_name};

        if session.engine.model() != l6fw::Model::L6 {
            return Err("l6-din-usb-forwarding requires L6 firmware".into());
        }
        let inputs = session
            .engine
            .inputs
            .as_ref()
            .ok_or("L6 DIN forwarding needs panel inputs")?;
        session.engine.connect_midi();
        let endpoint = endpoint_name(din_port_name_for_model(l6fw::Model::L6));
        let endpoint_deadline = Instant::now() + Duration::from_secs(10);
        let mut din = loop {
            match crate::host_midi::connect(&endpoint) {
                Ok(client) => break client,
                Err(error)
                    if error.contains("missing native MIDI")
                        && Instant::now() < endpoint_deadline =>
                {
                    session.poll()?;
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error.into()),
            }
        };
        wait_guest_duration(
            session,
            300,
            Instant::now() + Duration::from_secs(wall_timeout_s),
        )?;
        let usb = session
            .engine
            .usb
            .as_ref()
            .ok_or("L6 USB MIDI host is unavailable")?;
        usb.connect(true)
            .map_err(|error| format!("cannot connect L6 USB device: {error}"))?;
        let usb_deadline = Instant::now() + Duration::from_secs(15);
        let endpoints = loop {
            match usb.enumerate() {
                Ok(configuration) => {
                    break configuration
                        .midi
                        .ok_or("L6 USB device has no MIDI endpoints")?;
                }
                Err(error)
                    if (error.raw_os_error() == Some(libc::ENODEV)
                        || error.raw_os_error() == Some(libc::EAGAIN)
                        || error.kind() == std::io::ErrorKind::InvalidData)
                        && Instant::now() < usb_deadline =>
                {
                    session.poll()?;
                    thread::sleep(Duration::from_millis(250));
                }
                Err(error) => {
                    return Err(format!("cannot enumerate L6 USB MIDI: {error}").into());
                }
            }
        };
        let _ = usb.try_receive(endpoints.input, 16_384);
        let probes: [&[u8]; 4] = [
            &[0xb3, 0x10, 0x20],
            &[0x93, 0x45, 0x64],
            &[0xf8],
            &[0xf0, 0x7d, 0x01, 0xf7],
        ];
        let start_guest_ms = inputs.panel.snapshot().guest_ms;
        for probe in probes {
            din.send_bytes(probe).map_err(std::io::Error::other)?;
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut forwarded_bytes = Vec::new();
            let mut forwarded = false;
            while Instant::now() < deadline {
                session.poll()?;
                match usb.try_receive(endpoints.input, 16_384) {
                    Ok(bytes) => {
                        let messages = l6max_host::usb_midi::decode(&bytes)
                            .map_err(|error| format!("cannot decode USB MIDI: {error}"))?;
                        for (cable, data) in messages {
                            if cable != 0
                                || (forwarded_bytes.is_empty() && data.first() != probe.first())
                            {
                                continue;
                            }
                            forwarded_bytes.extend(data);
                            if forwarded_bytes == probe {
                                forwarded = true;
                                break;
                            }
                            if !probe.starts_with(&forwarded_bytes) {
                                forwarded_bytes.clear();
                            }
                        }
                        if forwarded {
                            break;
                        }
                    }
                    Err(error) if error.raw_os_error() == Some(libc::EAGAIN) => {}
                    Err(error) => return Err(format!("USB MIDI receive failed: {error}").into()),
                }
                thread::sleep(Duration::from_millis(5));
            }
            if !forwarded {
                return Err(format!(
                    "L6 DIN-to-USB forwarding mismatch: no USB cable-0 message matched {probe:02x?} (received {forwarded_bytes:02x?})"
                )
                .into());
            }
            tracing::info!(
                event = "l6_din_usb_forwarded",
                bytes = ?probe,
                guest_ms = inputs.panel.snapshot().guest_ms,
                "L6 DIN message reached USB MIDI IN unchanged"
            );
        }
        let guest_elapsed = inputs
            .panel
            .snapshot()
            .guest_ms
            .wrapping_sub(start_guest_ms);
        din.release();
        Ok(
            json!({"probes":probes.iter().map(|probe| probe.to_vec()).collect::<Vec<_>>(),"forwarded_count":probes.len(),"guest_elapsed_ms":guest_elapsed,"din_endpoint":endpoint,"usb_cable":0}),
        )
    }

    fn run_l6max_recorder_playback(
        &mut self,
        session: &Session,
        options: &Options,
        target: u32,
        guest_timeout_ms: u32,
        wall_timeout_s: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        if session.engine.model() != l6fw::Model::L6max || target != 4 {
            return Err(std::io::Error::other(
                "l6max-recorder-playback requires L6max firmware and MAIN target 4",
            )
            .into());
        }
        if std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_none() {
            return Err(std::io::Error::other(
                "l6max-recorder-playback requires L6_AUDIO_SAMPLE_TRACE=1",
            )
            .into());
        }
        if guest_timeout_ms < 10_000 {
            return Err(std::io::Error::other(
                "l6max-recorder-playback guest timeout must be at least 10000 ms",
            )
            .into());
        }
        let inputs = session
            .engine
            .inputs
            .as_ref()
            .ok_or("L6max recorder playback has no MAIN input")?;
        let qmp = &session.engine.management[0];
        let socket = options
            .gdb_socket
            .as_ref()
            .ok_or("l6max-recorder-playback requires GDB")?;
        let mut debugger =
            l6max_diagnostics::gdb::Gdb::connect(socket).map_err(std::io::Error::other)?;
        let trace_path = options.logs.join("engine.trace");
        const PREFIX: &str = "l6 audio TX samples sai=00384000";
        let baseline = fs::read_to_string(&trace_path)?;
        let baseline_lines = baseline
            .lines()
            .filter(|line| line.starts_with(PREFIX))
            .collect::<Vec<_>>();
        let baseline_signature = baseline_lines
            .last()
            .and_then(|line| tx_sample_signature(line));

        qmp.stop()?;
        const SUBMIT: u32 = 0x8004_5458;
        debugger.breakpoint(SUBMIT, true)?;
        let before_applied = inputs.main.snapshot().applied;
        inputs.main.down(target, 10_000)?;
        let input_deadline = Instant::now() + Duration::from_secs(wall_timeout_s.min(30));
        loop {
            session.poll().map_err(std::io::Error::other)?;
            let status = inputs.main.snapshot();
            if !status.connected {
                return Err(std::io::Error::other(format!(
                    "MAIN input disconnected during L6max Play: {status:?}"
                ))
                .into());
            }
            if status.applied > before_applied {
                break;
            }
            if Instant::now() >= input_deadline {
                return Err(std::io::Error::other("L6max Play press was not acknowledged").into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        debugger.continue_until_stop(Duration::from_secs(10))?;
        let submit_pc = debugger.read_register(15)? & !1;
        let submit_args = (0..4)
            .map(|register| debugger.read_register(register))
            .collect::<Result<Vec<_>, _>>()?;
        if submit_pc != SUBMIT || submit_args.get(2) != Some(&0x2c) {
            return Err(std::io::Error::other(format!(
                "L6max target 4 did not submit Play event 0x2c at {SUBMIT:#010x}: pc={submit_pc:#010x}, args={submit_args:?}"
            ))
            .into());
        }
        debugger.breakpoint(SUBMIT, false)?;
        const SEND_RETURN: u32 = 0x8002_80a0;
        debugger.breakpoint(SEND_RETURN, true)?;
        debugger.continue_until_stop(Duration::from_secs(5))?;
        let send_pc = debugger.read_register(15)? & !1;
        let send_result = debugger.read_register(0)?;
        if send_pc != SEND_RETURN || send_result != 0 {
            return Err(std::io::Error::other(format!(
                "L6max Play queue submission failed: pc={send_pc:#010x}, result={send_result:#010x}"
            ))
            .into());
        }
        debugger.breakpoint(SEND_RETURN, false)?;

        const HANDLER: u32 = 0x8003_6a40;
        debugger.breakpoint(HANDLER, true)?;
        debugger.continue_until_stop(Duration::from_secs(10))?;
        let handler_pc = debugger.read_register(15)? & !1;
        let event_pointer = debugger.read_register(0)?;
        if handler_pc != HANDLER {
            return Err(std::io::Error::other(format!(
                "L6max Play dispatch stopped at {handler_pc:#010x}, expected {HANDLER:#010x}"
            ))
            .into());
        }
        let event = qmp.read_memory(0, event_pointer, 20)?;
        let event_fields = event
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        if event_fields.first() != Some(&0) || event_fields.get(2) != Some(&0x2c) {
            return Err(std::io::Error::other(format!(
                "L6max main-button handler received unexpected event {event_fields:08x?}"
            ))
            .into());
        }
        debugger.breakpoint(HANDLER, false)?;

        const CALLBACK_SCHEDULER: u32 = 0x8008_bd90;
        debugger.breakpoint(CALLBACK_SCHEDULER, true)?;
        let callback_deadline = Instant::now() + Duration::from_secs(10);
        let callback_target = loop {
            if Instant::now() >= callback_deadline {
                return Err(std::io::Error::other(
                    "L6max Play handler did not schedule callback slot 2",
                )
                .into());
            }
            debugger
                .continue_until_stop(callback_deadline.saturating_duration_since(Instant::now()))?;
            let pc = debugger.read_register(15)? & !1;
            if pc != CALLBACK_SCHEDULER {
                return Err(std::io::Error::other(format!(
                    "L6max callback scheduler stopped at {pc:#010x}"
                ))
                .into());
            }
            let callback = debugger.read_register(0)?;
            let caller = debugger.read_register(14)? & !1;
            if callback == 2 && (0x8003_6a40..0x8003_728c).contains(&caller) {
                let window =
                    u32::from_le_bytes(qmp.read_memory(0, 0x8020_2610, 4)?.try_into().unwrap());
                let table =
                    u32::from_le_bytes(qmp.read_memory(0, window + 24, 4)?.try_into().unwrap());
                let target =
                    u32::from_le_bytes(qmp.read_memory(0, table + 8, 4)?.try_into().unwrap()) & !1;
                if !(0x8000_0000..0x8100_0000).contains(&target) {
                    return Err(std::io::Error::other(format!(
                        "L6max Play callback target is invalid: {target:#010x}"
                    ))
                    .into());
                }
                break target;
            }
            debugger.breakpoint(CALLBACK_SCHEDULER, false)?;
            debugger.request("s")?;
            debugger.breakpoint(CALLBACK_SCHEDULER, true)?;
        };
        debugger.breakpoint(CALLBACK_SCHEDULER, false)?;
        debugger.breakpoint(callback_target, true)?;
        debugger.continue_until_stop(Duration::from_secs(5))?;
        let callback_pc = debugger.read_register(15)? & !1;
        if callback_pc != callback_target {
            return Err(std::io::Error::other(format!(
                "L6max Play callback stopped at {callback_pc:#010x}, expected {callback_target:#010x}"
            ))
            .into());
        }
        debugger.breakpoint(callback_target, false)?;
        const RECORDER_ACTION: u32 = 0x8004_6d68;
        debugger.breakpoint(RECORDER_ACTION, true)?;
        debugger.continue_until_stop(Duration::from_secs(5))?;
        let action_pc = debugger.read_register(15)? & !1;
        if action_pc != RECORDER_ACTION {
            return Err(std::io::Error::other(format!(
                "L6max Play callback did not reach recorder action: stopped at {action_pc:#010x}"
            ))
            .into());
        }
        debugger.breakpoint(RECORDER_ACTION, false)?;
        const STREAM_READ: u32 = 0x8004_a130;
        debugger.breakpoint(STREAM_READ, true)?;
        let stream_read_hit = match debugger.continue_until_stop(Duration::from_secs(10)) {
            Ok(_) => {
                let pc = debugger.read_register(15)? & !1;
                if pc != STREAM_READ {
                    return Err(std::io::Error::other(format!(
                        "L6max recorder stream-read probe stopped at {pc:#010x}, expected {STREAM_READ:#010x}"
                    ))
                    .into());
                }
                true
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt()?;
                false
            }
            Err(error) => return Err(error.into()),
        };
        debugger.breakpoint(STREAM_READ, false)?;
        qmp.resume()?;

        let sample_deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
        wait_guest_duration(session, 10_000, sample_deadline).map_err(std::io::Error::other)?;
        let after = fs::read_to_string(&trace_path)?;
        let samples = after
            .lines()
            .filter(|line| line.starts_with(PREFIX))
            .skip(baseline_lines.len())
            .collect::<Vec<_>>();
        let nonzero_samples = samples
            .iter()
            .filter(|line| trace_u64_field(line, "nonzero").unwrap_or(0) > 0)
            .count();
        let tx_changed = samples.iter().any(|line| {
            let signature = tx_sample_signature(line);
            signature.is_some() && signature != baseline_signature
        });
        qmp.stop()?;
        let tcd = qmp.read_memory(0, 0x400e_91c0, 32)?;
        let source = u32::from_le_bytes(tcd[0..4].try_into().unwrap());
        let source_words = if (0x2000_0000..0x2008_0000).contains(&source) {
            qmp.read_memory(0, source, 16)?
                .chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        let source_has_audio = source_words
            .iter()
            .any(|word| *word != 0 && *word != u32::MAX);
        let evidence = json!({
            "model":"l6max", "target":target, "event":0x2c,
            "event_fields":event_fields, "callback_target":callback_target,
            "recorder_action":action_pc, "stream_read_reached":stream_read_hit,
            "guest_ms":inputs.panel.snapshot().guest_ms,
            "tx_sample_windows":samples.len(), "nonzero_sample_windows":nonzero_samples,
            "tx_signature_changed":tx_changed, "channel14_source":source,
            "channel14_source_words":source_words, "source_has_audio":source_has_audio,
        });
        fs::write(
            options.logs.join("l6max-recorder-playback.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
        if !stream_read_hit || nonzero_samples == 0 || !tx_changed || !source_has_audio {
            return Err(std::io::Error::other(format!(
                "L6max recorder playback entered the firmware action but produced no verified audio: {evidence}"
            ))
            .into());
        }
        Ok(evidence)
    }

    fn run_l6max_din_mixer_control(
        &mut self,
        session: &Session,
        expect_din_ring_change: bool,
        guest_timeout_ms: u32,
        wall_timeout_s: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        use crate::host_midi::Client as _;

        if session.engine.model() != l6fw::Model::L6max {
            return Err("l6max-din-mixer-control requires L6max firmware".into());
        }
        let inputs = session
            .engine
            .inputs
            .as_ref()
            .ok_or("DIN mixer control needs panel inputs")?;
        let model = session.engine.model();
        let din_name = l6max_host::host_midi::endpoint_name(
            l6max_host::host_midi::din_port_name_for_model(model),
        );
        let mut din = connect_native_midi(session, &din_name)?;
        let usb = session
            .engine
            .usb
            .clone()
            .ok_or("USB host unavailable for mixer-control reference path")?;
        let mut mixer =
            crate::host_midi::DirectUsbClient::connect(usb, 1).map_err(std::io::Error::other)?;
        let ring = || l6max_host::indicators::rings(inputs.panel.snapshot().indicators)[0];
        let timeout = Duration::from_secs(wall_timeout_s.min(3));

        let usb_before = ring();
        mixer
            .send(&[0xb0, 0x51, 80])
            .map_err(std::io::Error::other)?;
        let usb_guest_start = inputs.panel.snapshot().guest_ms;
        let usb_deadline = Instant::now() + timeout;
        let usb_after = loop {
            session.poll()?;
            let current = ring();
            if current != usb_before {
                break current;
            }
            let elapsed = inputs
                .panel
                .snapshot()
                .guest_ms
                .wrapping_sub(usb_guest_start);
            if Instant::now() >= usb_deadline || elapsed > guest_timeout_ms {
                return Err(
                    "USB Mixer Control reference CC did not change the channel-1 ring".into(),
                );
            }
            thread::sleep(Duration::from_millis(5));
        };
        // A ring update spans several panel scan rows, so the first changed
        // sample can be a partial value. Take the DIN baseline only after the
        // USB-driven ring has held one value for 200 guest ms.
        let (mut settled, mut since) = (usb_after, inputs.panel.snapshot().guest_ms);
        let usb_after = loop {
            session.poll()?;
            let now = inputs.panel.snapshot().guest_ms;
            let current = ring();
            if current != settled {
                (settled, since) = (current, now);
            } else if now.wrapping_sub(since) >= 200 {
                break settled;
            }
            if Instant::now() >= usb_deadline + timeout
                || now.wrapping_sub(usb_guest_start) > guest_timeout_ms
            {
                return Err("USB Mixer Control ring did not settle".into());
            }
            thread::sleep(Duration::from_millis(5));
        };

        let din_before = ring();
        din.send(&[0xb0, 0x51, 40]).map_err(std::io::Error::other)?;
        let din_guest_start = inputs.panel.snapshot().guest_ms;
        let din_deadline = Instant::now() + timeout;
        let din_after = loop {
            session.poll()?;
            let current = ring();
            if current != din_before {
                if !expect_din_ring_change {
                    return Err(format!(
                        "stock L6max DIN CC unexpectedly changed the channel-1 ring ({din_before:#x} -> {current:#x})"
                    )
                    .into());
                }
                break current;
            }
            let elapsed = inputs
                .panel
                .snapshot()
                .guest_ms
                .wrapping_sub(din_guest_start);
            if elapsed >= 500 {
                if expect_din_ring_change {
                    return Err("patched L6max DIN CC did not change the channel-1 ring".into());
                }
                break current;
            }
            if Instant::now() >= din_deadline || elapsed > guest_timeout_ms {
                return Err("L6max DIN mixer check timed out before its observation window".into());
            }
            thread::sleep(Duration::from_millis(5));
        };

        while din.poll().map_err(std::io::Error::other)?.is_some() {}
        inputs.tap(22, 50).map_err(std::io::Error::other)?;
        let echo_deadline = Instant::now() + Duration::from_millis(600);
        let mut echoed = Vec::new();
        while Instant::now() < echo_deadline {
            session.poll()?;
            if let Some(message) = din.poll().map_err(std::io::Error::other)? {
                echoed.push(message);
            }
            thread::sleep(Duration::from_millis(5));
        }
        if !echoed.is_empty() {
            return Err(format!(
                "Mixer Control via MIDI left DIN OUT mute echo enabled: {echoed:02x?}"
            )
            .into());
        }
        Ok(json!({
            "model": "l6max",
            "usb_mixer_ring_before": usb_before,
            "usb_mixer_ring_after": usb_after,
            "din_ring_before": din_before,
            "din_ring_after": din_after,
            "expect_din_ring_change": expect_din_ring_change,
            "din_mute_echo": echoed,
        }))
    }

    fn run_host_midi_sync(
        &mut self,
        session: &Session,
        guest_timeout_ms: u32,
        wall_timeout_s: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let model = session.engine.model();
        let client = self
            .host_midi
            .as_mut()
            .ok_or("host-midi-sync requires a preceding host-midi-round-trip action")?;
        let start = session
            .engine
            .inputs
            .as_ref()
            .ok_or("MIDI test needs inputs")?
            .panel
            .snapshot()
            .guest_ms;
        let mut l6_remaining = Vec::new();
        let (expected_count, expected_status) = if model == l6fw::Model::L6 {
            let qmp = &session.engine.management[0];
            let channel = qmp.read_memory(0, 0x804410f0, 1)?[0];
            if channel > 15 {
                return Err(std::io::Error::other(format!(
                    "invalid configured L6 MIDI channel {channel}"
                ))
                .into());
            }
            for cc in qmp.read_memory(0, 0x804410f5, 54)? {
                if cc != 0xff {
                    if let Some((_, count)) =
                        l6_remaining.iter_mut().find(|(mapped, _)| *mapped == cc)
                    {
                        *count += 1;
                    } else {
                        l6_remaining.push((cc, 1usize));
                    }
                }
            }
            let count = l6_remaining.iter().map(|(_, count)| count).sum::<usize>();
            if count == 0 {
                return Err(std::io::Error::other("L6 MIDI CC map has no enabled entries").into());
            }
            (count, Some(0xb0 | channel))
        } else {
            (94, None)
        };
        // Leftover encoder CCs would otherwise count as replies.
        let mut quiet_since = Instant::now();
        while quiet_since.elapsed() < Duration::from_millis(500) {
            session.poll()?;
            while client.poll().map_err(std::io::Error::other)?.is_some() {
                quiet_since = Instant::now();
            }
            thread::sleep(Duration::from_millis(5));
        }
        client
            .send(&[0xb0, 127, 127])
            .map_err(std::io::Error::other)?;
        let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
        let mut replies = Vec::with_capacity(expected_count);
        loop {
            session.poll()?;
            while let Some(message) = client.poll().map_err(std::io::Error::other)? {
                if let Some(status) = expected_status {
                    if message[0] != status {
                        continue;
                    }
                    if let Some((_, count)) = l6_remaining
                        .iter_mut()
                        .find(|(mapped, count)| *mapped == message[1] && *count > 0)
                    {
                        *count -= 1;
                        replies.push(message);
                    }
                } else {
                    // MIDI Control adds recorder state as CC 115 to the
                    // existing 94-parameter sync response.
                    if message[1] == 115 {
                        continue;
                    }
                    replies.push(message);
                }
                if replies.len() == expected_count {
                    return Ok(
                        json!({"request":[0xb0,127,127],"expected_replies":expected_count,"reply_count":replies.len(),"replies":replies}),
                    );
                }
            }
            let elapsed = session
                .engine
                .inputs
                .as_ref()
                .unwrap()
                .panel
                .snapshot()
                .guest_ms
                .wrapping_sub(start);
            if elapsed > guest_timeout_ms || Instant::now() >= deadline {
                return Err(std::io::Error::other(format!(
                    "midi-sync returned {}/{} expected Control Change replies before deadline",
                    replies.len(),
                    expected_count
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn run_l6_midi_sync(
        &mut self,
        session: &Session,
        options: &Options,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        const IDLE: u32 = 0x80046d50;
        const HOOK: u32 = 0x8002f490;
        const SENDER: u32 = 0x80031648;
        const EVENT: u32 = 0x841ff000;
        const MIDI_CHANNEL: u32 = 0x804410f0;
        const CC_MAP: u32 = 0x804410f5;
        if session.engine.model() != l6fw::Model::L6 {
            return Err(std::io::Error::other("l6-midi-sync requires L6 firmware").into());
        }
        let socket = options
            .gdb_socket
            .as_ref()
            .ok_or("l6-midi-sync requires a configured GDB socket")?;
        if self.gdb.is_none() {
            self.gdb = Some(l6max_diagnostics::gdb::Gdb::connect(socket)?);
        }
        let gdb = self.gdb.as_mut().unwrap();
        let qmp = &session.engine.management[0];
        let payload = if options.sd_image.is_some() {
            let main = selected_main_image(options).map_err(std::io::Error::other)?;
            let (edits, _, _, _, _, _, _) =
                sd_fixture_edits(options, &main, session.engine.model())
                    .map_err(std::io::Error::other)?;
            let entry_offset = edits
                .iter()
                .find(|(address, _, _)| *address == HOOK)
                .map(|(_, _, offset)| *offset)
                .ok_or("dynamic SD bundle has no L6 midi-sync entry")?;
            let registry = 0x801f_8000;
            let loaded_base =
                u32::from_le_bytes(qmp.read_memory(0, registry + 160, 4)?.try_into().unwrap());
            let slot = sd_dynamic_handler_slot(options, session.engine.model(), registry, HOOK)
                .map_err(std::io::Error::other)?;
            let entry = u32::from_le_bytes(qmp.read_memory(0, slot, 4)?.try_into().unwrap());
            if loaded_base == 0 || entry != (loaded_base + entry_offset | 1) {
                return Err(
                    "allocated L6 midi-sync handler does not match the bundle offset".into(),
                );
            }
            entry & !1
        } else {
            let patch: Value =
                serde_json::from_slice(&fs::read(options.firmware_dir.join("patch.json"))?)?;
            patch["hooks"]
                .as_array()
                .ok_or("patch.json has no hooks")?
                .iter()
                .find(|hook| hook["symbol"] == "midi_in_hook")
                .ok_or("patch.json has no L6 MIDI entry hook")?["target"]
                .as_u64()
                .ok_or("L6 MIDI entry hook target is not numeric")? as u32
                & !1
        };
        gdb.breakpoint(IDLE, true)?;
        gdb.continue_until_stop(Duration::from_secs(60))?;
        let pc = gdb.read_register(15)? & !1;
        if pc != IDLE {
            return Err(std::io::Error::other(format!(
                "expected L6 Idle task at {IDLE:#x}, stopped at {pc:#x}"
            ))
            .into());
        }
        let channel = qmp.read_memory(0, MIDI_CHANNEL, 1)?[0];
        if channel > 15 {
            return Err(std::io::Error::other(format!(
                "invalid configured L6 MIDI channel {channel}"
            ))
            .into());
        }
        let cc_map = qmp.read_memory(0, CC_MAP, 54)?;
        let mut expected_ccs = Vec::new();
        for channel_index in 0..6 {
            for group in 0..9 {
                let cc = cc_map[group * 6 + channel_index];
                if cc != 0xff {
                    expected_ccs.push(cc);
                }
            }
        }
        if expected_ccs.is_empty() {
            return Err(std::io::Error::other("L6 MIDI CC map has no enabled entries").into());
        }
        let mut event = [0u8; 8];
        event[4..7].copy_from_slice(&[0xb0 | channel, 127, 127]);
        gdb.write(EVENT, &event)?;
        gdb.breakpoint(payload, true)?;
        gdb.breakpoint(SENDER, true)?;
        gdb.register(0, EVENT)?;
        gdb.register(14, IDLE | 1)?;
        gdb.register(15, HOOK)?;
        gdb.continue_until_stop(Duration::from_secs(10))?;
        let entry_pc = gdb.read_register(15)? & !1;
        if entry_pc != payload {
            return Err(std::io::Error::other(format!(
                "L6 hook branched to {entry_pc:#x}, expected {payload:#x}"
            ))
            .into());
        }
        let entry_message = qmp.read_memory(0, EVENT, 7)?;
        if entry_message != event[..7] {
            return Err(std::io::Error::other(format!(
                "L6 sync message changed before hook entry: {entry_message:02x?}"
            ))
            .into());
        }
        gdb.breakpoint(payload, false)?;
        gdb.continue_until_stop(Duration::from_secs(10))?;

        let mut replies = Vec::new();
        let mut sender_entries = Vec::new();
        let mut expected_index = 0usize;
        let mut completed = false;
        for _ in 0..expected_ccs.len() + 64 {
            let pc = gdb.read_register(15)? & !1;
            if pc == IDLE {
                return Err(std::io::Error::other(format!(
                    "L6 midi-sync returned after {expected_index}/{} mapped CCs",
                    expected_ccs.len()
                ))
                .into());
            }
            if pc != SENDER {
                return Err(std::io::Error::other(format!(
                    "L6 midi-sync stopped unexpectedly at {pc:#x}"
                ))
                .into());
            }
            let buffer = gdb.read_register(0)?;
            let length = gdb.read_register(1)?;
            let route = gdb.read_register(2)?;
            let return_pc = gdb.read_register(14)? & !1;
            if length != 3 || route != 1 {
                return Err(std::io::Error::other(format!(
                    "L6 queue sender got length {length}, route {route}"
                ))
                .into());
            }
            let bytes = qmp.read_memory(0, buffer, 3)?;
            if bytes[0] != (0xb0 | channel) || bytes[1] == 0xff || bytes[2] > 127 {
                return Err(std::io::Error::other(format!(
                    "invalid mapped L6 Control Change: {bytes:02x?}"
                ))
                .into());
            }
            sender_entries.push(json!({"buffer":buffer,"return_pc":return_pc,"bytes":bytes}));
            replies.push([bytes[0], bytes[1], bytes[2]]);
            if bytes[1] == expected_ccs[expected_index] {
                expected_index += 1;
            }
            if expected_index == expected_ccs.len() {
                gdb.breakpoint(SENDER, false)?;
                gdb.breakpoint(return_pc, true)?;
                gdb.continue_until_stop(Duration::from_secs(10))?;
                let stopped_pc = gdb.read_register(15)? & !1;
                gdb.breakpoint(return_pc, false)?;
                if stopped_pc != return_pc {
                    return Err(std::io::Error::other(format!(
                        "queue sender stopped at {stopped_pc:#x}, expected return {return_pc:#x}"
                    ))
                    .into());
                }
                completed = true;
                break;
            }
            gdb.request("s")?;
            gdb.continue_until_stop(Duration::from_secs(10))?;
        }
        if !completed {
            return Err(std::io::Error::other(format!(
                "L6 midi-sync queued {} replies but matched only {expected_index}/{} enabled CC-map entries",
                replies.len(), expected_ccs.len()
            ))
            .into());
        }
        qmp.resume()?;
        Ok(
            json!({"channel":channel,"route":1,"expected_ccs":expected_ccs,"replies":replies,"sender_entries":sender_entries}),
        )
    }

    fn run_l6_midi_ring_pin_sweep(
        &mut self,
        session: &Session,
        cc: u8,
        wall_timeout_s: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        if session.engine.model() != l6fw::Model::L6 || !(0x53..=0x58).contains(&cc) {
            return Err(std::io::Error::other(
                "L6 ring-pin sweep requires L6 firmware and CC 83 through 88",
            )
            .into());
        }
        let client = self
            .host_midi
            .as_mut()
            .ok_or("L6 ring-pin sweep requires a preceding direct USB MIDI operation")?;
        let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
        client.send(&[0xb0, cc, 0]).map_err(std::io::Error::other)?;
        wait_guest_duration(session, 1000, deadline).map_err(std::io::Error::other)?;
        let expected_ring = usize::from(cc - 0x53);
        let mut changed_segments = 0u32;
        let mut observations = Vec::new();
        for value in [16, 32, 48, 64, 80, 96, 100, 112, 127, 0] {
            let before = session
                .engine
                .inputs
                .as_ref()
                .ok_or("L6 ring-pin sweep needs panel inputs")?
                .panel
                .snapshot()
                .indicators;
            client
                .send(&[0xb0, cc, value])
                .map_err(std::io::Error::other)?;
            wait_guest_duration(session, 500, deadline).map_err(std::io::Error::other)?;
            let after = session
                .engine
                .inputs
                .as_ref()
                .unwrap()
                .panel
                .snapshot()
                .indicators;
            let before_rings = l6max_host::indicators::rings(before);
            let after_rings = l6max_host::indicators::rings(after);
            let changes =
                std::array::from_fn::<_, 8, _>(|ring| before_rings[ring] ^ after_rings[ring]);
            if changes
                .iter()
                .enumerate()
                .any(|(ring, changed)| ring != expected_ring && *changed != 0)
            {
                return Err(std::io::Error::other(format!(
                    "L6 CC {cc} unexpectedly changed decoded ring masks {changes:03x?}"
                ))
                .into());
            }
            changed_segments |= changes[expected_ring];
            observations.push(
                json!({"value":value,"before":before_rings,"after":after_rings,"changed":changes}),
            );
        }
        if changed_segments != 0x1fe {
            return Err(std::io::Error::other(format!(
                "L6 CC {cc} changed ring {} segments {changed_segments:#05x}, expected 0x1fe",
                expected_ring + 1
            ))
            .into());
        }
        Ok(
            json!({"cc":cc,"ring":expected_ring + 1,"changed_segments":changed_segments,"observations":observations}),
        )
    }

    fn run_l6_midi_mute_indicators(
        &mut self,
        session: &Session,
        wall_timeout_s: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        if session.engine.model() != l6fw::Model::L6 {
            return Err(
                std::io::Error::other("L6 mute-indicator probe requires L6 firmware").into(),
            );
        }
        let client = self
            .host_midi
            .as_mut()
            .ok_or("L6 mute-indicator probe requires a preceding direct USB MIDI operation")?;
        let expected = [
            [0, 0, 0, 0, 1 << 11, 0, 0, 0],
            [0, 0, 0, 0, 1 << 10, 0, 0, 0],
            [0, 0, 0, 0, 0, 1 << 11, 0, 0],
            [0, 0, 0, 0, 0, 1 << 10, 0, 0],
            [0, 0, 0, 0, 0, 0, 1 << 11, 0],
            [0, 0, 0, 0, 0, 0, 1 << 10, 0],
        ];
        let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
        let mut observations = Vec::new();
        for (channel, cc) in [0x5d, 0x5e, 0x5f, 0x66, 0x67, 0x68].into_iter().enumerate() {
            let before = session
                .engine
                .inputs
                .as_ref()
                .ok_or("L6 mute-indicator probe needs panel inputs")?
                .panel
                .snapshot()
                .indicators;
            client
                .send(&[0xb0, cc, 127])
                .map_err(std::io::Error::other)?;
            wait_guest_duration(session, 500, deadline).map_err(std::io::Error::other)?;
            let active = session
                .engine
                .inputs
                .as_ref()
                .unwrap()
                .panel
                .snapshot()
                .indicators;
            client.send(&[0xb0, cc, 0]).map_err(std::io::Error::other)?;
            wait_guest_duration(session, 500, deadline).map_err(std::io::Error::other)?;
            let released = session
                .engine
                .inputs
                .as_ref()
                .unwrap()
                .panel
                .snapshot()
                .indicators;
            let on = std::array::from_fn::<_, 8, _>(|row| before[row] ^ active[row]);
            let off = std::array::from_fn::<_, 8, _>(|row| active[row] ^ released[row]);
            if on != expected[channel] || off != expected[channel] {
                return Err(std::io::Error::other(format!(
                    "L6 channel-{} mute CC {cc:#04x} indicator mismatch: on={on:?}, off={off:?}",
                    channel + 1
                ))
                .into());
            }
            observations.push(json!({"channel":channel + 1,"cc":cc,"on":on,"off":off}));
        }
        Ok(json!({"channels":observations}))
    }

    fn execute_inner(
        &mut self,
        session: &Session,
        options: &Options,
        operation: Operation,
        guest_timeout_ms: u32,
        wall_timeout_s: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        match operation {
            Operation::ExpectRecorder => {
                let mode = session.engine.management[0].read_memory(0, 0x80462d9c, 4)?;
                if mode != 0u32.to_le_bytes() {
                    return Err(
                        format!("restart did not restore recorder mode: {mode:02x?}").into(),
                    );
                }
                Ok(json!({"startup_mode":"recorder"}))
            }
            Operation::ExpectMemory { address, bytes } => {
                let actual = session.engine.management[0].read_memory(0, address, bytes.len())?;
                if actual != bytes {
                    return Err(format!(
                        "memory at {address:#010x} is {actual:02x?}, expected {bytes:02x?}"
                    )
                    .into());
                }
                Ok(json!({"address":format!("{address:#010x}"),"bytes":actual}))
            }
            Operation::ExpectSdPatch {
                loaded,
                chain_count,
            } => {
                let main = selected_main_image(options).map_err(std::io::Error::other)?;
                let (edits, link_base, payload, _, _relocations, module_hooks, module_images) =
                    sd_fixture_edits(options, &main, session.engine.model())
                        .map_err(std::io::Error::other)?;
                let qmp = &session.engine.management[0];
                let (image_base, loader_calls): (u32, &[u32]) = match session.engine.model() {
                    l6fw::Model::L6 => (0x8000_1000, &[0x8000_9a60, 0x8000_9aa0]),
                    l6fw::Model::L6max => (
                        0x8000_0000,
                        &[
                            0x8000_fe08,
                            0x8000_e9fa,
                            0x8000_ea24,
                            0x8001_0118,
                            0x8002_bc7e,
                            0x8000_ebac,
                        ],
                    ),
                };
                for &address in loader_calls {
                    let offset = (address - image_base) as usize;
                    let expected = main
                        .get(offset..offset + 4)
                        .ok_or("Patch loader image too short")?;
                    if qmp.read_memory(0, address, 4)? != expected {
                        return Err(
                            format!("persistent loader hook missing at {address:#010x}").into()
                        );
                    }
                }
                for (address, original, _entry_offset) in &edits {
                    let offset = (*address - image_base) as usize;
                    let expected = &main[offset..offset + original.len()];
                    let actual = qmp.read_memory(0, *address, expected.len())?;
                    if actual != expected {
                        return Err(format!(
                            "persistent SD dispatch stub missing at {address:#010x}"
                        )
                        .into());
                    }
                }
                let module_base = match session.engine.model() {
                    l6fw::Model::L6 => 0x801f_0000,
                    l6fw::Model::L6max => 0x8021_0000,
                };
                let registry = module_base + 0x8000;
                let loaded_base =
                    u32::from_le_bytes(qmp.read_memory(0, registry + 160, 4)?.try_into().unwrap());
                let loaded_len =
                    u32::from_le_bytes(qmp.read_memory(0, registry + 164, 4)?.try_into().unwrap());
                if loaded != (loaded_base != 0) || (loaded && loaded_len as usize != payload.len())
                {
                    let stage = u32::from_le_bytes(
                        qmp.read_memory(0, registry + 4, 4)?.try_into().unwrap(),
                    );
                    return Err(format!(
                        "SD allocation state loaded={loaded}, base={loaded_base:#010x}, len={loaded_len}, loader stage={stage:#x}"
                    )
                    .into());
                }
                let claim =
                    u32::from_le_bytes(qmp.read_memory(0, registry + 8, 4)?.try_into().unwrap());
                if (claim == 1) != loaded {
                    let stage = u32::from_le_bytes(
                        qmp.read_memory(0, registry + 4, 4)?.try_into().unwrap(),
                    );
                    return Err(format!(
                        "SD module loaded={loaded}, claim={claim:#x}, loader stage={stage:#x}"
                    )
                    .into());
                }
                let expected_function = if chain_count.is_some() {
                    module_hooks
                        .iter()
                        .find(|hook| {
                            module_hooks
                                .iter()
                                .filter(|other| other.address == hook.address)
                                .count()
                                >= chain_count.unwrap_or(1) as usize
                        })
                        .or_else(|| module_hooks.first())
                        .map(|hook| hook.address)
                } else {
                    module_hooks.first().map(|hook| hook.address)
                };
                let chain_row_size =
                    core::mem::size_of::<patch_loader_core::dynamic::FunctionHookChain>() as u32;
                let mut expected_chains = std::collections::BTreeMap::<u32, u32>::new();
                for module_hook in &module_hooks {
                    *expected_chains.entry(module_hook.address).or_default() += 1;
                }
                for (address, module_count) in expected_chains {
                    let row = (0..patch_loader_core::dynamic::MAX_FUNCTION_CHAINS)
                        .map(|index| registry + 0x100 + index as u32 * chain_row_size)
                        .find(|row| {
                            qmp.read_memory(0, *row, 4).ok().is_some_and(|bytes| {
                                u32::from_le_bytes(bytes.try_into().unwrap()) == address
                            })
                        });
                    let actual_count = row
                        .map(|row| {
                            qmp.read_memory(0, row + 4, 4)
                                .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                        })
                        .transpose()?
                        .unwrap_or(0);
                    let expected_count = if !loaded {
                        0
                    } else if Some(address) == expected_function {
                        chain_count.map_or(module_count, u32::from)
                    } else {
                        module_count
                    };
                    if actual_count != expected_count {
                        return Err(format!(
                            "SD hook site {address:#010x} has {actual_count} registered modules, expected {expected_count}"
                        )
                        .into());
                    }
                    if loaded {
                        let row = row.ok_or("registered hook site has no chain row")?;
                        let chain_head =
                            u32::from_le_bytes(qmp.read_memory(0, row + 8, 4)?.try_into().unwrap());
                        let slot = sd_dynamic_handler_slot(
                            options,
                            session.engine.model(),
                            registry,
                            address,
                        )
                        .map_err(std::io::Error::other)?;
                        let published_head =
                            u32::from_le_bytes(qmp.read_memory(0, slot, 4)?.try_into().unwrap());
                        if published_head != chain_head {
                            return Err(format!(
                                "dispatch slot for {address:#010x} points to {published_head:#010x}, chain head is {chain_head:#010x}"
                            )
                            .into());
                        }
                    }
                }
                let chain_address = if let Some(address) = expected_function {
                    (0..patch_loader_core::dynamic::MAX_FUNCTION_CHAINS)
                        .map(|index| registry + 0x100 + index as u32 * chain_row_size)
                        .find(|row| {
                            qmp.read_memory(0, *row, 4).ok().is_some_and(|bytes| {
                                u32::from_le_bytes(bytes.try_into().unwrap()) == address
                            })
                        })
                        .unwrap_or(registry + 0x100)
                } else {
                    registry + 0x100
                };
                let registered_function =
                    u32::from_le_bytes(qmp.read_memory(0, chain_address, 4)?.try_into().unwrap());
                let registered_count = u32::from_le_bytes(
                    qmp.read_memory(0, chain_address + 4, 4)?
                        .try_into()
                        .unwrap(),
                );
                if registered_count > patch_loader_core::dynamic::MAX_HOOKS_PER_FUNCTION as u32
                    || loaded != (registered_count != 0)
                    || (registered_count != 0 && Some(registered_function) != expected_function)
                    || chain_count.is_some_and(|expected| registered_count != u32::from(expected))
                {
                    return Err(format!(
                        "SD hook chain state loaded={loaded}, function={registered_function:#010x}, count={registered_count}, expected function={expected_function:?}, expected count={chain_count:?}"
                    )
                    .into());
                }
                let chain_head = if registered_count == 0 {
                    0
                } else {
                    u32::from_le_bytes(
                        qmp.read_memory(0, chain_address + 8, 4)?
                            .try_into()
                            .unwrap(),
                    )
                };
                let published_head = if let Some(address) = expected_function {
                    let slot =
                        sd_dynamic_handler_slot(options, session.engine.model(), registry, address)
                            .map_err(std::io::Error::other)?;
                    u32::from_le_bytes(qmp.read_memory(0, slot, 4)?.try_into().unwrap())
                } else {
                    0
                };
                if published_head != chain_head {
                    return Err(format!(
                        "dynamic dispatch head {published_head:#010x} differs from hook-array head {chain_head:#010x}"
                    )
                    .into());
                }
                let mut chain_links = Vec::with_capacity(registered_count as usize);
                let mut prior_order = None;
                for index in 0..registered_count as usize {
                    let link = chain_address + 8 + index as u32 * 16;
                    let mut bytes = [0; 16];
                    bytes.copy_from_slice(&qmp.read_memory(0, link, 16)?);
                    let entry = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                    let next_slot = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                    let order = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
                    let module = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
                    if entry & 1 == 0
                        || module == 0
                        || prior_order.is_some_and(|prior| order <= prior)
                    {
                        return Err(format!(
                            "invalid SD hook link {index}: entry={entry:#010x}, module={module:#010x}, order={order}"
                        )
                        .into());
                    }
                    prior_order = Some(order);
                    chain_links.push((entry, next_slot, order, module));
                }
                for (address, _, _entry_offset) in &edits {
                    let slot = if module_hooks.iter().any(|hook| hook.address == *address) {
                        sd_dynamic_handler_slot(options, session.engine.model(), registry, *address)
                            .map_err(std::io::Error::other)?
                    } else if *address == 0x8003_e8ee {
                        registry + 16
                    } else {
                        let index = match *address {
                            0x8000_6110 => 0,
                            0x8000_64c0 => 1,
                            0x8000_6660 => 2,
                            0x8000_63b8 => 3,
                            0x8000_6370 => 4,
                            0x8000_68a0 => 5,
                            0x8000_62d0 => 6,
                            0x8000_6070 => 7,
                            0x8000_6348 => 8,
                            0x8000_65f8 => 9,
                            0x8000_6560 => 10,
                            0x8000_68f8 => 11,
                            0x8000_69b0 => 12,
                            0x8000_6bd0 => 13,
                            0x8000_7080 => 14,
                            0x8000_70b0 => 15,
                            0x8000_7090 => 16,
                            0x8000_6fa8 => 17,
                            0x8006_1ab0 => 18,
                            0x8000_6d50 => 19,
                            0x8000_6d00 => 20,
                            0x8006_1738 => 21,
                            0x8000_5ee0 => 22,
                            0x8002_4eb8 => 23,
                            0x8008_bd40 => 24,
                            0x8000_6060 => 25,
                            0x8000_5e70 => 26,
                            0x8003_6c74 => 27,
                            0x8000_648e => 28,
                            0x8000_6126 => 29,
                            0x8000_62c0 => 30,
                            _ => {
                                return Err(
                                    format!("no SD dispatch slot for {address:#010x}").into()
                                );
                            }
                        };
                        registry + 32 + index * 4
                    };
                    let handler =
                        u32::from_le_bytes(qmp.read_memory(0, slot, 4)?.try_into().unwrap());
                    let valid = if !loaded {
                        handler == 0
                    } else if Some(*address) == expected_function {
                        handler == chain_head
                    } else {
                        handler != 0 && handler & 1 != 0
                    };
                    if !valid {
                        return Err(format!(
                            "SD handler at {slot:#010x} is invalid: {handler:#010x}"
                        )
                        .into());
                    }
                }
                if loaded {
                    let hook = module_hooks
                        .iter()
                        .find(|hook| Some(hook.address) == expected_function)
                        .ok_or("dynamic patch bundle has no hook record")?;
                    let root = sd_fixture_root(options).map_err(std::io::Error::other)?;
                    let ordered_chain = root.join("sd-chain-first.BIN").is_file();
                    let patch_report: Value = serde_json::from_slice(&fs::read(
                        root.join("sd-patches-build/patch.json"),
                    )?)?;
                    let pass_through_entry_offset = if ordered_chain {
                        Some(
                            patch_report["patches"][0]["pass_through_entry_offsets"][0]
                                .as_u64()
                                .ok_or("ordered-chain module has no pass-through entry")?
                                as u32,
                        )
                    } else {
                        None
                    };
                    let loader_report: Value = serde_json::from_slice(&fs::read(
                        root.join("patch-loader-firmware/patch.json"),
                    )?)?;
                    let _loader_patch = loader_report["patches"]
                        .as_array()
                        .and_then(|patches| {
                            patches.iter().find(|patch| patch["name"] == "patch-loader")
                        })
                        .ok_or("loader report has no patch-loader record")?;
                    let dispatch_site = loader_report["patches"]
                        .as_array()
                        .and_then(|patches| {
                            patches
                                .iter()
                                .find(|patch| patch["name"] == "dynamic-dispatch")
                        })
                        .and_then(|patch| patch["sites"].as_array())
                        .and_then(|sites| {
                            sites.iter().find(|site| {
                                site["address"].as_u64() == Some(u64::from(hook.address))
                            })
                        })
                        .ok_or("loader report has no dispatcher for the registered function")?;
                    let fallback = u32::try_from(
                        dispatch_site["fallback"]
                            .as_u64()
                            .ok_or("loader report has no function continuation")?,
                    )? | 1;
                    if chain_count == Some(2) {
                        let first = chain_links.first().ok_or("empty expected hook chain")?;
                        let second = chain_links.get(1).ok_or("short expected hook chain")?;
                        if first.2 >= second.2 {
                            return Err(
                                "filename priority was not used to order shared hooks".into()
                            );
                        }
                    }
                    for (index, (entry, next_slot, _order, module)) in
                        chain_links.iter().enumerate()
                    {
                        let (module_payload, module_relocations, module_payload_hooks) =
                            module_images
                                .iter()
                                .find_map(|(payload, relocations, hooks)| {
                                    hooks
                                        .iter()
                                        .find(|candidate| {
                                            candidate.address == hook.address
                                                && (*entry
                                                    == ((*module + candidate.entry_offset) | 1)
                                                    || ordered_chain
                                                        && index == 0
                                                        && *entry
                                                            == ((*module
                                                                + pass_through_entry_offset
                                                                    .unwrap_or(u32::MAX))
                                                                | 1))
                                                && *next_slot
                                                    == *module + candidate.next_slot_offset
                                        })
                                        .map(|_| (payload, relocations, hooks))
                                })
                                .ok_or_else(|| {
                                    format!(
                                        "hook {index} does not match any module's hook metadata"
                                    )
                                })?;
                        let expected_next =
                            chain_links.get(index + 1).map_or(fallback, |next| next.0);
                        let actual_next = u32::from_le_bytes(
                            qmp.read_memory(0, *next_slot, 4)?.try_into().unwrap(),
                        );
                        if actual_next != expected_next {
                            return Err(format!(
                                "hook {index} continues to {actual_next:#010x}, expected {expected_next:#010x}"
                            )
                            .into());
                        }
                        let mut expected = module_payload.clone();
                        patch_loader_core::bundle::relocate(
                            &mut expected,
                            module_relocations.clone().into_iter(),
                            link_base,
                            *module,
                        )
                        .map_err(|error| format!("invalid fixture relocations: {error:?}"))?;
                        for payload_hook in module_payload_hooks {
                            if payload_hook.next_slot_offset == u32::MAX {
                                continue;
                            }
                            let slot = *module + payload_hook.next_slot_offset;
                            let target = qmp.read_memory(0, slot, 4)?;
                            let slot_offset = payload_hook.next_slot_offset as usize;
                            expected[slot_offset..slot_offset + 4].copy_from_slice(&target);
                        }
                        for (chunk_index, chunk) in expected.chunks(4096).enumerate() {
                            let address = *module + (chunk_index * 4096) as u32;
                            if qmp.read_memory(0, address, chunk.len())? != chunk {
                                return Err(format!(
                                    "allocated SD module {index} does not match its relocated bundle"
                                )
                                .into());
                            }
                        }
                        if (*entry & !1) < *module {
                            return Err(format!("hook {index} entry precedes its module").into());
                        }
                    }
                    if let Some((latest_payload, latest_relocations, latest_hooks)) =
                        module_images.last()
                    {
                        let mut expected = latest_payload.clone();
                        patch_loader_core::bundle::relocate(
                            &mut expected,
                            latest_relocations.clone().into_iter(),
                            link_base,
                            loaded_base,
                        )
                        .map_err(|error| format!("invalid latest module relocations: {error:?}"))?;
                        for latest_hook in latest_hooks {
                            if latest_hook.next_slot_offset == u32::MAX {
                                continue;
                            }
                            let slot = loaded_base + latest_hook.next_slot_offset;
                            let target = qmp.read_memory(0, slot, 4)?;
                            let offset = latest_hook.next_slot_offset as usize;
                            expected[offset..offset + 4].copy_from_slice(&target);
                        }
                        for (chunk_index, chunk) in expected.chunks(4096).enumerate() {
                            let address = loaded_base + (chunk_index * 4096) as u32;
                            if qmp.read_memory(0, address, chunk.len())? != chunk {
                                return Err(
                                    "latest allocated SD module does not match its relocated bundle"
                                        .into(),
                                );
                            }
                        }
                    }
                }
                Ok(
                    json!({"loaded":loaded,"guarded_hooks":edits.len(),"module_bytes":if loaded { payload.len() } else { 0 },"hook_chain_count":registered_count,"hook_chain_orders":chain_links.iter().map(|link|link.2).collect::<Vec<_>>()}),
                )
            }
            Operation::ExpectHeap { minimum_free } => {
                let heap =
                    l6max_host::heap::read(&session.engine.management[0], session.engine.model())?;
                if heap.minimum_ever_free < minimum_free {
                    return Err(format!(
                        "heap fell to {} free bytes, below the required {minimum_free}: {heap:?}",
                        heap.minimum_ever_free
                    )
                    .into());
                }
                Ok(json!({"heap":heap,"peak_used":heap.peak_used()}))
            }
            Operation::HoldChord {
                targets,
                duration_ms,
            } => {
                let inputs = session.engine.inputs.as_ref().unwrap();
                let start = inputs.panel.snapshot().guest_ms;
                let hold = duration_ms.saturating_add(1000).min(60_000);
                for target in &targets {
                    inputs.down(*target, hold)?;
                }
                let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
                loop {
                    session.poll()?;
                    let main = inputs.main.snapshot();
                    let panel = inputs.panel.snapshot();
                    if main.powered_off || panel.powered_off {
                        return Err("guest powered off during held chord".into());
                    }
                    let elapsed = panel.guest_ms.wrapping_sub(start);
                    if main.applied >= main.sent
                        && panel.applied >= panel.sent
                        && elapsed >= duration_ms
                    {
                        break;
                    }
                    if Instant::now() > deadline || elapsed > guest_timeout_ms {
                        return Err("held chord was not applied within its deadline".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                for target in targets.iter().rev() {
                    inputs.up(*target)?;
                }
                loop {
                    session.poll()?;
                    let main = inputs.main.snapshot();
                    let panel = inputs.panel.snapshot();
                    if main.applied >= main.sent && panel.applied >= panel.sent {
                        break;
                    }
                    if Instant::now() > deadline {
                        return Err("held chord release was not acknowledged".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(
                    json!({"targets":targets,"held_guest_ms":inputs.panel.snapshot().guest_ms.wrapping_sub(start)}),
                )
            }
            Operation::PanelEvent {
                target,
                column,
                row,
                event,
            } => {
                const DISPATCHER: u32 = 0x800393d0;
                const EVENT_SUBMIT: u32 = 0x80020ca0;
                const REPORT: u32 = 0x808cb54d;
                let inputs = &session.engine.inputs.as_ref().unwrap().panel;
                let qmp = &session.engine.management[0];
                let socket = options
                    .gdb_socket
                    .as_ref()
                    .ok_or("panel-event requires a configured GDB socket")?;
                if self.gdb.is_none() {
                    self.gdb = Some(l6max_diagnostics::gdb::Gdb::connect(socket)?);
                }
                let gdb = self.gdb.as_mut().unwrap();
                qmp.stop()?;
                gdb.breakpoint(DISPATCHER, true)?;
                gdb.breakpoint(EVENT_SUBMIT, true)?;
                inputs.down(target, 0)?;
                gdb.continue_until_stop(Duration::from_secs(15))?;
                let pc = gdb.read_register(15)? & !1;
                if pc != DISPATCHER {
                    return Err(format!(
                        "panel target {target} stopped at {pc:#x}, not dispatcher"
                    )
                    .into());
                }
                gdb.breakpoint(DISPATCHER, false)?;
                gdb.continue_until_stop(Duration::from_secs(15))?;
                let pc = gdb.read_register(15)? & !1;
                if pc != EVENT_SUBMIT {
                    return Err(
                        format!("panel target {target} reached {pc:#x}, not event submit").into(),
                    );
                }
                let response = gdb.request(&format!("m{REPORT:x},3"))?;
                if response.len() != 6 {
                    return Err(format!("unexpected panel report bytes: {response}").into());
                }
                let bytes = (0..3)
                    .map(|index| u8::from_str_radix(&response[index * 2..index * 2 + 2], 16))
                    .collect::<Result<Vec<_>, _>>()?;
                if !matches!(bytes[0], 0x90 | 0x91) || (bytes[1], bytes[2]) != (column, row) {
                    return Err(format!(
                        "panel target {target} report {bytes:02x?}, expected ({column},{row})"
                    )
                    .into());
                }
                let state = gdb.read_register(1)?;
                let observed_event = gdb.read_register(2)?;
                if state != 0 || observed_event != u32::from(event) {
                    return Err(format!("panel target {target} submitted state {state} event {observed_event}, expected press-state 0 event {event}").into());
                }
                let entry = if column == 0 {
                    0x800a1834 + u32::from(row) * 0x14
                } else {
                    0x800a15b4 + u32::from(column) * 0xa0 + u32::from(row) * 0x14
                };
                let mapping = qmp.read_memory(0, entry + 8, 4)?;
                let mapped_event = u32::from_le_bytes(mapping.try_into().unwrap());
                if mapped_event != u32::from(event) {
                    return Err(format!(
                        "MAIN map ({column},{row}) is {mapped_event}, expected {event}"
                    )
                    .into());
                }
                gdb.breakpoint(EVENT_SUBMIT, false)?;
                qmp.resume()?;
                inputs.up(target)?;
                let started = inputs.snapshot().guest_ms;
                let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
                loop {
                    session.poll()?;
                    if inputs.snapshot().guest_ms.wrapping_sub(started) >= 250 {
                        break;
                    }
                    if Instant::now() > deadline {
                        return Err("panel event release timed out".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(json!({"target":target,"column":column,"row":row,"event":event,"report":bytes}))
            }
            Operation::AudioInputTransport {
                command,
                position_ms,
            } => {
                use l6max_host::protocol;
                let target = match command.as_str() {
                    "stop" => protocol::AUDIO_INPUT_STOP,
                    "play" => protocol::AUDIO_INPUT_PLAY,
                    "pause" => protocol::AUDIO_INPUT_PAUSE,
                    "seek" => protocol::AUDIO_INPUT_SEEK,
                    other => return Err(format!("unknown audio input command {other}").into()),
                };
                let main = &session.engine.inputs.as_ref().unwrap().main;
                let sequence = main.audio_input_transport(target, position_ms)?;
                Ok(json!({"command": command, "position_ms": position_ms, "sequence": sequence}))
            }
            Operation::AudioInputLoad { channel, loaded } => {
                let main = &session.engine.inputs.as_ref().unwrap().main;
                let before = main.snapshot().rejected;
                let sequence = main.audio_input_load(channel - 1, loaded)?;
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_secs(wall_timeout_s);
                while main
                    .snapshot()
                    .last_ack
                    .is_none_or(|ack| ack.sequence != sequence)
                {
                    if std::time::Instant::now() >= deadline {
                        return Err("audio input load was not acknowledged".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                if main.snapshot().rejected != before {
                    return Err(format!("channel {channel} has no prepared audio").into());
                }
                Ok(json!({"channel": channel, "loaded": loaded}))
            }
            Operation::ExpectAudioInput {
                playing,
                min_ms,
                max_ms,
            } => {
                let status = session.engine.inputs.as_ref().unwrap().main.snapshot();
                let evidence = json!({
                    "playing": status.audio_input_playing,
                    "position_ms": status.audio_input_position_ms,
                });
                if status.audio_input_playing != playing
                    || !(min_ms..=max_ms).contains(&status.audio_input_position_ms)
                {
                    return Err(format!(
                        "audio input state {evidence} is not playing={playing}, {min_ms}..={max_ms} ms"
                    )
                    .into());
                }
                Ok(evidence)
            }
            Operation::ExpectTone {
                file,
                hz,
                levels,
                tolerance_db,
            } => {
                const RATE: usize = 48000;
                let frames = match &file {
                    None => {
                        let path = std::env::var_os("L6_AUDIO_RAW_CAPTURE")
                            .ok_or("expect-output-tone requires L6_AUDIO_RAW_CAPTURE")?;
                        let frames = waveform::decode_capture(&fs::read(path)?);
                        frames[frames.len().saturating_sub(RATE)..].to_vec()
                    }
                    Some(file) => {
                        let card = options
                            .sd_image
                            .as_ref()
                            .ok_or("expect-recorded-tone requires an SD card")?;
                        let mut volume = sd_image::open(card)?;
                        let project = volume
                            .entries("RECORDER")?
                            .into_iter()
                            .filter(|entry| entry.directory && !entry.name.starts_with('.'))
                            .map(|entry| entry.name)
                            .max()
                            .ok_or("the card has no recorder project")?;
                        let bytes = volume.read(&format!("RECORDER/{project}/{file}"))?;
                        let frames = waveform::decode_wav(&bytes)?;
                        frames[RATE.min(frames.len())..].to_vec()
                    }
                };
                if frames.len() < RATE {
                    return Err(format!("only {} frames to measure", frames.len()).into());
                }
                let measured = waveform::tone_levels_db(&frames[..RATE], hz, RATE as f64);
                let evidence =
                    json!({"file": file, "hz": hz, "measured_db": measured, "expected_db": levels});
                for (side, (measured, expected)) in
                    ["left", "right"].iter().zip(measured.iter().zip(levels))
                {
                    let matches = match expected {
                        Some(expected) => (measured - expected).abs() <= tolerance_db,
                        None => *measured < -70.,
                    };
                    if !matches {
                        return Err(format!("{side} tone mismatch: {evidence}").into());
                    }
                }
                Ok(evidence)
            }
            Operation::ExpectCardEntry { path } => {
                let card = options
                    .sd_image
                    .as_ref()
                    .ok_or("expect-card-entry requires an SD card")?;
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_secs(wall_timeout_s);
                loop {
                    if sd_image::entry_exists(card, &path)? {
                        return Ok(json!({ "card_entry": path }));
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(format!("{path} did not appear on the SD card").into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
            Operation::L6maxTapHandler => {
                const TAP_TARGET: u32 = 48;
                const TAP_PRESS_HANDLER: u32 = 0x80055468;
                let inputs = &session.engine.inputs.as_ref().unwrap().main;
                let qmp = &session.engine.management[0];
                let socket = options
                    .gdb_socket
                    .as_ref()
                    .ok_or("l6max-tap-handler requires a configured GDB socket")?;
                if self.gdb.is_none() {
                    self.gdb = Some(l6max_diagnostics::gdb::Gdb::connect(socket)?);
                }
                let gdb = self.gdb.as_mut().unwrap();
                qmp.stop()?;
                gdb.breakpoint(TAP_PRESS_HANDLER, true)?;
                let applied = inputs.snapshot().applied;
                inputs.down(TAP_TARGET, 0)?;
                wait_l6_input(
                    session,
                    || inputs.snapshot().applied > applied,
                    "TAP press acknowledgement",
                )?;
                let stopped = gdb.continue_until_stop(Duration::from_secs(15));
                if let Err(error) = stopped {
                    let _ = gdb.interrupt();
                    let _ = gdb.breakpoint(TAP_PRESS_HANDLER, false);
                    let _ = qmp.resume();
                    return Err(format!("TAP press did not reach its handler: {error}").into());
                }
                let pc = gdb.read_register(15)? & !1;
                gdb.breakpoint(TAP_PRESS_HANDLER, false)?;
                qmp.resume()?;
                if pc != TAP_PRESS_HANDLER {
                    return Err(format!(
                        "TAP press stopped at {pc:#010x}, expected {TAP_PRESS_HANDLER:#010x}"
                    )
                    .into());
                }
                let applied = inputs.snapshot().applied;
                inputs.up(TAP_TARGET)?;
                wait_l6_input(
                    session,
                    || inputs.snapshot().applied > applied,
                    "TAP release acknowledgement",
                )?;
                Ok(json!({"target":TAP_TARGET,"handler":TAP_PRESS_HANDLER}))
            }
            Operation::L6MainGpioEvent { target, event } => {
                const EVENT_SUBMIT: u32 = 0x80020ca0;
                const SOUND_PAD_EVENT_HANDLER: u32 = 0x800408b0;
                let inputs = &session.engine.inputs.as_ref().unwrap().main;
                let qmp = &session.engine.management[0];
                let socket = options
                    .gdb_socket
                    .as_ref()
                    .ok_or("l6-main-gpio-event requires a configured GDB socket")?;
                if self.gdb.is_none() {
                    self.gdb = Some(l6max_diagnostics::gdb::Gdb::connect(socket)?);
                }
                let gdb = self.gdb.as_mut().unwrap();
                qmp.stop()?;
                gdb.breakpoint(EVENT_SUBMIT, true)?;
                let applied = inputs.snapshot().applied;
                inputs.down(target, 0)?;
                wait_l6_input(
                    session,
                    || inputs.snapshot().applied > applied,
                    "MAIN GPIO press acknowledgement",
                )?;
                gdb.continue_until_stop(Duration::from_secs(15))?;
                let pc = gdb.read_register(15)? & !1;
                if pc != EVENT_SUBMIT {
                    return Err(format!(
                        "L6 MAIN target {target} stopped at {pc:#010x}, expected event submit {EVENT_SUBMIT:#010x}"
                    )
                    .into());
                }
                let args = (0..4)
                    .map(|register| gdb.read_register(register))
                    .collect::<Result<Vec<_>, _>>()?;
                if args[2] != u32::from(event) {
                    return Err(format!(
                        "L6 MAIN target {target} submitted event {:#04x}, expected {event:#04x}",
                        args[2]
                    )
                    .into());
                }
                let dispatch_arguments = if (0x15..=0x18).contains(&event) {
                    gdb.breakpoint(EVENT_SUBMIT, false)?;
                    gdb.breakpoint(SOUND_PAD_EVENT_HANDLER, true)?;
                    match gdb.continue_until_stop(Duration::from_secs(15)) {
                        Ok(_) => {
                            let dispatch_pc = gdb.read_register(15)? & !1;
                            if dispatch_pc != SOUND_PAD_EVENT_HANDLER {
                                return Err(format!(
                                    "L6 MAIN target {target} stopped at {dispatch_pc:#010x}, expected sound-pad event handler {SOUND_PAD_EVENT_HANDLER:#010x}"
                                )
                                .into());
                            }
                            let dispatch_args = (0..3)
                                .map(|register| gdb.read_register(register))
                                .collect::<Result<Vec<_>, _>>()?;
                            gdb.breakpoint(SOUND_PAD_EVENT_HANDLER, false)?;
                            Some(dispatch_args)
                        }
                        Err(error) => {
                            let _ = gdb.interrupt();
                            let _ = gdb.breakpoint(SOUND_PAD_EVENT_HANDLER, false);
                            let _ = qmp.resume();
                            return Err(format!(
                                "L6 MAIN target {target} submitted event {event:#04x} but did not reach the sound-pad event handler: {error}"
                            )
                            .into());
                        }
                    }
                } else {
                    gdb.breakpoint(EVENT_SUBMIT, false)?;
                    None
                };
                qmp.resume()?;
                let applied = inputs.snapshot().applied;
                inputs.up(target)?;
                wait_l6_input(
                    session,
                    || inputs.snapshot().applied > applied,
                    "MAIN GPIO release acknowledgement",
                )?;
                wait_guest_duration(
                    session,
                    100,
                    Instant::now() + Duration::from_secs(wall_timeout_s),
                )?;
                Ok(
                    json!({"target":target,"event":event,"arguments":args,"dispatch_arguments":dispatch_arguments}),
                )
            }
            Operation::L6UsbStartup => {
                Ok(
                    run_l6_usb_startup(session, options, guest_timeout_ms, wall_timeout_s)
                        .map_err(std::io::Error::other)?,
                )
            }
            Operation::L6DinEventDispatch { group, cc, value } => {
                run_l6_din_event_dispatch(session, options, group, cc, value)
            }
            Operation::L6InputSweep => {
                Ok(run_l6_input_sweep(session, options).map_err(std::io::Error::other)?)
            }
            Operation::L6DinSweep => {
                Ok(run_l6_din_sweep(session, options).map_err(std::io::Error::other)?)
            }
            Operation::L6maxInputTiming => {
                Ok(run_l6max_input_timing(session, options).map_err(std::io::Error::other)?)
            }
            Operation::L6maxPanelKeyMap => {
                Ok(run_l6max_panel_key_map(session, wall_timeout_s)
                    .map_err(std::io::Error::other)?)
            }
            Operation::L6maxChannelSignalPwm => Ok(run_l6max_channel_signal_pwm(
                session,
                options,
                guest_timeout_ms,
                wall_timeout_s,
            )
            .map_err(std::io::Error::other)?),
            Operation::L6LevelMeterPwm => {
                Ok(
                    run_l6_level_meter_pwm(session, options, guest_timeout_ms, wall_timeout_s)
                        .map_err(std::io::Error::other)?,
                )
            }
            Operation::Restart => Err("restart is supervised by the session runner".into()),
            Operation::WarmReset => {
                session.engine.management[0].system_reset()?;
                Ok(json!({"reset":"qmp-system-reset"}))
            }
            Operation::PowerPress { hold_ms, powered } => {
                let inputs = session.engine.inputs.as_ref().unwrap();
                let started = inputs.panel.snapshot().guest_ms;
                if powered {
                    inputs.tap(53, hold_ms)?;
                } else {
                    inputs.main.down(53, hold_ms)?;
                }
                let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
                loop {
                    session.poll()?;
                    let off = inputs.main.snapshot().powered_off;
                    let elapsed = inputs.panel.snapshot().guest_ms.wrapping_sub(started);
                    if powered && off {
                        return Err("short power press cut power".into());
                    }
                    if !powered && off {
                        if session.engine.management[0].query_status()?.status != "shutdown" {
                            return Err("power-off did not shut down QEMU".into());
                        }
                        if session.engine.model() == l6fw::Model::L6
                            && !(400..=800).contains(&elapsed)
                        {
                            return Err(format!("L6 cutoff took {elapsed} guest ms").into());
                        }
                        thread::sleep(Duration::from_millis(100));
                        let stopped = inputs.panel.snapshot().guest_ms;
                        thread::sleep(Duration::from_millis(200));
                        if inputs.panel.snapshot().guest_ms != stopped {
                            return Err("guest clock advanced while powered off".into());
                        }
                        return Ok(
                            json!({"powered":false,"held_guest_ms":elapsed,"clock_stopped":true}),
                        );
                    }
                    if powered && elapsed >= hold_ms + 1000 {
                        return Ok(json!({"powered":true,"held_guest_ms":hold_ms}));
                    }
                    if Instant::now() > deadline || elapsed > guest_timeout_ms {
                        return Err("power transition timed out".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            Operation::ExpectSettings { restored } => {
                let state = options
                    .state_dir
                    .as_ref()
                    .ok_or("settings assertion requires persistent state")?;
                let saved = fs::read(state.join("panel-rtc.bin"))?;
                let panel = fs::read(state.join("panel-flash.bin"))?;
                let nor = fs::read(state.join("main-nor.bin"))?;
                if session.engine.model() != l6fw::Model::L6max {
                    return Err("settings layout currently supports L6max 1.10 only".into());
                }
                if panel.get(0x3ffc..0x4000) != Some(b"0100")
                    || nor.get(0x1f7ffc..0x1f8000) != Some(b"0110")
                    || nor.get(0x1f6ffc..0x1f7000) != Some(b"0100")
                {
                    return Err("persisted firmware version records do not match".into());
                }
                let time = u32::from_le_bytes(saved.get(..4).ok_or("short RTC file")?.try_into()?);
                let calendar =
                    u32::from_le_bytes(saved.get(4..8).ok_or("short RTC file")?.try_into()?);
                if restored {
                    let (before, bank) = self
                        .saved
                        .as_ref()
                        .ok_or("restore assertion needs saved-settings checkpoint")?;
                    if before[4..8] != saved[4..8]
                        || u32::from_le_bytes(before[..4].try_into()?) & 0x003f7f00
                            != time & 0x003f7f00
                    {
                        return Err("RTC fields changed across restart".into());
                    }
                    let mut trace = fs::read_to_string(options.logs.join("engine.trace"))?;
                    if let Ok(panel) = fs::read_to_string(options.logs.join("engine-panel.trace")) {
                        trace.push_str(&panel);
                    }
                    if trace.contains("l6 panel ROM write") {
                        return Err("panel reprogrammed on restart".into());
                    }
                    if !trace.contains(&format!("addr={:08x} ipcr1=000017e0", bank + 4)) {
                        return Err("settings bank was not reloaded".into());
                    }
                    let timestamp = |prefix: &str| -> Option<u32> {
                        trace.lines().find_map(|line| {
                            line.strip_prefix(prefix)?
                                .split_whitespace()
                                .next()?
                                .parse()
                                .ok()
                        })
                    };
                    let request =
                        timestamp("l6 main RTC request at ").ok_or("missing RTC request")?;
                    let response = timestamp("l6 panel RTC response start at ")
                        .ok_or("missing RTC response")?;
                    if response < request || response - request >= 2000 {
                        return Err("RTC response missed startup deadline".into());
                    }
                } else {
                    if calendar & 0x00ff1f3f != 0x00260202 || time & 0x003f7f00 != 0x00010100 {
                        return Err("date editor did not save all five fields".into());
                    }
                    let bank = [0x1f9000usize, 0x1fc000]
                        .into_iter()
                        .find(|&base| {
                            nor.get(base..base + 4).is_some_and(|b| {
                                (1..=3).contains(&u32::from_le_bytes(b.try_into().unwrap()))
                            })
                        })
                        .ok_or("settings bank not committed")?;
                    self.saved = Some((saved.clone(), bank));
                }
                Ok(json!({"restored":restored,"rtc":saved}))
            }
            Operation::SdCard { inserted } => {
                if inserted {
                    session.engine.management[0].insert_sd(
                        options
                            .sd_image
                            .as_ref()
                            .ok_or("SD insertion requires a fixture card")?,
                    )?;
                } else {
                    session.engine.management[0].eject_sd()?;
                }
                Ok(json!({"inserted":inserted}))
            }
            Operation::UsbDescriptors => {
                let usb = session
                    .engine
                    .usb
                    .as_ref()
                    .ok_or("USB backend unavailable")?;
                wait_usb_controller(session, usb, guest_timeout_ms, wall_timeout_s)?;
                let (descriptor, config, configuration) = usb.enumerate_with_descriptors()?;
                self.midi = configuration.midi;
                fs::write(options.logs.join("usb-device.bin"), &descriptor)?;
                fs::write(options.logs.join("usb-config.bin"), &config)?;
                Ok(
                    json!({"vid":u16::from_le_bytes([descriptor[8],descriptor[9]]),"pid":u16::from_le_bytes([descriptor[10],descriptor[11]]),"configuration_bytes":config.len(),"interfaces":config[4]}),
                )
            }
            Operation::UsbMidi { value } => {
                let usb = session
                    .engine
                    .usb
                    .as_ref()
                    .ok_or("USB backend unavailable")?;
                let endpoints = self
                    .midi
                    .ok_or("usb-midi needs a preceding USB MIDI enumeration")?;
                let cc = if session.engine.model() == l6fw::Model::L6 {
                    0x53
                } else {
                    0x51
                };
                let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
                let start = session
                    .engine
                    .inputs
                    .as_ref()
                    .unwrap()
                    .panel
                    .snapshot()
                    .guest_ms;
                let mut midi = Vec::new();
                loop {
                    session.poll()?;
                    match usb.try_receive(endpoints.input, 16384) {
                        Ok(bytes) => {
                            // The preceding encoder step's channel-1 CC.
                            let matching = l6max_host::usb_midi::decode(&bytes)?
                                .iter()
                                .any(|(_, msg)| msg.starts_with(&[0xb0, cc]));
                            midi.extend_from_slice(&bytes);
                            fs::write(options.logs.join("usb-midi.bin"), &midi)?;
                            if matching {
                                break;
                            }
                        }
                        Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => {}
                        Err(e) => return Err(e.into()),
                    }
                    if Instant::now() > deadline
                        || session
                            .engine
                            .inputs
                            .as_ref()
                            .unwrap()
                            .panel
                            .snapshot()
                            .guest_ms
                            .wrapping_sub(start)
                            > guest_timeout_ms
                    {
                        return Err("USB MIDI receive deadline exceeded".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                fs::write(options.logs.join("usb-midi.bin"), &midi)?;
                usb.send(endpoints.output, &[0x1b, 0xb0, cc, value])?;
                Ok(json!({"received":midi,"sent":[0x1b,0xb0,cc,value]}))
            }
            Operation::HostMidiRoundTrip => {
                self.run_host_midi_round_trip(session, guest_timeout_ms, wall_timeout_s, None, true)
            }
            Operation::DirectUsbMidiRoundTrip {
                cable,
                expect_ring_change,
            } => self.run_host_midi_round_trip(
                session,
                guest_timeout_ms,
                wall_timeout_s,
                Some(cable),
                expect_ring_change,
            ),
            Operation::L6maxDinMixerControl {
                expect_din_ring_change,
            } => self.run_l6max_din_mixer_control(
                session,
                expect_din_ring_change,
                guest_timeout_ms,
                wall_timeout_s,
            ),
            Operation::L6DinUsbForwarding => {
                self.run_l6_din_usb_forwarding(session, wall_timeout_s)
            }
            Operation::HostMidiSync => {
                self.run_host_midi_sync(session, guest_timeout_ms, wall_timeout_s)
            }
            Operation::L6MidiSync => Ok(self.run_l6_midi_sync(session, options)?),
            Operation::L6MidiRingPinSweep { cc } => {
                self.run_l6_midi_ring_pin_sweep(session, cc, wall_timeout_s)
            }
            Operation::L6MidiMuteIndicators => {
                self.run_l6_midi_mute_indicators(session, wall_timeout_s)
            }
            Operation::UsbSysex { requests } => {
                let usb = session
                    .engine
                    .usb
                    .as_ref()
                    .ok_or("USB backend unavailable")?;
                let endpoints = self
                    .midi
                    .ok_or("usb-sysex needs a preceding USB MIDI enumeration")?;
                let start = session
                    .engine
                    .inputs
                    .as_ref()
                    .unwrap()
                    .panel
                    .snapshot()
                    .guest_ms;
                let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
                let mut evidence = Vec::new();
                for request in requests {
                    let reply_timeout_ms = request.reply_timeout_ms.unwrap_or(400);
                    let wire = l6max_host::usb_midi::Encoder::default().push(2, &request.bytes);
                    usb.send(endpoints.output, &wire)?;
                    let request_start = session
                        .engine
                        .inputs
                        .as_ref()
                        .unwrap()
                        .panel
                        .snapshot()
                        .guest_ms;
                    let mut pending = Vec::new();
                    let mut replies: Vec<Vec<u8>> = Vec::new();
                    loop {
                        session.poll()?;
                        match usb.try_receive(endpoints.input, 16_384) {
                            Ok(data) => {
                                for (cable, fragment) in l6max_host::usb_midi::decode(&data)? {
                                    if cable != 2 {
                                        continue;
                                    }
                                    for byte in fragment {
                                        if byte == 0xf0 {
                                            pending.clear();
                                        }
                                        if byte == 0xf0 || !pending.is_empty() {
                                            pending.push(byte);
                                            if byte == 0xf7 {
                                                replies.push(std::mem::take(&mut pending));
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => {}
                            Err(e) => return Err(e.into()),
                        }
                        let elapsed = session
                            .engine
                            .inputs
                            .as_ref()
                            .unwrap()
                            .panel
                            .snapshot()
                            .guest_ms
                            .wrapping_sub(request_start);
                        if elapsed >= reply_timeout_ms {
                            break;
                        }
                        if Instant::now() > deadline
                            || session
                                .engine
                                .inputs
                                .as_ref()
                                .unwrap()
                                .panel
                                .snapshot()
                                .guest_ms
                                .wrapping_sub(start)
                                > guest_timeout_ms
                        {
                            return Err(format!(
                                "USB SysEx deadline exceeded for request {:02x?}",
                                request.bytes
                            )
                            .into());
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    if !pending.is_empty() {
                        return Err(format!("truncated USB SysEx reply: {pending:02x?}").into());
                    }
                    if request.expect_reply && replies.is_empty() {
                        return Err(format!(
                            "missing SysEx reply for request {:02x?}",
                            request.bytes
                        )
                        .into());
                    }
                    if let Some(minimum) = request.min_reply_bytes {
                        if !replies.iter().any(|reply| reply.len() >= minimum) {
                            return Err(format!(
                                "SysEx reply for {:02x?} was shorter than {minimum} bytes: {replies:02x?}",
                                request.bytes
                            )
                            .into());
                        }
                    }
                    if let Some(prefix) = &request.expect_prefix {
                        if !replies
                            .iter()
                            .any(|reply| reply.starts_with(prefix) && reply.last() == Some(&0xf7))
                        {
                            return Err(format!(
                                "missing SysEx reply {prefix:02x?}; got {replies:02x?}"
                            )
                            .into());
                        }
                    } else if !request.expect_reply && !replies.is_empty() {
                        return Err(format!("unexpected SysEx reply: {replies:02x?}").into());
                    }
                    for prefix in &request.reject_prefixes {
                        if replies.iter().any(|reply| reply.starts_with(prefix)) {
                            return Err(format!(
                                "SysEx reply matched rejected prefix {prefix:02x?}: {replies:02x?}"
                            )
                            .into());
                        }
                    }
                    evidence.push(json!({"sent":request.bytes,"replies":replies}));
                }
                Ok(json!({"exchanges":evidence}))
            }
            Operation::EditorPadAssignments => {
                let endpoints = self
                    .midi
                    .ok_or("editor-pad-assignments needs a preceding USB MIDI enumeration")?;
                run_editor_pad_assignments(
                    session,
                    options,
                    endpoints,
                    guest_timeout_ms,
                    wall_timeout_s,
                )
            }
            Operation::EditorPadPlayback { pad } => {
                let endpoints = self
                    .midi
                    .ok_or("editor-pad-playback needs preceding USB MIDI enumeration")?;
                run_editor_pad_playback(
                    session,
                    options,
                    endpoints,
                    pad,
                    guest_timeout_ms,
                    wall_timeout_s,
                )
            }
            Operation::L6maxUiPadAssignment { pad } => {
                let endpoints = self
                    .midi
                    .ok_or("l6max-ui-pad-assignment needs preceding USB MIDI enumeration")?;
                run_l6max_ui_pad_assignment(
                    session,
                    options,
                    endpoints,
                    pad,
                    guest_timeout_ms,
                    wall_timeout_s,
                )
            }
            Operation::SoundPadPlayback {
                pad,
                target,
                assignment,
                require_pad_led_change,
                playback_window_ms,
                require_stereo_meter,
                expect_source,
                max_residual_db,
            } => run_sound_pad_playback(
                session,
                options,
                pad,
                target,
                assignment,
                require_pad_led_change,
                playback_window_ms,
                require_stereo_meter,
                expect_source.as_deref(),
                max_residual_db,
                guest_timeout_ms,
                wall_timeout_s,
            ),
            Operation::L6RecorderPlayback { target } => {
                run_l6_recorder_playback(session, options, target, guest_timeout_ms, wall_timeout_s)
            }
            Operation::L6maxRecorderPlayback { target } => self.run_l6max_recorder_playback(
                session,
                options,
                target,
                guest_timeout_ms,
                wall_timeout_s,
            ),
            Operation::UsbStorage => {
                let usb = session
                    .engine
                    .usb
                    .as_ref()
                    .ok_or("USB backend unavailable")?;
                let configuration = usb.enumerate()?;
                let mut disk = l6max_host::usb_storage::MassStorage::new(
                    usb,
                    configuration
                        .storage
                        .ok_or("firmware did not expose mass storage")?,
                );
                let inquiry = disk.inquiry()?;
                disk.ready()?;
                let (blocks, size) = disk.capacity()?;
                if size != 512 || blocks == 0 {
                    return Err("invalid sector geometry".into());
                }
                let boot = disk.read_sector(0)?;
                use std::io::Read;
                let mut expected = [0; 512];
                fs::File::open(
                    options
                        .sd_image
                        .as_ref()
                        .ok_or("USB storage requires an SD card")?,
                )?
                .read_exact(&mut expected)?;
                if boot != expected {
                    return Err("USB sector did not match attached SD image".into());
                }
                let original = disk.read_sector((blocks - 1) as u32)?;
                disk.write_sector((blocks - 1) as u32, &original)?;
                if disk.read_sector((blocks - 1) as u32)? != original {
                    return Err("USB sector write/read mismatch".into());
                }
                let disk = l6max_host::usb_storage::UsbDisk::new(disk)?;
                let mut volume = sd_image::Volume::open(disk)?;
                let filename = format!("L6T{:05X}.TXT", std::process::id() & 0xfffff);
                let expected = (0..1537)
                    .map(|index| ((index * 37 + 11) % 256) as u8)
                    .collect::<Vec<_>>();
                if volume.exists(&filename) {
                    return Err("USB FAT roundtrip filename already exists".into());
                }
                volume.write(&filename, &expected)?;
                volume.flush()?;
                let actual = volume.read(&filename)?;
                volume.remove(&filename)?;
                if expected != actual {
                    return Err(format!(
                        "USB FAT file roundtrip mismatch: wrote {} bytes, read {} bytes",
                        expected.len(),
                        actual.len()
                    )
                    .into());
                }
                volume.flush()?;
                Ok(
                    json!({"inquiry":String::from_utf8_lossy(&inquiry[8..36]),"blocks":blocks,"sector_size":size,"sector_round_trip":true,"fat_file_round_trip_bytes":expected.len()}),
                )
            }
            Operation::AudioCallbacks => {
                let points = callbacks(session.engine.model());
                let model = session.engine.model();
                let qmp = &session.engine.management[0];
                qmp.stop()?;
                let mut debugger = l6max_diagnostics::gdb::Gdb::connect(
                    options
                        .gdb_socket
                        .as_ref()
                        .ok_or("missing audio debugger socket")?,
                )?;
                for (address, _) in points {
                    debugger.breakpoint(*address, true)?;
                }
                let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
                let start = session
                    .engine
                    .inputs
                    .as_ref()
                    .unwrap()
                    .panel
                    .snapshot()
                    .guest_ms;
                qmp.resume()?;
                let mut hits: Vec<Value> = Vec::new();
                while hits.len() < points.len() {
                    session.poll()?;
                    if Instant::now() > deadline
                        || session
                            .engine
                            .inputs
                            .as_ref()
                            .unwrap()
                            .panel
                            .snapshot()
                            .guest_ms
                            .wrapping_sub(start)
                            > guest_timeout_ms
                    {
                        let missing: Vec<_> = points
                            .iter()
                            .map(|(_, name)| *name)
                            .filter(|name| !hits.iter().any(|hit| hit["callback"] == *name))
                            .collect();
                        return Err(format!(
                            "audio callback deadline exceeded; not reached: {missing:?}"
                        )
                        .into());
                    }
                    if !qmp.query_status()?.running {
                        let pc = debugger.read_register(15)? & !1;
                        let (_, name) = points
                            .iter()
                            .find(|(a, _)| *a == pc)
                            .ok_or_else(|| format!("unexpected audio stop at {pc:#x}"))?;
                        hits.push(json!({"callback":name,"address":format!("{pc:#010x}"),"r0_r3":(0..4).map(|i| debugger.read_register(i)).collect::<Result<Vec<_>,_>>()?,"lr":debugger.read_register(14)?,"guest_ms":session.engine.inputs.as_ref().unwrap().panel.snapshot().guest_ms}));
                        debugger.breakpoint(pc, false)?;
                        qmp.resume()?;
                    } else {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                let trace = fs::read_to_string(options.logs.join("engine.trace"))?;
                let dma = verify_sai_edma_trace(model, &trace)?;
                let evidence = json!({"breakpoint_hits":hits,"sai_edma":dma});
                fs::write(
                    options.logs.join("audio-path-smoke.json"),
                    serde_json::to_vec_pretty(&evidence)?,
                )?;
                Ok(evidence)
            }
            Operation::KnobDialog => {
                run_knob_dialog(session, options, guest_timeout_ms, wall_timeout_s)
            }
            Operation::FirmwareUpdate => {
                Err("firmware-update scenario must run at the host lifecycle boundary".into())
            }
            Operation::WasmPatch => {
                Err("wasm-patch scenario must run at the host lifecycle boundary".into())
            }
        }
    }
}

/// Verify the generated per-patch module before inspecting its live registry.
type SdEdits = (
    Vec<(u32, Vec<u8>, u32)>,
    u32,
    Vec<u8>,
    Option<u32>,
    Vec<patch_loader_core::bundle::Relocation>,
    Vec<patch_loader_core::dynamic::Hook>,
    Vec<(
        Vec<u8>,
        Vec<patch_loader_core::bundle::Relocation>,
        Vec<patch_loader_core::dynamic::Hook>,
    )>,
);
fn sd_fixture_root(options: &Options) -> Result<&std::path::Path, String> {
    options
        .logs
        .ancestors()
        .find(|dir| dir.join("sd-stock-firmware/main_firmware.bin").is_file())
        .ok_or_else(|| "missing generated SD fixture".into())
}
fn sd_fixture_edits(options: &Options, main: &[u8], model: l6fw::Model) -> Result<SdEdits, String> {
    use sha2::{Digest, Sha256};
    let root = sd_fixture_root(options)?;
    let stock =
        fs::read(root.join("sd-stock-firmware/main_firmware.bin")).map_err(|e| e.to_string())?;
    let (patch_model, image_base) = match model {
        l6fw::Model::L6 => (firmware_patch_core::FirmwareModel::L6, 0x8000_1000u32),
        l6fw::Model::L6max => (firmware_patch_core::FirmwareModel::L6max, 0x8000_0000u32),
    };
    firmware_patch_core::verify_firmware_for(patch_model, &stock)?;
    let loader: Value = serde_json::from_slice(
        &fs::read(root.join("patch-loader-firmware/patch.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if loader["patched_sha256"].as_str() != Some(format!("{:x}", Sha256::digest(main)).as_str()) {
        return Err("Patch loader image does not match its generated report".into());
    }
    let patch_output = root.join("sd-patches-build");
    let patch_report_path = patch_output.join("patch.json");
    if !patch_report_path.exists() {
        return Ok((
            Vec::new(),
            0,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
    }
    let patch_report: Value =
        serde_json::from_slice(&fs::read(&patch_report_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    fn parse_module(
        bytes: &[u8],
        stock: &[u8],
        image_base: u32,
    ) -> Result<
        (
            Vec<(u32, Vec<u8>, u32)>,
            Vec<u8>,
            Option<u32>,
            Vec<patch_loader_core::bundle::Relocation>,
            Vec<patch_loader_core::dynamic::Hook>,
        ),
        String,
    > {
        if bytes.len() < patch_loader_core::dynamic::HEADER_LEN_V1 || &bytes[..4] != b"L6SB" {
            return Err("invalid SD patch file or CRC".into());
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        let header_len = patch_loader_core::dynamic::HEADER_LEN_V1;
        if version != patch_loader_core::dynamic::FORMAT_VERSION
            || bytes.len() < header_len
            || u16::from_le_bytes(bytes[6..8].try_into().unwrap())
                != patch_loader_core::dynamic::LOADER_ABI
            || patch_loader_core::dynamic::checksum(bytes)
                != Some(u32::from_le_bytes(bytes[24..28].try_into().unwrap()))
        {
            return Err("invalid SD patch file or CRC".into());
        }
        let code_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let memory_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let hook_count = u16::from_le_bytes(bytes[16..18].try_into().unwrap()) as usize;
        let relocation_count = u16::from_le_bytes(bytes[18..20].try_into().unwrap()) as usize;
        let expected_len = header_len
            .checked_add(code_len)
            .and_then(|n| {
                n.checked_add(hook_count.checked_mul(patch_loader_core::dynamic::HOOK_RECORD_LEN)?)
            })
            .and_then(|n| n.checked_add(relocation_count.checked_mul(12)?))
            .ok_or("SD patch size overflow")?;
        if expected_len != bytes.len()
            || code_len > memory_len
            || memory_len > 0x60000
            || hook_count > patch_loader_core::dynamic::MAX_HOOKS_PER_MODULE
            || relocation_count > 256
        {
            return Err("invalid SD patch lengths or counts".into());
        }
        struct Reader<'a>(&'a [u8]);
        impl patch_loader_core::bundle::Reader for Reader<'_> {
            type Error = ();
            fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
                let count = out.len().min(self.0.len());
                out[..count].copy_from_slice(&self.0[..count]);
                self.0 = &self.0[count..];
                Ok(count)
            }
        }
        let mut reader = Reader(bytes);
        let pending = patch_loader_core::dynamic::read_header(
            &mut reader,
            0x60000,
            patch_loader_core::dynamic::MAX_HOOKS_PER_MODULE,
            256,
        )
        .map_err(|e| format!("invalid SD patch header: {e:?}"))?;
        let mut payload = vec![0; memory_len];
        let mut hooks = vec![patch_loader_core::dynamic::Hook::default(); hook_count];
        let header = patch_loader_core::dynamic::read_body(
            &mut reader,
            pending,
            &mut payload,
            &mut hooks,
            0,
            0,
        )
        .map_err(|e| format!("invalid SD patch body: {e:?}"))?;
        let module_hooks = hooks.clone();
        let mut edits = Vec::with_capacity(hook_count);
        for hook in hooks {
            let offset = hook
                .address
                .checked_sub(image_base)
                .ok_or("hook precedes firmware image")? as usize;
            let end = offset.checked_add(4).ok_or("hook range overflow")?;
            let original = stock
                .get(offset..end)
                .ok_or("hook exceeds firmware image")?
                .to_vec();
            edits.push((hook.address, original, hook.entry_offset));
        }
        let mut relocations = Vec::with_capacity(relocation_count);
        let reloc_start =
            header_len + code_len + hook_count * patch_loader_core::dynamic::HOOK_RECORD_LEN;
        for bytes in bytes[reloc_start..].chunks_exact(12) {
            let kind = u32::from_le_bytes(bytes[..4].try_into().unwrap());
            let first = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            let second = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
            relocations.push(match kind {
                1 => patch_loader_core::bundle::Relocation::Abs32 { offset: first },
                2 => patch_loader_core::bundle::Relocation::ThumbMov32 {
                    low_offset: first,
                    high_offset: second,
                },
                _ => return Err("invalid relocation kind".into()),
            });
        }
        Ok((
            edits,
            payload,
            (header.init_offset != 0).then_some(header.init_offset),
            relocations,
            module_hooks,
        ))
    }
    let mut all_edits = Vec::new();
    let mut all_hooks = Vec::new();
    let mut module_images = Vec::new();
    let mut last = None;
    for patch in patch_report["patches"]
        .as_array()
        .ok_or("dynamic report has no patch list")?
    {
        let filename = patch["file"]
            .as_str()
            .and_then(|file| std::path::Path::new(file).file_name())
            .ok_or("dynamic patch report has no module filename")?;
        let bytes =
            fs::read(patch_output.join("PATCHES").join(filename)).map_err(|e| e.to_string())?;
        let (edits, payload, init, relocations, hooks) = parse_module(&bytes, &stock, image_base)?;
        all_edits.extend(edits);
        all_hooks.extend(hooks.iter().copied());
        module_images.push((payload.clone(), relocations.clone(), hooks));
        last = Some((payload, init, relocations));
    }
    let (payload, init, relocations) = last.ok_or("dynamic report contains no modules")?;
    Ok((
        all_edits,
        0,
        payload,
        init,
        relocations,
        all_hooks,
        module_images,
    ))
}

fn sd_dynamic_handler_slot(
    options: &Options,
    _model: l6fw::Model,
    registry: u32,
    function_address: u32,
) -> Result<u32, String> {
    let root = sd_fixture_root(options)?;
    let report: Value = serde_json::from_slice(
        &fs::read(root.join("patch-loader-firmware/patch.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let sites = report["patches"]
        .as_array()
        .and_then(|patches| {
            patches
                .iter()
                .find(|patch| patch["name"] == "dynamic-dispatch")
        })
        .and_then(|patch| patch["sites"].as_array())
        .ok_or("Patch loader report has no dynamic dispatch site table")?;
    let index = sites
        .iter()
        .position(|site| site["address"].as_u64() == Some(u64::from(function_address)))
        .ok_or_else(|| format!("function {function_address:#010x} has no SD dispatch slot"))?;
    let table_offset = 0x100u32
        .checked_add(
            u32::try_from(core::mem::size_of::<
                [patch_loader_core::dynamic::FunctionHookChain;
                    patch_loader_core::dynamic::MAX_FUNCTION_CHAINS],
            >())
            .map_err(|e| e.to_string())?,
        )
        .ok_or("dynamic handler table offset overflow")?;
    registry
        .checked_add(table_offset)
        .and_then(|base| base.checked_add(u32::try_from(index).ok()?.checked_mul(4)?))
        .ok_or_else(|| "dynamic handler slot address overflow".into())
}

fn sd_knob_edits(options: &Options, main: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    let root = sd_fixture_root(options)?;
    let report: Value = serde_json::from_slice(
        &fs::read(root.join("sd-bundle-build/patch.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if !report["patches"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|patch| patch.as_str() == Some("knob-dialog") || patch["name"] == "knob-dialog")
    }) {
        return Err("SD fixture does not include knob-dialog".into());
    }
    let (records, _payload_base, _payload, init, _relocations, _module_hooks, _module_images) =
        sd_fixture_edits(options, main, l6fw::Model::L6max)?;
    if init.is_none() {
        return Err("SD knob-dialog has no late initializer".into());
    }
    let edits = records
        .into_iter()
        .map(|(address, original, _entry_offset)| {
            let offset = (address - 0x8000_0000) as usize;
            (address, main[offset..offset + original.len()].to_vec())
        })
        .collect::<Vec<_>>();
    Ok(edits)
}

fn run_knob_dialog(
    session: &Session,
    options: &Options,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    use l6max_host::indicators::rings;
    use std::time::Instant;

    const MAIN_SHA256: [&str; 4] = [
        // Templates regenerated from the current patch sources.
        "98898165368380606e8a76954a4da92438030ef7a3f878e5a302ce84cfa7fe5a",
        "76925b4e017083910ef33c2d52e3930d3cdad819cc8b4ffe5f782dd2f07212d7",
        // Recorder-fix release rebuild of the same knob-dialog patch, version 0112.
        "90c7cbdd0fee494bdcaf805730b1eb3931117d27d96ab651fe5dc604695c3010",
        "863ee4475b728e9a7b1d04fcdd4eb28555aee3fc8009eca9c36d2a3032b5fc32",
    ];
    let main = selected_main_image(options).map_err(std::io::Error::other)?;
    use sha2::{Digest, Sha256};
    let dynamic_knob_dialog = if options.sd_image.is_some() {
        sd_fixture_edits(options, &main, l6fw::Model::L6max)
            .ok()
            .is_some_and(|(_, _, _, _, _, _, module_images)| {
                module_images.iter().any(|(payload, _, _)| {
                    payload
                        .windows(b"KNOB_WIN1".len())
                        .any(|window| window == b"KNOB_WIN1")
                })
            })
    } else {
        false
    };
    let statically_patched = main.get(0x40c..0x410) == Some(&0x8007eea9u32.to_le_bytes());
    let sd_edits = if statically_patched
        || MAIN_SHA256.contains(&format!("{:x}", Sha256::digest(&main)).as_str())
    {
        None
    } else {
        Some(sd_knob_edits(options, &main).map_err(std::io::Error::other)?)
    };
    if main.get(0x40c..0x410) != Some(&0x8007eea9u32.to_le_bytes()) {
        return Err("patched startup hook does not reserve popup RAM".into());
    }
    let inputs = session.engine.inputs.as_ref().unwrap();
    let qmp = &session.engine.management[0];
    let encoder_report_baseline = fs::read_to_string(options.logs.join("engine.trace"))?
        .matches("l6 panel USART1 TX a1\n")
        .count() as u64;
    let start = inputs.panel.snapshot().guest_ms;
    wait_guest(session, start, 40_000, guest_timeout_ms, wall_timeout_s)?;
    if let Some(edits) = &sd_edits {
        for (address, expected) in edits {
            if qmp.read_memory(0, *address, expected.len())? != *expected {
                return Err(format!("SD patch not active at {address:#010x}").into());
            }
        }
        let (_, link_base, mut expected, _, relocations, _, _) =
            sd_fixture_edits(options, &main, l6fw::Model::L6max).map_err(std::io::Error::other)?;
        let registry = link_base + 0x8000;
        let loaded_base =
            u32::from_le_bytes(qmp.read_memory(0, registry + 160, 4)?.try_into().unwrap());
        if loaded_base == 0 {
            return Err("SD knob-dialog module allocation is missing".into());
        }
        patch_loader_core::bundle::relocate(
            &mut expected,
            relocations.into_iter(),
            link_base,
            loaded_base,
        )
        .map_err(|error| format!("invalid knob-dialog relocations: {error:?}"))?;
        for (index, chunk) in expected.chunks(4096).enumerate() {
            let address = loaded_base + (index * 4096) as u32;
            if qmp.read_memory(0, address, chunk.len())? != chunk {
                return Err("SD knob-dialog bytes do not match the relocated bundle".into());
            }
        }
        let slot = qmp.read_memory(0, 0x8020a2a0, 4)?;
        if u32::from_le_bytes(slot.try_into().unwrap()) & 1 == 0 {
            return Err("SD knob-dialog late initialization did not install its sidecar".into());
        }
    }
    inputs.tap(3, 50)?;
    let after = inputs.panel.snapshot().guest_ms;
    wait_guest(session, after, 1_950, guest_timeout_ms, wall_timeout_s)?;
    inputs.tap(3, 50)?;
    let after = inputs.panel.snapshot().guest_ms;
    wait_guest(session, after, 4_950, guest_timeout_ms, wall_timeout_s)?;

    let before = rings(inputs.panel.snapshot().indicators);
    let original_resources = qmp.read_memory(0, 0x802059b8, 12)?;
    let original_bitmap = qmp.read_memory(0, 0x80207458, 4)?;
    let original_header = qmp.read_memory(0, 0x8020098c, 6)?;
    let original_value_y = qmp.read_memory(0, 0x8020099c, 2)?;
    let original_polarity = qmp.read_memory(0, 0x80200986, 2)?;
    let lengths_table = u32::from_le_bytes(qmp.read_memory(0, 0x802050fc, 4)?.try_into().unwrap());
    let original_lengths = qmp.read_memory(0, lengths_table + 229 * 2, 6)?;
    let mut original_text = Vec::new();
    for (pointer, capacity) in original_resources
        .chunks_exact(4)
        .zip(original_lengths.chunks_exact(2))
    {
        let address = u32::from_le_bytes(pointer.try_into().unwrap());
        let count = (usize::from(u16::from_le_bytes(capacity.try_into()?)) + 1) * 2;
        original_text.push((address, qmp.read_memory(0, address, count)?));
    }
    let abandoned_page = qmp.read_memory(0, 0x841ff000, 4096)?;
    let heap = || -> Result<Vec<u32>, Box<dyn std::error::Error>> {
        Ok(qmp
            .read_memory(0, 0x8020ab1c, 16)?
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect())
    };
    let text_resource = |index: u32| -> Result<String, Box<dyn std::error::Error>> {
        let pointer = u32::from_le_bytes(
            qmp.read_memory(0, 0x802059b8 + 4 * index, 4)?
                .try_into()
                .unwrap(),
        );
        let bytes = qmp.read_memory(0, pointer, 48)?;
        let text = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|c| *c != 0)
            .collect::<Vec<_>>();
        Ok(String::from_utf16(&text)?)
    };
    let capture = |duration: u32, name: &str| -> Result<[u8; 1024], Box<dyn std::error::Error>> {
        let start = inputs.panel.snapshot().guest_ms;
        let begin = Instant::now();
        let raw = loop {
            let data = session.poll()?;
            let elapsed = inputs.panel.snapshot().guest_ms.wrapping_sub(start);
            if elapsed >= duration {
                break data.display;
            }
            if elapsed > guest_timeout_ms || begin.elapsed() > Duration::from_secs(wall_timeout_s) {
                return Err("knob-dialog display capture exceeded its deadline".into());
            }
            thread::sleep(Duration::from_millis(5));
        };
        image::DynamicImage::ImageLuma8(image::GrayImage::from_fn(128, 64, |x, y| {
            let index = ((63 - y as usize) / 8) * 128 + x as usize;
            image::Luma([if raw[index] & (1 << ((63 - y as usize) % 8)) != 0 {
                255
            } else {
                0
            }])
        }))
        .resize(768, 384, image::imageops::FilterType::Nearest)
        .save(options.logs.join(format!("{name}.png")))?;
        Ok(raw)
    };
    let expected_value = |mode: u32, raw: u8| -> String {
        let table = |offset: usize| {
            f32::from_le_bytes(
                main[offset + raw as usize * 4..offset + raw as usize * 4 + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        match mode {
            0 | 2 | 3 => {
                let db = table(0xa5138);
                if db > 0.0 {
                    format!("+{db:.2} dB")
                } else {
                    format!("{db:.2} dB")
                }
            }
            1 => {
                let hz = table(0xa5738);
                if hz < 1000.0 {
                    format!("{hz:.0} Hz")
                } else {
                    format!("{:.2} kHz", hz / 1000.0)
                }
            }
            8 => match raw {
                63 | 64 => "CENTER".into(),
                0..=62 => format!("L {}%", ((63 - raw as u32) * 100 + 31) / 63),
                _ => format!("R {}%", ((raw as u32 - 64) * 100 + 31) / 63),
            },
            _ => format!("{}%", (raw as u32 * 100 + 63) / 127),
        }
    };
    if dynamic_knob_dialog
        || main
            .windows(b"KNOB_WIN1".len())
            .any(|window| window == b"KNOB_WIN1")
    {
        let inputs = session.engine.inputs.as_ref().unwrap();
        // Check the first-use guard on an actual non-recorder screen before
        // the dialog has initialized state or claimed its display callback.
        const RECORDER_HOME: u32 = 0x8020_0408;
        if u32::from_le_bytes(qmp.read_memory(0, 0x8020_2610, 4)?.try_into().unwrap())
            != RECORDER_HOME
        {
            return Err("knob-dialog test did not start from the recorder screen".into());
        }
        if qmp.read_memory(0, 0x8020_73c8, 4)? != 0u32.to_le_bytes()
            || qmp.read_memory(0, 0x8020_a59c + 20, 4)? != 0u32.to_le_bytes()
        {
            return Err(
                "knob-dialog state or callback was already active before the guard test".into(),
            );
        }
        inputs.main.tap(0, 120)?;
        let menu_start = inputs.panel.snapshot().guest_ms;
        wait_guest(session, menu_start, 500, guest_timeout_ms, wall_timeout_s)?;
        let non_recorder_window =
            u32::from_le_bytes(qmp.read_memory(0, 0x8020_2610, 4)?.try_into().unwrap());
        if non_recorder_window == RECORDER_HOME {
            return Err("Menu button did not leave the recorder screen for the guard test".into());
        }
        capture(100, "knob-dialog-non-recorder-before")?;
        inputs.knob(0, -1)?;
        let non_recorder_knob = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            non_recorder_knob,
            500,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        qmp.stop()?;
        let non_recorder_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let non_recorder_slot5 = qmp.read_memory(0, 0x8020_a59c + 20, 4)?;
        let non_recorder_window_after =
            u32::from_le_bytes(qmp.read_memory(0, 0x8020_2610, 4)?.try_into().unwrap());
        let non_recorder_no_dialog = non_recorder_state == 0u32.to_le_bytes()
            && non_recorder_slot5 == 0u32.to_le_bytes()
            && non_recorder_window_after == non_recorder_window;
        qmp.resume()?;
        if !non_recorder_no_dialog {
            return Err(format!(
                "knob event opened or changed the dialog outside recorder: state={non_recorder_state:02x?}, slot5={non_recorder_slot5:02x?}, window={non_recorder_window_after:#010x}"
            )
            .into());
        }
        inputs.main.tap(0, 120)?;
        let return_start = inputs.panel.snapshot().guest_ms;
        wait_guest(session, return_start, 500, guest_timeout_ms, wall_timeout_s)?;
        if u32::from_le_bytes(qmp.read_memory(0, 0x8020_2610, 4)?.try_into().unwrap())
            != RECORDER_HOME
        {
            return Err("Menu button did not return to recorder before dialog checks".into());
        }
        let non_recorder_guard_passed = true;

        let display_callbacks = qmp.read_memory(0, 0x8020_a59c, 24)?;
        let display_callback_target =
            u32::from_le_bytes(display_callbacks[20..24].try_into().unwrap());
        let slot0 = u32::from_le_bytes(display_callbacks[0..4].try_into().unwrap());
        let slot2 = u32::from_le_bytes(display_callbacks[8..12].try_into().unwrap());
        if display_callback_target != 0 || slot0 != 0 || slot2 != 0 {
            return Err(format!(
                "expected an empty slot 5 before the first knob event; slot0={slot0:#010x}, slot2={slot2:#010x}, slot5={display_callback_target:#010x}"
            )
            .into());
        }
        inputs.tap(35, 50)?;
        let pressed = inputs.panel.snapshot().guest_ms;
        wait_guest(session, pressed, 500, guest_timeout_ms, wall_timeout_s)?;
        let baseline = capture(100, "window-scene-before")?;
        let original_window = qmp.read_memory(0, 0x80202610, 4)?;
        let original_bitmap = qmp.read_memory(0, 0x80207458, 4)?;
        let original_bitmap_descriptor = qmp.read_memory(0, 0x800c_b964, 8)?;
        let original_state_handle = qmp.read_memory(0, 0x8020_73c8, 4)?;
        if u32::from_le_bytes(original_bitmap.clone().try_into().unwrap()) != 0x800c_b964 {
            return Err(
                "stock bitmap-table entry 36 did not point at its guarded ROM descriptor".into(),
            );
        }
        if original_state_handle != 0u32.to_le_bytes() {
            return Err(format!(
                "bitmap-table slot 0 is not the expected null sentinel: {original_state_handle:02x?}"
            )
            .into());
        }
        let original_notification_semaphore = qmp.read_memory(0, 0x8020a2a0, 4)?;
        let notification_semaphore =
            u32::from_le_bytes(original_notification_semaphore.clone().try_into().unwrap());
        if notification_semaphore == 0 || notification_semaphore & 1 != 0 {
            return Err(format!(
                "expected the stock untagged notification semaphore, got {notification_semaphore:#010x}"
            )
            .into());
        }
        let text_table = u32::from_le_bytes(qmp.read_memory(0, 0x80205118, 4)?.try_into().unwrap());
        let decode_text = |address: u32| -> Result<String, Box<dyn std::error::Error>> {
            let pointer = u32::from_le_bytes(qmp.read_memory(0, address, 4)?.try_into().unwrap());
            let bytes = qmp.read_memory(0, pointer, 48)?;
            let units = bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .take_while(|c| *c != 0)
                .collect::<Vec<_>>();
            Ok(String::from_utf16(&units)?)
        };
        let capture_demo = |name: &str| -> Result<[u8; 1024], Box<dyn std::error::Error>> {
            let mut frame = capture(100, name)?;
            for _ in 0..6 {
                if frame.iter().map(|byte| byte.count_ones()).sum::<u32>() >= 100 {
                    return Ok(frame);
                }
                frame = capture(100, name)?;
            }
            Err(format!("demo frame {name} stayed blank").into())
        };
        let original_resources = qmp.read_memory(0, text_table + 229 * 4, 8)?;
        let shared_resource_pointer = u32::from_le_bytes(
            qmp.read_memory(0, text_table + 179 * 4, 4)?
                .try_into()
                .unwrap(),
        );
        let original_shared_resource = qmp.read_memory(0, shared_resource_pointer, 512)?;
        let original_lengths_table =
            u32::from_le_bytes(qmp.read_memory(0, 0x802050fc, 4)?.try_into().unwrap());
        let original_lengths = qmp.read_memory(0, original_lengths_table + 229 * 2, 4)?;
        let widget_pointer = |index: u32| -> Result<u32, Box<dyn std::error::Error>> {
            Ok(u32::from_le_bytes(
                qmp.read_memory(0, 0x80200af4 + index * 4, 4)?
                    .try_into()
                    .unwrap(),
            ))
        };
        let widget7 = widget_pointer(7)?;
        let widget33 = widget_pointer(33)?;
        let original_widget7 = qmp.read_memory(0, widget7, 16)?;
        let original_widget33 = qmp.read_memory(0, widget33, 16)?;
        if original_widget7.as_slice() != [1, 0, 0, 0, 10, 0, 1, 0, 1, 0, 126, 0, 1, 0, 1, 2]
            || original_widget33.as_slice() != [1, 0, 1, 0, 40, 0, 30, 0, 33, 0, 67, 0, 1, 0, 1, 0]
        {
            return Err("recorder widgets do not match the guarded firmware defaults".into());
        }
        let original_shared_capacity = qmp.read_memory(0, original_lengths_table + 179 * 2, 2)?;
        if u16::from_le_bytes(original_shared_capacity.try_into().unwrap()) != 255
            || original_shared_resource
                .chunks_exact(2)
                .take(255)
                .any(|unit| unit != b"*\0")
            || original_shared_resource[510..512] != [0, 0]
        {
            return Err(
                "shared GUI resource 179 is not the expected free 255-character buffer".into(),
            );
        }
        let background_pointer = |index: u32| -> Result<u32, Box<dyn std::error::Error>> {
            Ok(u32::from_le_bytes(
                qmp.read_memory(0, 0x80200670 + index * 4, 4)?
                    .try_into()
                    .unwrap(),
            ))
        };
        let background10 = background_pointer(10)?;
        let background37 = background_pointer(37)?;
        let background48 = background_pointer(48)?;
        let original_background_layers = [
            qmp.read_memory(0, background10, 1)?,
            qmp.read_memory(0, background37, 1)?,
            qmp.read_memory(0, background48, 1)?,
        ];
        let heap_before = heap()?;
        let socket = options
            .gdb_socket
            .as_ref()
            .ok_or("knob-dialog lock-contention check needs GDB")?;
        let mut debugger = l6max_diagnostics::gdb::Gdb::connect(socket)?;

        // A descriptor drift must make the patch decline first use without
        // allocating state or claiming its callback slot.
        qmp.stop()?;
        debugger.write(widget7 + 6, &2u16.to_le_bytes())?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let mismatch_start = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            mismatch_start,
            500,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        qmp.stop()?;
        let mismatch_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let mismatch_slot5 = qmp.read_memory(0, 0x8020_a59c + 20, 4)?;
        let mismatch_heap = heap()?;
        debugger.write(widget7 + 6, &1u16.to_le_bytes())?;
        qmp.resume()?;
        if mismatch_state != 0u32.to_le_bytes()
            || mismatch_slot5 != 0u32.to_le_bytes()
            || mismatch_heap != heap_before
        {
            return Err(format!(
                "descriptor drift was not rejected cleanly: state={mismatch_state:02x?}, slot5={mismatch_slot5:02x?}, heap={mismatch_heap:?}"
            )
            .into());
        }

        // Resource 179 is borrowed by the dialog only if no other initialized
        // widget points at it. Simulate another patch claiming that resource.
        let alias_widget = widget_pointer(60)?;
        let original_alias_resource = qmp.read_memory(0, alias_widget + 4, 2)?;
        qmp.stop()?;
        debugger.write(alias_widget + 4, &179u16.to_le_bytes())?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let alias_start = inputs.panel.snapshot().guest_ms;
        wait_guest(session, alias_start, 500, guest_timeout_ms, wall_timeout_s)?;
        qmp.stop()?;
        let alias_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let alias_slot5 = qmp.read_memory(0, 0x8020_a59c + 20, 4)?;
        let alias_heap = heap()?;
        debugger.write(alias_widget + 4, &original_alias_resource)?;
        qmp.resume()?;
        if alias_state != 0u32.to_le_bytes()
            || alias_slot5 != 0u32.to_le_bytes()
            || alias_heap != heap_before
        {
            return Err(format!(
                "shared text resource alias was not rejected cleanly: state={alias_state:02x?}, slot5={alias_slot5:02x?}, heap={alias_heap:?}"
            )
            .into());
        }

        // Layer 4 is reserved for this scene. An existing descriptor on that
        // layer must also prevent first use from claiming it.
        let original_alias_layer = qmp.read_memory(0, alias_widget, 2)?;
        qmp.stop()?;
        debugger.write(alias_widget, &4u16.to_le_bytes())?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let layer_start = inputs.panel.snapshot().guest_ms;
        wait_guest(session, layer_start, 500, guest_timeout_ms, wall_timeout_s)?;
        qmp.stop()?;
        let layer_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let layer_slot5 = qmp.read_memory(0, 0x8020_a59c + 20, 4)?;
        let layer_heap = heap()?;
        debugger.write(alias_widget, &original_alias_layer)?;
        qmp.resume()?;
        if layer_state != 0u32.to_le_bytes()
            || layer_slot5 != 0u32.to_le_bytes()
            || layer_heap != heap_before
        {
            return Err(format!(
                "scene layer collision was not rejected cleanly: state={layer_state:02x?}, slot5={layer_slot5:02x?}, heap={layer_heap:?}"
            )
            .into());
        }

        // The compact state record restores fixed stock layers, so reject a
        // changed borrowed widget layer instead of trying to save metadata.
        let original_widget_layer = qmp.read_memory(0, widget7, 1)?;
        qmp.stop()?;
        debugger.write(widget7, &[0])?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let widget_layer_start = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            widget_layer_start,
            500,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        qmp.stop()?;
        let widget_layer_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let widget_layer_slot5 = qmp.read_memory(0, 0x8020_a59c + 20, 4)?;
        let widget_layer_heap = heap()?;
        debugger.write(widget7, &original_widget_layer)?;
        qmp.resume()?;
        let widget_layer_guard_rejected = widget_layer_state == 0u32.to_le_bytes()
            && widget_layer_slot5 == 0u32.to_le_bytes()
            && widget_layer_heap == heap_before;
        if !widget_layer_guard_rejected {
            return Err(format!(
                "changed widget layer was not rejected cleanly: state={widget_layer_state:02x?}, slot5={widget_layer_slot5:02x?}, heap={widget_layer_heap:?}"
            )
            .into());
        }

        // Background descriptors have the same fixed-layer restoration
        // contract and must pass the same first-use preflight.
        qmp.stop()?;
        debugger.write(background10, &[1])?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let background_layer_start = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            background_layer_start,
            500,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        qmp.stop()?;
        let background_layer_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let background_layer_slot5 = qmp.read_memory(0, 0x8020_a59c + 20, 4)?;
        let background_layer_heap = heap()?;
        debugger.write(background10, &original_background_layers[0])?;
        qmp.resume()?;
        let background_layer_guard_rejected = background_layer_state == 0u32.to_le_bytes()
            && background_layer_slot5 == 0u32.to_le_bytes()
            && background_layer_heap == heap_before;
        if !background_layer_guard_rejected {
            return Err(format!(
                "changed background layer was not rejected cleanly: state={background_layer_state:02x?}, slot5={background_layer_slot5:02x?}, heap={background_layer_heap:?}"
            )
            .into());
        }

        // Simulate the notification manager owning its semaphore during first
        // use. The scene must not depend on that unrelated lock.
        qmp.stop()?;
        let semaphore_count = qmp.read_memory(0, notification_semaphore + 56, 4)?;
        if semaphore_count != 1u32.to_le_bytes() {
            qmp.resume()?;
            return Err("notification semaphore was not available before contention check".into());
        }
        debugger.write(notification_semaphore + 56, &0u32.to_le_bytes())?;
        // Stop at callback registration after the mixer-side scene preflight
        // has passed. Change a borrowed descriptor before DisplayCycle2 can
        // render so the callback-side preflight gets exercised deterministically.
        const DISPLAY_CALLBACK_REGISTER: u32 = 0x8002_4ed0;
        debugger.breakpoint(DISPLAY_CALLBACK_REGISTER, true)?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let registration_deadline = Instant::now() + Duration::from_secs(10);
        while qmp.query_status()?.running {
            session.poll()?;
            if Instant::now() >= registration_deadline {
                let _ = debugger.breakpoint(DISPLAY_CALLBACK_REGISTER, false);
                let _ = qmp.resume();
                return Err("first-use callback registration breakpoint timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        let registration_pc = debugger.read_register(15)? & !1;
        let callback_slot = debugger.read_register(0)?;
        let callback_pointer = debugger.read_register(1)?;
        if registration_pc != DISPLAY_CALLBACK_REGISTER
            || callback_slot != 5
            || callback_pointer & 1 == 0
        {
            let _ = debugger.breakpoint(DISPLAY_CALLBACK_REGISTER, false);
            let _ = qmp.resume();
            return Err(format!(
                "unexpected display callback registration: pc={registration_pc:#010x}, slot={callback_slot}, callback={callback_pointer:#010x}"
            )
            .into());
        }
        let callback_entry = callback_pointer & !1;
        debugger.breakpoint(callback_entry, true)?;
        debugger.breakpoint(DISPLAY_CALLBACK_REGISTER, false)?;
        debugger.write(alias_widget, &4u16.to_le_bytes())?;
        qmp.resume()?;
        let callback_deadline = Instant::now() + Duration::from_secs(10);
        while qmp.query_status()?.running {
            session.poll()?;
            if Instant::now() >= callback_deadline {
                let _ = debugger.breakpoint(callback_entry, false);
                let _ = qmp.resume();
                return Err("first DisplayCycle2 callback breakpoint timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        let callback_pc = debugger.read_register(15)? & !1;
        if callback_pc != callback_entry {
            let _ = debugger.breakpoint(callback_entry, false);
            let _ = qmp.resume();
            return Err(format!(
                "first-use race probe stopped at {callback_pc:#010x}, not callback {callback_entry:#010x}"
            )
            .into());
        }
        debugger.breakpoint(callback_entry, false)?;
        qmp.resume()?;
        let race_frame = capture(150, "window-scene-callback-preflight-race")?;
        qmp.stop()?;
        let race_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let race_slot5 = qmp.read_memory(0, 0x8020_a59c + 20, 4)?;
        let callback_preflight_race_rejected = race_frame == baseline
            && u16::from_le_bytes(race_state[..2].try_into().unwrap()) == 0x4b4e
            && u32::from_le_bytes(race_slot5.clone().try_into().unwrap()) == callback_pointer
            && qmp.read_memory(0, alias_widget, 1)? == [4]
            && qmp.read_memory(0, widget7, 1)? == [1]
            && qmp.read_memory(0, background10, 1)? == [0]
            && qmp.read_memory(0, shared_resource_pointer, 512)? == original_shared_resource;
        debugger.write(alias_widget, &original_alias_layer)?;
        qmp.resume()?;
        if !callback_preflight_race_rejected {
            return Err(format!(
                "callback-side scene preflight failed to reject descriptor drift: state={race_state:02x?}, slot5={race_slot5:02x?}, frame_unchanged={} ",
                race_frame == baseline
            )
            .into());
        }

        // Retry while the notification semaphore is still held. The scene
        // should now pass its callback-side preflight and render without using
        // the notification manager's lock.
        inputs.knob(0, -1)?;
        let contention_start = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            contention_start,
            500,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        qmp.stop()?;
        let first_state = qmp.read_memory(0, 0x8020_73c8, 4)?;
        let first_slot5 =
            u32::from_le_bytes(qmp.read_memory(0, 0x8020_a59c + 20, 4)?.try_into().unwrap());
        let heap_while_locked = heap()?;
        let notification_lock_still_held =
            qmp.read_memory(0, notification_semaphore + 56, 4)? == 0u32.to_le_bytes();
        let dialog_without_notification_lock =
            u16::from_le_bytes(first_state[..2].try_into().unwrap()) == 0x4b4e
                && first_slot5 & 1 == 1
                && qmp.read_memory(0, 0x8020_73c8 + 3, 1)? == u8::MAX.to_le_bytes()
                && qmp.read_memory(0, widget7, 1)? == [4]
                && qmp.read_memory(0, background10, 1)? == [4]
                && notification_lock_still_held;
        debugger.write(notification_semaphore + 56, &semaphore_count)?;
        qmp.resume()?;
        if !dialog_without_notification_lock
            || heap_while_locked[2] != heap_before[2]
            || heap_while_locked[3] != heap_before[3]
        {
            return Err(format!(
                "dialog first use failed: state={first_state:02x?}, slot5={first_slot5:#010x}, heap={heap_while_locked:?}, notification_lock_still_held={notification_lock_still_held}, widget_x=({}, {})",
                u16::from_le_bytes([original_widget7[6], original_widget7[7]]),
                u16::from_le_bytes([original_widget33[6], original_widget33[7]])
            )
            .into());
        }

        inputs.knob(0, -3)?;
        let active = capture(500, "knob-dialog-active")?;
        let active_callbacks = qmp.read_memory(0, 0x8020_a59c, 24)?;
        let active_slot5 = u32::from_le_bytes(active_callbacks[20..24].try_into().unwrap());
        if active_slot5 & 1 == 0
            || u32::from_le_bytes(active_callbacks[0..4].try_into().unwrap()) != 0
            || u32::from_le_bytes(active_callbacks[8..12].try_into().unwrap()) != 0
        {
            return Err(format!(
                "first knob event did not claim only free callback slot 5; slots0/2/5={:#010x}/{:#010x}/{active_slot5:#010x}",
                u32::from_le_bytes(active_callbacks[0..4].try_into().unwrap()),
                u32::from_le_bytes(active_callbacks[8..12].try_into().unwrap()),
            )
            .into());
        }
        if active == baseline {
            return Err("custom overlay did not change the LCD".into());
        }

        // Capture emulator frames while the web demo values move in both
        // directions. The web animation plays these captures at 20 fps.
        capture_demo("knob-dialog-demo-level-start")?;
        let mut level_peak_reached = false;
        for index in 0..70 {
            let displayed = decode_text(text_table + 179 * 4)?;
            let value = displayed
                .strip_suffix('%')
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(0);
            if value >= 69 {
                level_peak_reached = true;
                break;
            }
            inputs.knob(0, 1)?;
            capture_demo(&format!("knob-dialog-demo-level-up-{index:02}"))?;
        }
        if !level_peak_reached {
            return Err("demo level sweep did not reach 69%".into());
        }
        capture(500, "knob-dialog-demo-level-peak")?;
        // The displayed percentage is rounded from a 0..127 firmware value,
        // so 60 encoder steps are not always enough to return to zero.
        for index in 0..128 {
            if decode_text(text_table + 179 * 4)? == "0%" {
                break;
            }
            inputs.knob(0, -1)?;
            capture_demo(&format!("knob-dialog-demo-level-down-{index:02}"))?;
        }
        if decode_text(text_table + 179 * 4)? != "0%" {
            return Err("demo level sweep did not return to zero".into());
        }
        capture(500, "knob-dialog-demo-level-zero")?;

        // Observe the producer and consumer task contexts directly. The knob
        // hook runs from the mixer update path; slot 5 runs from DisplayCycle2.
        // This determines whether the atomic mailbox can be replaced safely.
        const CURRENT_TCB: u32 = 0x808a_3af0;
        let knob_hook_entry = thumb_bl_target(
            0x8003_6c74,
            qmp.read_memory(0, 0x8003_6c74, 4)?
                .try_into()
                .map_err(|_| "patched mixer call is not four bytes")?,
        )
        .ok_or("patched mixer dispatcher call is not a Thumb BL")?;
        let callback_probe_entry = active_slot5 & !1;
        qmp.stop()?;
        debugger.breakpoint(knob_hook_entry, true)?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let task_probe_deadline = Instant::now() + Duration::from_secs(10);
        while qmp.query_status()?.running {
            session.poll()?;
            if Instant::now() >= task_probe_deadline {
                return Err("knob hook task-context breakpoint timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        let hook_pc = debugger.read_register(15)? & !1;
        if hook_pc != knob_hook_entry {
            return Err(format!(
                "task probe stopped at {hook_pc:#010x}, not knob hook {knob_hook_entry:#010x}"
            )
            .into());
        }
        let hook_task = u32::from_le_bytes(qmp.read_memory(0, CURRENT_TCB, 4)?.try_into().unwrap());
        debugger.breakpoint(knob_hook_entry, false)?;
        debugger.breakpoint(callback_probe_entry, true)?;
        qmp.resume()?;

        let task_probe_deadline = Instant::now() + Duration::from_secs(10);
        while qmp.query_status()?.running {
            session.poll()?;
            if Instant::now() >= task_probe_deadline {
                let _ = debugger.breakpoint(callback_probe_entry, false);
                let _ = qmp.resume();
                return Err("DisplayCycle2 task-context breakpoint timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        let callback_pc = debugger.read_register(15)? & !1;
        if callback_pc != callback_probe_entry {
            return Err(format!(
                "task probe stopped at {callback_pc:#010x}, not DisplayCycle2 callback"
            )
            .into());
        }
        let callback_task =
            u32::from_le_bytes(qmp.read_memory(0, CURRENT_TCB, 4)?.try_into().unwrap());
        if hook_task == callback_task {
            return Err(
                "mixer hook and DisplayCycle2 callback unexpectedly share a task; re-evaluate the atomic mailbox"
                    .into(),
            );
        }
        debugger.breakpoint(callback_probe_entry, false)?;
        qmp.resume()?;
        capture(150, "window-scene-task-context-probe")?;

        if u32::from_le_bytes(qmp.read_memory(0, 0x80202610, 4)?.try_into().unwrap())
            != u32::from_le_bytes(original_window.clone().try_into().unwrap())
        {
            return Err("custom overlay replaced the active firmware window".into());
        }
        if qmp.read_memory(0, widget7, 1)? != [4]
            || qmp.read_memory(0, widget33, 1)? != [4]
            || qmp.read_memory(0, background10, 1)? != [4]
            || qmp.read_memory(0, background37, 1)? != [4]
            || qmp.read_memory(0, background48, 1)? != [4]
        {
            return Err("custom overlay did not use compositor layer 4".into());
        }
        if qmp.read_memory(0, widget7 + 6, 2)? != 0u16.to_le_bytes()
            || qmp.read_memory(0, widget33 + 6, 2)? != 0u16.to_le_bytes()
        {
            return Err("custom overlay widgets were not positioned at the left edge".into());
        }
        for widget in [widget7, widget33] {
            let resource =
                u16::from_le_bytes(qmp.read_memory(0, widget + 4, 2)?.try_into().unwrap());
            if resource != 179 {
                return Err(format!(
                    "dialog widget did not use shared text resource 179: {resource}"
                )
                .into());
            }
        }
        if u32::from_le_bytes(
            qmp.read_memory(0, text_table + 179 * 4, 4)?
                .try_into()
                .unwrap(),
        ) != shared_resource_pointer
        {
            return Err("dialog redirected the shared text-resource pointer".into());
        }
        let black_pixel = |x: usize, y: usize| {
            let index = ((63 - y) / 8) * 128 + x;
            active[index] & (1 << ((63 - y) % 8)) == 0
        };
        if ![(0, 0), (127, 0), (0, 63), (127, 63)]
            .into_iter()
            .all(|(x, y)| black_pixel(x, y))
        {
            return Err("stock bitmap 22 did not paint the overlay background black".into());
        }
        let state = 0x8020_73c8u32;
        if u16::from_le_bytes(qmp.read_memory(0, state, 2)?.try_into().unwrap()) != 0x4b4e
            || qmp.read_memory(0, 0x80207458, 4)? != original_bitmap
        {
            return Err("in-place state signature or stock bitmap-36 descriptor is invalid".into());
        }
        let heap_live = heap()?;
        if heap_live[0] != heap_before[0]
            || heap_live[2] != heap_before[2]
            || heap_live[3] != heap_before[3]
        {
            return Err(format!(
                "dialog state should not allocate heap: before={heap_before:?}, live={heap_live:?}"
            )
            .into());
        }
        let state_heap_bytes = heap_before[0]
            .checked_sub(heap_live[0])
            .ok_or("firmware heap free-byte count increased while allocating dialog state")?;
        if state_heap_bytes != 0 {
            return Err(format!(
                "dialog state should not consume heap bytes, got {state_heap_bytes}"
            )
            .into());
        }
        let null_bitmap_background = background_pointer(47)?;
        if u16::from_le_bytes(
            qmp.read_memory(0, null_bitmap_background + 2, 2)?
                .try_into()
                .unwrap(),
        ) != 0
        {
            return Err("background 47 no longer uses bitmap ID 0".into());
        }
        let before_null_bitmap_draw = capture(50, "window-scene-before-null-bitmap-probe")?;
        qmp.stop()?;
        debugger.call_thumb(0x8000_5e70, &[47], Duration::from_secs(5))?;
        qmp.resume()?;
        let after_null_bitmap_draw = capture(100, "window-scene-after-null-bitmap-probe")?;
        if before_null_bitmap_draw != after_null_bitmap_draw
            || u16::from_le_bytes(qmp.read_memory(0, state, 2)?.try_into().unwrap()) != 0x4b4e
            || qmp.read_memory(0, 0x80207458, 4)? != original_bitmap
        {
            return Err(
                "bitmap ID 0 did not safely short-circuit while holding the state handle".into(),
            );
        }
        let value_text = decode_text(text_table + 179 * 4)?;
        let raw = qmp.read_memory(0, 0x8045ff84 + 0x120, 1)?[0];
        if !knob_display_matches(9, &expected_value(9, raw), &value_text) {
            return Err(format!("custom window value mismatch: {value_text:?}").into());
        }
        let refreshed = capture(500, "window-scene-refreshed")?;
        if active != refreshed {
            return Err("recorder redraw overwrote the custom overlay layers".into());
        }

        // Hold the notification semaphore while a second value is rendered.
        // The patch's dedicated layer/widget resources and persistent bitmap
        // descriptor must remain usable without taking this unrelated lock.
        qmp.stop()?;
        let semaphore_count = qmp.read_memory(0, notification_semaphore + 56, 4)?;
        if semaphore_count != 1u32.to_le_bytes() {
            qmp.resume()?;
            return Err(
                "notification semaphore was not available before callback contention check".into(),
            );
        }
        debugger.write(notification_semaphore + 56, &0u32.to_le_bytes())?;
        qmp.resume()?;
        inputs.knob(0, -1)?;
        let callback_busy_start = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            callback_busy_start,
            300,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        qmp.stop()?;
        let pending_during_contention = qmp.read_memory(0, state + 3, 1)?;
        let lock_held_during_render =
            qmp.read_memory(0, notification_semaphore + 56, 4)? == 0u32.to_le_bytes();
        let dialog_value_during_lock = decode_text(text_table + 179 * 4)?;
        let raw_during_lock = qmp.read_memory(0, 0x8045ff84 + 0x120, 1)?[0];
        debugger.write(notification_semaphore + 56, &semaphore_count)?;
        qmp.resume()?;
        if pending_during_contention != u8::MAX.to_le_bytes()
            || !lock_held_during_render
            || !knob_display_matches(
                9,
                &expected_value(9, raw_during_lock),
                &dialog_value_during_lock,
            )
        {
            return Err(format!(
                "dialog did not process the latest update while the notification semaphore was unavailable: pending={pending_during_contention:02x?}, value={dialog_value_during_lock:?}"
            )
            .into());
        }
        let mut previous_mode_frame = capture(500, "window-scene-after-callback-contention")?;
        let pending_after_contention = qmp.read_memory(0, state + 3, 1)?;
        if pending_after_contention != u8::MAX.to_le_bytes() {
            return Err(format!(
                "display callback did not consume the pending update while the lock was held: pending={pending_after_contention:02x?}"
            )
            .into());
        }
        for (mode, button, name) in [
            (9, 35, "level"),
            (0, 28, "high"),
            (1, 33, "freq"),
            (2, 38, "mid"),
            (3, 43, "low"),
            (4, 10, "aux1"),
            (5, 15, "aux2"),
            (6, 20, "efx"),
            (7, 25, "sub-mix"),
            (8, 30, "pan"),
        ] {
            let selection_applied = inputs.panel.snapshot().applied;
            inputs.tap(button, 50)?;
            wait_l6_input(
                session,
                || inputs.panel.snapshot().applied > selection_applied,
                "knob-dialog parameter-selection input acknowledgement",
            )?;
            let selection = inputs.panel.snapshot().guest_ms;
            wait_guest(session, selection, 300, guest_timeout_ms, wall_timeout_s)?;
            let encoder_applied = inputs.panel.snapshot().applied;
            inputs.knob(0, -1)?;
            wait_l6_input(
                session,
                || inputs.panel.snapshot().applied > encoder_applied,
                "knob-dialog parameter encoder acknowledgement",
            )?;
            let frame = capture(400, &format!("window-scene-{name}"))?;
            let offset = [2, 5, 4, 6, 15, 16, 12, 9, 8, 0x120][mode as usize];
            let raw = qmp.read_memory(0, 0x8045ff84 + offset, 1)?[0];
            let displayed = decode_text(text_table + 179 * 4)?;
            if !knob_display_matches(mode, &expected_value(mode, raw), &displayed) {
                return Err(format!(
                    "scene mode {name} displayed {displayed:?} for raw value {raw}"
                )
                .into());
            }
            if mode != 9
                && !(0..17).any(|y| {
                    let index = ((63 - y) / 8) * 128;
                    frame[index..index + 128] != previous_mode_frame[index..index + 128]
                })
            {
                return Err(format!("scene title did not change for mode {name}").into());
            }
            let mode_heap = heap()?;
            if mode_heap[2] != heap_live[2] || mode_heap[3] != heap_live[3] {
                return Err(format!(
                    "mode {name} changed the persistent heap allocation: {mode_heap:?}"
                )
                .into());
            }
            previous_mode_frame = frame;
        }
        // The preceding mode walk ends on Pan with a verified live scene.
        // Sweep from center through left 50% to right 50%, capturing at 20 fps.
        capture_demo("knob-dialog-demo-pan-center")?;
        if decode_text(text_table + 179 * 4)? != "CENTER" {
            return Err("demo pan sweep did not start at center".into());
        }
        const PAN_VALUE: u32 = 0x8045_ff84 + 8;
        let pan_start = qmp.read_memory(0, PAN_VALUE, 1)?[0];
        for name in ["left", "right"] {
            let mut reached_target = false;
            for index in 0..100 {
                let raw = qmp.read_memory(0, PAN_VALUE, 1)?[0];
                let displayed = decode_text(text_table + 179 * 4)?;
                let target_reached = if name == "left" {
                    displayed
                        .strip_prefix("L ")
                        .and_then(|value| value.strip_suffix('%'))
                        .and_then(|value| value.parse::<u8>().ok())
                        .is_some_and(|value| value >= 48)
                } else {
                    displayed
                        .strip_prefix("R ")
                        .and_then(|value| value.strip_suffix('%'))
                        .and_then(|value| value.parse::<u8>().ok())
                        .is_some_and(|value| value >= 48)
                };
                if target_reached {
                    reached_target = true;
                    break;
                }
                let target = if name == "left" { 32 } else { 95 };
                inputs.knob(0, if raw < target { 1 } else { -1 })?;
                capture_demo(&format!("knob-dialog-demo-pan-{name}-{index:02}"))?;
            }
            if !reached_target {
                return Err(format!("demo pan sweep did not reach {name} 50%").into());
            }
            capture(500, &format!("knob-dialog-demo-pan-{name}-end"))?;
        }
        for _ in 0..100 {
            let raw = qmp.read_memory(0, PAN_VALUE, 1)?[0];
            if raw == pan_start {
                break;
            }
            inputs.knob(0, if raw < pan_start { 1 } else { -1 })?;
            capture(50, "knob-dialog-demo-pan-reset")?;
        }
        inputs.tap(35, 50)?;
        let channel_selection = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            channel_selection,
            300,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        let channel0_level = 0x804600a4;
        let channel1_level = 0x804600a5;
        let channel0_raw = qmp.read_memory(0, channel0_level, 1)?[0];
        let mut channel1_raw = qmp.read_memory(0, channel1_level, 1)?[0];
        for delta in [-1, 1, -2, 2] {
            inputs.knob(1, delta)?;
            capture(250, "window-scene-channel-2-level")?;
            channel1_raw = qmp.read_memory(0, channel1_level, 1)?[0];
            if channel1_raw != channel0_raw {
                break;
            }
        }
        let channel1_display = decode_text(text_table + 179 * 4)?;
        if channel1_raw == channel0_raw
            || !knob_display_matches(9, &expected_value(9, channel1_raw), &channel1_display)
        {
            return Err(format!(
                "channel-2 event did not display its own LEVEL value: channel0={channel0_raw}, channel1={channel1_raw}, displayed={channel1_display:?}"
            )
            .into());
        }
        for (tag, description) in [(0x0f, "invalid mode 15"), (0xf0, "invalid channel 15")] {
            qmp.stop()?;
            debugger.write(state + 3, &[tag])?;
            qmp.resume()?;
            let invalid_tag_start = inputs.panel.snapshot().guest_ms;
            wait_guest(
                session,
                invalid_tag_start,
                250,
                guest_timeout_ms,
                wall_timeout_s,
            )?;
            qmp.stop()?;
            let discarded = qmp.read_memory(0, state + 3, 1)? == u8::MAX.to_le_bytes();
            qmp.resume()?;
            if !discarded {
                return Err(
                    format!("display callback did not safely discard {description}").into(),
                );
            }
        }
        inputs.knob(0, -1)?;
        capture(100, "window-scene-before-invalid-state-metadata")?;
        qmp.stop()?;
        let metadata = qmp.read_memory(0, state, 4)?;
        let metadata_word = u32::from_le_bytes(metadata.clone().try_into().unwrap());
        let invalid_metadata = metadata_word ^ 1;
        debugger.write(state, &invalid_metadata.to_le_bytes())?;
        qmp.resume()?;
        let invalid_metadata_start = inputs.panel.snapshot().guest_ms;
        wait_guest(
            session,
            invalid_metadata_start,
            150,
            guest_timeout_ms,
            wall_timeout_s,
        )?;
        qmp.stop()?;
        let invalid_metadata_rejected = qmp.read_memory(0, state, 4)?
            == invalid_metadata.to_le_bytes()
            && qmp.read_memory(0, 0x80207458, 4)? == original_bitmap
            && [widget7, widget33, background10, background37, background48]
                .into_iter()
                .all(|descriptor| {
                    qmp.read_memory(0, descriptor, 1)
                        .is_ok_and(|layer| layer[0] == 4)
                });
        debugger.write(state, &metadata)?;
        qmp.resume()?;
        if !invalid_metadata_rejected {
            return Err("invalid state signature changed an overlay descriptor".into());
        }
        inputs.knob(0, -1)?;
        capture(100, "window-scene-after-invalid-state-metadata")?;
        let invalid_tag_discarded = true;
        let lit_pixels = |y0: usize, y1: usize| {
            (y0..y1)
                .flat_map(|y| {
                    (0..128).map(move |x| {
                        let index = ((63 - y) / 8) * 128 + x;
                        refreshed[index] & (1 << ((63 - y) % 8)) != 0
                    })
                })
                .filter(|lit| *lit)
                .count()
        };
        if lit_pixels(0, 17) < 10 || lit_pixels(30, 48) < 5 {
            return Err("shared text resource did not render both dialog lines".into());
        }
        let scene_text_before_notification = qmp.read_memory(0, shared_resource_pointer, 512)?;
        // Exercise the stock notification renderer over the layer-4 scene.
        // This calls the generic firmware API directly, without relying on
        // the patch's removed notification hooks.
        qmp.stop()?;
        let callback_entry = active_slot5 & !1;
        debugger.breakpoint(callback_entry, true)?;
        qmp.resume()?;
        let breakpoint_deadline = Instant::now() + Duration::from_secs(15);
        while qmp.query_status()?.running {
            session.poll()?;
            if Instant::now() >= breakpoint_deadline {
                return Err("dialog callback breakpoint was not reached".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        debugger.breakpoint(callback_entry, false)?;
        // Message 4 uses the three-line notification background, bitmap 36.
        debugger.register(0, 4)?;
        debugger.register(15, 0x8000_6660)?;
        qmp.resume()?;
        let notification_start = inputs.panel.snapshot().guest_ms;
        let mut notification_frame = capture(100, "window-scene-stock-notification")?;
        while notification_frame == refreshed {
            let elapsed = inputs
                .panel
                .snapshot()
                .guest_ms
                .wrapping_sub(notification_start);
            if elapsed >= 1_500 {
                return Err(
                    "stock notification did not render its frame over the custom scene".into(),
                );
            }
            notification_frame = capture(100, "window-scene-stock-notification")?;
        }
        let callback_after_notification = qmp.read_memory(0, 0x8020a59c + 20, 4)?;
        let resource_pointer_after_notification = qmp.read_memory(0, text_table + 179 * 4, 4)?;
        let resource_after_notification = qmp.read_memory(0, shared_resource_pointer, 512)?;
        if callback_after_notification != active_slot5.to_le_bytes()
            || resource_pointer_after_notification != shared_resource_pointer.to_le_bytes()
            || resource_after_notification != scene_text_before_notification
        {
            return Err(format!(
                "stock notification changed scene state: callback {callback_after_notification:02x?} vs {active_slot5:08x}, resource pointer {resource_pointer_after_notification:02x?} vs {shared_resource_pointer:08x}, resource modified={}",
                resource_after_notification != scene_text_before_notification
            ).into());
        }
        // The dialog has its own inactivity timeout. Wait for the notification
        // frame to clear instead of sleeping past that timeout as well.
        let after_notification = loop {
            let frame = capture(100, "window-scene-after-notification")?;
            if frame != notification_frame {
                break frame;
            }
            let elapsed = inputs
                .panel
                .snapshot()
                .guest_ms
                .wrapping_sub(notification_start);
            if elapsed >= 5_000 {
                return Err("stock notification did not clear after its timeout".into());
            }
        };
        let callback_after_expiry = qmp.read_memory(0, 0x8020a59c + 20, 4)?;
        let resource_pointer_after_expiry = qmp.read_memory(0, text_table + 179 * 4, 4)?;
        let resource_after_expiry = qmp.read_memory(0, shared_resource_pointer, 512)?;
        if callback_after_expiry != active_slot5.to_le_bytes()
            || resource_pointer_after_expiry != shared_resource_pointer.to_le_bytes()
            || resource_after_expiry != scene_text_before_notification
        {
            return Err(format!(
                "stock notification expiry damaged or prematurely removed the custom scene: callback {callback_after_expiry:02x?} vs {active_slot5:08x}, resource pointer {resource_pointer_after_expiry:02x?} vs {shared_resource_pointer:08x}, resource modified={}, guest ms since notification {}",
                resource_after_expiry != scene_text_before_notification,
                inputs.panel.snapshot().guest_ms.wrapping_sub(notification_start)
            ).into());
        }
        let timeout_start = inputs.panel.snapshot().guest_ms;
        loop {
            let ticks = qmp.read_memory(0, state + 2, 1)?[0];
            if ticks == 0 {
                break;
            }
            let now = inputs.panel.snapshot().guest_ms;
            if now.wrapping_sub(timeout_start) >= 5_000 {
                return Err(format!(
                    "custom scene timeout did not finish after notification expiry; {ticks} ticks remain"
                )
                .into());
            }
            wait_guest(session, now, 100, guest_timeout_ms, wall_timeout_s)?;
        }
        let timeout_cleared = capture(150, "knob-dialog-timeout-cleared")?;
        if timeout_cleared == after_notification {
            return Err("knob dialog timeout screenshot still shows the custom scene".into());
        }
        if timeout_cleared == notification_frame {
            return Err("knob dialog timeout screenshot still shows the notification".into());
        }
        let restore_checks = [
            (
                "window",
                qmp.read_memory(0, 0x80202610, 4)? == original_window,
            ),
            (
                "state signature",
                u16::from_le_bytes(qmp.read_memory(0, state, 2)?.try_into().unwrap()) == 0x4b4e,
            ),
            (
                "bitmap table",
                qmp.read_memory(0, 0x80207458, 4)? == original_bitmap,
            ),
            (
                "bitmap descriptor",
                qmp.read_memory(
                    0,
                    u32::from_le_bytes(original_bitmap.clone().try_into().unwrap()),
                    8,
                )? == original_bitmap_descriptor,
            ),
            (
                "notification semaphore",
                qmp.read_memory(0, 0x8020a2a0, 4)? == original_notification_semaphore,
            ),
            (
                "text resource table",
                qmp.read_memory(0, text_table + 229 * 4, 8)? == original_resources,
            ),
            (
                "shared resource pointer",
                qmp.read_memory(0, text_table + 179 * 4, 4)?
                    == shared_resource_pointer.to_le_bytes(),
            ),
            (
                "shared resource contents",
                qmp.read_memory(0, shared_resource_pointer, 512)? == original_shared_resource,
            ),
            (
                "text lengths",
                qmp.read_memory(0, original_lengths_table + 229 * 2, 4)? == original_lengths,
            ),
            (
                "widget 7",
                qmp.read_memory(0, widget7, 16)? == original_widget7,
            ),
            (
                "widget 33",
                qmp.read_memory(0, widget33, 16)? == original_widget33,
            ),
            (
                "background layer 10",
                qmp.read_memory(0, background10, 1)? == original_background_layers[0],
            ),
            (
                "background layer 37",
                qmp.read_memory(0, background37, 1)? == original_background_layers[1],
            ),
            (
                "background layer 48",
                qmp.read_memory(0, background48, 1)? == original_background_layers[2],
            ),
            (
                "display callback slot 5",
                u32::from_le_bytes(qmp.read_memory(0, 0x8020_a59c + 20, 4)?.try_into().unwrap())
                    == active_slot5,
            ),
        ];
        let mismatches = restore_checks
            .iter()
            .filter_map(|(name, matches)| (!matches).then_some(*name))
            .collect::<Vec<_>>();
        if !mismatches.is_empty() {
            return Err(format!(
                "custom overlay did not restore borrowed resources or preserve stock bitmap 36: {mismatches:?}"
            )
            .into());
        }
        let heap_after = heap()?;
        if heap_after[0] != heap_live[0]
            || heap_after[2] != heap_live[2]
            || heap_after[3] != heap_live[3]
        {
            return Err(format!(
                "in-place scene state caused heap changes: live={heap_live:?}, after={heap_after:?}"
            )
            .into());
        }
        // Prove that the DisplayCycle2 callback restores borrowed global
        // descriptors on navigation, rather than relying on expiry cleanup.
        inputs.knob(0, -3)?;
        capture(400, "window-scene-before-navigation")?;
        inputs.main.tap(0, 120)?;
        capture(500, "window-scene-navigation-returned")?;
        let navigated_window = qmp.read_memory(0, 0x80202610, 4)?;
        if navigated_window == original_window
            || u16::from_le_bytes(qmp.read_memory(0, state, 2)?.try_into().unwrap()) != 0x4b4e
            || qmp.read_memory(0, 0x80207458, 4)? != original_bitmap
            || qmp.read_memory(
                0,
                u32::from_le_bytes(original_bitmap.clone().try_into().unwrap()),
                8,
            )? != original_bitmap_descriptor
            || qmp.read_memory(0, 0x8020a2a0, 4)? != original_notification_semaphore
            || qmp.read_memory(0, shared_resource_pointer, 512)? != original_shared_resource
            || qmp.read_memory(0, widget7, 16)? != original_widget7
            || qmp.read_memory(0, widget33, 16)? != original_widget33
            || qmp.read_memory(0, background10, 1)? != original_background_layers[0]
            || qmp.read_memory(0, background37, 1)? != original_background_layers[1]
            || qmp.read_memory(0, background48, 1)? != original_background_layers[2]
            || u32::from_le_bytes(qmp.read_memory(0, 0x8020_a59c + 20, 4)?.try_into().unwrap())
                != active_slot5
        {
            return Err(
                "window-push cleanup did not restore the dialog's borrowed resources".into(),
            );
        }
        return Ok(json!({
            "overlay_layer": 4,
            "black_background": true,
            "notification_semaphore_not_required_for_first_use": true,
            "callback_preflight_race_rejected": callback_preflight_race_rejected,
            "notification_semaphore_not_required_for_refresh": true,
            "stock_notification_overlap": true,
            "stock_notification_expiry": true,
            "bitmap_zero_guard_safe": true,
            "timeout_return": true,
            "window_change_restore": true,
            "non_recorder_first_use_rejected": non_recorder_guard_passed,
            "dialog_active_screenshot": "knob-dialog-active.png",
            "dialog_timeout_screenshot": "knob-dialog-timeout-cleared.png",
            "resource_restore": true,
            "shared_resource_reuse": true,
            "shared_resource_alias_rejected": true,
            "scene_layer_collision_rejected": true,
            "changed_widget_layer_rejected": widget_layer_guard_rejected,
            "changed_background_layer_rejected": background_layer_guard_rejected,
            "nonzero_channel_value_matches": true,
            "invalid_mailbox_tag_discarded": invalid_tag_discarded,
            "invalid_state_signature_rejected": invalid_metadata_rejected,
            "stock_notification_bitmap_untouched": true,
            "state_allocation_requested_bytes": 0,
            "state_heap_bytes_consumed": state_heap_bytes,
            "state_retained_for_boot": true,
            "display_callback_slot": 5,
            "display_callback_target": format!("{active_slot5:#010x}"),
            "mixer_hook_task_control_block": format!("{hook_task:#010x}"),
            "display_callback_task_control_block": format!("{callback_task:#010x}"),
            "mixer_and_callback_share_task": hook_task == callback_task,
            "display_callback_remains_registered": true
        }));
    }
    let mut reports = 0u64;
    for (mode, button, name) in [
        (9, 35, "level"),
        (0, 28, "high"),
        (1, 33, "freq"),
        (2, 38, "mid"),
        (3, 43, "low"),
        (4, 10, "aux1"),
        (5, 15, "aux2"),
        (6, 20, "efx"),
        (7, 25, "sub-mix"),
        (8, 30, "pan"),
    ] {
        inputs.tap(button, 50)?;
        let pressed = inputs.panel.snapshot().guest_ms;
        wait_guest(session, pressed, 500, guest_timeout_ms, wall_timeout_s)?;
        let baseline = capture(150, &format!("{name}-before"))?;
        let heap_before = heap()?;
        inputs.knob(0, -3)?;
        reports += 3;
        let frame = capture(700, name)?;
        if !frame.iter().any(|b| *b != 0) {
            return Err(format!("{name}: empty display frame").into());
        }
        if qmp.read_memory(0, 0x8020a2c0, 1)?[0] != 1 {
            return Err(format!("{name}: knob did not show firmware notification").into());
        }
        let heap_live = heap()?;
        if heap_live[2] != heap_before[2] + 1 || heap_live[3] != heap_before[3] {
            return Err(format!(
                "{name}: popup heap allocation mismatch: before={heap_before:?}, live={heap_live:?}, window={:?}, bitmap={:?}",
                qmp.read_memory(0, 0x80202610, 4)?,
                qmp.read_memory(0, 0x80207458, 4)?
            )
            .into());
        }
        let allocation = u32::from_le_bytes(qmp.read_memory(0, 0x80207458, 4)?.try_into().unwrap());
        if !(0x808a4150..0x80921140).contains(&allocation) {
            return Err(format!("{name}: popup state outside firmware heap").into());
        }
        let allocated_size =
            u32::from_le_bytes(qmp.read_memory(0, allocation - 4, 4)?.try_into().unwrap());
        if allocated_size & 0x80000000 == 0
            || qmp.read_memory(0, allocation + 8, 4)? != 0x4b4e4f42u32.to_le_bytes()
        {
            return Err(format!("{name}: popup allocator ownership marker invalid").into());
        }
        if text_resource(0)? != name.to_uppercase() {
            return Err(format!("{name}: wrong title").into());
        }
        let offset = [2, 5, 4, 6, 15, 16, 12, 9, 8, 0x120][mode as usize];
        let raw = qmp.read_memory(0, 0x8045ff84 + offset, 1)?[0];
        if !knob_display_matches(mode, &expected_value(mode, raw), &text_resource(1)?)
            || !text_resource(2)?.is_empty()
        {
            return Err(format!("{name}: formatted value/channel line mismatch").into());
        }
        if qmp.read_memory(0, 0x8020099c, 2)? != 32u16.to_le_bytes() {
            return Err(format!("{name}: value not vertically centered").into());
        }
        inputs.knob(0, 2)?;
        reports += 2;
        let changed = capture(500, &format!("{name}-changed"))?;
        if frame == changed {
            return Err(format!("{name}: value did not update").into());
        }
        let mut matched = false;
        let mut last_observed = None;
        for _ in 0..20 {
            let raw = qmp.read_memory(0, 0x8045ff84 + offset, 1)?[0];
            let expected = expected_value(mode, raw);
            let displayed = text_resource(1)?;
            last_observed = Some((raw, expected.clone(), displayed.clone()));
            if knob_display_matches(mode, &expected, &displayed) {
                matched = true;
                break;
            }
            let tick = inputs.panel.snapshot().guest_ms;
            wait_guest(session, tick, 50, guest_timeout_ms, wall_timeout_s)?;
        }
        if !matched {
            let (raw, expected, displayed) = last_observed.unwrap();
            return Err(format!(
                "{name}: value did not settle to firmware state (raw={raw}, expected={expected:?}, displayed={displayed:?})"
            )
            .into());
        }
        let refreshed_heap = heap()?;
        if refreshed_heap[2] != heap_live[2] {
            return Err(format!("{name}: popup refresh allocated another block ({heap_live:?} -> {refreshed_heap:?})").into());
        }
        wait_knob_popup_expiry(session, guest_timeout_ms, wall_timeout_s)?;
        let expired = capture(150, &format!("{name}-expired"))?;
        for y in 9..16usize {
            for x in 9..119usize {
                let index = ((63 - y) / 8) * 128 + x;
                let mask = 1 << ((63 - y) % 8);
                if expired[index] & mask != baseline[index] & mask {
                    return Err(
                        format!("{name}: title left pixel at ({x},{y}) after expiry").into(),
                    );
                }
            }
        }
        if qmp.read_memory(0, 0x802059b8, 12)? != original_resources
            || qmp.read_memory(0, 0x80207458, 4)? != original_bitmap
            || qmp.read_memory(0, 0x8020098c, 6)? != original_header
            || qmp.read_memory(0, 0x8020099c, 2)? != original_value_y
            || qmp.read_memory(0, 0x80200986, 2)? != original_polarity
        {
            return Err(format!("{name}: Done widget resources/layout were not restored").into());
        }
        let heap_after = heap()?;
        if heap_after[3] != heap_before[3] + 1
            || heap_after[0] != heap_before[0]
            || qmp.read_memory(0, lengths_table + 229 * 2, 6)? != original_lengths
        {
            return Err(format!("{name}: popup leak or original capacities not restored").into());
        }
        for (address, bytes) in &original_text {
            if qmp.read_memory(0, *address, bytes.len())? != *bytes {
                return Err(format!("{name}: original Done text overwritten").into());
            }
        }
    }
    inputs.tap(35, 50)?;
    let pressed = inputs.panel.snapshot().guest_ms;
    wait_guest(session, pressed, 150, guest_timeout_ms, wall_timeout_s)?;
    inputs.knob(1, -1)?;
    reports += 1;
    capture(400, "channel-2-level")?;
    if text_resource(0)? != "LEVEL" {
        return Err("channel-2 encoder did not update through notification".into());
    }
    inputs.tap(28, 50)?;
    let pressed = inputs.panel.snapshot().guest_ms;
    wait_guest(session, pressed, 150, guest_timeout_ms, wall_timeout_s)?;
    inputs.knob(1, -1)?;
    reports += 1;
    capture(400, "channel-2-high")?;
    if text_resource(0)? != "HIGH" {
        return Err("blue-strip button did not pass through notification".into());
    }
    wait_knob_popup_expiry(session, guest_timeout_ms, wall_timeout_s)?;
    if qmp.read_memory(0, 0x802059b8, 12)? != original_resources {
        return Err("final popup did not restore Done resources".into());
    }
    let actual = fs::read_to_string(options.logs.join("engine.trace"))?
        .matches("l6 panel USART1 TX a1\n")
        .count() as u64
        - encoder_report_baseline;
    if actual != reports {
        return Err(format!("encoder report count {actual}, expected {reports}").into());
    }
    if before[0] == rings(inputs.panel.snapshot().indicators)[0] {
        return Err("channel 1 ring did not change".into());
    }
    if qmp.read_memory(0, 0x841ff000, 4096)? != abandoned_page {
        return Err("patch wrote abandoned RAM page".into());
    }
    Ok(
        json!({"modes":10,"popup_updates":true,"guest_timed_expiry":true,"encoder_reports":actual,"heap_cleanup":true}),
    )
}

fn knob_display_matches(mode: u32, expected: &str, displayed: &str) -> bool {
    if expected == displayed {
        return true;
    }
    if mode != 1 {
        return false;
    }
    let Some((expected_number, expected_unit)) = expected.rsplit_once(' ') else {
        return false;
    };
    let Some((displayed_number, displayed_unit)) = displayed.rsplit_once(' ') else {
        return false;
    };
    if expected_unit != displayed_unit {
        return false;
    }
    let (Ok(expected), Ok(displayed)) = (
        expected_number.parse::<f32>(),
        displayed_number.parse::<f32>(),
    ) else {
        return false;
    };
    let tolerance = match expected_unit {
        "Hz" => 1.0,
        "kHz" => 0.01,
        _ => return false,
    };
    (expected - displayed).abs() <= tolerance
}

fn wait_guest(
    session: &Session,
    start: u32,
    duration: u32,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let wall = Instant::now();
    loop {
        session.poll()?;
        let elapsed = session
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .panel
            .snapshot()
            .guest_ms
            .wrapping_sub(start);
        if elapsed >= duration {
            return Ok(());
        }
        if elapsed > guest_timeout_ms || wall.elapsed() > Duration::from_secs(wall_timeout_s) {
            return Err("knob-dialog setup exceeded its deadline".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_knob_popup_expiry(
    session: &Session,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let qmp = &session.engine.management[0];
    let start = session
        .engine
        .inputs
        .as_ref()
        .unwrap()
        .panel
        .snapshot()
        .guest_ms;
    let wall = Instant::now();
    let mut previous_busy = false;
    let mut previous_elapsed = 0u16;
    let mut observed_busy_pause = false;
    loop {
        session.poll()?;
        let state = qmp.read_memory(0, 0x8020a2c0, 10)?;
        let busy = state[8] != 0;
        let elapsed = u16::from_le_bytes([state[4], state[5]]);
        if busy {
            observed_busy_pause = true;
            if previous_busy && elapsed != previous_elapsed {
                return Err("busy overlay advanced popup timer".into());
            }
        }
        previous_busy = busy;
        previous_elapsed = elapsed;
        if state[0] == 0 {
            return Ok(());
        }
        let guest = session
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .panel
            .snapshot()
            .guest_ms
            .wrapping_sub(start);
        if guest > guest_timeout_ms || wall.elapsed() > Duration::from_secs(wall_timeout_s) {
            return Err(
                format!("popup did not expire; busy pause observed={observed_busy_pause}").into(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn run_l6_din_event_dispatch(
    session: &Session,
    options: &Options,
    group: u8,
    cc: u8,
    value: u8,
) -> Result<Value, Box<dyn std::error::Error>> {
    use l6max_diagnostics::gdb::Gdb;
    use std::io;

    const IDLE: u32 = 0x80046d50;
    const MAIN_TASK: u32 = 0x8006dd70;
    const MAIN_WAIT_CALL: u32 = 0x8002c216;
    const MAIN_WAIT_RETURN: u32 = 0x8002c21a;
    const MAIN_STATE_RETURN: u32 = 0x8002c21e;
    const MAIN_QUEUE_READER: u32 = 0x80020690;
    const MAIN_WAIT_GIVE: u32 = 0x800763d8;
    const DIN_EVENT_SUBMIT: u32 = 0x80020ca0;
    const CLASS2_HANDLER: u32 = 0x8002c510;
    const EQ_SETTER: u32 = 0x8003e408;
    const CHANNEL_SETTERS: [u32; 9] = [
        0x8003e408, 0x8003e340, 0x8003e408, 0x8003e408, 0x8003d860, 0x8003d860, 0x8003e0c8,
        0x8003db10, 0x8003da58,
    ];
    let qmp = &session.engine.management[0];
    let socket = options
        .gdb_socket
        .as_ref()
        .ok_or("l6-din-event-dispatch requires a GDB socket")?;
    let mut gdb = Gdb::connect(socket)?;
    tracing::info!(
        event = "l6_event_dispatch_stage",
        stage = "gdb_connected",
        "L6 event-dispatch debugger connected"
    );
    let read_state = || -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
        Ok((
            qmp.read_memory(0, 0x80440dca, 0x7e)?,
            qmp.read_memory(0, 0x80440e9a, 6)?,
        ))
    };
    let changed_bytes = |before: &(Vec<u8>, Vec<u8>), after: &(Vec<u8>, Vec<u8>)| {
        before
            .0
            .iter()
            .zip(&after.0)
            .chain(before.1.iter().zip(&after.1))
            .filter(|(old, new)| old != new)
            .count()
    };
    let pc = |gdb: &mut Gdb| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(gdb.read_register(15)? & !1)
    };

    gdb.breakpoint(MAIN_TASK, true)?;
    tracing::info!(
        event = "l6_event_dispatch_stage",
        stage = "wait_main_task",
        "Waiting for L6 Main task entry"
    );
    gdb.continue_until_stop(Duration::from_secs(60))?;
    if pc(&mut gdb)? != MAIN_TASK {
        return Err(io::Error::other("L6 Main task entry breakpoint was missed").into());
    }
    gdb.breakpoint(MAIN_TASK, false)?;
    gdb.breakpoint(MAIN_WAIT_CALL, true)?;
    tracing::info!(
        event = "l6_event_dispatch_stage",
        stage = "wait_main_semaphore",
        "Waiting for L6 Main semaphore call"
    );
    if let Err(error) = gdb.continue_until_stop(Duration::from_secs(30)) {
        let stop = gdb
            .interrupt()
            .unwrap_or_else(|interrupt| format!("interrupt failed: {interrupt}"));
        let stopped_pc = gdb
            .read_register(15)
            .map(|register| format!("{:#x}", register & !1))
            .unwrap_or_else(|read| format!("unavailable: {read}"));
        return Err(io::Error::other(format!(
            "waiting for Main semaphore call failed: {error}; interrupt reply {stop}; PC {stopped_pc}"
        ))
        .into());
    }
    if pc(&mut gdb)? != MAIN_WAIT_CALL || gdb.read_register(1)? != u32::MAX {
        return Err(io::Error::other("L6 Main did not enter its infinite semaphore wait").into());
    }
    let main_wait = gdb.read_register(0)?;
    gdb.breakpoint(MAIN_WAIT_CALL, false)?;
    gdb.breakpoint(MAIN_WAIT_RETURN, true)?;
    gdb.breakpoint(IDLE, true)?;
    tracing::info!(
        event = "l6_event_dispatch_stage",
        stage = "wait_main_block",
        "Waiting for Main to block before event injection"
    );
    gdb.continue_until_stop(Duration::from_secs(10))?;
    if pc(&mut gdb)? != IDLE {
        return Err(io::Error::other("L6 Main did not block before DIN injection").into());
    }
    let before = read_state()?;
    tracing::info!(
        event = "l6_event_dispatch_stage",
        stage = "inject_din_event",
        "Injecting L6 DIN event"
    );

    let stack = gdb.read_register(13)?;
    gdb.write(stack, &0u32.to_le_bytes())?;
    gdb.register(0, 2)?;
    gdb.register(1, u32::from(group))?;
    gdb.register(2, u32::from(cc))?;
    gdb.register(3, u32::from(value))?;
    gdb.register(14, IDLE | 1)?;
    gdb.register(15, DIN_EVENT_SUBMIT)?;
    gdb.continue_until_stop(Duration::from_secs(10))?;
    if pc(&mut gdb)? != IDLE {
        return Err(io::Error::other("L6 DIN event producer did not return to Idle").into());
    }

    gdb.register(0, main_wait)?;
    gdb.register(1, 0)?;
    gdb.register(2, 0)?;
    gdb.register(3, 0)?;
    gdb.register(14, IDLE | 1)?;
    gdb.register(15, MAIN_WAIT_GIVE)?;
    gdb.continue_until_stop(Duration::from_secs(10))?;
    if pc(&mut gdb)? != MAIN_WAIT_RETURN {
        return Err(io::Error::other("L6 Main did not wake from its state wait").into());
    }
    gdb.breakpoint(MAIN_WAIT_RETURN, false)?;
    gdb.breakpoint(MAIN_STATE_RETURN, true)?;
    gdb.continue_until_stop(Duration::from_secs(10))?;
    if pc(&mut gdb)? != MAIN_STATE_RETURN {
        return Err(io::Error::other("L6 Main state-selector return was missed").into());
    }
    let state = gdb.read_register(0)?;
    gdb.breakpoint(MAIN_STATE_RETURN, false)?;
    gdb.breakpoint(IDLE, false)?;
    gdb.breakpoint(MAIN_QUEUE_READER, true)?;
    tracing::info!(
        event = "l6_event_dispatch_stage",
        stage = "wait_main_queue_reader",
        "Waiting for Main event queue reader"
    );
    for _ in 0..8 {
        gdb.continue_until_stop(Duration::from_secs(30))?;
        let stopped = pc(&mut gdb)?;
        if stopped == MAIN_QUEUE_READER {
            break;
        }
        if stopped == IDLE {
            gdb.breakpoint(IDLE, false)?;
        } else {
            return Err(io::Error::other(format!(
                "L6 Main initialization stopped at {stopped:#x}"
            ))
            .into());
        }
    }
    if pc(&mut gdb)? != MAIN_QUEUE_READER {
        return Err(io::Error::other("L6 Main did not reach its queue reader").into());
    }
    gdb.breakpoint(MAIN_QUEUE_READER, false)?;
    gdb.breakpoint(CLASS2_HANDLER, true)?;
    tracing::info!(
        event = "l6_event_dispatch_stage",
        stage = "wait_class2_handler",
        "Waiting for class-2 event handler"
    );
    let mut observed_event = false;
    for _ in 0..8 {
        gdb.continue_until_stop(Duration::from_secs(30))?;
        let stopped = pc(&mut gdb)?;
        if stopped == CLASS2_HANDLER {
            let address = gdb.read_register(0)?;
            let bytes = qmp.read_memory(0, address, 20)?;
            let words = bytes
                .chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                .collect::<Vec<_>>();
            if words == [2, u32::from(group), u32::from(cc), u32::from(value), 0] {
                observed_event = true;
                break;
            }
        } else if stopped != MAIN_QUEUE_READER {
            return Err(io::Error::other(format!("L6 event queue stopped at {stopped:#x}")).into());
        }
    }
    if !observed_event {
        return Err(io::Error::other("L6 Main did not dispatch the injected DIN event").into());
    }
    gdb.breakpoint(CLASS2_HANDLER, false)?;
    gdb.breakpoint(EQ_SETTER, true)?;
    gdb.continue_until_stop(Duration::from_secs(10))?;
    if pc(&mut gdb)? != EQ_SETTER {
        return Err(io::Error::other("queued DIN CC did not reach the EQ setter").into());
    }
    let stack = gdb.read_register(13)?;
    let setter_value = u32::from_le_bytes(qmp.read_memory(0, stack, 4)?.try_into().unwrap());
    gdb.breakpoint(EQ_SETTER, false)?;
    gdb.breakpoint(IDLE, true)?;
    gdb.continue_until_stop(Duration::from_secs(10))?;
    let after_event = read_state()?;
    let event_changed = changed_bytes(&before, &after_event);
    if setter_value != u32::from(value) || event_changed == 0 {
        return Err(io::Error::other(format!(
            "queued DIN CC expected setter value {value:#x} and a mixer-state change, got {setter_value:#x} and {event_changed} changed bytes"
        ))
        .into());
    }

    let mut setters = Vec::new();
    for (index, setter) in CHANNEL_SETTERS.into_iter().enumerate() {
        let before = read_state()?;
        let direct_value = 0x20 + index as u32;
        let stack = gdb.read_register(13)?;
        gdb.write(stack, &direct_value.to_le_bytes())?;
        gdb.register(0, 0)?;
        gdb.register(1, index as u32)?;
        gdb.register(2, 1)?;
        gdb.register(3, 0)?;
        gdb.register(14, IDLE | 1)?;
        gdb.register(15, setter)?;
        gdb.continue_until_stop(Duration::from_secs(10))?;
        if pc(&mut gdb)? != IDLE {
            return Err(io::Error::other(format!(
                "L6 setter for group {index} did not return to Idle"
            ))
            .into());
        }
        let changed = changed_bytes(&before, &read_state()?);
        if changed == 0 {
            return Err(io::Error::other(format!(
                "L6 setter for group {index} did not change mixer RAM"
            ))
            .into());
        }
        setters.push(json!({"group":index,"value":direct_value,"changed_bytes":changed}));
    }
    tracing::info!(
        event = "l6_din_event_dispatched",
        group,
        cc,
        value,
        state,
        event_changed,
        setter_value,
        "L6 DIN event reached the class-2 handler and mixer setter"
    );
    Ok(
        json!({"group":group,"cc":cc,"value":value,"main_state":state,"event_changed_bytes":event_changed,"setter_value":setter_value,"channel_setters":setters}),
    )
}

// Revision-specific catalog names retained from the qualified audio-path probe.
fn callbacks(model: l6fw::Model) -> &'static [(u32, &'static str)] {
    match model {
        l6fw::Model::L6 => &[
            (0x800335f8, "l6_sai1_rx_dma_callback"),
            (0x800180f8, "l6_audio_input_block_semaphore_callback"),
            (0x80012388, "l6_audio_staging_ring_producer"),
            (0x80010698, "l6_audio_process_block_consumer"),
            (0x8004fa98, "l6_dsp_stream_callback"),
        ],
        l6fw::Model::L6max => &[
            (0x800149b0, "audio_ring_consumer"),
            (0x80066180, "dsp_stream_callback"),
        ],
    }
}

fn verify_sai_edma_trace(
    model: l6fw::Model,
    trace: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let paths: &[(u32, u32, u32)] = match model {
        l6fw::Model::L6 => &[(0x4038_4000, 14, 15), (0x4038_c000, 12, 13)],
        l6fw::Model::L6max => &[(0x4038_4000, 14, 15)],
    };
    let timestamp = |line: &str| {
        line.split("at=")
            .nth(1)
            .and_then(|value| value.strip_suffix("ms"))
            .and_then(|value| value.parse::<u64>().ok())
    };
    let lines = trace.lines().collect::<Vec<_>>();
    let mut enable_times = Vec::new();
    for (base, _, _) in paths {
        for (direction, offset) in [("TX", 0x08), ("RX", 0x88)] {
            let address = base + offset;
            let enabled_at = lines
                .iter()
                .filter(|line| {
                    line.contains(&format!("write addr={address:08x}"))
                        && line.contains("value=80040001")
                })
                .filter_map(|line| timestamp(line))
                .max()
                .ok_or_else(|| {
                    format!(
                        "{} SAI at {base:#010x} did not enable {direction} FIFO DMA requests",
                        crate::scenario::model_name(model)
                    )
                })?;
            enable_times.push(enabled_at);
        }
    }
    let latest_enable = enable_times.into_iter().max().unwrap_or_default();
    let active = lines
        .iter()
        .copied()
        .filter(|line| timestamp(line).is_some_and(|time| time >= latest_enable))
        .collect::<Vec<_>>();
    let mut streams = Vec::new();
    for (base, tx_channel, rx_channel) in paths {
        for (direction, fifo_offset, channel) in
            [("TX", 0x20, tx_channel), ("RX", 0xa0, rx_channel)]
        {
            let fifo = base + fifo_offset;
            if !active
                .iter()
                .any(|line| line.contains(&format!("addr={fifo:08x}")))
            {
                return Err(format!(
                    "{} SAI at {base:#010x} had no post-enable {direction} FIFO traffic",
                    crate::scenario::model_name(model)
                )
                .into());
            }
            let tcd = 0x400e_9000 + channel * 0x20;
            if !active
                .iter()
                .any(|line| line.contains(&format!("addr={tcd:08x}")))
            {
                return Err(format!(
                    "{} SAI {direction} at {base:#010x} had no eDMA channel {channel} TCD activity",
                    crate::scenario::model_name(model)
                )
                .into());
            }
            streams.push(json!({
                "sai_base": format!("{base:#010x}"),
                "direction": direction,
                "fifo_address": format!("{fifo:#010x}"),
                "edma_channel": channel,
                "tcd_address": format!("{tcd:#010x}"),
            }));
        }
    }
    Ok(json!({"latest_enable_ms":latest_enable,"streams":streams}))
}

fn run_editor_pad_assignments(
    session: &Session,
    options: &Options,
    endpoints: l6max_host::usb::Endpoints,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    let usb = session
        .engine
        .usb
        .as_ref()
        .ok_or("USB backend unavailable")?
        .clone();
    let start_guest = session
        .engine
        .inputs
        .as_ref()
        .ok_or("editor-pad-assignments needs panel inputs")?
        .panel
        .snapshot()
        .guest_ms;
    let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    let mut encoder = l6max_host::usb_midi::Encoder::default();
    let identity = encoder.push(2, &[0xf0, 0x7e, 0, 0x06, 0x01, 0xf7]);
    usb.send(endpoints.output, &identity)?;
    let identity_replies = collect_editor_sysex(
        session,
        &usb,
        endpoints.input,
        500,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    if !identity_replies
        .iter()
        .any(|reply| reply.starts_with(&[0xf0, 0x7e, 0, 0x06, 0x02, 0x52, 0x72, 0]))
    {
        return Err(format!("Editor identity reply missing: {identity_replies:02x?}").into());
    }

    // The firmware discards Editor commands until 2B advances its session
    // from identity-only state 1 to command state 2. Match the legacy
    // DeviceInitializationService sequence before querying pad file lists.
    let global = editor_exchange(
        session,
        &usb,
        endpoints,
        &mut encoder,
        &[0xf0, 0x52, 0, 0, 0x2b, 0xf7],
        500,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    require_editor_reply(&global, &[0xf0, 0x52, 0, 0, 0x2a], "global settings")?;
    let app_version = editor_exchange(
        session,
        &usb,
        endpoints,
        &mut encoder,
        &[
            0xf0, 0x52, 0, 0, 0x45, 0x05, 0x02, 0, 0, 0, 0, 0, 0x27, 0, 0xf7,
        ],
        500,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    require_editor_reply(
        &app_version,
        &[0xf0, 0x52, 0, 0, 0, 0x11],
        "application version",
    )?;
    let sd_info = editor_exchange(
        session,
        &usb,
        endpoints,
        &mut encoder,
        &[0xf0, 0x52, 0, 0, 0x67, 0x01, 0xf7],
        500,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    require_editor_reply(
        &sd_info,
        &[0xf0, 0x52, 0, 0, 0x67, 0x00],
        "SD card information",
    )?;

    // Start keep-alive after initialization, matching the Editor's sequence.
    // The inspected L6 1.10 fixture does not reply to 31 0B; L6max does.
    if session.engine.model() == l6fw::Model::L6max {
        let keep_alive = editor_exchange(
            session,
            &usb,
            endpoints,
            &mut encoder,
            &[0xf0, 0x52, 0, 0, 0x31, 0x0b, 0xf7],
            500,
            start_guest,
            guest_timeout_ms,
            deadline,
        )?;
        require_editor_reply(&keep_alive, &[0xf0, 0x52, 0, 0, 0, 0x0b], "keep-alive")?;
    }

    let mut assignments = Vec::new();
    for pad in 0u8..4 {
        refresh_editor_session(
            session,
            &usb,
            endpoints,
            &mut encoder,
            start_guest,
            guest_timeout_ms,
            deadline,
        )?;
        let count_request = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 0, pad, 0xf7]);
        usb.send(endpoints.output, &count_request)?;
        let count_replies = collect_editor_sysex(
            session,
            &usb,
            endpoints.input,
            500,
            start_guest,
            guest_timeout_ms,
            deadline,
        )?;
        let file_count = editor_pad_file_count(&count_replies, pad);
        let file_count = file_count.ok_or_else(|| {
            format!(
                "Editor returned no file count for Sound Pad {}: {count_replies:02x?}",
                pad + 1
            )
        })?;
        if file_count == 0 {
            return Err(format!("Editor lists no WAV files for Sound Pad {}", pad + 1).into());
        }

        let mut filename_reply = None;
        for _ in 0..4 {
            refresh_editor_session(
                session,
                &usb,
                endpoints,
                &mut encoder,
                start_guest,
                guest_timeout_ms,
                deadline,
            )?;
            let request = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 1, pad, 0, 0, 0xf7]);
            usb.send(endpoints.output, &request)?;
            let replies = collect_editor_sysex(
                session,
                &usb,
                endpoints.input,
                750,
                start_guest,
                guest_timeout_ms,
                deadline,
            )?;
            filename_reply = replies
                .into_iter()
                .find(|reply| editor_pad_filename_valid(reply, pad));
            if filename_reply.is_some() {
                break;
            }
        }
        let filename_reply = filename_reply.ok_or_else(|| {
            format!(
                "Editor returned no valid filename for Sound Pad {}",
                pad + 1
            )
        })?;
        let assignment_args = filename_reply[6..filename_reply.len() - 1].to_vec();
        let mut assignment = vec![0xf0, 0x52, 0, 0, 0x31, 0x05];
        assignment.extend_from_slice(&assignment_args);
        assignment.push(0xf7);
        let wire = encoder.push(2, &assignment);
        usb.send(endpoints.output, &wire)?;
        let mut assignment_replies = collect_editor_sysex(
            session,
            &usb,
            endpoints.input,
            300,
            start_guest,
            guest_timeout_ms,
            deadline,
        )?;
        if answer_editor_pad_dialog(&usb, endpoints.output, &mut encoder, &assignment_replies)? {
            assignment_replies.extend(collect_editor_sysex(
                session,
                &usb,
                endpoints.input,
                300,
                start_guest,
                guest_timeout_ms,
                deadline,
            )?);
        }

        let mut state_replies = Vec::new();
        let mut assigned = false;
        for _ in 0..8 {
            refresh_editor_session(
                session,
                &usb,
                endpoints,
                &mut encoder,
                start_guest,
                guest_timeout_ms,
                deadline,
            )?;
            let request = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 2, pad, 0xf7]);
            usb.send(endpoints.output, &request)?;
            state_replies = collect_editor_sysex(
                session,
                &usb,
                endpoints.input,
                250,
                start_guest,
                guest_timeout_ms,
                deadline,
            )?;
            if answer_editor_pad_dialog(&usb, endpoints.output, &mut encoder, &state_replies)? {
                state_replies.extend(collect_editor_sysex(
                    session,
                    &usb,
                    endpoints.input,
                    300,
                    start_guest,
                    guest_timeout_ms,
                    deadline,
                )?);
            }
            assigned = editor_pad_assignment_matches(&state_replies, pad, &assignment_args);
            if assigned {
                break;
            }
        }
        if !assigned {
            return Err(format!(
                "Editor assignment query did not match the selected file for Sound Pad {}: {state_replies:02x?}",
                pad + 1
            )
            .into());
        }
        assignments.push(json!({
            "pad": pad + 1,
            "file_count": file_count,
            "filename_reply": filename_reply,
            "assignment_replies": assignment_replies,
            "assignment_state": state_replies,
        }));
    }
    let evidence = json!({"assignments":assignments});
    fs::write(
        options.logs.join("editor-pad-assignments.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    Ok(evidence)
}

fn run_editor_pad_playback(
    session: &Session,
    options: &Options,
    endpoints: l6max_host::usb::Endpoints,
    pad: u8,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    if session.engine.model() != l6fw::Model::L6 || pad > 3 {
        return Err(std::io::Error::other("editor-pad-playback requires L6 and pad 0..3").into());
    }
    if std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_none() {
        return Err(std::io::Error::other(
            "editor-pad-playback requires L6_AUDIO_SAMPLE_TRACE=1 in the case environment",
        )
        .into());
    }
    let usb = session
        .engine
        .usb
        .as_ref()
        .ok_or("USB backend unavailable")?
        .clone();
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("editor-pad-playback needs panel inputs")?;
    let start_guest = inputs.panel.snapshot().guest_ms;
    let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    let trace_path = options.logs.join("engine.trace");
    let audio_prefix = "l6 audio TX samples sai=00384000";
    let audio_before = fs::read_to_string(&trace_path)?
        .lines()
        .filter(|line| line.starts_with(audio_prefix))
        .count();
    let mut encoder = l6max_host::usb_midi::Encoder::default();
    let play = editor_exchange(
        session,
        &usb,
        endpoints,
        &mut encoder,
        &[0xf0, 0x52, 0, 0, 0x31, 0x08, pad, 1, 0xf7],
        100,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    let dialog_answered = answer_editor_pad_dialog(&usb, endpoints.output, &mut encoder, &play)?;
    let mut state_replies = Vec::new();
    for _ in 0..12 {
        state_replies = editor_exchange(
            session,
            &usb,
            endpoints,
            &mut encoder,
            &[0xf0, 0x52, 0, 0, 0x46, 0x03, pad, 0xf7],
            250,
            start_guest,
            guest_timeout_ms,
            deadline,
        )?;
        if state_replies
            .iter()
            .any(|reply| reply.get(4..8) == Some(&[0x45, 0x03, pad, 0x01]))
        {
            break;
        }
        if answer_editor_pad_dialog(&usb, endpoints.output, &mut encoder, &state_replies)? {
            let _ = collect_editor_sysex(
                session,
                &usb,
                endpoints.input,
                300,
                start_guest,
                guest_timeout_ms,
                deadline,
            )?;
        }
    }
    let playing = state_replies
        .iter()
        .any(|reply| reply.get(4..8) == Some(&[0x45, 0x03, pad, 0x01]));
    if !playing {
        return Err(format!(
            "Editor Sound Pad {} did not enter playing state: {state_replies:02x?}",
            pad + 1
        )
        .into());
    }
    wait_guest_duration(session, 1_200, deadline).map_err(std::io::Error::other)?;
    let trace = fs::read_to_string(&trace_path)?;
    let samples = trace
        .lines()
        .filter(|line| line.starts_with(audio_prefix))
        .skip(audio_before)
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("nonzero="))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .collect::<Vec<_>>();
    let nonzero_audio = samples.iter().any(|count| *count > 0);
    let evidence = json!({
        "pad": pad + 1,
        "play_replies": play,
        "state_replies": state_replies,
        "assignment_dialog_answered": dialog_answered,
        "playing_state": playing,
        "nonzero_audio_samples": samples,
        "guest_elapsed_ms": inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest),
    });
    fs::write(
        options
            .logs
            .join(format!("editor-pad-playback-{}.json", pad + 1)),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    if !nonzero_audio {
        return Err(format!("Editor Sound Pad {} entered playing state but produced no nonzero SAI1 TX samples: {evidence}", pad + 1).into());
    }
    Ok(evidence)
}

fn run_l6max_ui_pad_assignment(
    session: &Session,
    options: &Options,
    endpoints: l6max_host::usb::Endpoints,
    pad: u8,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    if session.engine.model() != l6fw::Model::L6max || pad > 3 {
        return Err(
            std::io::Error::other("l6max-ui-pad-assignment requires L6max and pad 0..3").into(),
        );
    }
    let usb = session
        .engine
        .usb
        .as_ref()
        .ok_or("USB backend unavailable")?
        .clone();
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or("L6max UI pad assignment needs panel inputs")?;
    let start_guest = inputs.panel.snapshot().guest_ms;
    let deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    let mut key_evidence = Vec::new();
    let tap = |target: u32,
               main: bool,
               key_evidence: &mut Vec<Value>|
     -> Result<(), Box<dyn std::error::Error>> {
        let before = if main {
            inputs.main.snapshot().applied
        } else {
            inputs.panel.snapshot().applied
        };
        let release_before = if main {
            inputs.main.snapshot().released
        } else {
            inputs.panel.snapshot().released
        };
        if main {
            inputs.main.tap(target, 120)?;
        } else {
            inputs.panel.tap(target, 120)?;
        }
        loop {
            session.poll().map_err(std::io::Error::other)?;
            let (applied, released) = if main {
                let snapshot = inputs.main.snapshot();
                (snapshot.applied, snapshot.released)
            } else {
                let snapshot = inputs.panel.snapshot();
                (snapshot.applied, snapshot.released)
            };
            if applied > before && released > release_before {
                key_evidence.push(
                    json!({"target":target,"main":main,"applied":applied,"released":released}),
                );
                break;
            }
            let elapsed = inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest);
            if Instant::now() >= deadline || elapsed >= guest_timeout_ms {
                return Err(
                    format!("L6max UI key {target} did not complete before its deadline").into(),
                );
            }
            thread::sleep(Duration::from_millis(3));
        }
        wait_guest_duration(session, 1_500, deadline).map_err(std::io::Error::other)?;
        Ok(())
    };

    // A "Data OK." popup can still be up, and it would swallow the Menu press.
    wait_guest_duration(session, 5000, deadline)?;

    // Sound Assign navigation: Menu, Confirm into Sound Pad, Confirm into Sound
    // Assign, select the pad, Confirm into its file list, Down past its first row,
    // None Assigned, and Confirm the first WAV. Menus keep their cursors between
    // visits, so only the first pad moves down to Sound Pad, and each later pad moves
    // one row from the last. A stray Down here reaches SD Card and its Format.
    tap(0, true, &mut key_evidence)?;
    let to_sound_pad: &[u32] = if pad == 0 { &[2, 2] } else { &[] };
    let to_pad: &[u32] = if pad == 0 { &[] } else { &[2] };
    for &button in to_sound_pad
        .iter()
        .chain(&[3, 3])
        .chain(to_pad)
        .chain(&[3, 2, 3])
    {
        // Each menu loads after its key, and the file list drops keys until then.
        wait_guest_duration(session, 2000, deadline)?;
        tap(button, false, &mut key_evidence)?;
    }
    // Back out through Sound Pad and Menu to the home screen, which saves the
    // assignment and leaves the next visit starting from the same place.
    for _ in 0..3 {
        wait_guest_duration(session, 2000, deadline)?;
        tap(0, true, &mut key_evidence)?;
    }
    wait_guest_duration(session, 2000, deadline)?;

    // Verify the UI assignment through the same Editor state query used by
    // the original probe, after opening an Editor session on cable 2.
    let mut encoder = l6max_host::usb_midi::Encoder::default();
    let identity = encoder.push(2, &[0xf0, 0x7e, 0, 0x06, 0x01, 0xf7]);
    usb.send(endpoints.output, &identity)?;
    let identity_replies = collect_editor_sysex(
        session,
        &usb,
        endpoints.input,
        500,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    if !identity_replies
        .iter()
        .any(|reply| reply.starts_with(&[0xf0, 0x7e, 0, 0x06, 0x02, 0x52, 0x72, 0]))
    {
        return Err(format!(
            "L6max Editor identity reply missing after UI assignment: {identity_replies:02x?}"
        )
        .into());
    }
    let global = editor_exchange(
        session,
        &usb,
        endpoints,
        &mut encoder,
        &[0xf0, 0x52, 0, 0, 0x2b, 0xf7],
        500,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    require_editor_reply(&global, &[0xf0, 0x52, 0, 0, 0x2a], "global settings")?;
    let keep_alive = editor_exchange(
        session,
        &usb,
        endpoints,
        &mut encoder,
        &[0xf0, 0x52, 0, 0, 0x31, 0x0b, 0xf7],
        250,
        start_guest,
        guest_timeout_ms,
        deadline,
    )?;
    require_editor_reply(&keep_alive, &[0xf0, 0x52, 0, 0, 0, 0x0b], "keep-alive")?;
    let mut assignment_replies = Vec::new();
    for _ in 0..8 {
        assignment_replies = editor_exchange(
            session,
            &usb,
            endpoints,
            &mut encoder,
            &[0xf0, 0x52, 0, 0, 0x46, 0x02, pad, 0xf7],
            300,
            start_guest,
            guest_timeout_ms,
            deadline,
        )?;
        if assignment_replies.iter().any(|reply| {
            reply.get(4..7) == Some(&[0x45, 0x02, pad])
                && reply.len() >= 12
                && reply.get(7..9) != Some(&[0x7f, 0x7f])
                && reply.get(9..11) != Some(&[0, 0])
        }) {
            break;
        }
    }
    let assigned = assignment_replies.iter().any(|reply| {
        reply.get(4..7) == Some(&[0x45, 0x02, pad])
            && reply.len() >= 12
            && reply.get(7..9) != Some(&[0x7f, 0x7f])
            && reply.get(9..11) != Some(&[0, 0])
    });
    let evidence = json!({
        "pad": pad + 1,
        "ui_keys": key_evidence,
        "identity_replies": identity_replies,
        "assignment_replies": assignment_replies,
        "assigned": assigned,
        "guest_elapsed_ms": inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest),
    });
    fs::write(
        options
            .logs
            .join(format!("l6max-ui-pad-assignment-{}.json", pad + 1)),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    if !assigned {
        return Err(format!(
            "L6max UI did not assign a WAV to Sound Pad {}: {evidence}",
            pad + 1
        )
        .into());
    }
    Ok(evidence)
}

fn run_sound_pad_playback(
    session: &Session,
    options: &Options,
    pad: u8,
    target: u32,
    assignment: PadAssignmentSource,
    require_pad_led_change: bool,
    playback_window_ms: u32,
    require_stereo_meter: bool,
    expect_source: Option<&str>,
    max_residual_db: f64,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    let model = session.engine.model();
    let capture_path = std::env::var_os("L6_AUDIO_RAW_CAPTURE");
    if expect_source.is_some() && capture_path.is_none() {
        return Err(std::io::Error::other(
            "sound-pad-playback expect_source requires L6_AUDIO_RAW_CAPTURE in the case environment",
        )
        .into());
    }
    let capture_baseline = match &capture_path {
        Some(path) => fs::metadata(path)?.len() as usize,
        None => 0,
    };
    if pad > 3 || target != 49 + u32::from(pad) {
        return Err(std::io::Error::other(
            "sound-pad-playback target must match pad (pad 0..3 maps to target 49..52)",
        )
        .into());
    }
    if std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_none() {
        return Err(std::io::Error::other(
            "sound-pad-playback requires L6_AUDIO_SAMPLE_TRACE=1 in the case environment",
        )
        .into());
    }
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or_else(|| std::io::Error::other("sound-pad-playback has no panel inputs"))?;
    let assignment_ready = if model == l6fw::Model::L6 {
        let address = 0x8044_337f + u32::from(pad);
        let start_guest = inputs.panel.snapshot().guest_ms;
        let deadline = Instant::now() + Duration::from_secs(wall_timeout_s.min(30));
        loop {
            let pending = session.engine.management[0].read_memory(0, address, 1)?[0];
            if pending == 0 {
                break json!({"address":format!("{address:#010x}"),"pending":pending});
            }
            let elapsed_guest = inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest);
            if elapsed_guest >= guest_timeout_ms || Instant::now() >= deadline {
                return Err(std::io::Error::other(format!(
                    "L6 saved pad {} assignment is still pending ({pending:#04x}) at {address:#010x}; refusing to press before the file assignment completes",
                    pad + 1
                ))
                .into());
            }
            wait_guest_duration(session, 100, deadline).map_err(std::io::Error::other)?;
        }
    } else {
        json!({"model":"l6max","source":format!("{assignment:?}")})
    };
    let trace_path = options.logs.join("engine.trace");
    let audio_prefix = "l6 audio TX samples sai=00384000";
    let trace_before = fs::read_to_string(&trace_path)?;
    let baseline_lines = trace_before
        .lines()
        .filter(|line| line.starts_with(audio_prefix))
        .count();
    let start_guest = inputs.panel.snapshot().guest_ms;
    let mut last_indicator_sample_ms = start_guest;
    let initial_panel = session.poll().map_err(std::io::Error::other)?;
    let mut indicator_samples = vec![json!({
        "elapsed_guest_ms": 0,
        "master_meter": master_meter_levels(model, initial_panel.indicator_levels),
        "indicator_levels": initial_panel.indicator_levels,
    })];
    let wall_deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    let guest_deadline_ms = guest_timeout_ms;
    let led_before = require_pad_led_change
        .then(|| sound_pad_led(model, inputs.panel.snapshot().indicators, pad));
    let applied_before = inputs.main.snapshot().applied;
    inputs
        .main
        .down(target, 500)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut pad_led_changed = false;
    let observe_until_guest_ms = 400u32;
    loop {
        let data = session.poll().map_err(std::io::Error::other)?;
        let panel = inputs.panel.snapshot();
        if panel.guest_ms != last_indicator_sample_ms {
            last_indicator_sample_ms = panel.guest_ms;
            indicator_samples.push(json!({
                "elapsed_guest_ms": panel.guest_ms.wrapping_sub(start_guest),
                "master_meter": master_meter_levels(model, data.indicator_levels),
                "indicator_levels": data.indicator_levels,
            }));
        }
        let main = inputs.main.snapshot();
        if !main.connected {
            return Err(std::io::Error::other(format!(
                "MAIN input disconnected during pad press: {main:?}"
            ))
            .into());
        }
        if main.applied > applied_before {
            if let Some(led_before) = led_before {
                pad_led_changed |= sound_pad_led(model, data.indicators, pad) != led_before;
            }
            let elapsed = inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest);
            if elapsed >= observe_until_guest_ms {
                break;
            }
        }
        let elapsed = inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest);
        if Instant::now() >= wall_deadline || elapsed >= guest_deadline_ms {
            return Err(std::io::Error::other(
                "sound-pad press did not settle before its deadline",
            )
            .into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let released_before = inputs.main.snapshot().applied;
    inputs
        .main
        .up(target)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    loop {
        let data = session.poll().map_err(std::io::Error::other)?;
        let panel = inputs.panel.snapshot();
        if panel.guest_ms != last_indicator_sample_ms {
            last_indicator_sample_ms = panel.guest_ms;
            indicator_samples.push(json!({
                "elapsed_guest_ms": panel.guest_ms.wrapping_sub(start_guest),
                "master_meter": master_meter_levels(model, data.indicator_levels),
                "indicator_levels": data.indicator_levels,
            }));
        }
        let main = inputs.main.snapshot();
        if !main.connected {
            return Err(std::io::Error::other(format!(
                "MAIN input disconnected during pad playback: {main:?}"
            ))
            .into());
        }
        if let Some(led_before) = led_before {
            pad_led_changed |= sound_pad_led(model, data.indicators, pad) != led_before;
        }
        let elapsed = inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest);
        if elapsed >= playback_window_ms && main.applied > released_before {
            break;
        }
        if Instant::now() >= wall_deadline || elapsed >= guest_deadline_ms {
            return Err(std::io::Error::other(format!(
                "sound-pad playback timed out at guest {elapsed} ms (limit {guest_deadline_ms} ms)"
            ))
            .into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let trace = fs::read_to_string(&trace_path)?;
    let samples = trace
        .lines()
        .filter(|line| line.starts_with(audio_prefix))
        .skip(baseline_lines)
        .filter_map(|line| {
            let field = |name: &str| {
                line.split_whitespace()
                    .find_map(|part| part.strip_prefix(&format!("{name}=")))?
                    .parse::<u64>()
                    .ok()
            };
            let at_ms = line
                .split_whitespace()
                .find_map(|part| part.strip_prefix("at="))?
                .strip_suffix("ms")?
                .parse::<u64>()
                .ok()?;
            Some(json!({
                "words": field("words")?,
                "nonzero": field("nonzero")?,
                "underruns": field("underruns")?,
                "first": line.split_whitespace().find_map(|part| part.strip_prefix("first="))?,
                "last": line.split_whitespace().find_map(|part| part.strip_prefix("last="))?,
                "at_ms": at_ms,
            }))
        })
        .collect::<Vec<_>>();
    let nonzero_audio = samples.iter().any(|sample| {
        let nonzero = sample["nonzero"].as_u64().unwrap_or(0);
        let words = sample["words"].as_u64().unwrap_or(0);
        let first = sample["first"].as_str().unwrap_or_default();
        let last = sample["last"].as_str().unwrap_or_default();
        nonzero > 0 && words > 0 && first != "ffffffff" && last != "ffffffff"
    });
    let isolated_levels = std::array::from_fn::<_, 2, _>(|side| {
        let mut seen = [false; 7];
        for sample in &indicator_samples {
            let meter = &sample["master_meter"];
            let level = meter[side].as_u64().unwrap_or(0) as usize;
            if meter[1 - side].as_u64() == Some(0) && (2..=6).contains(&level) {
                seen[level] = true;
            }
        }
        (2..=6).filter(|&level| seen[level]).collect::<Vec<_>>()
    });
    let left_only = isolated_levels[0].len() >= 2;
    let right_only = isolated_levels[1].len() >= 2;
    let waveform = expect_source
        .map(|source| {
            let capture = fs::read(capture_path.as_ref().unwrap())?;
            compare_pad_waveform(
                &options.logs.join("card.img"),
                source,
                &capture[capture_baseline.min(capture.len())..],
            )
        })
        .transpose()?;
    let evidence = json!({
        "waveform": waveform,
        "stereo_meter": {"required": require_stereo_meter, "left_only": left_only, "right_only": right_only, "isolated_levels": isolated_levels},
        "pad": pad + 1,
        "target": target,
        "assignment_source": format!("{assignment:?}"),
        "assignment_ready": assignment_ready,
        "led": if model == l6fw::Model::L6 {
            usize::from(pad)
        } else {
            27 + usize::from(pad)
        },
        "pad_led_check_enabled": require_pad_led_change,
        "pad_led_changed_from_baseline": pad_led_changed,
        "indicator_samples": indicator_samples,
        "audio_samples": samples,
        "nonzero_audio_after_press": nonzero_audio,
        "guest_window_ms": inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest),
    });
    fs::write(
        options.logs.join("sound-pad-playback.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    if let Some(waveform) = &waveform {
        let residual = waveform["residual_db"].as_f64().unwrap_or(f64::INFINITY);
        if residual > max_residual_db {
            return Err(std::io::Error::other(format!(
                "sound-pad waveform residual {residual:.1} dB exceeds {max_residual_db:.1} dB: {waveform}"
            ))
            .into());
        }
    }
    if !nonzero_audio
        || (require_pad_led_change && !pad_led_changed)
        || (require_stereo_meter && !(left_only && right_only))
    {
        return Err(std::io::Error::other(format!(
            "sound-pad playback assertion failed: audio={nonzero_audio}, pad LED changed={pad_led_changed}, pad LED required={require_pad_led_change}, stereo meter required={require_stereo_meter}, left only={left_only}, right only={right_only}; details saved to sound-pad-playback.json"
        )).into());
    }
    Ok(evidence)
}

fn compare_pad_waveform(
    card: &std::path::Path,
    source: &str,
    capture: &[u8],
) -> Result<Value, Box<dyn std::error::Error>> {
    let wav = sd_image::read_file(card, source, u64::MAX)?;
    let source = waveform::decode_wav(&wav).map_err(std::io::Error::other)?;
    waveform::compare(&source, &waveform::decode_capture(capture))
        .map_err(|error| std::io::Error::other(error).into())
}

fn master_meter_levels(model: l6fw::Model, levels: [[u8; 24]; 8]) -> [u8; 2] {
    match model {
        l6fw::Model::L6 => l6max_host::indicators::l6_master_meter_levels(levels),
        l6fw::Model::L6max => l6max_host::indicators::l6max_master_meter_levels(levels),
    }
}

fn run_l6_recorder_playback(
    session: &Session,
    options: &Options,
    target: u32,
    guest_timeout_ms: u32,
    wall_timeout_s: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    if session.engine.model() != l6fw::Model::L6 || target != 4 {
        return Err(std::io::Error::other(
            "l6-recorder-playback requires L6 firmware and MAIN target 4",
        )
        .into());
    }
    if std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_none() {
        return Err(std::io::Error::other(
            "l6-recorder-playback requires L6_AUDIO_SAMPLE_TRACE=1 in the case environment",
        )
        .into());
    }
    const PREFIX: &str = "l6 audio TX samples sai=00384000";
    const POST_PRESS_SETTLE_MS: u32 = 5000;
    let total_guest_ms = POST_PRESS_SETTLE_MS;
    if guest_timeout_ms < total_guest_ms {
        return Err(std::io::Error::other(
            "l6-recorder-playback guest timeout must be at least 5000 ms",
        )
        .into());
    }
    let inputs = session
        .engine
        .inputs
        .as_ref()
        .ok_or_else(|| std::io::Error::other("l6-recorder-playback has no panel inputs"))?;
    let trace_path = options.logs.join("engine.trace");
    let trace_before = fs::read_to_string(&trace_path)?;
    let baseline_samples = trace_before
        .lines()
        .filter(|line| line.starts_with(PREFIX))
        .collect::<Vec<_>>();
    let baseline_signature = baseline_samples
        .last()
        .and_then(|line| tx_sample_signature(line));
    let qmp = &session.engine.management[0];
    let gdb_socket = options
        .gdb_socket
        .as_ref()
        .ok_or_else(|| std::io::Error::other("l6-recorder-playback requires GDB"))?;
    let mut debugger = l6max_diagnostics::gdb::Gdb::connect(gdb_socket).map_err(|error| {
        std::io::Error::other(format!("connecting recorder GDB socket: {error}"))
    })?;
    qmp.stop().map_err(|error| {
        std::io::Error::other(format!(
            "stopping QEMU before recorder breakpoint setup: {error}"
        ))
    })?;
    debugger.breakpoint(0x8002_c9d8, true).map_err(|error| {
        std::io::Error::other(format!(
            "setting L6 system-event dispatcher breakpoint: {error}"
        ))
    })?;
    debugger.breakpoint(0x8000_8a08, true).map_err(|error| {
        std::io::Error::other(format!("setting L6 Play handler breakpoint: {error}"))
    })?;
    debugger.breakpoint(0x8004_b890, true).map_err(|error| {
        std::io::Error::other(format!("setting L6 recorder callback breakpoint: {error}"))
    })?;
    let start_guest_ms = inputs.panel.snapshot().guest_ms;
    let before_applied = inputs.main.snapshot().applied;
    inputs
        .main
        .down(target, 10_000)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let wall_deadline = Instant::now() + Duration::from_secs(wall_timeout_s);
    loop {
        session.poll().map_err(std::io::Error::other)?;
        let main = inputs.main.snapshot();
        if !main.connected {
            return Err(std::io::Error::other(format!(
                "MAIN input disconnected during recorder Play: {main:?}"
            ))
            .into());
        }
        if main.applied > before_applied {
            break;
        }
        if Instant::now() >= wall_deadline {
            return Err(
                std::io::Error::other("L6 recorder Play press was not acknowledged").into(),
            );
        }
        thread::sleep(Duration::from_millis(5));
    }
    debugger
        .continue_until_stop(Duration::from_secs(10))
        .map_err(|error| {
            std::io::Error::other(format!("waiting for L6 system-event dispatcher: {error}"))
        })?;
    let dispatcher_pc = debugger.read_register(15).map_err(std::io::Error::other)? & !1;
    let event_pointer = debugger.read_register(0).map_err(std::io::Error::other)?;
    let event = qmp
        .read_memory(0, event_pointer, 20)
        .map_err(|error| std::io::Error::other(format!("reading dispatched L6 event: {error}")))?;
    let event_state = u32::from_le_bytes(event[0..4].try_into().unwrap());
    let event_class = u32::from_le_bytes(event[4..8].try_into().unwrap());
    let event_code = u32::from_le_bytes(event[8..12].try_into().unwrap());
    if dispatcher_pc != 0x8002_c9d8 {
        return Err(std::io::Error::other(format!(
            "L6 Play event did not reach class-0 dispatcher (pc={dispatcher_pc:#010x}, event={event:02x?})"
        )).into());
    }
    if event_state != 0 || event_class != 0 || event_code != 0x20 {
        return Err(std::io::Error::other(format!(
            "L6 class-0 dispatcher received an unexpected event (state={event_state}, class={event_class}, code={event_code:#x}, bytes={event:02x?})"
        )).into());
    }
    // A breakpoint left at the current PC would stop here again.
    debugger
        .breakpoint(0x8002_c9d8, false)
        .map_err(|error| std::io::Error::other(format!("clearing dispatcher: {error}")))?;
    debugger
        .continue_until_stop(Duration::from_secs(10))
        .map_err(|error| std::io::Error::other(format!("waiting for L6 Play handler: {error}")))?;
    let handler_pc = debugger.read_register(15).map_err(std::io::Error::other)? & !1;
    let handler_argument = debugger.read_register(0).map_err(std::io::Error::other)?;
    let mode = u32::from_le_bytes(
        qmp.read_memory(0, 0x8057_8ebc, 4)
            .map_err(|error| std::io::Error::other(format!("reading L6 recorder mode: {error}")))?
            .try_into()
            .unwrap(),
    );
    let selection_key = qmp.read_memory(0, 0x8061_01a8, 8).map_err(|error| {
        std::io::Error::other(format!("reading selected L6 recording key: {error}"))
    })?;
    if handler_pc != 0x8000_8a08
        || handler_argument != 1
        || mode != 0
        || selection_key.iter().all(|byte| *byte == 0)
    {
        return Err(std::io::Error::other(format!(
            "L6 Play handler did not accept the selected mode-0 recording (pc={handler_pc:#010x}, argument={handler_argument}, mode={mode}, selection={selection_key:02x?})"
        ))
        .into());
    }
    debugger
        .breakpoint(0x8000_8a08, false)
        .map_err(|error| std::io::Error::other(format!("clearing Play handler: {error}")))?;
    debugger
        .continue_until_stop(Duration::from_secs(10))
        .map_err(|error| {
            std::io::Error::other(format!("waiting for L6 recorder callback: {error}"))
        })?;
    let callback_pc = debugger.read_register(15).map_err(std::io::Error::other)? & !1;
    if callback_pc != 0x8004_b890 {
        return Err(std::io::Error::other(format!(
            "L6 recorder playback callback stopped at {callback_pc:#010x}, expected 0x8004b890"
        ))
        .into());
    }
    debugger
        .breakpoint(0x8004_b890, false)
        .map_err(std::io::Error::other)?;
    qmp.resume().map_err(|error| {
        std::io::Error::other(format!("resuming QEMU after L6 recorder callback: {error}"))
    })?;
    wait_guest_duration(session, POST_PRESS_SETTLE_MS, wall_deadline)
        .map_err(std::io::Error::other)?;
    let release_before = inputs.main.snapshot().applied;
    inputs
        .main
        .up(target)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    loop {
        session.poll().map_err(std::io::Error::other)?;
        let main = inputs.main.snapshot();
        if main.applied > release_before {
            break;
        }
        if Instant::now() >= wall_deadline {
            return Err(
                std::io::Error::other("L6 recorder Play release was not acknowledged").into(),
            );
        }
        thread::sleep(Duration::from_millis(5));
    }
    qmp.stop().map_err(|error| {
        std::io::Error::other(format!("stopping QEMU for L6 SAI DMA inspection: {error}"))
    })?;
    let tcd = qmp.read_memory(0, 0x400e_91c0, 32).map_err(|error| {
        std::io::Error::other(format!("reading L6 SAI channel-14 TCD: {error}"))
    })?;
    let source = u32::from_le_bytes(tcd[0..4].try_into().unwrap());
    let source_data = qmp.read_memory(0, source, 16).map_err(|error| {
        std::io::Error::other(format!("reading L6 SAI channel-14 DMA source: {error}"))
    })?;
    let source_words = source_data
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
        .collect::<Vec<_>>();
    let source_has_audio = source_words
        .iter()
        .any(|word| *word != 0 && *word != u32::MAX);
    let trace = fs::read_to_string(&trace_path)?;
    let samples = trace
        .lines()
        .filter(|line| line.starts_with(PREFIX))
        .skip(baseline_samples.len())
        .filter_map(|line| {
            Some(json!({
                "at_ms": trace_u64_field(line, "at")?,
                "first": trace_hex_field(line, "first")?,
                "last": trace_hex_field(line, "last")?,
            }))
        })
        .collect::<Vec<_>>();
    let tx_changed = samples.iter().any(|sample| {
        let signature = (sample["first"].as_str(), sample["last"].as_str());
        baseline_signature.as_ref().is_some_and(|baseline| {
            signature != (Some(baseline.0.as_str()), Some(baseline.1.as_str()))
        })
    });
    let evidence = json!({
        "target": target,
        "guest_window_ms": inputs.panel.snapshot().guest_ms.wrapping_sub(start_guest_ms),
        "dispatched_event_pointer": event_pointer,
        "dispatched_event": event,
        "handler_argument": handler_argument,
        "recorder_mode": mode,
        "selection_key": selection_key,
        "callback_pc": callback_pc,
        "post_press_settle_ms": POST_PRESS_SETTLE_MS,
        "baseline_signature": baseline_signature,
        "samples": samples,
        "tx_sample_signature_changed_after_press": tx_changed,
        "channel_14_tcd_source": source,
        "channel_14_source_words": source_words,
        "channel_14_source_has_audio": source_has_audio,
    });
    fs::write(
        options.logs.join("recorder-playback.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    if !tx_changed || !source_has_audio {
        return Err(std::io::Error::other(format!(
            "L6 recorder playback audio assertion failed (TX changed={tx_changed}, channel-14 source has audio={source_has_audio}): {evidence}"
        ))
        .into());
    }
    Ok(evidence)
}

fn tx_sample_signature(line: &str) -> Option<(String, String)> {
    Some((
        trace_hex_field(line, "first")?,
        trace_hex_field(line, "last")?,
    ))
}

fn trace_hex_field(line: &str, field: &str) -> Option<String> {
    let value = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{field}=")))?;
    Some(value.to_ascii_lowercase())
}

fn trace_u64_field(line: &str, field: &str) -> Option<u64> {
    let value = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{field}=")))?;
    value
        .strip_suffix("ms")
        .unwrap_or(value)
        .parse::<u64>()
        .ok()
}

fn sound_pad_led(model: l6fw::Model, indicators: [u32; 8], pad: u8) -> bool {
    match model {
        l6fw::Model::L6 => l6max_host::indicators::l6_sound_pad_leds(indicators)[pad as usize],
        l6fw::Model::L6max => l6max_host::indicators::leds(indicators)[27 + pad as usize],
    }
}

#[allow(clippy::too_many_arguments)]
fn editor_exchange(
    session: &Session,
    usb: &l6max_host::usb::UsbHost,
    endpoints: l6max_host::usb::Endpoints,
    encoder: &mut l6max_host::usb_midi::Encoder,
    message: &[u8],
    timeout_ms: u32,
    start_guest: u32,
    guest_timeout_ms: u32,
    wall_deadline: Instant,
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let wire = encoder.push(2, message);
    usb.send(endpoints.output, &wire)?;
    collect_editor_sysex(
        session,
        usb,
        endpoints.input,
        timeout_ms,
        start_guest,
        guest_timeout_ms,
        wall_deadline,
    )
}

fn refresh_editor_session(
    session: &Session,
    usb: &l6max_host::usb::UsbHost,
    endpoints: l6max_host::usb::Endpoints,
    encoder: &mut l6max_host::usb_midi::Encoder,
    start_guest: u32,
    guest_timeout_ms: u32,
    wall_deadline: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    if session.engine.model() == l6fw::Model::L6 {
        return Ok(());
    }
    let replies = editor_exchange(
        session,
        usb,
        endpoints,
        encoder,
        &[0xf0, 0x52, 0, 0, 0x31, 0x0b, 0xf7],
        250,
        start_guest,
        guest_timeout_ms,
        wall_deadline,
    )?;
    require_editor_reply(&replies, &[0xf0, 0x52, 0, 0, 0, 0x0b], "keep-alive")?;
    Ok(())
}

fn require_editor_reply(
    replies: &[Vec<u8>],
    prefix: &[u8],
    operation: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if replies.iter().any(|reply| reply.starts_with(prefix)) {
        Ok(())
    } else {
        Err(format!("Editor {operation} reply missing: {replies:02x?}").into())
    }
}

fn collect_editor_sysex(
    session: &Session,
    usb: &l6max_host::usb::UsbHost,
    input: u8,
    timeout_ms: u32,
    start_guest: u32,
    guest_timeout_ms: u32,
    wall_deadline: Instant,
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let started = session
        .engine
        .inputs
        .as_ref()
        .unwrap()
        .panel
        .snapshot()
        .guest_ms;
    let mut pending = Vec::new();
    let mut replies = Vec::new();
    loop {
        session.poll().map_err(std::io::Error::other)?;
        match usb.try_receive(input, 16_384) {
            Ok(data) => {
                for (cable, fragment) in l6max_host::usb_midi::decode(&data)? {
                    if cable != 2 {
                        continue;
                    }
                    for byte in fragment {
                        if byte == 0xf0 {
                            pending.clear();
                        }
                        if byte == 0xf0 || !pending.is_empty() {
                            pending.push(byte);
                            if byte == 0xf7 {
                                replies.push(std::mem::take(&mut pending));
                            }
                        }
                    }
                }
            }
            Err(error) if error.raw_os_error() == Some(libc::EAGAIN) => {}
            Err(error) => return Err(error.into()),
        }
        let now = session
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .panel
            .snapshot()
            .guest_ms;
        if now.wrapping_sub(started) >= timeout_ms {
            if !pending.is_empty() {
                return Err(format!("truncated Editor SysEx reply: {pending:02x?}").into());
            }
            return Ok(replies);
        }
        if Instant::now() >= wall_deadline || now.wrapping_sub(start_guest) > guest_timeout_ms {
            return Err("Editor pad assignment exceeded its guest or wall deadline".into());
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn answer_editor_pad_dialog(
    usb: &l6max_host::usb::UsbHost,
    output: u8,
    encoder: &mut l6max_host::usb_midi::Encoder,
    replies: &[Vec<u8>],
) -> Result<bool, Box<dyn std::error::Error>> {
    let pending = replies.iter().any(|reply| {
        reply.get(4..6) == Some(&[0x45, 0x06]) || reply.get(4..7) == Some(&[0x31, 0x10, 0x06])
    });
    if !pending {
        return Ok(false);
    }
    let answer = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x31, 0x12, 0x06, 0x01, 0xf7]);
    usb.send(output, &answer)?;
    Ok(true)
}

fn editor_pad_file_count(replies: &[Vec<u8>], pad: u8) -> Option<u16> {
    replies.iter().find_map(|reply| {
        (reply.get(4..7) == Some(&[0x45, 0x00, pad]) && reply.len() >= 10)
            .then(|| ((reply[7] as u16) << 7) | reply[8] as u16)
    })
}

fn editor_pad_filename_valid(reply: &[u8], pad: u8) -> bool {
    reply.get(4..7) == Some(&[0x45, 0x01, pad])
        && reply.len() >= 12
        && reply.get(7..9) != Some(&[0x7f, 0x7f])
        && reply.get(9..11) != Some(&[0, 0])
}

fn editor_pad_assignment_matches(replies: &[Vec<u8>], pad: u8, args: &[u8]) -> bool {
    replies.iter().any(|reply| {
        reply.get(4..7) == Some(&[0x45, 0x02, pad])
            && reply.get(6..reply.len().saturating_sub(1)) == Some(args)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        create_factory_card, editor_pad_assignment_matches, editor_pad_file_count,
        editor_pad_filename_valid, knob_display_matches, sound_pad_led, verify_sai_edma_trace,
    };
    use l6fw::Model;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn sound_pad_playback_uses_the_selected_models_led_wiring() {
        let mut l6_rows = [0; 8];
        l6_rows[3] = 1 << 15;
        assert!(sound_pad_led(Model::L6, l6_rows, 0));
        assert!(!sound_pad_led(Model::L6max, l6_rows, 0));

        let mut l6max_rows = [0; 8];
        l6max_rows[1] = 1 << 21;
        assert!(sound_pad_led(Model::L6max, l6max_rows, 0));
        assert!(!sound_pad_led(Model::L6, l6max_rows, 0));
    }

    #[test]
    fn factory_card_contains_the_jig_marker_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("factory-jig-{}-{nonce}.img", std::process::id()));
        create_factory_card(&path).unwrap();
        let contents = sd_image::open(&path)
            .unwrap()
            .read("D445_JIG_000.txt")
            .unwrap();
        assert_eq!(contents, b"emulator fixture; no calibration parameters\n");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn sai_edma_trace_checks_model_specific_fifo_and_descriptor_paths() {
        let trace = concat!(
            "write addr=40384008 value=80040001 at=10ms\n",
            "write addr=40384088 value=80040001 at=11ms\n",
            "write addr=40384020 value=00000001 at=12ms\n",
            "write addr=400e91c0 value=00000001 at=13ms\n",
            "write addr=403840a0 value=00000001 at=14ms\n",
            "write addr=400e91e0 value=00000001 at=15ms\n",
            "write addr=4038c008 value=80040001 at=16ms\n",
            "write addr=4038c088 value=80040001 at=17ms\n",
            "write addr=4038c020 value=00000001 at=18ms\n",
            "write addr=400e9180 value=00000001 at=19ms\n",
            "write addr=4038c0a0 value=00000001 at=20ms\n",
            "write addr=400e91a0 value=00000001 at=21ms\n",
            "write addr=40384020 value=00000001 at=22ms\n",
            "write addr=400e91c0 value=00000001 at=23ms\n",
            "write addr=403840a0 value=00000001 at=24ms\n",
            "write addr=400e91e0 value=00000001 at=25ms\n",
        );
        assert!(verify_sai_edma_trace(Model::L6, trace).is_ok());
        assert!(verify_sai_edma_trace(Model::L6max, trace).is_ok());
        let without_sai2 = trace
            .lines()
            .filter(|line| !line.contains("4038c"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(verify_sai_edma_trace(Model::L6max, &without_sai2).is_ok());
        assert!(verify_sai_edma_trace(Model::L6, &without_sai2).is_err());
    }

    #[test]
    fn editor_pad_reply_parser_distinguishes_empty_and_confirmed_assignments() {
        let count = vec![240, 82, 0, 0, 69, 0, 2, 0, 4, 247];
        assert_eq!(editor_pad_file_count(&[count], 2), Some(4));
        let none_assigned = vec![240, 82, 0, 0, 69, 1, 2, 127, 127, 0, 0, 247];
        assert!(!editor_pad_filename_valid(&none_assigned, 2));
        let filename = vec![240, 82, 0, 0, 69, 1, 2, 0, 0, 4, 0, 1, 2, 3, 4, 247];
        assert!(editor_pad_filename_valid(&filename, 2));
        let args = filename[6..filename.len() - 1].to_vec();
        let assigned = vec![240, 82, 0, 0, 69, 2]
            .into_iter()
            .chain(args.iter().copied())
            .chain([247])
            .collect::<Vec<_>>();
        assert!(editor_pad_assignment_matches(&[assigned.clone()], 2, &args));
        assert!(!editor_pad_assignment_matches(&[assigned], 1, &args));
    }

    #[test]
    fn frequency_dialog_accepts_one_unit_of_display_rounding() {
        assert!(knob_display_matches(1, "646 Hz", "647 Hz"));
        assert!(knob_display_matches(1, "1.23 kHz", "1.24 kHz"));
        assert!(!knob_display_matches(1, "646 Hz", "648 Hz"));
        assert!(!knob_display_matches(0, "646 Hz", "647 Hz"));
    }
}
