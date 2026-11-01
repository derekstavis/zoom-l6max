//! Live integration check: QMP lifecycle, paused guest deadlines, discrete
//! button packets, visible selection changes, and encoder input.
use l6max_diagnostics::gdb::Gdb;
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
    input::{EngineInput, InputHub},
    qmp::QmpClient,
    usb::UsbHost,
    usb_midi::{Encoder, decode},
};

use std::{
    cell::{Cell, RefCell},
    fs::{self},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

type Frame = [u8; 1024];
type L6maxPanelSample = (u64, [u32; 8], [u8; 2]);
const L6MAX_RECORDER_EVENT_TRACE_POINTS: [u32; 9] = [
    0x8002_8088, // generic firmware event enqueue (button scanner uses this path)
    0x8003_6a40, // main button event handler
    0x8008_bd90, // button callback scheduler
    0x8008_be38, // selected callback indirect call
    0x8004_e818, // Record callback selected by callback slot 1
    0x8004_7238, // Record start action
    0x8004_d8ce, // return from the Record start action
    0x8004_6ce8, // enqueue the asynchronous recorder-start callback
    0x8004_7268, // asynchronous RecPlayCtl_RecStart callback
];
fn pump(display: &PublishedDisplay, latest: &mut Option<Frame>) -> Result<(), String> {
    if let Some(frame) = display.sample().map_err(|error| error.to_string())? {
        *latest = Some(frame);
    }
    Ok(())
}
fn pump_for(
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
    duration: Duration,
) -> Result<(), String> {
    let end = Instant::now() + duration;
    loop {
        pump(display, latest)?;
        if Instant::now() >= end {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn pump_until_guest_ms(
    inputs: &InputHub,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
    target_ms: u32,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    while inputs.panel.snapshot().guest_ms < target_ms {
        pump(display, latest)?;
        if Instant::now() >= deadline {
            return Err(format!(
                "guest clock did not reach {target_ms} ms (stopped at {} ms)",
                inputs.panel.snapshot().guest_ms
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    pump(display, latest)
}
fn screen_settle_delay() -> Duration {
    let millis = std::env::var("L6_SCREEN_SETTLE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_500);
    Duration::from_millis(millis)
}
fn boot_settle_delay() -> Duration {
    let millis = std::env::var("L6_BOOT_SETTLE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(42_000);
    Duration::from_millis(millis)
}
fn sd_insert_settle_delay() -> Result<Duration, String> {
    let millis = std::env::var("L6_SD_INSERT_SETTLE_MS")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid L6_SD_INSERT_SETTLE_MS: {error}"))
        })
        .transpose()?
        .unwrap_or(3_000);
    if millis > 300_000 {
        return Err("L6_SD_INSERT_SETTLE_MS must be between 0 and 300000".into());
    }
    Ok(Duration::from_millis(millis))
}
fn save(raw: &[u8], path: &Path) -> Result<(), String> {
    image::GrayImage::from_fn(128, 64, |x, y| {
        let bit = (raw[((63 - y as usize) / 8) * 128 + x as usize] >> ((63 - y as usize) % 8)) & 1;
        image::Luma([if bit != 0 { 255 } else { 0 }])
    })
    .save(path)
    .map_err(|e| e.to_string())
}
fn wait_applied(
    input: &EngineInput,
    count: u64,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        pump(display, latest)?;
        let status = input.snapshot();
        if status.rejected != 0 || !status.connected {
            return Err(format!("input channel failed: {status:?}"));
        }
        if status.applied >= count {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!("input deadline: {status:?}"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_rejected(input: &EngineInput, count: u64) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let status = input.snapshot();
        if !status.connected {
            return Err(format!("input channel disconnected: {status:?}"));
        }
        if status.rejected >= count {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!("expected rejected input: {status:?}"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_released(input: &EngineInput, count: u64) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let status = input.snapshot();
        if !status.connected {
            return Err(format!("input channel disconnected: {status:?}"));
        }
        if status.released >= count {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!("expected released input: {status:?}"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn trace_count(logs: &Path, pattern: &str) -> Result<usize, String> {
    Ok(fs::read_to_string(logs.join("engine.trace"))
        .map_err(|e| e.to_string())?
        .matches(pattern)
        .count())
}

fn l6max_sound_pad_targets() -> Result<Vec<u32>, String> {
    let value = std::env::var("L6MAX_SOUND_PAD_TARGET").map_err(|_| {
        "set L6MAX_SOUND_PAD_TARGET to an explicit L6max Sound Pad input target".to_owned()
    })?;
    if value == "all" {
        return Ok((49..=52).collect());
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|target| (49..=52).contains(target))
        .map(|target| vec![target])
        .ok_or_else(|| {
            "L6MAX_SOUND_PAD_TARGET must be 49 to 52, or all for an Editor-isolated sweep".into()
        })
}

fn l6max_sound_pad_target() -> Result<u32, String> {
    let targets = l6max_sound_pad_targets()?;
    if targets.len() == 1 {
        Ok(targets[0])
    } else {
        Err(
            "L6MAX_SOUND_PAD_TARGET=all is supported only with L6MAX_EDITOR_PAD_ASSIGN_TRACE"
                .into(),
        )
    }
}

fn answer_pad_assignment_dialog(
    usb: &UsbHost,
    endpoints: l6max_host::usb::Endpoints,
    encoder: &mut Encoder,
    replies: &[Vec<u8>],
    answered: &mut bool,
) -> Result<bool, String> {
    let l6max_prompt = replies
        .iter()
        .any(|reply| reply.get(4..6) == Some(&[0x45, 0x06]));
    // L6 reports the pending confirmation as an assignment error code; the
    // L6max Editor sends the equivalent 45 06 notification.
    let l6_prompt = replies
        .iter()
        .any(|reply| reply.get(4..7) == Some(&[0x31, 0x10, 0x06]));
    if *answered || !(l6_prompt || l6max_prompt) {
        return Ok(false);
    }
    // Parameter 31 12 takes dialog 06 and answer 1 (yes) on both models.
    let answer = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x31, 0x12, 0x06, 0x01, 0xf7]);
    usb.send(endpoints.output, &answer)
        .map_err(|error| error.to_string())?;
    *answered = true;
    println!("L6 Editor confirmed the pending pad assignment dialog.");
    Ok(true)
}
fn l6max_pad_runtime(qmp: &QmpClient) -> Result<String, String> {
    let led_records = qmp
        .read_memory(0, 0x806293f4, 8)
        .map_err(|error| error.to_string())?;
    let active_pad = qmp
        .read_memory(0, 0x80463005, 1)
        .map_err(|error| error.to_string())?;
    let sampler_busy = qmp
        .read_memory(0, 0x80463001, 4)
        .map_err(|error| error.to_string())?;
    let playback_flags = (0..4)
        .map(|pad| qmp.read_memory(0, 0x806ea265 + pad * 0x44, 1))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "LED records={led_records:02x?} active_pad={active_pad:02x?} sampler_busy={sampler_busy:02x?} playback_flags={playback_flags:02x?}"
    ))
}
fn sample_l6max_panel_rows(
    inputs: &InputHub,
    qmp: &QmpClient,
    pad_state_address: u32,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
    duration: Duration,
) -> Result<Vec<L6maxPanelSample>, String> {
    let start = Instant::now();
    let mut samples = Vec::new();
    while start.elapsed() < duration {
        pump(display, latest)?;
        let record = qmp
            .read_memory(0, pad_state_address, 2)
            .map_err(|error| error.to_string())?;
        samples.push((
            start.elapsed().as_millis() as u64,
            inputs.panel.snapshot().indicators,
            [record[0], record[1]],
        ));
        thread::sleep(Duration::from_millis(20));
    }
    Ok(samples)
}
fn summarize_l6max_panel_rows(
    logs: &Path,
    target: u32,
    baseline: [u32; 8],
    held: [u32; 8],
    baseline_pad_record: [u8; 2],
    samples: &[L6maxPanelSample],
) -> Result<(Vec<usize>, [u32; 8], Vec<[u8; 2]>), String> {
    let leds_before = l6max_host::indicators::leds(baseline);
    let mut changed_leds = [false; 72];
    let mut changed_rows = [0u32; 8];
    for rows in std::iter::once(&held).chain(samples.iter().map(|(_, rows, _)| rows)) {
        let leds = l6max_host::indicators::leds(*rows);
        for (index, (changed, led)) in changed_leds.iter_mut().zip(leds).enumerate() {
            *changed |= led != leds_before[index];
        }
        for row in 0..8 {
            changed_rows[row] |= baseline[row] ^ rows[row];
        }
    }
    let changed_led_indices = changed_leds
        .iter()
        .enumerate()
        .filter_map(|(index, changed)| changed.then_some(index))
        .collect::<Vec<_>>();
    let sample_path = logs.join(format!("l6max-pad-target-{target}-led-rows.tsv"));
    let mut sample_text =
        String::from("elapsed_ms\trow0\trow1\trow2\trow3\trow4\trow5\trow6\trow7\tpad_record\n");
    let mut sampled_pad_records = vec![baseline_pad_record];
    for (elapsed_ms, rows, pad_record) in samples {
        sample_text.push_str(&format!(
            "{elapsed_ms}\t{:06x}\t{:06x}\t{:06x}\t{:06x}\t{:06x}\t{:06x}\t{:06x}\t{:06x}\t{:02x}{:02x}\n",
            rows[0], rows[1], rows[2], rows[3], rows[4], rows[5], rows[6], rows[7],
            pad_record[0], pad_record[1]
        ));
        if sampled_pad_records.last() != Some(pad_record) {
            sampled_pad_records.push(*pad_record);
        }
    }
    fs::write(sample_path, sample_text).map_err(|error| error.to_string())?;
    Ok((changed_led_indices, changed_rows, sampled_pad_records))
}
fn wait_trace_count(logs: &Path, pattern: &str, count: usize) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if trace_count(logs, pattern)? >= count {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!(
                "UART report deadline for {pattern:?}: expected {count}"
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn verify_audio_trace(model: l6fw::Model, logs: &Path) -> Result<(), String> {
    let trace = fs::read_to_string(logs.join("engine.trace")).map_err(|e| e.to_string())?;
    let sai_paths: &[(u32, u32, u32)] = match model {
        l6fw::Model::L6 => &[(0x4038_4000, 14, 15), (0x4038_c000, 12, 13)],
        l6fw::Model::L6max => &[(0x4038_4000, 14, 15)],
    };
    let mut latest_enable_ms = 0u64;
    for (base, _, _) in sai_paths {
        let tx_addr = base + 0x08;
        let rx_addr = base + 0x88;
        let tx_enabled = trace
            .lines()
            .filter_map(|line| {
                let expected = format!("write addr={tx_addr:08x}");
                if !line.contains(&expected) || !line.contains("value=80040001") {
                    return None;
                }
                line.split("at=")
                    .nth(1)
                    .and_then(|time| time.strip_suffix("ms"))
                    .and_then(|time| time.parse::<u64>().ok())
            })
            .max();
        let rx_enabled = trace
            .lines()
            .filter_map(|line| {
                let expected = format!("write addr={rx_addr:08x}");
                if !line.contains(&expected) || !line.contains("value=80040001") {
                    return None;
                }
                line.split("at=")
                    .nth(1)
                    .and_then(|time| time.strip_suffix("ms"))
                    .and_then(|time| time.parse::<u64>().ok())
            })
            .max();
        let (Some(tx_ms), Some(rx_ms)) = (tx_enabled, rx_enabled) else {
            return Err(format!(
                "{} SAI at {base:#010x} did not enable TX/RX FIFO DMA requests; run with L6_AUDIO_TRACE=1",
                model_name(model)
            ));
        };
        latest_enable_ms = latest_enable_ms.max(tx_ms).max(rx_ms);
    }

    let post_enable = trace
        .lines()
        .filter(|line| {
            line.split("at=")
                .nth(1)
                .and_then(|time| time.strip_suffix("ms"))
                .and_then(|time| time.parse::<u64>().ok())
                .is_some_and(|time| time >= latest_enable_ms)
        })
        .collect::<Vec<_>>();
    for (base, tx_channel, rx_channel) in sai_paths {
        for (direction, fifo_address, channel) in [
            ("TX", base + 0x20, tx_channel),
            ("RX", base + 0xa0, rx_channel),
        ] {
            if !post_enable
                .iter()
                .any(|line| line.contains(&format!("addr={fifo_address:08x}")))
            {
                return Err(format!(
                    "{} SAI at {base:#010x} had no post-enable {direction} FIFO traffic",
                    model_name(model)
                ));
            }
            let tcd_address = 0x400e_9000 + channel * 0x20;
            if !post_enable
                .iter()
                .any(|line| line.contains(&format!("addr={tcd_address:08x}")))
            {
                return Err(format!(
                    "{} SAI {direction} at {base:#010x} had no eDMA channel {channel} TCD activity",
                    model_name(model)
                ));
            }
        }
    }
    println!(
        "{} clocked audio smoke passed: every configured SAI TX/RX FIFO and its mapped eDMA channel were active.",
        model_name(model)
    );
    Ok(())
}

fn model_name(model: l6fw::Model) -> &'static str {
    match model {
        l6fw::Model::L6 => "L6",
        l6fw::Model::L6max => "L6max",
    }
}

fn audio_post_play_settle() -> Result<Duration, String> {
    let milliseconds = std::env::var("L6_AUDIO_POST_PLAY_SETTLE_MS")
        .ok()
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| "L6_AUDIO_POST_PLAY_SETTLE_MS must be an integer".to_owned())
        })
        .transpose()?
        .unwrap_or(5_000);
    if milliseconds > 30_000 {
        return Err("L6_AUDIO_POST_PLAY_SETTLE_MS must be between 0 and 30000".into());
    }
    Ok(Duration::from_millis(milliseconds.into()))
}

fn run_l6_recorder_record_stop_probe(
    inputs: &InputHub,
    qmp: &QmpClient,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
) -> Result<(), String> {
    let panel_rows = || inputs.panel.snapshot().indicators;
    let transport_led_records = || qmp.read_memory(0, 0x806293c6, 4).map_err(|e| e.to_string());
    let play_stop_state = || qmp.read_memory(0, 0x806293c8, 2).map_err(|e| e.to_string());
    let panel_tap = |target: u32,
                     duration: Duration,
                     display: &PublishedDisplay,
                     latest: &mut Option<Frame>|
     -> Result<(), String> {
        let applied = inputs.panel.snapshot().applied;
        let released = inputs.panel.snapshot().released;
        inputs
            .panel
            .tap(target, duration.as_millis() as u32)
            .map_err(|e| e.to_string())?;
        wait_applied(&inputs.panel, applied + 1, display, latest)?;
        wait_released(&inputs.panel, released + 1)?;
        pump_for(display, latest, duration)
    };

    println!(
        "L6 recorder sequence starting: initial rows={:06x?}, transport LEDs={:02x?}, PlayStop={:02x?}",
        &panel_rows()[..8],
        transport_led_records()?,
        play_stop_state()?
    );
    panel_tap(24, Duration::from_millis(200), display, latest)?;
    println!(
        "L6 recorder after Record start: rows={:06x?}, transport LEDs={:02x?}, PlayStop={:02x?}",
        &panel_rows()[..8],
        transport_led_records()?,
        play_stop_state()?
    );
    pump_for(display, latest, Duration::from_secs(3))?;
    panel_tap(24, Duration::from_millis(200), display, latest)?;
    println!(
        "L6 recorder after Record stop: rows={:06x?}, transport LEDs={:02x?}, PlayStop={:02x?}",
        &panel_rows()[..8],
        transport_led_records()?,
        play_stop_state()?
    );
    pump_for(display, latest, Duration::from_millis(1000))?;
    println!(
        "Skipped recorder playback check: GPIO2.25 submits Play event 0x20, but this no-card probe cannot verify playback or the Play/Stop lamp."
    );
    println!("L6 recorder Record-start/stop probe completed.");
    Ok(())
}

fn run_l6_play_gpio_runtime_probe(
    inputs: &InputHub,
    qmp: &QmpClient,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
    logs: &Path,
    gdb_socket: &Path,
) -> Result<(), String> {
    let play_stop = || qmp.read_memory(0, 0x806293c8, 2).map_err(|e| e.to_string());
    let transport_leds = || qmp.read_memory(0, 0x806293c6, 4).map_err(|e| e.to_string());
    // The diagnostic reaches this path at the end of the boot checks, but L6's
    // GPIO2.25 Play reader is also sampled during startup. Let that scan finish
    // before treating a press as a runtime transport action.
    pump_for(display, latest, Duration::from_secs(5))?;
    let rows_before = inputs.panel.snapshot().indicators;
    let state_before = play_stop()?;
    let leds_before = transport_leds()?;
    let tx_before = trace_count(logs, "l6 audio TX samples sai=00384000")?;
    let tx_trace_before =
        fs::read_to_string(logs.join("engine.trace")).map_err(|e| e.to_string())?;
    let tx_signature = |line: &str| {
        line.split_whitespace()
            .filter(|field| field.starts_with("first=") || field.starts_with("last="))
            .collect::<String>()
    };
    let baseline_tx_signature = tx_trace_before
        .lines()
        .filter(|line| line.starts_with("l6 audio TX samples sai=00384000"))
        .last()
        .map(tx_signature);

    let mut debugger = Gdb::connect(gdb_socket).map_err(|e| e.to_string())?;
    qmp.stop().map_err(|e| e.to_string())?;
    debugger
        .breakpoint(0x80020ca0, true)
        .map_err(|e| e.to_string())?;
    let applied = inputs.main.snapshot().applied;
    inputs.main.down(4, 10_000).map_err(|e| e.to_string())?;
    wait_applied(&inputs.main, applied + 1, display, latest)?;
    let dispatch_deadline = Instant::now() + Duration::from_secs(5);
    let mut dispatch = None;
    while Instant::now() < dispatch_deadline {
        let remaining = dispatch_deadline.saturating_duration_since(Instant::now());
        match debugger.continue_until_stop(remaining) {
            Ok(_) => {
                let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                let lr = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                let register_args = (0..4)
                    .map(|register| debugger.read_register(register))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                let sp = debugger.read_register(13).map_err(|e| e.to_string())?;
                let fifth_argument = u32::from_le_bytes(
                    qmp.read_memory(0, sp, 4)
                        .map_err(|e| e.to_string())?
                        .try_into()
                        .unwrap(),
                );
                if pc != 0x80020ca0 {
                    return Err(format!(
                        "GPIO2.25 Play probe stopped at {pc:#010x}, expected event submit 0x80020ca0"
                    ));
                }
                // Other system events can reach this shared submitter while
                // the input scan is running. L6 Play is event 0x20 from GPIO2.25.
                if register_args[2] == 0x20 {
                    dispatch = Some((pc, lr, register_args, fifth_argument));
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // A timed-out remote continue leaves QEMU running. Interrupt
                // via GDB so subsequent snapshots happen at a known stop.
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "GPIO2.25 Play probe timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
                break;
            }
            Err(error) => {
                return Err(format!(
                    "waiting for GPIO2.25 Play event submission: {error}"
                ));
            }
        }
    }
    debugger
        .breakpoint(0x80020ca0, false)
        .map_err(|e| e.to_string())?;
    let rows_at_submit = inputs.panel.snapshot().indicators;
    let state_at_submit = play_stop()?;
    let leds_at_submit = transport_leds()?;
    let play_event_gate = qmp
        .read_memory(0, 0x8060e4a4, 1)
        .map_err(|error| error.to_string())?;
    let mut play_handler_argument = None;
    let mut play_handler_mode = None;
    let mut play_handler_guards = None;
    let mut play_callback_eligible = false;
    let mut play_callback_hit = None;
    if dispatch.is_some() {
        debugger
            .breakpoint(0x80008a08, true)
            .map_err(|error| error.to_string())?;
        match debugger.continue_until_stop(Duration::from_secs(5)) {
            Ok(_) => {
                let pc = debugger
                    .read_register(15)
                    .map_err(|error| error.to_string())?
                    & !1;
                if pc != 0x80008a08 {
                    return Err(format!(
                        "Play event trace stopped at {pc:#010x}, expected l6_handle_play_button_event 0x80008a08"
                    ));
                }
                play_handler_argument = Some(
                    debugger
                        .read_register(0)
                        .map_err(|error| error.to_string())?,
                );
                play_handler_mode = Some(u32::from_le_bytes(
                    qmp.read_memory(0, 0x80578ebc, 4)
                        .map_err(|error| error.to_string())?
                        .try_into()
                        .unwrap(),
                ));
                let read_u32 = |address| -> Result<u32, String> {
                    Ok(u32::from_le_bytes(
                        qmp.read_memory(0, address, 4)
                            .map_err(|error| error.to_string())?
                            .try_into()
                            .unwrap(),
                    ))
                };
                let recorder_state = read_u32(0x801f5f00)?;
                let route_state = read_u32(0x80443108)?;
                let recorder_option = qmp
                    .read_memory(0, 0x801f8f5a, 1)
                    .map_err(|error| error.to_string())?[0];
                let play_gate = read_u32(0x80578ec0)?;
                let media_state = qmp
                    .read_memory(0, 0x801f8c74, 1)
                    .map_err(|error| error.to_string())?[0];
                let recorder_selection_key = qmp
                    .read_memory(0, 0x806101a8, 8)
                    .map_err(|error| error.to_string())?;
                play_callback_eligible = play_handler_mode == Some(0)
                    && recorder_selection_key.iter().any(|byte| *byte != 0);
                play_handler_guards = Some((
                    recorder_state,
                    route_state,
                    recorder_option,
                    play_gate,
                    media_state,
                    recorder_selection_key,
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "waiting for l6_handle_play_button_event timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
            }
            Err(error) => {
                return Err(format!("waiting for L6 Play handler: {error}"));
            }
        }
        debugger
            .breakpoint(0x80008a08, false)
            .map_err(|error| error.to_string())?;
        if play_callback_eligible {
            debugger
                .breakpoint(0x8004b890, true)
                .map_err(|error| error.to_string())?;
            match debugger.continue_until_stop(Duration::from_secs(5)) {
                Ok(_) => {
                    let pc = debugger
                        .read_register(15)
                        .map_err(|error| error.to_string())?
                        & !1;
                    if pc != 0x8004b890 {
                        return Err(format!(
                            "Play callback trace stopped at {pc:#010x}, expected mode-0 callback 0x8004b890"
                        ));
                    }
                    play_callback_hit = Some(
                        debugger
                            .read_register(0)
                            .map_err(|error| error.to_string())?,
                    );
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    debugger.interrupt().map_err(|interrupt| {
                        format!(
                            "waiting for mode-0 Play callback timed out ({error}) and GDB interrupt failed: {interrupt}"
                        )
                    })?;
                }
                Err(error) => {
                    return Err(format!("waiting for mode-0 Play callback: {error}"));
                }
            }
            debugger
                .breakpoint(0x8004b890, false)
                .map_err(|error| error.to_string())?;
        }
        if play_callback_eligible {
            println!("L6 mode-0 playback callback reached with r0={play_callback_hit:?}");
        }
    }
    let settle_ms = std::env::var("L6_PLAY_SETTLE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_500);
    if settle_ms > 10_000 {
        return Err("L6_PLAY_SETTLE_MS must be between 0 and 10000".into());
    }
    if dispatch.is_some() && settle_ms != 0 {
        // Event submission acknowledges the GPIO scan, not completion of the
        // firmware action or its display/audio updates. Let those settle
        // before taking the post-press sample.
        qmp.resume().map_err(|error| error.to_string())?;
        pump_for(display, latest, Duration::from_millis(settle_ms))?;
        qmp.stop().map_err(|error| error.to_string())?;
    }
    if std::env::var_os("L6_AUDIO_DMA_DUMP").is_some() {
        dump_l6_audio_tx_buffer(qmp, 14, logs)?;
        dump_l6_audio_tx_buffer(qmp, 12, logs)?;
    }
    let rows_after_settle = inputs.panel.snapshot().indicators;
    let state_after_settle = play_stop()?;
    let leds_after_settle = transport_leds()?;
    let read_probe_u32 = |address: u32| -> Option<u32> {
        qmp.read_memory(0, address, 4)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let sai_tx_state = [
        [0x4038_4008, 0x4038_400c, 0x4038_4040],
        [0x4038_c008, 0x4038_c00c, 0x4038_c040],
    ]
    .map(|registers| registers.map(read_probe_u32));
    let edma_erq = read_probe_u32(0x400e_800c);
    let audio_tcds = (12..=15)
        .map(|channel| {
            let address = 0x400e_9180 + (channel - 12) * 0x20;
            let bytes = qmp.read_memory(0, address, 32).ok()?;
            let u32_at =
                |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            let u16_at =
                |offset: usize| u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
            let source = u32_at(0);
            let source_words = qmp.read_memory(0, source, 16).ok().map(|bytes| {
                bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                    .collect::<Vec<_>>()
            });
            Some((
                channel,
                source,
                u32_at(8),
                u32_at(16),
                u16_at(22),
                u16_at(28),
                u16_at(30),
                source_words,
            ))
        })
        .collect::<Vec<_>>();
    let trace = fs::read_to_string(logs.join("engine.trace")).map_err(|e| e.to_string())?;
    let tx_sample_changed_after_press = trace
        .lines()
        .filter(|line| line.starts_with("l6 audio TX samples sai=00384000"))
        .skip(tx_before)
        .any(|line| Some(tx_signature(line)) != baseline_tx_signature);
    let sai1_tx_source_has_audio = audio_tcds
        .iter()
        .flatten()
        .find(|(channel, ..)| *channel == 14)
        .and_then(|(_, _, _, _, _, _, _, source_words)| source_words.as_ref())
        .is_some_and(|words| words.iter().any(|word| *word != 0 && *word != u32::MAX));
    let recorder_audio_verified = dispatch.is_some()
        && play_callback_eligible
        && play_callback_hit.is_some()
        && tx_sample_changed_after_press
        && sai1_tx_source_has_audio;
    if std::env::var_os("L6_REQUIRE_RECORDER_AUDIO").is_some() && !recorder_audio_verified {
        return Err(format!(
            "L6 recorder audio assertion failed: event={:?}, eligible={play_callback_eligible}, callback={play_callback_hit:?}, changing SAI1 TX={tx_sample_changed_after_press}, channel-14 source has sample data={sai1_tx_source_has_audio}",
            dispatch.map(|(_, _, args, _)| args)
        ));
    }
    if recorder_audio_verified {
        println!(
            "L6 recorder audio verified: the mode-0 callback ran, channel-14 TX source contains sample data, and SAI1 TX samples changed after Play."
        );
    }
    println!(
        "L6 runtime Play target 4 (GPIO2.25) held for 5s: event submit={}; event gate={play_event_gate:02x?}; handler argument={play_handler_argument:?}; recorder mode={play_handler_mode:?}; handler guards (0x801f5f00, 0x80443108, option 0x41, 0x80578ec0, 0x801f8c74, recorder selection key 0x806101a8)={play_handler_guards:?}; settle={settle_ms}ms; panel rows={rows_before:06x?}->{rows_at_submit:06x?}->{rows_after_settle:06x?}; transport LEDs={leds_before:02x?}->{leds_at_submit:06x?}->{leds_after_settle:06x?}; PlayStop={state_before:02x?}->{state_at_submit:02x?}->{state_after_settle:02x?}; SAI1 TX sample signature changed after press={tx_sample_changed_after_press}",
        dispatch
            .map(|(pc, lr, register_args, fifth_argument)| {
                let kind = if pc == 0x80020ca0 {
                    "l6 event submit"
                } else {
                    "FreeRTOS xQueueGenericSend"
                };
                format!(
                    "{kind} pc={pc:#010x} lr={lr:#010x} args r0-r3={register_args:08x?}, arg4=[sp]={fifth_argument:#010x}"
                )
            })
            .unwrap_or_else(|| "no Play event-submit hit while held for 5s".to_owned())
    );
    println!(
        "L6 playback peripheral snapshot after settle: SAI registers [TCSR,TCR1,TFR] [SAI1, SAI2]={sai_tx_state:08x?}; eDMA ERQ={edma_erq:08x?}; channels 12-15 TCD (channel, SADDR, NBYTES, DADDR, CITER, CSR, BITER, source words)={audio_tcds:?}"
    );
    Ok(())
}

fn run_l6max_recorder_playback_probe(
    inputs: &InputHub,
    qmp: &QmpClient,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
    logs: &Path,
    gdb_socket: &Path,
) -> Result<(), String> {
    if std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_none() {
        return Err("L6MAX_RECORDER_PLAYBACK_PROBE needs L6_AUDIO_SAMPLE_TRACE=1".into());
    }
    // The saved state should already be past date, battery, and card setup.
    // Let the SAI startup stream establish a baseline before pressing Play.
    pump_for(display, latest, Duration::from_secs(3))?;
    if let Some(value) = std::env::var_os("L6MAX_PLAY_PRE_DELAY_MS") {
        let delay = value
            .to_string_lossy()
            .parse::<u64>()
            .map_err(|error| format!("invalid L6MAX_PLAY_PRE_DELAY_MS: {error}"))?;
        if delay > 120_000 {
            return Err("L6MAX_PLAY_PRE_DELAY_MS must be at most 120000".into());
        }
        if delay > 0 {
            println!("Waiting {delay} ms before L6max recorder Play input");
            pump_for(display, latest, Duration::from_millis(delay))?;
        }
    }
    let mut play_sai1_rx_dma_disabled_before_input = false;
    let tx_before = trace_count(logs, "l6 audio TX samples sai=00384000")?;
    let gpio_reads_before = trace_count(logs, "l6 MAIN GPIO port-read candidate=4")?;
    let mut play_pre_stream_source_scan: Option<(Vec<u32>, u32)> = None;
    let mut debugger = Gdb::connect(gdb_socket).map_err(|error| error.to_string())?;
    qmp.stop().map_err(|error| error.to_string())?;
    if std::env::var_os("L6MAX_TRACE_RECORDER_EVENT_PATH").is_some() {
        trace_l6max_recorder_event_path(inputs, qmp, display, latest, &mut debugger)?;
    }
    if std::env::var_os("L6MAX_TEST_DISABLE_SAI1_RX_DMA_BEFORE_PLAY").is_some() {
        debugger.write(0x400e801a, &[15]).map_err(|error| {
            format!("disabling SAI1 RX eDMA channel 15 before Play input: {error}")
        })?;
        play_sai1_rx_dma_disabled_before_input = true;
    }
    if std::env::var_os("L6MAX_TRACE_STREAM_SCAN_BEFORE_PLAY").is_some() {
        debugger
            .breakpoint(0x80007e08, true)
            .map_err(|error| error.to_string())?;
        match debugger.continue_until_stop(Duration::from_secs(3)) {
            Ok(_) => {
                let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                if pc != 0x80007e08 {
                    return Err(format!(
                        "pre-Play source-scan trace stopped at {pc:#010x}, expected 0x80007e08"
                    ));
                }
                let args = (0..4)
                    .map(|register| debugger.read_register(register))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?;
                let return_address = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                play_pre_stream_source_scan = Some((args, return_address));
                debugger
                    .request("s")
                    .map_err(|error| format!("stepping into pre-Play source scan: {error}"))?;
                debugger
                    .breakpoint(0x80007e08, false)
                    .map_err(|error| error.to_string())?;
                debugger
                    .breakpoint(return_address, true)
                    .map_err(|error| error.to_string())?;
                match debugger.continue_until_stop(Duration::from_secs(20)) {
                    Ok(_) => {
                        let return_pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                        if return_pc != return_address {
                            return Err(format!(
                                "pre-Play source scan returned at {return_pc:#010x}, expected {return_address:#010x}"
                            ));
                        }
                        debugger
                            .breakpoint(return_address, false)
                            .map_err(|error| error.to_string())?;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        debugger.interrupt().map_err(|interrupt| {
                            format!(
                                "pre-Play source scan did not return within 20 seconds ({error}); GDB interrupt failed: {interrupt}"
                            )
                        })?;
                        return Err("pre-Play source scan did not return within 20 seconds".into());
                    }
                    Err(error) => {
                        return Err(format!("waiting for pre-Play source scan return: {error}"));
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "pre-Play source-scan watch timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
                debugger
                    .breakpoint(0x80007e08, false)
                    .map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(format!("waiting for pre-Play source scan: {error}")),
        }
    }
    debugger
        .breakpoint(0x80045458, true)
        .map_err(|error| error.to_string())?;
    let applied = inputs.main.snapshot().applied;
    inputs
        .main
        .down(4, 250)
        .map_err(|error| error.to_string())?;
    wait_applied(&inputs.main, applied + 1, display, latest)?;
    let dispatch_deadline = Instant::now() + Duration::from_secs(5);
    let mut play_event_submission = None;
    while Instant::now() < dispatch_deadline {
        let remaining = dispatch_deadline.saturating_duration_since(Instant::now());
        match debugger.continue_until_stop(remaining) {
            Ok(_) => {
                let pc = debugger
                    .read_register(15)
                    .map_err(|error| error.to_string())?
                    & !1;
                let lr = debugger
                    .read_register(14)
                    .map_err(|error| error.to_string())?
                    & !1;
                if pc != 0x80045458 {
                    return Err(format!(
                        "L6max event trace stopped at {pc:#010x}, expected queue send 0x80045458"
                    ));
                }
                let args = (0..4)
                    .map(|register| debugger.read_register(register))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?;
                let sp = debugger
                    .read_register(13)
                    .map_err(|error| error.to_string())?;
                let fifth = u32::from_le_bytes(
                    qmp.read_memory(0, sp, 4)
                        .map_err(|error| error.to_string())?
                        .try_into()
                        .unwrap(),
                );
                if args[2] == 0x2c {
                    play_event_submission = Some((pc, lr, args, fifth));
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6max Play queue-send trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
                break;
            }
            Err(error) => return Err(format!("waiting for L6max Play queue send: {error}")),
        }
    }
    debugger
        .breakpoint(0x80045458, false)
        .map_err(|error| error.to_string())?;
    if let Some((pc, lr, args, fifth)) = &play_event_submission {
        println!(
            "L6max Play event submission call reached at pc={pc:#010x} lr={lr:#010x}: arguments={args:08x?}, fifth={fifth:#010x}"
        );
    }
    println!(
        "L6max SAI1 RX eDMA channel 15 disabled before Play input: {play_sai1_rx_dma_disabled_before_input}"
    );
    if std::env::var_os("L6MAX_REQUIRE_PLAY_SUBMISSION").is_some()
        && play_event_submission.is_none()
    {
        return Err("L6max Play event 0x2c did not reach its submission callsite".into());
    }
    let mut play_submit_return = None;
    if play_event_submission.is_some() {
        debugger
            .breakpoint(0x800280a0, true)
            .map_err(|error| error.to_string())?;
        match debugger.continue_until_stop(Duration::from_secs(5)) {
            Ok(_) => {
                let pc = debugger
                    .read_register(15)
                    .map_err(|error| error.to_string())?
                    & !1;
                if pc != 0x800280a0 {
                    return Err(format!(
                        "L6max Play helper trace stopped at {pc:#010x}, expected return 0x800280a0"
                    ));
                }
                play_submit_return = Some(
                    debugger
                        .read_register(0)
                        .map_err(|error| error.to_string())?,
                );
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6max Play helper return trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
            }
            Err(error) => return Err(format!("waiting for L6max Play helper return: {error}")),
        }
        debugger
            .breakpoint(0x800280a0, false)
            .map_err(|error| error.to_string())?;
    }
    if std::env::var_os("L6MAX_REQUIRE_PLAY_SUBMIT_RETURN").is_some()
        && play_submit_return != Some(0)
    {
        return Err(format!(
            "L6max Play queue send did not return success 0 at 0x800280a0: result={play_submit_return:?}"
        ));
    }
    if let Some(result) = play_submit_return {
        println!("L6max Play submission helper returned r0={result:#010x}");
    }
    let mut play_handler_fields = None;
    let mut play_callback_latch_before = None;
    let mut play_notification_active_before = None;
    if play_submit_return == Some(0) {
        // Capture the queued event at its class-0 main-button handler selected by the event loop.
        debugger
            .breakpoint(0x80036a40, true)
            .map_err(|error| error.to_string())?;
        let dispatch_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < dispatch_deadline {
            let remaining = dispatch_deadline.saturating_duration_since(Instant::now());
            match debugger.continue_until_stop(remaining) {
                Ok(_) => {
                    let pc = debugger
                        .read_register(15)
                        .map_err(|error| error.to_string())?
                        & !1;
                    if pc != 0x80036a40 {
                        return Err(format!(
                            "L6max event-dispatch trace stopped at {pc:#010x}, expected main-button handler 0x80036a40"
                        ));
                    }
                    let event_ptr = debugger
                        .read_register(0)
                        .map_err(|error| error.to_string())?;
                    let event_bytes = qmp
                        .read_memory(0, event_ptr, 20)
                        .map_err(|error| error.to_string())?;
                    let fields: [u32; 5] = std::array::from_fn(|index| {
                        u32::from_le_bytes(
                            event_bytes[index * 4..index * 4 + 4].try_into().unwrap(),
                        )
                    });
                    if fields[0] == 0 && fields[2] == 0x2c {
                        play_handler_fields = Some(fields);
                        play_callback_latch_before = Some(
                            qmp.read_memory(0, 0x805db2e2, 1)
                                .map_err(|error| error.to_string())?[0],
                        );
                        play_notification_active_before = Some(
                            qmp.read_memory(0, 0x8020a2c0, 1)
                                .map_err(|error| error.to_string())?[0],
                        );
                        break;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    debugger.interrupt().map_err(|interrupt| {
                        format!(
                            "L6max dispatcher trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                        )
                    })?;
                    break;
                }
                Err(error) => {
                    return Err(format!("waiting for L6max event dispatcher: {error}"));
                }
            }
        }
        debugger
            .breakpoint(0x80036a40, false)
            .map_err(|error| error.to_string())?;
    }
    if std::env::var_os("L6MAX_REQUIRE_PLAY_HANDLER").is_some() && play_handler_fields.is_none() {
        return Err(format!(
            "L6max Play event did not reach main-button handler as route 0/code 0x2c: fields={play_handler_fields:?}"
        ));
    }
    if let Some(fields) = play_handler_fields {
        println!(
            "L6max Play event reached main-button handler 0x80036a40: fields={fields:08x?}, notification-active={play_notification_active_before:?}, callback-slot-2 latch before dispatch={play_callback_latch_before:?}"
        );
    }
    let mut play_callback_route = None;
    let mut play_callback_target_hit = false;
    let mut play_recorder_action_hit = false;
    let mut play_recorder_action_gates = None;
    let mut play_recorder_paths = None;
    let mut play_recorder_scan_globals = None;
    let mut play_recorder_post_audio_state = None;
    let mut play_stream_read = None;
    let mut play_stream_open = None;
    let mut play_stream_window_update = None;
    let mut play_stream_source_scan = None;
    let mut play_stream_state_update: Option<(Vec<u32>, u32)> = None;
    let mut play_recorder_audio_clock_result = None;
    let mut play_recorder_gate_checks: Vec<(u32, u32, u32)> = Vec::new();
    if play_handler_fields.is_some() {
        // The Play case selects callback slot 2 through this scheduler.
        debugger
            .breakpoint(0x8008bd90, true)
            .map_err(|error| error.to_string())?;
        let callback_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < callback_deadline {
            let remaining = callback_deadline.saturating_duration_since(Instant::now());
            match debugger.continue_until_stop(remaining) {
                Ok(_) => {
                    let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                    if pc != 0x8008bd90 {
                        return Err(format!(
                            "L6max callback trace stopped at {pc:#010x}, expected 0x8008bd90"
                        ));
                    }
                    let callback = debugger.read_register(0).map_err(|e| e.to_string())?;
                    let state = debugger.read_register(1).map_err(|e| e.to_string())?;
                    let lr = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                    if callback == 2 && (0x80036a40..0x8003728c).contains(&lr) {
                        let read_u32 = |address| -> Result<u32, String> {
                            Ok(u32::from_le_bytes(
                                qmp.read_memory(0, address, 4)
                                    .map_err(|error| error.to_string())?
                                    .try_into()
                                    .unwrap(),
                            ))
                        };
                        let window = read_u32(0x80202610)?;
                        let callback_table = read_u32(window + 24)?;
                        let callback_target = read_u32(callback_table + 8)? & !1;
                        play_callback_route = Some((callback, state, lr, callback_target));
                        break;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    debugger.interrupt().map_err(|interrupt| format!("L6max Play callback trace timed out ({error}) and GDB interrupt failed: {interrupt}"))?;
                    break;
                }
                Err(error) => {
                    return Err(format!(
                        "waiting for L6max Play callback scheduling: {error}"
                    ));
                }
            }
        }
        debugger
            .breakpoint(0x8008bd90, false)
            .map_err(|e| e.to_string())?;
    }
    if std::env::var_os("L6MAX_REQUIRE_PLAY_CALLBACK_QUEUE").is_some()
        && play_callback_route.is_none()
    {
        return Err("L6max Play handler did not schedule callback slot 2".into());
    }
    if let Some((_, _, _, callback_target)) = play_callback_route {
        if callback_target != 0 {
            debugger
                .breakpoint(callback_target, true)
                .map_err(|error| error.to_string())?;
            match debugger.continue_until_stop(Duration::from_secs(5)) {
                Ok(_) => {
                    let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                    if pc != callback_target {
                        return Err(format!(
                            "L6max Play callback target trace stopped at {pc:#010x}, expected {callback_target:#010x}"
                        ));
                    }
                    play_callback_target_hit = true;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    debugger.interrupt().map_err(|interrupt| {
                        format!(
                            "L6max Play callback target trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                        )
                    })?;
                }
                Err(error) => {
                    return Err(format!("waiting for L6max Play callback target: {error}"));
                }
            }
            debugger
                .breakpoint(callback_target, false)
                .map_err(|error| error.to_string())?;
            if play_callback_target_hit {
                // The selected window callback can pass through a shared state gate
                // before it invokes the recorder Play action.
                debugger
                    .breakpoint(0x80046d68, true)
                    .map_err(|error| error.to_string())?;
                match debugger.continue_until_stop(Duration::from_secs(5)) {
                    Ok(_) => {
                        let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                        if pc != 0x80046d68 {
                            return Err(format!(
                                "L6max recorder action trace stopped at {pc:#010x}, expected 0x80046d68"
                            ));
                        }
                        play_recorder_action_hit = true;
                        let read_u32 = |address| -> Result<u32, String> {
                            Ok(u32::from_le_bytes(
                                qmp.read_memory(0, address, 4)
                                    .map_err(|error| error.to_string())?
                                    .try_into()
                                    .unwrap(),
                            ))
                        };
                        let read_byte = |address| -> Result<u8, String> {
                            Ok(qmp
                                .read_memory(0, address, 1)
                                .map_err(|error| error.to_string())?[0])
                        };
                        let read_u64 = |address| -> Result<u64, String> {
                            Ok(u64::from_le_bytes(
                                qmp.read_memory(0, address, 8)
                                    .map_err(|error| error.to_string())?
                                    .try_into()
                                    .unwrap(),
                            ))
                        };
                        let read_utf16 = |address| -> Result<String, String> {
                            let bytes = qmp
                                .read_memory(0, address, 128)
                                .map_err(|error| error.to_string())?;
                            let units = bytes
                                .chunks_exact(2)
                                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                                .take_while(|unit| *unit != 0)
                                .collect::<Vec<_>>();
                            Ok(String::from_utf16_lossy(&units))
                        };
                        play_recorder_action_gates = Some((
                            read_byte(0x80463005)?,
                            read_u32(0x80462d74)?,
                            read_byte(0x8020a5ca)?,
                            read_byte(0x8020a28e)?,
                            read_byte(0x8020a9ec)?,
                            read_u32(0x8020a9f0)?,
                            read_u64(0x805c7c28)?,
                            read_u64(0x20014b40)?,
                        ));
                        play_recorder_paths =
                            Some((read_utf16(0x805c7a00)?, read_utf16(0x805c7c00)?));
                        play_recorder_scan_globals = Some((
                            read_u32(0x80460b9c)?,
                            read_u32(0x80460ba0)?,
                            read_byte(0x8045ff84)?,
                        ));
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        debugger.interrupt().map_err(|interrupt| {
                            format!(
                                "L6max recorder action trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                            )
                        })?;
                    }
                    Err(error) => {
                        return Err(format!("waiting for L6max recorder Play action: {error}"));
                    }
                }
                debugger
                    .breakpoint(0x80046d68, false)
                    .map_err(|error| error.to_string())?;
                // The recorder Play action returns unless its gate returns -1. Stop at
                // the instruction immediately after that call to record the
                // exact runtime gate value before the branch executes.
                if play_recorder_action_hit {
                    debugger
                        .breakpoint(0x80046d6e, true)
                        .map_err(|error| error.to_string())?;
                    match debugger.continue_until_stop(Duration::from_secs(3)) {
                        Ok(_) => {
                            let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                            if pc != 0x80046d6e {
                                return Err(format!(
                                    "recorder clock-gate trace stopped at {pc:#010x}, expected 0x80046d6e"
                                ));
                            }
                            play_recorder_audio_clock_result =
                                Some(debugger.read_register(0).map_err(|e| e.to_string())?);
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            debugger.interrupt().map_err(|interrupt| {
                                format!(
                                    "recorder clock-gate trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                                )
                            })?;
                        }
                        Err(error) => {
                            return Err(format!("waiting for recorder clock-gate result: {error}"));
                        }
                    }
                    debugger
                        .breakpoint(0x80046d6e, false)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
    }
    if std::env::var_os("L6MAX_REQUIRE_PLAY_CALLBACK_RUN").is_some() && !play_callback_target_hit {
        return Err(format!(
            "L6max Play callback target was not entered: route={play_callback_route:?}"
        ));
    }
    if std::env::var_os("L6MAX_REQUIRE_RECORDER_ACTION").is_some() && !play_recorder_action_hit {
        return Err(format!(
            "L6max Play callback did not enter recorder action 0x80046d68: route={play_callback_route:?}, callback_entered={play_callback_target_hit}"
        ));
    }
    if let Some((callback, state, lr, callback_target)) = play_callback_route {
        println!(
            "L6max Play handler scheduled callback slot {callback} with state={state:#010x} at caller {lr:#010x}; callback target={callback_target:#010x}, entered={play_callback_target_hit}; recorder action 0x80046d68 entered={play_recorder_action_hit}, gates={play_recorder_action_gates:?}, file-path-buffers={play_recorder_paths:?}, scanner-config-globals={play_recorder_scan_globals:?}"
        );
    }
    let mut play_callback_queue_trace: Vec<(
        String,
        String,
        String,
        String,
        Vec<String>,
        Option<Vec<String>>,
        u32,
    )> = Vec::new();
    let mut play_callback_queue_send_return = None;
    let mut play_callback_queue_send_stop = None;
    let mut play_sai1_rx_dma_disabled = false;
    let mut play_callback_queue_after_settle: Option<String> = None;
    let mut play_recplay_task_state: Option<String> = None;
    let mut play_stream_task_states = Vec::new();
    let mut play_stream_queue_state: Option<String> = None;
    let mut play_stream_channel_state: Option<String> = None;
    let mut play_stream_channel_metadata: Option<String> = None;
    let mut play_sai_register_state: Option<String> = None;
    let mut play_sai_edma_tcd_state: Option<String> = None;
    let mut play_callback_dispatches = Vec::new();
    if play_recorder_action_hit && std::env::var_os("L6MAX_TRACE_RECORDER_QUEUE").is_some() {
        // Observe FreeRTOS queue send/receive calls made by the recorder task
        // cluster. At each wrapper entry, r0 is the queue and r1 points to the
        // 32-byte message; LR identifies the firmware callsite.
        let trace_points = [
            0x8005f5d8, 0x8005f588, 0x800469f4, 0x80046a18, 0x80046a1c, 0x80046a1e, 0x80046d80,
            0x8004e7f0, 0x8000f7a0, 0x80046ea2, 0x80046ebc, 0x80046ec2, 0x80046f2a, 0x80046f32,
            0x80046f3a,
        ];
        for address in trace_points {
            debugger
                .breakpoint(address, true)
                .map_err(|error| error.to_string())?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && play_callback_queue_trace.len() < 32 {
            match debugger.continue_until_stop(deadline.saturating_duration_since(Instant::now())) {
                Ok(_) => {
                    let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                    if !trace_points.contains(&pc) {
                        return Err(format!(
                            "recorder queue trace stopped at {pc:#010x}, expected {trace_points:08x?}"
                        ));
                    }
                    if matches!(
                        pc,
                        0x80046a18 | 0x80046a1c | 0x80046a1e | 0x80046d80 | 0x8004e7f0 | 0x8000f7a0
                    ) {
                        let args = (0..4)
                            .map(|register| debugger.read_register(register))
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|error| error.to_string())?;
                        let lr = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                        if pc == 0x8000f7a0 {
                            play_stream_state_update = Some((args, lr));
                        } else {
                            play_callback_dispatches.push((pc, args, lr));
                        }
                        continue;
                    }
                    if matches!(
                        pc,
                        0x80046ea2 | 0x80046ebc | 0x80046ec2 | 0x80046f2a | 0x80046f32 | 0x80046f3a
                    ) {
                        let r0 = debugger.read_register(0).map_err(|e| e.to_string())?;
                        let r1 = debugger.read_register(1).map_err(|e| e.to_string())?;
                        play_recorder_gate_checks.push((pc, r0, r1));
                        // Step past the matched return-site instruction so
                        // the still-installed breakpoint does not trap the
                        // same instruction again before the following guard.
                        debugger.request("s").map_err(|error| {
                            format!("stepping recorder gate at {pc:#010x}: {error}")
                        })?;
                        continue;
                    }
                    let (queue, buffer, caller, operation) = if pc == 0x800469f4 {
                        let queue = u32::from_le_bytes(
                            qmp.read_memory(0, 0x8020a9d8, 4)
                                .map_err(|e| e.to_string())?
                                .try_into()
                                .unwrap(),
                        );
                        (
                            queue,
                            debugger.read_register(4).map_err(|e| e.to_string())?,
                            pc,
                            "receive-return",
                        )
                    } else {
                        (
                            debugger.read_register(0).map_err(|e| e.to_string())?,
                            debugger.read_register(1).map_err(|e| e.to_string())?,
                            debugger.read_register(14).map_err(|e| e.to_string())? & !1,
                            if pc == 0x8005f5d8 { "send" } else { "receive" },
                        )
                    };
                    if !(0x80046000..0x80048000).contains(&caller) {
                        continue;
                    }
                    if pc == 0x8005f5d8
                        && caller == 0x80046cd8
                        && play_callback_queue_send_return.is_none()
                    {
                        debugger
                            .breakpoint(caller, true)
                            .map_err(|error| error.to_string())?;
                        let return_deadline = Instant::now() + Duration::from_millis(500);
                        while Instant::now() < return_deadline {
                            match debugger.continue_until_stop(
                                return_deadline.saturating_duration_since(Instant::now()),
                            ) {
                                Ok(_) => {
                                    let return_pc =
                                        debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                                    if return_pc == caller {
                                        play_callback_queue_send_return = Some(
                                            debugger.read_register(0).map_err(|e| e.to_string())?,
                                        );
                                        break;
                                    }
                                    if return_pc == 0x80046d80 || return_pc == 0x8004e7f0 {
                                        let args = (0..4)
                                            .map(|register| debugger.read_register(register))
                                            .collect::<Result<Vec<_>, _>>()
                                            .map_err(|error| error.to_string())?;
                                        let lr = debugger
                                            .read_register(14)
                                            .map_err(|e| e.to_string())?
                                            & !1;
                                        play_callback_dispatches.push((return_pc, args, lr));
                                        continue;
                                    }
                                    if !trace_points.contains(&return_pc) {
                                        return Err(format!(
                                            "recorder queue send return trace stopped at {return_pc:#010x}"
                                        ));
                                    }
                                }
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) =>
                                {
                                    debugger.interrupt().map_err(|interrupt| {
                                        format!(
                                            "recorder queue send return trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                                        )
                                    })?;
                                    let pc =
                                        debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                                    let lr =
                                        debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                                    play_callback_queue_send_stop = Some((pc, lr));
                                    break;
                                }
                                Err(error) => {
                                    return Err(format!(
                                        "waiting for recorder queue send result: {error}"
                                    ));
                                }
                            }
                        }
                        debugger
                            .breakpoint(caller, false)
                            .map_err(|error| error.to_string())?;
                    }
                    let payload = qmp.read_memory(0, buffer, 32).map_err(|e| e.to_string())?;
                    let words = payload
                        .chunks_exact(4)
                        .map(|word| {
                            format!("{:#010x}", u32::from_le_bytes(word.try_into().unwrap()))
                        })
                        .collect::<Vec<_>>();
                    let queue_state = qmp.read_memory(0, queue, 80).ok().map(|bytes| {
                        bytes
                            .chunks_exact(4)
                            .map(|word| {
                                format!("{:#010x}", u32::from_le_bytes(word.try_into().unwrap()))
                            })
                            .collect::<Vec<_>>()
                    });
                    let event = (
                        operation.to_string(),
                        format!("{caller:#010x}"),
                        format!("{queue:#010x}"),
                        format!("{buffer:#010x}"),
                        words,
                        queue_state,
                    );
                    let repeated = play_callback_queue_trace.last().is_some_and(|last| {
                        last.0 == event.0
                            && last.1 == event.1
                            && last.2 == event.2
                            && last.3 == event.3
                            && last.4 == event.4
                            && last.5 == event.5
                    });
                    if repeated {
                        play_callback_queue_trace.last_mut().unwrap().6 += 1;
                    } else {
                        play_callback_queue_trace
                            .push((event.0, event.1, event.2, event.3, event.4, event.5, 1u32));
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    debugger.interrupt().map_err(|interrupt| {
                        format!(
                            "recorder callback queue trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                        )
                    })?;
                    break;
                }
                Err(error) => {
                    return Err(format!("waiting for recorder callback queue: {error}"));
                }
            }
        }
        if std::env::var_os("L6MAX_TEST_DISABLE_SAI1_RX_DMA_AFTER_PLAY_QUEUE_TRACE").is_some() {
            debugger.write(0x400e801a, &[15]).map_err(|error| {
                format!("disabling SAI1 RX eDMA channel 15 after Play queue trace: {error}")
            })?;
            play_sai1_rx_dma_disabled = true;
        }
        for address in trace_points {
            debugger
                .breakpoint(address, false)
                .map_err(|error| error.to_string())?;
        }
    }
    if play_recorder_action_hit && std::env::var_os("L6MAX_TRACE_STREAM_SCAN").is_some() {
        debugger
            .breakpoint(0x80007e08, true)
            .map_err(|error| error.to_string())?;
        match debugger.continue_until_stop(Duration::from_secs(3)) {
            Ok(_) => {
                let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                if pc == 0x80007e08 {
                    let args = (0..4)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    let lr = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                    play_stream_source_scan = Some((args, lr));
                } else {
                    return Err(format!(
                        "L6max source-scan trace stopped at {pc:#010x}, expected scanner entry 0x80007e08"
                    ));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6max source-scan trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
            }
            Err(error) => return Err(format!("waiting for L6max source scan: {error}")),
        }
        debugger
            .breakpoint(0x80007e08, false)
            .map_err(|error| error.to_string())?;
    }
    if play_recorder_action_hit && std::env::var_os("L6MAX_TRACE_STREAM_WINDOW_UPDATE").is_some() {
        debugger
            .breakpoint(0x8000f7e0, true)
            .map_err(|error| error.to_string())?;
        match debugger.continue_until_stop(Duration::from_secs(3)) {
            Ok(_) => {
                let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                if pc == 0x8000f7e0 {
                    let args = (0..4)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    let lr = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                    play_stream_window_update = Some((args, lr));
                } else {
                    return Err(format!(
                        "L6max stream-window trace stopped at {pc:#010x}, expected updater entry 0x8000f7e0"
                    ));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6max stream-window trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
            }
            Err(error) => return Err(format!("waiting for L6max stream-window update: {error}")),
        }
        debugger
            .breakpoint(0x8000f7e0, false)
            .map_err(|error| error.to_string())?;
    }
    if play_recorder_action_hit && std::env::var_os("L6MAX_TRACE_STREAM_OPEN").is_some() {
        debugger
            .breakpoint(0x8004a250, true)
            .map_err(|error| error.to_string())?;
        match debugger.continue_until_stop(Duration::from_secs(3)) {
            Ok(_) => {
                let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                if pc == 0x8004a250 {
                    let args = (0..4)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    let lr = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                    play_stream_open = Some((args, lr));
                } else {
                    return Err(format!(
                        "L6max stream-open trace stopped at {pc:#010x}, expected wrapper entry 0x8004a250"
                    ));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6max stream-open trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
            }
            Err(error) => return Err(format!("waiting for L6max stream-file open: {error}")),
        }
        debugger
            .breakpoint(0x8004a250, false)
            .map_err(|error| error.to_string())?;
    }
    if play_recorder_action_hit && std::env::var_os("L6MAX_TRACE_STREAM_READ").is_some() {
        debugger
            .breakpoint(0x8004a130, true)
            .map_err(|error| error.to_string())?;
        match debugger.continue_until_stop(Duration::from_secs(3)) {
            Ok(_) => {
                let pc = debugger.read_register(15).map_err(|e| e.to_string())? & !1;
                if pc == 0x8004a130 {
                    let args = (0..4)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    let lr = debugger.read_register(14).map_err(|e| e.to_string())? & !1;
                    play_stream_read = Some((args, lr));
                } else {
                    return Err(format!(
                        "L6max stream-read trace stopped at {pc:#010x}, expected worker callsite 0x8004a130"
                    ));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6max stream-read trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                    )
                })?;
            }
            Err(error) => return Err(format!("waiting for L6max stream-read worker: {error}")),
        }
        debugger
            .breakpoint(0x8004a130, false)
            .map_err(|error| error.to_string())?;
    }
    qmp.resume().map_err(|error| error.to_string())?;
    let settle_ms = std::env::var("L6MAX_RECORDER_SETTLE_MS")
        .unwrap_or_else(|_| "5000".to_string())
        .parse::<u64>()
        .map_err(|error| format!("invalid L6MAX_RECORDER_SETTLE_MS: {error}"))?;
    pump_for(display, latest, Duration::from_millis(settle_ms))?;
    if std::env::var_os("L6MAX_TRACE_RECORDER_QUEUE").is_some() {
        qmp.stop().map_err(|error| error.to_string())?;
        let queue = u32::from_le_bytes(
            qmp.read_memory(0, 0x8020a9d8, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let queue_state = qmp.read_memory(0, queue, 80).map_err(|e| e.to_string())?;
        let queue_words = queue_state
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        let semaphore = u32::from_le_bytes(
            qmp.read_memory(0, 0x8020a9e0, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let semaphore_state = qmp
            .read_memory(0, semaphore, 80)
            .map_err(|error| error.to_string())?;
        let semaphore_words = semaphore_state
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        let callback_busy = u32::from_le_bytes(
            qmp.read_memory(0, 0x8020a9f0, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let runtime_pc = debugger
            .read_register(15)
            .map_err(|error| error.to_string())?
            & !1;
        let runtime_lr = debugger
            .read_register(14)
            .map_err(|error| error.to_string())?
            & !1;
        let runtime_sp = debugger
            .read_register(13)
            .map_err(|error| error.to_string())?;
        let current_tcb = u32::from_le_bytes(
            qmp.read_memory(0, 0x808a3af0, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let tcb = qmp
            .read_memory(0, current_tcb, 148)
            .map_err(|error| error.to_string())?;
        let tcb_word =
            |index: usize| u32::from_le_bytes(tcb[index * 4..index * 4 + 4].try_into().unwrap());
        let saved_sp = tcb_word(0);
        let task_priority = tcb_word(11);
        let stack_base = tcb_word(12);
        let stack_end = tcb_word(25);
        let task_name = String::from_utf8_lossy(&tcb[52..100])
            .split('\0')
            .next()
            .unwrap_or_default()
            .to_string();
        let heap_start = 0x808a3000u32;
        let heap_length = 0x80000usize;
        let task_name_bytes = b"RecPlayCtl\0";
        let mut found_recplay_tcb = None;
        let mut heap_offset = 0usize;
        while heap_offset < heap_length {
            let chunk_size = (heap_length - heap_offset).min(4096);
            let chunk = qmp
                .read_memory(0, heap_start + heap_offset as u32, chunk_size)
                .map_err(|error| error.to_string())?;
            if let Some(name_offset) = chunk
                .windows(task_name_bytes.len())
                .position(|window| window == task_name_bytes)
            {
                let tcb_address = heap_start + heap_offset as u32 + name_offset as u32 - 52;
                let candidate = qmp
                    .read_memory(0, tcb_address, 148)
                    .map_err(|error| error.to_string())?;
                if candidate.get(52..52 + task_name_bytes.len()) == Some(task_name_bytes) {
                    found_recplay_tcb = Some((tcb_address, candidate));
                    break;
                }
            }
            heap_offset += 4080;
        }
        if let Some((recplay_tcb_address, recplay_tcb)) = found_recplay_tcb {
            let recplay_word = |index: usize| {
                u32::from_le_bytes(recplay_tcb[index * 4..index * 4 + 4].try_into().unwrap())
            };
            let recplay_saved_sp = recplay_word(0);
            let recplay_priority = recplay_word(11);
            let recplay_stack_base = recplay_word(12);
            let recplay_stack_end = recplay_word(25);
            let recplay_state_container = recplay_word(5);
            let recplay_event_container = recplay_word(10);
            let recplay_state_value = recplay_word(1);
            let tick_count = u32::from_le_bytes(
                qmp.read_memory(0, 0x8020ab4c, 4)
                    .map_err(|error| error.to_string())?
                    .try_into()
                    .unwrap(),
            );
            let recplay_ready_list = 0x808a3af4 + recplay_priority * 20;
            let recplay_stack = qmp
                .read_memory(0, recplay_saved_sp, 160)
                .map_err(|error| error.to_string())?;
            let stack_word = |index: usize| {
                u32::from_le_bytes(recplay_stack[index * 4..index * 4 + 4].try_into().unwrap())
            };
            let exc_return = stack_word(8);
            let stacked_pc_index = if exc_return & 0x10 == 0 { 31 } else { 15 };
            let recplay_saved_lr = stack_word(stacked_pc_index - 1) & !1;
            let recplay_saved_pc = stack_word(stacked_pc_index) & !1;
            fs::write(
                logs.join("l6max-recorder-recplayctl-stack.bin"),
                recplay_stack,
            )
            .map_err(|error| error.to_string())?;
            play_recplay_task_state = Some(format!(
                "tcb={recplay_tcb_address:#010x}, priority={recplay_priority}, state_container={recplay_state_container:#010x}, event_container={recplay_event_container:#010x}, state_item_value={recplay_state_value}, tick_count={tick_count}, ready_list={recplay_ready_list:#010x}, ready={}, saved_sp={recplay_saved_sp:#010x}, task_stack=[{recplay_stack_base:#010x},{recplay_stack_end:#010x}], stacked_pc={recplay_saved_pc:#010x}, stacked_lr={recplay_saved_lr:#010x}, exc_return={exc_return:#010x}",
                recplay_state_container == recplay_ready_list
            ));
        }
        for task_name_bytes in [&b"StreamFileAccess\0"[..], &b"StreamPreLoad\0"[..]] {
            let mut found_task = None;
            let mut heap_offset = 0usize;
            while heap_offset < heap_length {
                let chunk_size = (heap_length - heap_offset).min(4096);
                let chunk = qmp
                    .read_memory(0, heap_start + heap_offset as u32, chunk_size)
                    .map_err(|error| error.to_string())?;
                if let Some(name_offset) = chunk
                    .windows(task_name_bytes.len())
                    .position(|window| window == task_name_bytes)
                {
                    if name_offset >= 52 {
                        let tcb_address = heap_start + heap_offset as u32 + name_offset as u32 - 52;
                        let candidate = qmp
                            .read_memory(0, tcb_address, 148)
                            .map_err(|error| error.to_string())?;
                        if candidate.get(52..52 + task_name_bytes.len()) == Some(task_name_bytes) {
                            found_task = Some((tcb_address, candidate));
                            break;
                        }
                    }
                }
                heap_offset += 4080;
            }
            if let Some((tcb_address, task_tcb)) = found_task {
                let word = |index: usize| {
                    u32::from_le_bytes(task_tcb[index * 4..index * 4 + 4].try_into().unwrap())
                };
                let saved_sp = word(0);
                let priority = word(11);
                let state_container = word(5);
                let event_container = word(10);
                let state_value = word(1);
                let ready_list = 0x808a3af4 + priority * 20;
                let stack = qmp
                    .read_memory(0, saved_sp, 208)
                    .map_err(|error| error.to_string())?;
                let stack_word = |index: usize| {
                    u32::from_le_bytes(stack[index * 4..index * 4 + 4].try_into().unwrap())
                };
                let exc_return = stack_word(8);
                let pc_index = if exc_return & 0x10 == 0 { 31 } else { 15 };
                let saved_lr = stack_word(pc_index - 1) & !1;
                let saved_pc = stack_word(pc_index) & !1;
                let task_name = String::from_utf8_lossy(&task_tcb[52..100])
                    .split('\0')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                fs::write(
                    logs.join(format!("l6max-recorder-{task_name}-stack.bin")),
                    &stack,
                )
                .map_err(|error| error.to_string())?;
                play_stream_task_states.push(format!(
                    "{task_name}: tcb={tcb_address:#010x}, priority={priority}, state_container={state_container:#010x}, event_container={event_container:#010x}, state_item_value={state_value}, ready={}, saved_sp={saved_sp:#010x}, stacked_pc={saved_pc:#010x}, stacked_lr={saved_lr:#010x}, exc_return={exc_return:#010x}",
                    state_container == ready_list
                ));
            }
        }
        let stream_queue = u32::from_le_bytes(
            qmp.read_memory(0, 0x8020a928, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let stream_queue_bytes = qmp
            .read_memory(0, stream_queue, 80)
            .map_err(|error| error.to_string())?;
        let stream_queue_words = stream_queue_bytes
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        let preload_callback = u32::from_le_bytes(
            qmp.read_memory(0, 0x807361dc, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let preload_control = qmp
            .read_memory(0, 0x8020a268, 0x14)
            .map_err(|error| error.to_string())?;
        let preload_word = |offset: usize| {
            u32::from_le_bytes(preload_control[offset..offset + 4].try_into().unwrap())
        };
        let preload_window = (preload_word(8), preload_word(12));
        let preload_skip_flag = preload_control[16];
        let stream_file_read_fn = u32::from_le_bytes(
            qmp.read_memory(0, 0x8073822c, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let stream_channel_mask = u32::from_le_bytes(
            qmp.read_memory(0, 0x80736530, 4)
                .map_err(|error| error.to_string())?
                .try_into()
                .unwrap(),
        );
        let mut stream_channels = Vec::new();
        for channel in 0..14u32 {
            let read_word = |address| -> Result<u32, String> {
                Ok(u32::from_le_bytes(
                    qmp.read_memory(0, address, 4)
                        .map_err(|error| error.to_string())?
                        .try_into()
                        .unwrap(),
                ))
            };
            stream_channels.push((
                channel,
                read_word(0x80738244 + channel * 4)?,
                read_word(0x80736548 + channel * 4)?,
                read_word(0x80736580 + channel * 4)?,
                read_word(0x807365b8 + channel * 4)?,
            ));
        }
        play_stream_channel_state = Some(format!(
            "channel_mask={stream_channel_mask:#010x}, read_fn={stream_file_read_fn:#010x}, channels(index,handle,offset,remaining,state)={stream_channels:?}"
        ));
        let recorder_channels = qmp
            .read_memory(0, 0x805cec68, 14 * 0x30)
            .map_err(|error| error.to_string())?;
        fs::write(
            logs.join("l6max-stream-channel-metadata.bin"),
            &recorder_channels,
        )
        .map_err(|error| error.to_string())?;
        let recorder_channel_summary = (0..14usize)
            .map(|channel| {
                let base = channel * 0x30;
                let words = (0..12usize)
                    .map(|word| {
                        u32::from_le_bytes(
                            recorder_channels[base + word * 4..base + word * 4 + 4]
                                .try_into()
                                .unwrap(),
                        )
                    })
                    .collect::<Vec<_>>();
                (channel, recorder_channels[base], words)
            })
            .collect::<Vec<_>>();
        play_stream_channel_metadata = Some(format!(
            "StreamPreLoad source-channel table at 0x805cec68 (index,status,12 words)={recorder_channel_summary:?}; raw=l6max-stream-channel-metadata.bin"
        ));
        let stream_semaphores = [0x8020a92c, 0x8020a930, 0x8020a938]
            .into_iter()
            .map(|address| -> Result<String, String> {
                let handle = u32::from_le_bytes(
                    qmp.read_memory(0, address, 4)
                        .map_err(|error| error.to_string())?
                        .try_into()
                        .unwrap(),
                );
                let bytes = qmp
                    .read_memory(0, handle, 80)
                    .map_err(|error| error.to_string())?;
                let words = bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                    .collect::<Vec<_>>();
                Ok(format!(
                    "{address:#010x}->{handle:#010x}:count={}",
                    words[14]
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        play_stream_queue_state = Some(format!(
            "queue={stream_queue:#010x}, messages={}, receivers={}, length={}, item_size={}, preload_callback={preload_callback:#010x}, window={preload_window:?}, preload_skip_flag={preload_skip_flag:#04x}, semaphores={stream_semaphores:?}",
            stream_queue_words[14],
            stream_queue_words[9],
            stream_queue_words[15],
            stream_queue_words[16]
        ));
        let runtime_bytes = qmp
            .read_memory(0, runtime_pc, 64)
            .map_err(|error| error.to_string())?;
        fs::write(logs.join("l6max-recorder-settled-pc.bin"), runtime_bytes)
            .map_err(|error| error.to_string())?;
        let stack_address = runtime_sp.saturating_sub(32);
        let stack_bytes = qmp
            .read_memory(0, stack_address, 96)
            .map_err(|error| error.to_string())?;
        fs::write(logs.join("l6max-recorder-settled-stack.bin"), stack_bytes)
            .map_err(|error| error.to_string())?;
        play_callback_queue_after_settle = Some(format!(
            "queue={queue:#010x}, messages={}, receivers={}, length={}, item_size={}, busy={callback_busy}, semaphore={}, current_tcb={current_tcb:#010x}, task={task_name:?}, priority={task_priority}, saved_sp={saved_sp:#010x}, task_stack=[{stack_base:#010x},{stack_end:#010x}], sp={runtime_sp:#010x}, pc={runtime_pc:#010x}, lr={runtime_lr:#010x}, sp_in_task_stack={}, dumps=l6max-recorder-settled-pc.bin+l6max-recorder-settled-stack.bin",
            queue_words[14],
            queue_words[9],
            queue_words[15],
            queue_words[16],
            semaphore_words[14],
            (stack_base..=stack_end).contains(&runtime_sp)
        ));
        qmp.resume().map_err(|error| error.to_string())?;
    }
    if std::env::var_os("L6_AUDIO_DMA_DUMP").is_some() {
        qmp.stop().map_err(|error| error.to_string())?;
        dump_l6_audio_tx_buffer(qmp, 14, logs)?;
        let read_u64 = |address| -> Result<u64, String> {
            Ok(u64::from_le_bytes(
                qmp.read_memory(0, address, 8)
                    .map_err(|error| error.to_string())?
                    .try_into()
                    .unwrap(),
            ))
        };
        play_recorder_post_audio_state = Some((read_u64(0x805c7c28)?, read_u64(0x20014b40)?));
        qmp.resume().map_err(|error| error.to_string())?;
    }
    let applied = inputs.main.snapshot().applied;
    inputs.main.up(4).map_err(|error| error.to_string())?;
    wait_applied(&inputs.main, applied + 1, display, latest)?;
    pump_for(display, latest, Duration::from_secs(5))?;
    if let Some(frame) = latest {
        save(frame, &logs.join("l6max-recorder-playback.png"))?;
    }
    let trace = fs::read_to_string(logs.join("engine.trace")).map_err(|error| error.to_string())?;
    let samples_after_play = trace
        .lines()
        .filter(|line| line.starts_with("l6 audio TX samples sai=00384000"))
        .skip(tx_before)
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("nonzero="))
                .and_then(|count| count.parse::<u64>().ok())
        })
        .collect::<Vec<_>>();
    let play_reader_matched = trace
        .lines()
        .filter(|line| line.starts_with("l6 MAIN GPIO port-read candidate=4"))
        .skip(gpio_reads_before)
        .any(|line| {
            line.contains("reader=1")
                && line.contains("tested_bit=20")
                && line.contains("matches_candidate=1")
        });
    let audio_started = samples_after_play.iter().any(|count| *count > 0);
    let play_callback_queue_send_stop = play_callback_queue_send_stop
        .map(|(pc, lr)| (format!("{pc:#010x}"), format!("{lr:#010x}")));
    if std::env::var_os("L6MAX_REQUIRE_PLAY_INPUT").is_some()
        && std::env::var_os("L6_MAIN_GPIO_TRACE").is_none()
    {
        return Err("L6MAX_REQUIRE_PLAY_INPUT needs L6_MAIN_GPIO_TRACE=1".into());
    }
    if std::env::var_os("L6MAX_REQUIRE_PLAY_INPUT").is_some() && !play_reader_matched {
        return Err("L6max GPIO reader slot 1 did not test GPIO1.20 (bit 20)".into());
    }
    if std::env::var_os("L6MAX_REQUIRE_RECORDER_AUDIO").is_some() && !audio_started {
        return Err(format!(
            "L6max recorder audio assertion failed: no nonzero SAI1 TX samples after Play; observed counts={samples_after_play:?}"
        ));
    }
    if std::env::var_os("L6MAX_REQUIRE_STREAM_READ").is_some() && play_stream_read.is_none() {
        return Err(
            "L6max recorder Play did not reach stream-file worker read callsite 0x8004a130".into(),
        );
    }
    if play_recorder_action_hit {
        let sai_registers = [
            ("TCSR", 0x08),
            ("TCR1", 0x0c),
            ("TCR2", 0x10),
            ("TCR3", 0x14),
            ("TCR4", 0x18),
            ("RCSR", 0x88),
            ("RCR1", 0x8c),
            ("RCR2", 0x90),
            ("RCR3", 0x94),
            ("RCR4", 0x98),
        ]
        .into_iter()
        .map(|(name, offset)| -> Result<(&str, u32), String> {
            let bytes = qmp
                .read_memory(0, 0x4038_4000 + offset, 4)
                .map_err(|error| error.to_string())?;
            Ok((name, u32::from_le_bytes(bytes.try_into().unwrap())))
        })
        .collect::<Result<Vec<_>, _>>()?;
        play_sai_register_state = Some(format!("SAI1 [{sai_registers:?}]"));
        let sai_tcds = (12..=15u32)
            .map(|channel| -> Result<(u32, Vec<u32>), String> {
                let address = 0x400e_9000 + channel * 32;
                let bytes = qmp
                    .read_memory(0, address, 32)
                    .map_err(|error| error.to_string())?;
                let words = bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                    .collect::<Vec<_>>();
                Ok((channel, words))
            })
            .collect::<Result<Vec<_>, _>>()?;
        play_sai_edma_tcd_state = Some(format!("eDMA channels 12-15 {sai_tcds:?}"));
    }
    println!(
        "L6max recorder Play target 4: GPIO reader slot 1 matched candidate GPIO1.20={play_reader_matched}; Play event submission call reached={}; helper returned={}; main-button handler reached={}; notification-active={play_notification_active_before:?}; callback-slot-2 latch before dispatch={play_callback_latch_before:?}; callback slot 2 scheduled={}; callback target entered={play_callback_target_hit}; recorder action entered={play_recorder_action_hit}, gates={play_recorder_action_gates:?}, audio-clock gate result={play_recorder_audio_clock_result:?}, gate checks={play_recorder_gate_checks:?}, file-path-buffers={play_recorder_paths:?}; scanner-config-globals={play_recorder_scan_globals:?}; pre-Play source scan={play_pre_stream_source_scan:?}; recorder callback send return={play_callback_queue_send_return:?}, SAI1 RX eDMA channel 15 disabled after queue trace={play_sai1_rx_dma_disabled}, GDB interrupt snapshot PC/LR={play_callback_queue_send_stop:?}; callback dispatches={play_callback_dispatches:?}; stream state update={play_stream_state_update:?}; queue events={play_callback_queue_trace:?}; queue/message-waiters/length/item-size/busy/semaphore-count/settled-PC/LR={play_callback_queue_after_settle:?}; RecPlayCtl task TCB and saved state={play_recplay_task_state:?}; stream source scan={play_stream_source_scan:?}; stream channel metadata={play_stream_channel_metadata:?}; stream task TCB states={play_stream_task_states:?}; stream queue/semaphores={play_stream_queue_state:?}; stream channel masks/files={play_stream_channel_state:?}; SAI1 runtime registers={play_sai_register_state:?}; stream-window updater={play_stream_window_update:?}; stream-file open wrapper={play_stream_open:?}; stream-read worker call={play_stream_read:?}; post-press duration/position={play_recorder_post_audio_state:?}; post-press SAI1 TX nonzero-word counts={samples_after_play:?}; audio started={audio_started}",
        play_event_submission.is_some(),
        play_submit_return.is_some(),
        play_handler_fields.is_some(),
        play_callback_route.is_some()
    );
    if let Some(tcds) = play_sai_edma_tcd_state {
        println!("L6max SAI receive/transmit TCD snapshot: {tcds}");
    }
    Ok(())
}

fn trace_l6max_recorder_event_path(
    inputs: &InputHub,
    qmp: &QmpClient,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
    debugger: &mut Gdb,
) -> Result<(), String> {
    for phase in ["Record start", "Record stop"] {
        qmp.stop()
            .map_err(|error| format!("stopping guest before {phase} trace: {error}"))?;
        for address in L6MAX_RECORDER_EVENT_TRACE_POINTS {
            debugger.breakpoint(address, true).map_err(|error| {
                format!("setting recorder event-path breakpoint at {address:#010x}: {error}")
            })?;
        }
        let before = inputs.main.snapshot();
        inputs
            .main
            .tap(5, 1_000)
            .map_err(|error| format!("sending L6max {phase} input: {error}"))?;

        let mut hits = Vec::new();
        let breakpoint_hit =
            trace_l6max_recorder_event_window(qmp, debugger, Duration::from_secs(6), &mut hits)?;
        wait_applied(&inputs.main, before.applied + 1, display, latest)?;
        for address in L6MAX_RECORDER_EVENT_TRACE_POINTS {
            if !hits.iter().any(|(hit, _, _)| *hit == address) {
                debugger
                    .breakpoint(address, false)
                    .map_err(|error| format!("clearing recorder event-path breakpoint: {error}"))?;
            }
        }
        let state = qmp
            .read_memory(0, 0x8062_93c6, 4)
            .map_err(|error| error.to_string())?;
        let refresh_gates = qmp
            .read_memory(0, 0x8051_e588, 0x20)
            .map_err(|error| error.to_string())?;
        let scanner_state = qmp
            .read_memory(0, 0x8066_6d44, 4)
            .map_err(|error| error.to_string())?;
        let scanner_stable_count = qmp
            .read_memory(0, 0x8066_6d94, 4)
            .map_err(|error| error.to_string())?;
        let record_action_state = qmp
            .read_memory(0, 0x8020_a9e8, 0x0c)
            .map_err(|error| error.to_string())?;
        println!(
            "L6max recorder event-path diagnostic {phase}: tap_ms=1000; breakpoint_hit={breakpoint_hit}; RecStart/RecStop={state:02x?}; reader-slot-2 state/count={scanner_state:02x?}/{scanner_stable_count:02x?}; recorder action state={record_action_state:02x?}; refresh globals={refresh_gates:02x?}; PC/LR/registers={hits:?}"
        );
        // A GDB timeout is ended with Ctrl-C, which leaves the guest paused.
        // Resume before waiting for the host's scheduled key release to land.
        qmp.resume()
            .map_err(|error| format!("resuming guest after Record trace: {error}"))?;
        wait_released(&inputs.main, before.released + 1)?;
    }
    qmp.stop()
        .map_err(|error| format!("stopping guest after Record traces: {error}"))?;
    Ok(())
}

fn trace_l6max_recorder_event_window(
    qmp: &QmpClient,
    debugger: &mut Gdb,
    duration: Duration,
    hits: &mut Vec<(u32, u32, [u32; 8])>,
) -> Result<bool, String> {
    let deadline = Instant::now() + duration;
    let mut breakpoint_hit = false;
    let mut dynamic_callback_target = None;
    while Instant::now() < deadline {
        match debugger.continue_until_stop(deadline.saturating_duration_since(Instant::now())) {
            Ok(_) => {
                breakpoint_hit = true;
                let pc = debugger
                    .read_register(15)
                    .map_err(|error| error.to_string())?
                    & !1;
                let lr = debugger
                    .read_register(14)
                    .map_err(|error| error.to_string())?
                    & !1;
                if L6MAX_RECORDER_EVENT_TRACE_POINTS.contains(&pc) {
                    let args = (0..8)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    let relevant = match pc {
                        0x8002_8088 => args[2] == 0x2b,
                        0x8003_6a40 => {
                            let event = qmp
                                .read_memory(0, args[0], 12)
                                .map_err(|error| error.to_string())?;
                            u32::from_le_bytes(event[8..12].try_into().unwrap()) == 0x2b
                        }
                        0x8008_bd90 => args[0] == 1 && args[1] == 0,
                        0x8008_be38 => args[0] == 0 && args[7] == 0x8004_e819,
                        0x8004_e818 => args[0] == 0,
                        0x8004_6ce8 => lr == 0x8004_d8ce,
                        _ => true,
                    };
                    if !relevant {
                        // These entry points are shared by unrelated events.
                        // Keep tracing until the Record event reaches the selected
                        // slot-1 callback with its active state.
                        debugger
                            .breakpoint(pc, false)
                            .and_then(|()| debugger.breakpoint(pc, true))
                            .map_err(|error| error.to_string())?;
                        continue;
                    }
                    hits.push((
                        pc,
                        lr,
                        [
                            args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
                        ],
                    ));
                    debugger
                        .breakpoint(pc, false)
                        .map_err(|error| error.to_string())?;
                    if pc == 0x8004_e818 {
                        let target = u32::from_le_bytes(
                            qmp.read_memory(0, 0x8089_8918, 4)
                                .map_err(|error| error.to_string())?
                                .try_into()
                                .unwrap(),
                        ) & !1;
                        debugger.breakpoint(target, true).map_err(|error| {
                            format!(
                                "setting selected Record action breakpoint {target:#010x}: {error}"
                            )
                        })?;
                        dynamic_callback_target = Some(target);
                    }
                } else if dynamic_callback_target == Some(pc) {
                    let args = (0..8)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    hits.push((
                        pc,
                        lr,
                        [
                            args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
                        ],
                    ));
                    debugger
                        .breakpoint(pc, false)
                        .map_err(|error| error.to_string())?;
                    dynamic_callback_target = None;
                } else {
                    return Err(format!(
                        "L6max recorder event trace stopped at unexpected PC {pc:#010x}"
                    ));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                        format!(
                            "L6max recorder event trace timed out ({error}) and GDB interrupt failed: {interrupt}"
                        )
                    })?;
                if let Some(target) = dynamic_callback_target.take() {
                    debugger.breakpoint(target, false).map_err(|clear| {
                        format!("clearing selected Record action breakpoint: {clear}")
                    })?;
                }
                break;
            }
            Err(error) => {
                return Err(format!("waiting for L6max recorder event trace: {error}"));
            }
        }
    }
    Ok(breakpoint_hit)
}

fn run_l6_inputs(
    inputs: &InputHub,
    qmp: &QmpClient,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
    logs: &Path,
    gdb_socket: Option<&Path>,
    gpio_event_gdb_socket: Option<&Path>,
) -> Result<(), String> {
    if std::env::var_os("L6_PLAY_GPIO_RUNTIME_PROBE").is_some() {
        let socket = gdb_socket.ok_or("L6 Play runtime probe requires the GDB socket")?;
        return run_l6_play_gpio_runtime_probe(inputs, qmp, display, latest, logs, socket);
    }
    if std::env::var_os("L6_RECORDER_RECORD_STOP").is_some() {
        return run_l6_recorder_record_stop_probe(inputs, qmp, display, latest);
    }
    // L6 TAP and sound-pad keys use the separate MAIN-MCU GPIO path.
    // Menu and the main-MCU Record input remain unsupported on L6; its Record
    // key is separately mapped through panel target 24.
    let trace_main_leds = std::env::var_os("L6_MAIN_BUTTON_LED_TRACE").is_some();
    let trace_only = std::env::var_os("L6_MAIN_GPIO_TRACE_ONLY").is_some();
    if trace_only && !trace_main_leds {
        return Err("L6_MAIN_GPIO_TRACE_ONLY requires L6_MAIN_BUTTON_LED_TRACE=1".into());
    }
    let main_button_hold = std::env::var("L6_MAIN_BUTTON_HOLD_MS")
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .filter(|ms| (1..=10_000).contains(ms))
                .ok_or_else(|| "L6_MAIN_BUTTON_HOLD_MS must be between 1 and 10000".to_owned())
        })
        .unwrap_or(Ok(400))?;
    let mut gpio_event_debugger = if let Some(socket) = gpio_event_gdb_socket {
        Some(Gdb::connect(socket).map_err(|error| error.to_string())?)
    } else {
        None
    };
    if trace_main_leds {
        let mut main_applied = inputs.main.snapshot().applied;
        let targets = match std::env::var("L6_MAIN_GPIO_PROBE_ONLY") {
            Ok(value) => match value.as_str() {
                "play" => {
                    return Err(
                        "Use L6_PLAY_GPIO_RUNTIME_PROBE=1 to trace L6 Play on GPIO2.25".into(),
                    );
                }
                "tap" => vec![48],
                "all" => vec![48, 49, 50, 51, 52],
                "none" => Vec::new(),
                "pad-1" => vec![49],
                "pad-2" => vec![50],
                "pad-3" => vec![51],
                "pad-4" => vec![52],
                _ => {
                    return Err(
                        "L6_MAIN_GPIO_PROBE_ONLY must be tap, pad-1..4, all, or none".into(),
                    );
                }
            },
            Err(_) => match std::env::var("L6_PAD_GPIO_PROBE_ONLY").as_deref() {
                Err(_) => vec![48, 49, 50, 51, 52],
                Ok("none") => Vec::new(),
                Ok("all") => vec![49, 50, 51, 52],
                Ok(value) => {
                    let pad = value
                        .parse::<u32>()
                        .ok()
                        .filter(|pad| *pad < 4)
                        .ok_or_else(|| {
                            "L6_PAD_GPIO_PROBE_ONLY must be 0, 1, 2, 3, all, or none".to_owned()
                        })?;
                    vec![49 + pad]
                }
            },
        };
        for target in targets {
            let expected_event_code = match target {
                48 => Some(0x19),
                49..=52 => Some(0x15 + target - 49),
                _ => None,
            };
            if gpio_event_debugger.is_some() && expected_event_code.is_some() {
                qmp.stop().map_err(|error| error.to_string())?;
            }
            let sampled = std::env::var_os("L6_MAIN_GPIO_TRACE").is_some();
            let require_pad_audio = std::env::var_os("L6_REQUIRE_PAD_FILES").is_some()
                && std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_some()
                && (49..=52).contains(&target);
            let audio_samples_before = if require_pad_audio {
                trace_count(logs, "l6 audio TX samples sai=00384000")?
            } else {
                0
            };
            let samples_before = if sampled {
                trace_count(logs, &format!("l6 MAIN GPIO port-read candidate={target}"))?
            } else {
                0
            };
            let before = inputs.panel.snapshot().indicators;
            let main_before = inputs.main.snapshot().main_gpio;
            let pad_led_before = qmp
                .read_memory(0, 0x806293f4, 8)
                .map_err(|error| error.to_string())?;
            let transport_led_before = qmp
                .read_memory(0, 0x806293c6, 4)
                .map_err(|error| error.to_string())?;
            inputs
                .main
                .down(target, 0)
                .map_err(|error| error.to_string())?;
            wait_applied(&inputs.main, main_applied + 1, display, latest)?;
            if let (Some(debugger), Some(expected_code)) =
                (gpio_event_debugger.as_mut(), expected_event_code)
            {
                let event = trace_l6_gpio_event_submit(
                    debugger,
                    qmp,
                    target,
                    expected_code,
                    Duration::from_millis(main_button_hold.saturating_add(2_000)),
                )?;
                if event.is_none() {
                    return Err(format!(
                        "L6 GPIO target {target} did not submit expected event {expected_code:#04x}"
                    ));
                }
                println!(
                    "L6 GPIO target {target} expected event {expected_code:#04x} submitted={}",
                    event.is_some()
                );
            }
            pump_for(display, latest, Duration::from_millis(main_button_hold))?;
            if sampled {
                wait_trace_count(
                    logs,
                    &format!("l6 MAIN GPIO port-read candidate={target}"),
                    samples_before + 1,
                )?;
            }
            let held = inputs.panel.snapshot().indicators;
            let main_held = inputs.main.snapshot().main_gpio;
            let pad_led_held = qmp
                .read_memory(0, 0x806293f4, 8)
                .map_err(|error| error.to_string())?;
            let transport_led_held = qmp
                .read_memory(0, 0x806293c6, 4)
                .map_err(|error| error.to_string())?;
            if std::env::var_os("L6_REQUIRE_PAD_FILES").is_some() && (49..=52).contains(&target) {
                let record_offset = ((target - 49) * 2) as usize;
                if pad_led_before[record_offset..record_offset + 2]
                    == pad_led_held[record_offset..record_offset + 2]
                    || pad_led_held[record_offset + 1] & 0x02 == 0
                {
                    return Err(format!(
                        "L6 MAIN target {target} did not activate its sound-pad record {}",
                        target - 49
                    ));
                }
            }
            inputs.main.up(target).map_err(|error| error.to_string())?;
            wait_applied(&inputs.main, main_applied + 2, display, latest)?;
            main_applied += 2;
            if require_pad_audio {
                // Audio sample traces are emitted once per virtual second.
                // Wait long enough to get a sample strictly after this press,
                // even when it lands just after the previous sample boundary.
                pump_for(display, latest, Duration::from_millis(2_200))?;
                let trace = fs::read_to_string(logs.join("engine.trace"))
                    .map_err(|error| error.to_string())?;
                let audio_changed = trace
                    .lines()
                    .filter(|line| line.starts_with("l6 audio TX samples sai=00384000"))
                    .skip(audio_samples_before)
                    .any(|line| {
                        ["first=", "last="].iter().any(|field| {
                            line.split_whitespace()
                                .find_map(|value| value.strip_prefix(field))
                                .is_some_and(|sample| sample != "ffffffff")
                        })
                    });
                if !audio_changed {
                    return Err(format!(
                        "L6 MAIN target {target} activated its pad record but produced no changing SAI1 TX samples"
                    ));
                }
            }
            let released = inputs.panel.snapshot().indicators;
            let main_released = inputs.main.snapshot().main_gpio;
            let pad_led_released = qmp
                .read_memory(0, 0x806293f4, 8)
                .map_err(|error| error.to_string())?;
            let transport_led_released = qmp
                .read_memory(0, 0x806293c6, 4)
                .map_err(|error| error.to_string())?;
            let changes = |after: [u32; 8]| {
                (0..8)
                    .filter_map(|row| {
                        let mask = before[row] ^ after[row];
                        (mask != 0).then_some((row, mask))
                    })
                    .collect::<Vec<_>>()
            };
            println!(
                "L6 MAIN target {target}: panel LEDs before={:06x?} held={:06x?} released={:06x?}; panel changes={:?}/{:?}; MAIN GPIO before={main_before:08x?} held={main_held:08x?} released={main_released:08x?}; sound-pad LED records={pad_led_before:02x?}/{pad_led_held:02x?}/{pad_led_released:02x?}; transport LED records={transport_led_before:02x?}/{transport_led_held:02x?}/{transport_led_released:02x?}",
                &before[..4],
                &held[..4],
                &released[..4],
                changes(held),
                changes(released)
            );
        }
        if trace_only {
            println!("L6 MAIN GPIO trace-only probe completed; remaining input sweep skipped.");
            return Ok(());
        }
    }
    for target in [0, 5] {
        let rejected = inputs.main.snapshot().rejected;
        inputs
            .main
            .tap(target, 120)
            .map_err(|error| error.to_string())?;
        wait_rejected(&inputs.main, rejected + 1)?;
    }

    // L6 scans 24 matrix positions (8 rows x 3 columns) and five direct GPIO
    // switches. Verify each physical input produces the firmware's press and
    // release reports through the real panel MCU.
    let mut targets: Vec<u32> = (7..31).collect();
    targets.extend([1, 2, 3, 6, 47]);
    for target in targets {
        let trace_record = trace_main_leds && target == 24;
        let indicator_before = trace_record.then(|| inputs.panel.snapshot().indicators);
        let transport_led_before = if trace_record {
            Some(
                qmp.read_memory(0, 0x806293c6, 4)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let presses_before = trace_count(logs, "l6 panel USART1 TX 91\n")?;
        let releases_before = trace_count(logs, "l6 panel USART1 TX 90\n")?;
        let initial = inputs.panel.snapshot().applied;
        let released = inputs.panel.snapshot().released;
        inputs
            .panel
            .tap(target, 120)
            .map_err(|error| error.to_string())?;
        wait_applied(&inputs.panel, initial + 1, display, latest)?;
        wait_released(&inputs.panel, released + 1)?;
        wait_trace_count(logs, "l6 panel USART1 TX 91\n", presses_before + 1)?;
        wait_trace_count(logs, "l6 panel USART1 TX 90\n", releases_before + 1)?;
        let presses = trace_count(logs, "l6 panel USART1 TX 91\n")? - presses_before;
        let releases = trace_count(logs, "l6 panel USART1 TX 90\n")? - releases_before;
        if (presses, releases) != (1, 1) {
            return Err(format!(
                "L6 panel input {target} emitted {presses} press and {releases} release reports"
            ));
        }
        if trace_record {
            pump_for(display, latest, Duration::from_millis(250))?;
            let indicator_after = inputs.panel.snapshot().indicators;
            let transport_led_after = qmp
                .read_memory(0, 0x806293c6, 4)
                .map_err(|error| error.to_string())?;
            let before = indicator_before.expect("record trace has indicator baseline");
            let changed_rows = (0..8)
                .filter_map(|row| {
                    let changed = before[row] ^ indicator_after[row];
                    (changed != 0).then_some((row, changed))
                })
                .collect::<Vec<_>>();
            println!(
                "L6 Record target {target}: indicator rows={:06x?}->{:06x?}, changed={changed_rows:?}; transport LED records={:02x?}->{transport_led_after:02x?}",
                &before[..8],
                &indicator_after[..8],
                transport_led_before.expect("record trace has RAM baseline")
            );
        }
    }

    let display_before = *latest;
    if let Some(frame) = display_before {
        save(&frame, &logs.join("l6-encoder-display-before.png"))?;
    }
    let encoders_before = trace_count(logs, "l6 panel USART1 TX a1\n")?;
    for encoder in 0..6 {
        let applied = inputs.panel.snapshot().applied;
        inputs
            .encoder(encoder, 1)
            .map_err(|error| error.to_string())?;
        wait_applied(&inputs.panel, applied + 1, display, latest)?;
    }
    pump_for(display, latest, Duration::from_millis(600))?;
    let encoders = trace_count(logs, "l6 panel USART1 TX a1\n")? - encoders_before;
    if encoders != 6 {
        return Err(format!("expected six L6 encoder reports, got {encoders}"));
    }
    let display_after = *latest;
    if let Some(frame) = display_after {
        save(&frame, &logs.join("l6-encoder-display-after.png"))?;
    }
    match (display_before, display_after) {
        (Some(before), Some(after)) => println!(
            "L6 encoder input changed the published display frame: {}",
            before != after
        ),
        _ => println!("No LCD frame expected: L6 has no screen."),
    }

    let rejected_before = inputs.panel.snapshot().rejected;
    inputs
        .panel
        .down(31, 50)
        .map_err(|error| error.to_string())?;
    wait_rejected(&inputs.panel, rejected_before + 1)?;
    let rejected_before = inputs.panel.snapshot().rejected;
    inputs.encoder(6, 1).map_err(|error| error.to_string())?;
    wait_rejected(&inputs.panel, rejected_before + 1)?;
    println!(
        "L6 input smoke passed: observed MAIN TAP and four sound-pad GPIOs, rejected unverified MAIN Menu/Record mappings, 24 matrix keys, five direct switches, six encoders, and model-specific bounds. Play target 4 maps to GPIO2.25 and runtime event 0x20; playback and the Play/Stop lamp remain unverified."
    );
    Ok(())
}

fn trace_l6_gpio_event_submit(
    debugger: &mut Gdb,
    qmp: &QmpClient,
    target: u32,
    expected_code: u32,
    timeout: Duration,
) -> Result<Option<[u32; 4]>, String> {
    const SUBMIT: u32 = 0x80020ca0;
    const STAGE: u32 = 0x8006e4b8;
    debugger
        .breakpoint(SUBMIT, true)
        .map_err(|error| error.to_string())?;
    let mut submit_breakpoint_enabled = true;
    let deadline = Instant::now() + timeout;
    let mut expected = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match debugger.continue_until_stop(remaining) {
            Ok(_) => {
                let pc = debugger
                    .read_register(15)
                    .map_err(|error| error.to_string())?
                    & !1;
                if pc != SUBMIT {
                    return Err(format!(
                        "L6 GPIO event trace stopped at {pc:#010x}, expected l6_submit_system_event"
                    ));
                }
                let args = (0..4)
                    .map(|register| debugger.read_register(register))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?;
                let args: [u32; 4] = args.try_into().unwrap();
                let sp = debugger
                    .read_register(13)
                    .map_err(|error| error.to_string())?;
                let fifth = u32::from_le_bytes(
                    qmp.read_memory(0, sp, 4)
                        .map_err(|error| error.to_string())?
                        .try_into()
                        .unwrap(),
                );
                println!("L6 GPIO target {target} submit args={args:08x?} fifth={fifth:#010x}");
                if args[2] == expected_code {
                    expected = Some(args);
                    debugger
                        .breakpoint(SUBMIT, false)
                        .and_then(|()| debugger.breakpoint(STAGE, true))
                        .map_err(|error| error.to_string())?;
                    submit_breakpoint_enabled = false;
                    let mut stage_breakpoint_enabled = true;
                    match debugger.continue_until_stop(Duration::from_secs(2)) {
                        Ok(_) => {
                            let stage_pc = debugger
                                .read_register(15)
                                .map_err(|error| error.to_string())?
                                & !1;
                            if stage_pc == STAGE {
                                let stage_args = (0..4)
                                    .map(|register| debugger.read_register(register))
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(|error| error.to_string())?;
                                let caller = debugger
                                    .read_register(14)
                                    .map_err(|error| error.to_string())?
                                    & !1;
                                println!(
                                    "L6 GPIO target {target} staged event args={stage_args:08x?} caller={caller:#010x}"
                                );
                                debugger
                                    .breakpoint(STAGE, false)
                                    .and_then(|()| debugger.breakpoint(caller, true))
                                    .map_err(|error| error.to_string())?;
                                stage_breakpoint_enabled = false;
                                match debugger.continue_until_stop(Duration::from_secs(2)) {
                                    Ok(_) => {
                                        let return_pc = debugger
                                            .read_register(15)
                                            .map_err(|error| error.to_string())?
                                            & !1;
                                        let return_value = debugger
                                            .read_register(0)
                                            .map_err(|error| error.to_string())?;
                                        println!(
                                            "L6 GPIO target {target} l6_stage_system_event return pc={return_pc:#010x} r0={return_value:#010x}"
                                        );
                                        debugger
                                            .breakpoint(caller, false)
                                            .map_err(|error| error.to_string())?;
                                        let triggered = trace_l6_class0_event_dispatch(
                                            debugger,
                                            qmp,
                                            target,
                                            expected_code,
                                        )?;
                                        println!(
                                            "L6 GPIO target {target} sound-pad trigger reached={triggered}"
                                        );
                                        if !triggered {
                                            return Err(format!(
                                                "L6 GPIO target {target} event reached no sound-pad trigger"
                                            ));
                                        }
                                    }
                                    Err(error)
                                        if matches!(
                                            error.kind(),
                                            std::io::ErrorKind::WouldBlock
                                                | std::io::ErrorKind::TimedOut
                                        ) =>
                                    {
                                        debugger.interrupt().map_err(|interrupt| {
                                            format!(
                                                "L6 stage-return wait timed out ({error}); GDB interrupt failed: {interrupt}"
                                            )
                                        })?;
                                        debugger
                                            .breakpoint(caller, false)
                                            .map_err(|error| error.to_string())?;
                                        println!(
                                            "L6 GPIO target {target} did not reach the stage return breakpoint"
                                        );
                                    }
                                    Err(error) => {
                                        return Err(format!(
                                            "waiting for L6 stage return: {error}"
                                        ));
                                    }
                                }
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            debugger.interrupt().map_err(|interrupt| {
                                format!(
                                    "L6 event-stage wait timed out ({error}); GDB interrupt failed: {interrupt}"
                                )
                            })?;
                            println!(
                                "L6 GPIO target {target} submit did not reach l6_stage_system_event"
                            );
                        }
                        Err(error) => {
                            return Err(format!("waiting for L6 event stage: {error}"));
                        }
                    }
                    if stage_breakpoint_enabled {
                        debugger
                            .breakpoint(STAGE, false)
                            .map_err(|error| error.to_string())?;
                    }
                    break;
                }
                debugger
                    .breakpoint(SUBMIT, false)
                    .and_then(|()| debugger.breakpoint(SUBMIT, true))
                    .map_err(|error| error.to_string())?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6 GPIO submit wait timed out ({error}); GDB interrupt failed: {interrupt}"
                    )
                })?;
                break;
            }
            Err(error) => return Err(format!("waiting for L6 GPIO event: {error}")),
        }
    }
    if submit_breakpoint_enabled {
        debugger
            .breakpoint(SUBMIT, false)
            .map_err(|error| error.to_string())?;
    }
    qmp.resume().map_err(|error| error.to_string())?;
    Ok(expected)
}

fn trace_l6_class0_event_dispatch(
    debugger: &mut Gdb,
    qmp: &QmpClient,
    target: u32,
    expected_code: u32,
) -> Result<bool, String> {
    const HANDLER: u32 = 0x8002c9d8;
    const SOUND_PAD_TRIGGER: u32 = 0x800408b0;
    debugger
        .breakpoint(HANDLER, true)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut found = false;
    let mut triggered = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match debugger.continue_until_stop(remaining) {
            Ok(_) => {
                let pc = debugger
                    .read_register(15)
                    .map_err(|error| error.to_string())?
                    & !1;
                if pc != HANDLER {
                    return Err(format!(
                        "L6 class-0 trace stopped at {pc:#010x}, expected handler {HANDLER:#010x}"
                    ));
                }
                let address = debugger
                    .read_register(0)
                    .map_err(|error| error.to_string())?;
                let bytes = qmp
                    .read_memory(0, address, 20)
                    .map_err(|error| error.to_string())?;
                let event: [u32; 5] = std::array::from_fn(|index| {
                    u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
                });
                println!("L6 GPIO target {target} class-0 handler event={event:08x?}");
                if event[0] == 0 && event[2] == expected_code {
                    found = true;
                    debugger
                        .breakpoint(HANDLER, false)
                        .and_then(|()| debugger.breakpoint(SOUND_PAD_TRIGGER, true))
                        .map_err(|error| error.to_string())?;
                    match debugger.continue_until_stop(Duration::from_secs(2)) {
                        Ok(_) => {
                            let trigger_pc = debugger
                                .read_register(15)
                                .map_err(|error| error.to_string())?
                                & !1;
                            if trigger_pc != SOUND_PAD_TRIGGER {
                                return Err(format!(
                                    "L6 sound-pad trace stopped at {trigger_pc:#010x}, expected {SOUND_PAD_TRIGGER:#010x}"
                                ));
                            }
                            triggered = true;
                            let args = (0..3)
                                .map(|register| debugger.read_register(register))
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|error| error.to_string())?;
                            println!("L6 GPIO target {target} sound-pad trigger args={args:08x?}");
                            for (name, address) in [
                                ("current pad", 0x801f5f00),
                                ("busy", 0x80443108),
                                ("assignment", 0x8044337f),
                                ("stream", 0x80441d88),
                                ("status", 0x80441d8c),
                            ] {
                                let value = qmp
                                    .read_memory(0, address, 4)
                                    .map_err(|error| error.to_string())?;
                                println!(
                                    "L6 GPIO target {target} sound-pad {name} state at {address:#010x}={value:02x?}"
                                );
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            debugger.interrupt().map_err(|interrupt| {
                                format!(
                                    "L6 sound-pad trigger wait timed out ({error}); GDB interrupt failed: {interrupt}"
                                )
                            })?;
                            println!("L6 GPIO target {target} did not call the sound-pad trigger");
                        }
                        Err(error) => {
                            return Err(format!("waiting for L6 sound-pad trigger: {error}"));
                        }
                    }
                    debugger
                        .breakpoint(SOUND_PAD_TRIGGER, false)
                        .map_err(|error| error.to_string())?;
                    break;
                }
                debugger
                    .breakpoint(HANDLER, false)
                    .and_then(|()| debugger.breakpoint(HANDLER, true))
                    .map_err(|error| error.to_string())?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6 class-0 event wait timed out ({error}); GDB interrupt failed: {interrupt}"
                    )
                })?;
                break;
            }
            Err(error) => return Err(format!("waiting for L6 class-0 event: {error}")),
        }
    }
    if !found {
        debugger
            .breakpoint(HANDLER, false)
            .map_err(|error| error.to_string())?;
    }
    qmp.resume().map_err(|error| error.to_string())?;
    Ok(found && triggered)
}

fn trace_l6_assignment_callback(
    debugger: &mut Gdb,
    qmp: &QmpClient,
    timeout: Duration,
) -> Result<bool, String> {
    const CALLBACK: u32 = 0x80049d60;
    debugger
        .breakpoint(CALLBACK, true)
        .map_err(|error| error.to_string())?;
    let result = (|| match debugger.continue_until_stop(timeout) {
        Ok(_) => {
            let pc = debugger
                .read_register(15)
                .map_err(|error| error.to_string())?
                & !1;
            if pc != CALLBACK {
                return Err(format!(
                    "L6 assignment callback trace stopped at {pc:#010x}, expected {CALLBACK:#010x}"
                ));
            }
            let args = debugger
                .read_register(0)
                .map_err(|error| error.to_string())?;
            let bytes = qmp
                .read_memory(0, args, 0x1c)
                .map_err(|error| error.to_string())?;
            println!("L6 assignment completion callback args at {args:#010x}: {bytes:02x?}");
            Ok(true)
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            debugger.interrupt().map_err(|interrupt| {
                    format!(
                        "L6 assignment callback wait timed out ({error}); GDB interrupt failed: {interrupt}"
                    )
                })?;
            Ok(false)
        }
        Err(error) => Err(format!("waiting for L6 assignment callback: {error}")),
    })();
    let remove_breakpoint = debugger
        .breakpoint(CALLBACK, false)
        .map_err(|error| error.to_string());
    let resume = qmp.resume().map_err(|error| error.to_string());
    remove_breakpoint?;
    resume?;
    result
}

struct L6EditorProbe {
    assignments: Vec<(u8, Vec<Vec<u8>>)>,
}

fn probe_l6_editor(
    usb: &UsbHost,
    qmp: &QmpClient,
    inputs: &InputHub,
    logs: &Path,
    assign_files: bool,
) -> Result<L6EditorProbe, String> {
    let playback_settle_ms = std::env::var("L6_EDITOR_PAD_PLAYBACK_SETTLE_MS")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid L6_EDITOR_PAD_PLAYBACK_SETTLE_MS: {error}"))
        })
        .transpose()?
        .unwrap_or(1_100);
    if playback_settle_ms > 10_000 {
        return Err("L6_EDITOR_PAD_PLAYBACK_SETTLE_MS must be between 0 and 10000".into());
    }
    let require_pad_files = std::env::var_os("L6_REQUIRE_PAD_FILES").is_some();
    let endpoints = usb
        .enumerate()
        .map_err(|error| error.to_string())?
        .midi
        .ok_or("USB MIDI interface unavailable")?;
    let mut encoder = Encoder::default();
    let identity = encoder.push(2, &[0xf0, 0x7e, 0, 0x06, 0x01, 0xf7]);
    usb.send(endpoints.output, &identity)
        .map_err(|error| error.to_string())?;
    let trace_playback_rows = Cell::new(false);
    let trace_playback_started = RefCell::new(None);
    let trace_playback_last_rows = RefCell::new(inputs.panel.snapshot().indicators);
    let dump_audio_dma = std::env::var_os("L6_AUDIO_DMA_DUMP").is_some();
    let receive_for = |duration: Duration| -> Result<Vec<Vec<u8>>, String> {
        let deadline = Instant::now() + duration;
        let mut pending = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut messages = Vec::new();
        while Instant::now() < deadline {
            if trace_playback_rows.get() {
                let rows = inputs.panel.snapshot().indicators;
                let previous_rows = *trace_playback_last_rows.borrow();
                if rows != previous_rows {
                    let changed_rows = (0..8)
                        .filter_map(|row| {
                            let changed = previous_rows[row] ^ rows[row];
                            (changed != 0).then_some((row, changed))
                        })
                        .collect::<Vec<_>>();
                    let elapsed_ms = trace_playback_started
                        .borrow()
                        .map(|started: Instant| started.elapsed().as_millis())
                        .unwrap_or_default();
                    println!(
                        "L6 Editor pad indicator trace: t={elapsed_ms}ms rows={:06x?}->{:06x?} changed={changed_rows:?}",
                        previous_rows, rows
                    );
                    trace_playback_last_rows.replace(rows);
                }
            }
            match usb.try_receive(endpoints.input, 16384) {
                Ok(bytes) => {
                    for (cable, message) in decode(&bytes).map_err(|error| error.to_string())? {
                        if cable < 4 {
                            let cable_index = cable as usize;
                            pending[cable_index].extend_from_slice(&message);
                            if message.contains(&0xf7) {
                                let mut complete = std::mem::take(&mut pending[cable_index]);
                                if let Some(end) = complete.iter().position(|byte| *byte == 0xf7) {
                                    complete.truncate(end + 1);
                                }
                                if cable == 2 {
                                    println!("L6 Editor cable 2 reply: {complete:02x?}");
                                }
                                messages.push(complete);
                            }
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(messages)
    };
    let _ = receive_for(Duration::from_millis(500))?;
    let request = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x2b, 0xf7]);
    usb.send(endpoints.output, &request)
        .map_err(|error| error.to_string())?;
    let _ = receive_for(Duration::from_millis(500))?;
    let keep_alive = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x31, 0x0b, 0xf7]);
    usb.send(endpoints.output, &keep_alive)
        .map_err(|error| error.to_string())?;
    let _ = receive_for(Duration::from_millis(100))?;
    let app_version = encoder.push(
        2,
        &[
            0xf0, 0x52, 0, 0, 0x45, 0x05, 0x02, 0, 0, 0, 0, 0, 0x27, 0, 0xf7,
        ],
    );
    usb.send(endpoints.output, &app_version)
        .map_err(|error| error.to_string())?;
    let _ = receive_for(Duration::from_millis(250))?;
    let sd_info = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x67, 0x01, 0xf7]);
    usb.send(endpoints.output, &sd_info)
        .map_err(|error| error.to_string())?;
    let _ = receive_for(Duration::from_millis(500))?;
    // Ask the firmware's SD browser for each pad's file count and first name.
    // This both verifies that the demo card is visible and gives assignment
    // messages the exact filename/index the firmware expects.
    let assign_only_pad = std::env::var("L6_EDITOR_PAD_ASSIGN_ONLY_PAD")
        .ok()
        .map(|value| {
            value
                .parse::<u8>()
                .ok()
                .filter(|pad| *pad < 4)
                .ok_or_else(|| "L6_EDITOR_PAD_ASSIGN_ONLY_PAD must be 0, 1, 2, or 3".to_owned())
        })
        .transpose()?;
    let assignment_pads =
        assign_only_pad.map_or_else(|| (0..4).collect::<Vec<_>>(), |pad| vec![pad]);
    let mut pad_assignments = Vec::new();
    for pad in assignment_pads {
        let count_request = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 0x00, pad, 0xf7]);
        usb.send(endpoints.output, &count_request)
            .map_err(|error| error.to_string())?;
        let replies = receive_for(Duration::from_millis(500))?;
        let file_count = replies.iter().find_map(|reply| {
            (reply.get(4..7) == Some(&[0x45, 0x00, pad]) && reply.len() >= 10)
                .then(|| ((reply[7] as u16) << 7) | reply[8] as u16)
        });
        if let Some(count) = file_count {
            println!("L6 Editor pad {pad} file count: {count}");
        }
        if require_pad_files && !file_count.is_some_and(|count| count > 0) {
            return Err(format!("L6 Editor pad {pad} has no enumerable WAV files"));
        }
        let mut names = Vec::new();
        for _ in 0..4 {
            let name_request = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 0x01, pad, 0, 0, 0xf7]);
            usb.send(endpoints.output, &name_request)
                .map_err(|error| error.to_string())?;
            // The SD browser can defer a filename reply while it processes
            // assignment events for another pad. Retry the idempotent query
            // instead of treating a transiently empty read as no WAV file.
            let replies = receive_for(Duration::from_millis(750))?;
            let found_filename = replies.iter().any(|reply| {
                reply.get(4..7) == Some(&[0x45, 0x01, pad])
                    && reply.len() >= 12
                    && reply.get(7..9) != Some(&[0x7f, 0x7f])
                    && reply.get(9..11) != Some(&[0, 0])
            });
            names.extend(replies);
            if found_filename {
                break;
            }
        }
        let mut assigned_args = None;
        let mut assignment_dialog_answered = false;
        for name_reply in names {
            println!("L6 Editor pad {pad} file index 0: {name_reply:02x?}");
            let has_filename = file_count.is_some_and(|count| count > 0)
                && name_reply.get(4..7) == Some(&[0x45, 0x01, pad])
                && name_reply.len() >= 12
                && name_reply.get(7..9) != Some(&[0x7f, 0x7f])
                && name_reply.get(9..11) != Some(&[0, 0]);
            if assign_files && has_filename {
                // File-name dump response can be fed back directly as the
                // assignment payload following command 31 05.
                let args = &name_reply[6..name_reply.len() - 1];
                let mut assignment = vec![0xf0, 0x52, 0, 0, 0x31, 0x05];
                assignment.extend_from_slice(args);
                assignment.push(0xf7);
                let packet = encoder.push(2, &assignment);
                usb.send(endpoints.output, &packet)
                    .map_err(|error| error.to_string())?;
                let mut assignment_replies = receive_for(Duration::from_millis(300))?;
                if answer_pad_assignment_dialog(
                    usb,
                    endpoints,
                    &mut encoder,
                    &assignment_replies,
                    &mut assignment_dialog_answered,
                )? {
                    assignment_replies.extend(receive_for(Duration::from_millis(300))?);
                }
                for reply in &assignment_replies {
                    println!("L6 Editor pad {pad} assignment reply: {reply:02x?}");
                }
                assigned_args = Some(args.to_vec());
            }
        }
        if assign_files && require_pad_files && assigned_args.is_none() {
            return Err(format!(
                "L6 Editor returned no valid filename for pad {pad}"
            ));
        }
        let mut assigned_replies = Vec::new();
        // Assignment updates can be deferred through the firmware event
        // queue, so poll the authoritative state rather than treating a
        // missing immediate acknowledgement as a rejected assignment.
        for _ in 0..8 {
            let assigned_request = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 0x02, pad, 0xf7]);
            usb.send(endpoints.output, &assigned_request)
                .map_err(|error| error.to_string())?;
            assigned_replies = receive_for(Duration::from_millis(250))?;
            if answer_pad_assignment_dialog(
                usb,
                endpoints,
                &mut encoder,
                &assigned_replies,
                &mut assignment_dialog_answered,
            )? {
                assigned_replies.extend(receive_for(Duration::from_millis(300))?);
            }
            if !assign_files
                || assigned_replies.iter().any(|reply| {
                    reply.get(4..7) == Some(&[0x45, 0x02, pad])
                        && reply.get(6..reply.len().saturating_sub(1)) == assigned_args.as_deref()
                })
            {
                break;
            }
        }
        for reply in &assigned_replies {
            println!("L6 Editor pad {pad} assignment state: {reply:02x?}");
        }
        if assign_files
            && require_pad_files
            && !assigned_replies.iter().any(|reply| {
                reply.get(4..7) == Some(&[0x45, 0x02, pad])
                    && reply.get(6..reply.len().saturating_sub(1)) == assigned_args.as_deref()
            })
        {
            return Err(format!("L6 Editor assignment query mismatch on pad {pad}"));
        }
        pad_assignments.push((pad, assigned_replies));
    }
    if std::env::var_os("L6_PAD_LED_SKIP_PLAYBACK").is_some()
        || std::env::var_os("L6MAX_EDITOR_PAD_ASSIGN_ONLY").is_some()
        || std::env::var_os("L6_SOUND_PAD_VERIFY_UI_ASSIGNMENT").is_some()
    {
        println!("Editor pad playback probe skipped after assignment checks.");
        return Ok(L6EditorProbe {
            assignments: pad_assignments,
        });
    }
    // Exercise the firmware's own pad-play state machine even when the SD
    // browser reports no playable files, and query each resulting state.
    let only_pad = std::env::var("L6_PAD_LED_PROBE_ONLY")
        .ok()
        .map(|value| {
            value
                .parse::<u8>()
                .ok()
                .filter(|pad| *pad < 4)
                .ok_or_else(|| "L6_PAD_LED_PROBE_ONLY must be 0, 1, 2, or 3".to_owned())
        })
        .transpose()?;
    let pads = only_pad.map_or_else(|| (0..4).collect::<Vec<_>>(), |pad| vec![pad]);
    for pad in pads {
        let mut playback_dialog_answered = false;
        let indicator_before = inputs.panel.snapshot().indicators;
        // The per-pad playing flag at 0x806ea265 + pad * 0x44 distinguishes a
        // failed play request from a bad observation of the separate LED record.
        let playback_flag_address = 0x806ea265 + u32::from(pad) * 0x44;
        let active_pad_before = qmp
            .read_memory(0, 0x80463005, 1)
            .map_err(|error| error.to_string())?;
        let sampler_state_before = qmp
            .read_memory(0, 0x80461a7c, 16)
            .map_err(|error| error.to_string())?;
        let sampler_busy_flags_before = qmp
            .read_memory(0, 0x80463001, 4)
            .map_err(|error| error.to_string())?;
        let playback_flag_before = qmp
            .read_memory(0, playback_flag_address, 1)
            .map_err(|error| error.to_string())?;
        let pad_led_before = qmp
            .read_memory(0, 0x806293f4, 8)
            .map_err(|error| error.to_string())?;
        let shared_led_before = qmp
            .read_memory(0, 0x806293c6, 2)
            .map_err(|error| error.to_string())?;
        trace_playback_last_rows.replace(indicator_before);
        trace_playback_started.replace(Some(Instant::now()));
        trace_playback_rows.set(true);
        let audio_samples_before = if std::env::var_os("L6_REQUIRE_PAD_AUDIO").is_some() {
            Some(trace_count(logs, "l6 audio TX samples sai=00384000")?)
        } else {
            None
        };
        let play = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x31, 0x08, pad, 1, 0xf7]);
        usb.send(endpoints.output, &play)
            .map_err(|error| error.to_string())?;
        let play_replies = receive_for(Duration::from_millis(100))?;
        if answer_pad_assignment_dialog(
            usb,
            endpoints,
            &mut encoder,
            &play_replies,
            &mut playback_dialog_answered,
        )? {
            let _ = receive_for(Duration::from_millis(300))?;
        }
        let mut state_replies = Vec::new();
        // Like file assignment, the play-button event is dispatched through
        // the firmware task queue. Poll the playing-state query so the test
        // does not mistake the immediate pre-event state for a rejection.
        for _ in 0..8 {
            let state = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 0x03, pad, 0xf7]);
            usb.send(endpoints.output, &state)
                .map_err(|error| error.to_string())?;
            state_replies = receive_for(Duration::from_millis(250))?;
            if answer_pad_assignment_dialog(
                usb,
                endpoints,
                &mut encoder,
                &state_replies,
                &mut playback_dialog_answered,
            )? {
                state_replies.extend(receive_for(Duration::from_millis(300))?);
            }
            if state_replies
                .iter()
                .any(|reply| reply.get(4..8) == Some(&[0x45, 0x03, pad, 0x01]))
            {
                break;
            }
        }
        let settle_replies = receive_for(Duration::from_millis(playback_settle_ms))?;
        if answer_pad_assignment_dialog(
            usb,
            endpoints,
            &mut encoder,
            &settle_replies,
            &mut playback_dialog_answered,
        )? {
            let _ = receive_for(Duration::from_millis(300))?;
            for _ in 0..8 {
                let state = encoder.push(2, &[0xf0, 0x52, 0, 0, 0x46, 0x03, pad, 0xf7]);
                usb.send(endpoints.output, &state)
                    .map_err(|error| error.to_string())?;
                state_replies = receive_for(Duration::from_millis(250))?;
                if state_replies
                    .iter()
                    .any(|reply| reply.get(4..8) == Some(&[0x45, 0x03, pad, 0x01]))
                {
                    break;
                }
            }
            let _ = receive_for(Duration::from_millis(playback_settle_ms))?;
        }
        trace_playback_rows.set(false);
        if dump_audio_dma {
            qmp.stop().map_err(|error| error.to_string())?;
            let dump_result = (|| {
                dump_l6_audio_tx_buffer(qmp, 14, logs)?;
                dump_l6_audio_tx_buffer(qmp, 12, logs)
            })();
            let resume_result = qmp.resume().map_err(|error| error.to_string());
            dump_result?;
            resume_result?;
        }
        let indicator_playing = inputs.panel.snapshot().indicators;
        let active_pad_playing = qmp
            .read_memory(0, 0x80463005, 1)
            .map_err(|error| error.to_string())?;
        let sampler_state_playing = qmp
            .read_memory(0, 0x80461a7c, 16)
            .map_err(|error| error.to_string())?;
        let sampler_busy_flags_playing = qmp
            .read_memory(0, 0x80463001, 4)
            .map_err(|error| error.to_string())?;
        let playback_flag_playing = qmp
            .read_memory(0, playback_flag_address, 1)
            .map_err(|error| error.to_string())?;
        let pad_led_playing = qmp
            .read_memory(0, 0x806293f4, 8)
            .map_err(|error| error.to_string())?;
        let shared_led_playing = qmp
            .read_memory(0, 0x806293c6, 2)
            .map_err(|error| error.to_string())?;
        let changed_rows = (0..8)
            .filter_map(|row| {
                let changed = indicator_before[row] ^ indicator_playing[row];
                (changed != 0).then_some((row, changed))
            })
            .collect::<Vec<_>>();
        println!(
            "L6 Editor pad {pad} playback indicators: rows={:06x?}->{:06x?}, changed={changed_rows:?}; active pad at 0x80463005={active_pad_before:02x?}->{active_pad_playing:02x?}; sampler arrays 0x80461a7c={sampler_state_before:02x?}->{sampler_state_playing:02x?}, busy flags 0x80463001={sampler_busy_flags_before:02x?}->{sampler_busy_flags_playing:02x?}; firmware flag at {playback_flag_address:#010x}={playback_flag_before:02x?}->{playback_flag_playing:02x?}; pad records={pad_led_before:02x?}->{pad_led_playing:02x?}; shared record={shared_led_before:02x?}->{shared_led_playing:02x?}",
            &indicator_before[..8],
            &indicator_playing[..8]
        );
        if require_pad_files
            && !state_replies
                .iter()
                .any(|reply| reply.get(4..8) == Some(&[0x45, 0x03, pad, 0x01]))
        {
            return Err(format!("L6 Editor pad {pad} did not enter playing state"));
        }
        if let Some(sample_lines_before) = audio_samples_before {
            let trace =
                fs::read_to_string(logs.join("engine.trace")).map_err(|error| error.to_string())?;
            let nonzero_tx = trace
                .lines()
                .filter(|line| line.starts_with("l6 audio TX samples sai=00384000"))
                .skip(sample_lines_before)
                .any(|line| {
                    line.split_whitespace()
                        .find_map(|field| field.strip_prefix("nonzero="))
                        .and_then(|count| count.parse::<u64>().ok())
                        .is_some_and(|count| count > 0)
                });
            if !nonzero_tx {
                return Err(format!(
                    "L6 Editor pad {pad} entered playing state but produced no nonzero SAI1 TX samples"
                ));
            }
        }
    }
    Ok(L6EditorProbe {
        assignments: pad_assignments,
    })
}

fn dump_l6_audio_tx_buffer(qmp: &QmpClient, channel: u32, logs: &Path) -> Result<(), String> {
    let tcd_address = 0x400e9000 + channel * 0x20;
    let tcd = qmp
        .read_memory(0, tcd_address, 0x20)
        .map_err(|error| format!("read eDMA channel {channel} TCD: {error}"))?;
    let saddr = u32::from_le_bytes(tcd[0..4].try_into().unwrap());
    let soff = i16::from_le_bytes(tcd[4..6].try_into().unwrap());
    let attr = u16::from_le_bytes(tcd[6..8].try_into().unwrap());
    let nbytes = u32::from_le_bytes(tcd[8..12].try_into().unwrap());
    let daddr = u32::from_le_bytes(tcd[16..20].try_into().unwrap());
    let doff = i16::from_le_bytes(tcd[20..22].try_into().unwrap());
    let citer = u16::from_le_bytes(tcd[22..24].try_into().unwrap());
    let dlast_sga = u32::from_le_bytes(tcd[24..28].try_into().unwrap());
    let csr = u16::from_le_bytes(tcd[28..30].try_into().unwrap());
    let biter = u16::from_le_bytes(tcd[30..32].try_into().unwrap());
    println!(
        "L6 eDMA TX channel {channel}: TCD={tcd_address:#010x} SADDR={saddr:#010x} SOFF={soff} ATTR={attr:#06x} NBYTES={nbytes} DADDR={daddr:#010x} DOFF={doff} CITER={citer:#06x}/{biter:#06x} DLAST_SGA={dlast_sga:#010x} CSR={csr:#06x}"
    );
    if nbytes == 0 || biter == 0 {
        println!("L6 eDMA TX channel {channel}: inactive TCD, no source buffer to capture");
        return Ok(());
    }
    let completed = u32::from(biter.saturating_sub(citer));
    let buffer_address = saddr
        .checked_sub(completed.saturating_mul(nbytes))
        .ok_or_else(|| format!("L6 eDMA channel {channel} source offset underflow"))?;
    let buffer_size = nbytes
        .checked_mul(u32::from(biter))
        .filter(|size| *size > 0 && *size <= 4096)
        .ok_or_else(|| format!("L6 eDMA channel {channel} buffer size is invalid"))?
        as usize;
    if (0x2000_0000..0x2008_0000).contains(&buffer_address) {
        let bytes = qmp
            .read_memory(0, buffer_address, buffer_size)
            .map_err(|error| format!("read eDMA channel {channel} source buffer: {error}"))?;
        let path = logs.join(format!("audio-tx-channel-{channel}.bin"));
        fs::write(&path, &bytes).map_err(|error| error.to_string())?;
        println!(
            "L6 eDMA TX channel {channel}: captured {buffer_size} bytes from TCD source block {buffer_address:#010x} to {}",
            path.display()
        );
    }
    if csr & 0x0010 != 0 && (0x2000_0000..0x2008_0000).contains(&dlast_sga) {
        let linked = qmp
            .read_memory(0, dlast_sga, 0x20)
            .map_err(|error| format!("read linked eDMA channel {channel} TCD: {error}"))?;
        let linked_saddr = u32::from_le_bytes(linked[0..4].try_into().unwrap());
        let linked_nbytes = u32::from_le_bytes(linked[8..12].try_into().unwrap());
        let linked_citer = u16::from_le_bytes(linked[22..24].try_into().unwrap());
        let linked_dlast_sga = u32::from_le_bytes(linked[24..28].try_into().unwrap());
        let linked_csr = u16::from_le_bytes(linked[28..30].try_into().unwrap());
        let linked_biter = u16::from_le_bytes(linked[30..32].try_into().unwrap());
        println!(
            "L6 eDMA TX channel {channel}: linked TCD={dlast_sga:#010x} SADDR={linked_saddr:#010x} NBYTES={linked_nbytes} CITER={linked_citer:#06x}/{linked_biter:#06x} DLAST_SGA={linked_dlast_sga:#010x} CSR={linked_csr:#06x}"
        );
        if linked_nbytes != 0 && linked_biter != 0 {
            let linked_completed = u32::from(linked_biter.saturating_sub(linked_citer));
            let linked_buffer = linked_saddr
                .checked_sub(linked_completed.saturating_mul(linked_nbytes))
                .ok_or_else(|| {
                    format!("L6 eDMA channel {channel} linked source offset underflow")
                })?;
            let linked_size = linked_nbytes
                .checked_mul(u32::from(linked_biter))
                .filter(|size| *size > 0 && *size <= 4096)
                .map(|size| size as usize);
            if let Some(size) =
                linked_size.filter(|_| (0x2000_0000..0x2008_0000).contains(&linked_buffer))
            {
                let bytes = qmp.read_memory(0, linked_buffer, size).map_err(|error| {
                    format!("read linked eDMA channel {channel} source buffer: {error}")
                })?;
                let path = logs.join(format!("audio-tx-channel-{channel}-linked.bin"));
                fs::write(&path, &bytes).map_err(|error| error.to_string())?;
                println!(
                    "L6 eDMA TX channel {channel}: captured linked source block {linked_buffer:#010x} ({size} bytes) to {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn run_l6max_panel_key_probe(
    inputs: &InputHub,
    qmp: &QmpClient,
    display: &PublishedDisplay,
    latest: &mut Option<Frame>,
) -> Result<(), String> {
    let lookup = qmp
        .read_memory(0, 0x800c_48c4, 0x1d * 8)
        .map_err(|error| error.to_string())?;
    for code in 0x19..=0x1c {
        let offset = code * 8;
        let bit = u32::from_le_bytes(lookup[offset..offset + 4].try_into().unwrap());
        let index = u32::from_le_bytes(lookup[offset + 4..offset + 8].try_into().unwrap());
        println!(
            "L6max channel-control key code {code:#04x}: state-bit={bit}, word-index={index:#010x}"
        );
    }
    let read_words = || -> Result<[u32; 6], String> {
        let bytes = qmp
            .read_memory(0, 0x8088_3554, 24)
            .map_err(|error| error.to_string())?;
        Ok(std::array::from_fn(|index| {
            u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
        }))
    };
    // The aliases 31/36/41 duplicate matrix positions already addressed by
    // 2/1/6. Skip target 4/5, which belong to the MAIN MCU.
    let targets = (1..=47)
        .filter(|target| !matches!(*target, 4 | 5 | 31 | 36 | 41))
        .collect::<Vec<_>>();
    for target in targets {
        let before = read_words()?;
        let applied = inputs.panel.snapshot().applied;
        inputs
            .panel
            .down(target, 0)
            .map_err(|error| error.to_string())?;
        wait_applied(&inputs.panel, applied + 1, display, latest)?;
        pump_for(display, latest, Duration::from_millis(100))?;
        let held = read_words()?;
        let applied = inputs.panel.snapshot().applied;
        inputs.panel.up(target).map_err(|error| error.to_string())?;
        wait_applied(&inputs.panel, applied + 1, display, latest)?;
        pump_for(display, latest, Duration::from_millis(100))?;
        let released = read_words()?;
        let changed = (0..6)
            .filter_map(|index| {
                let delta = before[index] ^ held[index];
                (delta != 0).then_some((index, delta))
            })
            .collect::<Vec<_>>();
        let release_delta = (0..6)
            .filter_map(|index| {
                let delta = before[index] ^ released[index];
                (delta != 0).then_some((index, delta))
            })
            .collect::<Vec<_>>();
        println!(
            "L6max panel target {target}: keys before={before:08x?} held={held:08x?} released={released:08x?}; changed={changed:?}; release-delta={release_delta:?}"
        );
    }
    Ok(())
}

pub(crate) fn run(args: Vec<std::ffi::OsString>) -> Result<(), String> {
    let mut options = Options::parse_args(args)?;
    let play_gpio_gdb_socket = if std::env::var_os("L6_PLAY_GPIO_RUNTIME_PROBE").is_some() {
        let socket = options.gdb_socket.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("l6-play-gpio-{}.gdb.sock", std::process::id()))
        });
        options.gdb_socket = Some(socket.clone());
        Some(socket)
    } else {
        None
    };
    let gpio_event_gdb_socket = if std::env::var_os("L6_MAIN_GPIO_EVENT_TRACE").is_some() {
        let socket = options.gdb_socket.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "l6-main-gpio-event-{}.gdb.sock",
                std::process::id()
            ))
        });
        options.gdb_socket = Some(socket.clone());
        Some(socket)
    } else {
        None
    };
    let l6max_recorder_gdb_socket = if std::env::var_os("L6MAX_RECORDER_PLAYBACK_PROBE").is_some() {
        let socket = options.gdb_socket.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "l6max-recorder-playback-{}.gdb.sock",
                std::process::id()
            ))
        });
        options.gdb_socket = Some(socket.clone());
        Some(socket)
    } else {
        None
    };
    let sound_pad_probe = std::env::var_os("L6_SOUND_PAD_PLAYBACK_PROBE");
    let editor_pad_probe_only = std::env::var_os("L6_EDITOR_PAD_PROBE_ONLY").is_some();
    let l6_gpio_editor_setup = std::env::var_os("L6_MAIN_GPIO_EDITOR_SETUP").is_some();
    if sound_pad_probe.is_some() {
        let _ = l6max_sound_pad_targets()?;
    }
    if editor_pad_probe_only && std::env::var_os("L6_MAIN_BUTTON_LED_TRACE").is_none() {
        return Err("L6_EDITOR_PAD_PROBE_ONLY requires L6_MAIN_BUTTON_LED_TRACE=1".into());
    }
    if l6_gpio_editor_setup && std::env::var_os("L6_MAIN_BUTTON_LED_TRACE").is_none() {
        return Err("L6_MAIN_GPIO_EDITOR_SETUP requires L6_MAIN_BUTTON_LED_TRACE=1".into());
    }
    if gpio_event_gdb_socket.is_some() && !l6_gpio_editor_setup {
        return Err("L6_MAIN_GPIO_EVENT_TRACE requires L6_MAIN_GPIO_EDITOR_SETUP=1".into());
    }
    let delayed_sd_image = std::env::var_os("L6_SOUND_PAD_SD_AFTER_SETUP").map(PathBuf::from);
    let skip_onboarding = (sound_pad_probe.is_some()
        || l6_gpio_editor_setup
        || std::env::var_os("L6MAX_EDITOR_PAD_ASSIGN_TRACE").is_some())
        && std::env::var_os("L6_SOUND_PAD_SKIP_ONBOARDING").is_some();
    if skip_onboarding && options.state_dir.is_none() {
        return Err("L6_SOUND_PAD_SKIP_ONBOARDING requires persistent --state-dir storage".into());
    }
    if sound_pad_probe.is_some()
        && std::env::var_os("L6_SOUND_PAD_SETUP_ONLY").is_none()
        && delayed_sd_image.is_none()
        && options.sd_image.is_none()
    {
        return Err(
            "L6_SOUND_PAD_PLAYBACK_PROBE needs --sd-image or L6_SOUND_PAD_SD_AFTER_SETUP".into(),
        );
    }
    if delayed_sd_image.is_some() {
        options.no_sd = true;
        options.sd_image = None;
    }
    let require_audio = std::env::var_os("L6_REQUIRE_AUDIO").is_some();
    if require_audio && std::env::var_os("L6_AUDIO_TRACE").is_none() {
        return Err("L6_REQUIRE_AUDIO needs L6_AUDIO_TRACE=1".into());
    }
    let trace_button_leds = std::env::var_os("L6_MAIN_BUTTON_LED_TRACE").is_some();
    let trace_only = std::env::var_os("L6_MAIN_GPIO_TRACE_ONLY").is_some();
    let engine = Engine::start(&options)?;
    let inputs = engine.inputs.as_ref().ok_or("QEMU is required")?;
    if l6_gpio_editor_setup && engine.model() != l6fw::Model::L6 {
        return Err("L6_MAIN_GPIO_EDITOR_SETUP requires L6 firmware".into());
    }
    if let Ok(value) = std::env::var("L6_SOUND_PAD_LEVEL") {
        let level = value
            .parse::<u16>()
            .ok()
            .filter(|level| *level <= 1023)
            .ok_or("L6_SOUND_PAD_LEVEL must be an ADC value from 0 to 1023")?;
        let mut analog_positions = inputs.analog_positions();
        analog_positions[0] = level;
        inputs
            .restore_analog_positions(analog_positions)
            .map_err(|error| error.to_string())?;
        println!("Sound Pad level ADC set to {level} before firmware startup.");
    }
    if trace_only && engine.model() != l6fw::Model::L6 {
        return Err("L6_MAIN_GPIO_TRACE_ONLY requires L6 firmware".into());
    }
    for client in &engine.management {
        if !client.query_status().map_err(|e| e.to_string())?.running {
            return Err("guest did not resume".into());
        }
    }
    let display = PublishedDisplay::new(
        engine
            .display_memory
            .as_ref()
            .ok_or("published display memory unavailable")?
            .map(),
        inputs.main.clone(),
    )
    .map_err(|error| error.to_string())?;
    let mut latest = None;
    println!("QMP running with both CPUs; consuming published frames while firmware boots...");
    if std::env::var_os("L6MAX_EDITOR_PAD_ASSIGN_TRACE").is_some() {
        if engine.model() != l6fw::Model::L6max {
            return Err("L6MAX_EDITOR_PAD_ASSIGN_TRACE requires L6max firmware".into());
        }
        // L6max's USB startup path normally needs recorder-function-4 at 40
        // and 42 seconds of guest time. Let hardware-model investigations
        // move that sequence later without tying it to host wall time.
        let setup_start_ms = std::env::var("L6MAX_EDITOR_PAD_ASSIGN_START_MS")
            .ok()
            .map(|value| {
                value
                    .parse::<u32>()
                    .map_err(|_| "L6MAX_EDITOR_PAD_ASSIGN_START_MS must be an integer")
            })
            .transpose()?;
        let setup_start_ms = setup_start_ms.unwrap_or(40_000);
        let setup_ready_ms = setup_start_ms
            .checked_add(7_000)
            .ok_or("L6MAX_EDITOR_PAD_ASSIGN_START_MS is too large")?;
        for target_ms in [setup_start_ms, setup_start_ms + 2_000] {
            pump_until_guest_ms(inputs, &display, &mut latest, target_ms)?;
            let applied = inputs.panel.snapshot().applied;
            let released = inputs.panel.snapshot().released;
            inputs.panel.tap(3, 50).map_err(|error| error.to_string())?;
            wait_applied(&inputs.panel, applied + 1, &display, &mut latest)?;
            wait_released(&inputs.panel, released + 1)?;
        }
        pump_until_guest_ms(inputs, &display, &mut latest, setup_ready_ms)?;
    } else if editor_pad_probe_only || l6_gpio_editor_setup {
        if engine.model() != l6fw::Model::L6 {
            return Err("L6 editor startup sequence requires L6 firmware".into());
        }
        // Match the verified L6 USB startup sequence: the two recorder
        // function presses precede USB enumeration at about 47 seconds.
        for (target_ms, next_ms) in [(40_000, 42_000), (42_000, 47_000)] {
            pump_until_guest_ms(inputs, &display, &mut latest, target_ms)?;
            let applied = inputs.panel.snapshot().applied;
            let released = inputs.panel.snapshot().released;
            inputs.panel.tap(3, 50).map_err(|error| error.to_string())?;
            wait_applied(&inputs.panel, applied + 1, &display, &mut latest)?;
            wait_released(&inputs.panel, released + 1)?;
            if next_ms == 47_000 {
                pump_until_guest_ms(inputs, &display, &mut latest, next_ms)?;
            }
        }
    } else {
        pump_for(&display, &mut latest, boot_settle_delay())?;
    }
    if std::env::var_os("L6MAX_PANEL_KEY_TRACE").is_some() {
        if engine.model() != l6fw::Model::L6max {
            return Err("L6MAX_PANEL_KEY_TRACE requires L6max firmware".into());
        }
        run_l6max_panel_key_probe(inputs, &engine.management[0], &display, &mut latest)?;
        return Ok(());
    }
    if std::env::var_os("L6MAX_EDITOR_PAD_ASSIGN_TRACE").is_some() {
        if engine.model() != l6fw::Model::L6max {
            return Err("L6MAX_EDITOR_PAD_ASSIGN_TRACE requires L6max firmware".into());
        }
        if delayed_sd_image.is_some() {
            return Err(
                "L6MAX_EDITOR_PAD_ASSIGN_TRACE needs a preformatted card attached at boot with --sd-image; delayed insertion enters SD-card setup".into(),
            );
        }
        let usb = engine
            .usb
            .as_ref()
            .ok_or("USB is required for Editor SysEx")?;
        probe_l6_editor(usb, &engine.management[0], inputs, &options.logs, true)?;
        if let Some(value) = sound_pad_probe.as_ref() {
            let pad = value
                .to_str()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|pad| *pad < 4)
                .ok_or("L6_SOUND_PAD_PLAYBACK_PROBE must be a pad index from 0 to 3")?;
            if std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_none() {
                return Err("L6_SOUND_PAD_PLAYBACK_PROBE needs L6_AUDIO_SAMPLE_TRACE=1".into());
            }
            if std::env::var_os("L6_PAD_LED_SKIP_PLAYBACK").is_none() {
                return Err(
                    "Set L6_PAD_LED_SKIP_PLAYBACK=1 so Editor playback cannot contaminate the physical-key audio check".into(),
                );
            }
            if std::env::var_os("L6_REQUIRE_PAD_FILES").is_none() {
                return Err(
                    "Set L6_REQUIRE_PAD_FILES=1 to verify Editor assigned WAVs before physical-key testing".into(),
                );
            }
            println!("Waiting for Editor assignment events to settle before GPIO testing.");
            pump_for(&display, &mut latest, Duration::from_secs(3))?;
            let targets = l6max_sound_pad_targets()?;
            let mut results = Vec::new();
            for target in targets.iter().copied() {
                let tested_pad = if targets.len() == 1 { pad } else { target - 49 };
                let pad_state_address = 0x806293f4 + tested_pad * 2;
                let pad_record_before = engine.management[0]
                    .read_memory(0, pad_state_address, 2)
                    .map_err(|error| error.to_string())?;
                let pad_record_before = [pad_record_before[0], pad_record_before[1]];
                let runtime_before = l6max_pad_runtime(&engine.management[0])?;
                let indicator_rows_before = inputs.panel.snapshot().indicators;
                let tx_lines_before =
                    trace_count(&options.logs, "l6 audio TX samples sai=00384000")?;
                let timing_only = std::env::var_os("L6_AUDIO_TIMING_TRACE").is_some();
                let playback_guest_start = inputs.panel.snapshot().guest_ms;
                let playback_wall_start = Instant::now();
                let applied = inputs.main.snapshot().applied;
                inputs
                    .main
                    .down(target, 500)
                    .map_err(|error| error.to_string())?;
                wait_applied(&inputs.main, applied + 1, &display, &mut latest)?;
                let mut indicator_samples = if timing_only {
                    pump_for(&display, &mut latest, Duration::from_millis(250))?;
                    Vec::new()
                } else {
                    sample_l6max_panel_rows(
                        inputs,
                        &engine.management[0],
                        pad_state_address,
                        &display,
                        &mut latest,
                        Duration::from_millis(250),
                    )?
                };
                if std::env::var_os("L6_AUDIO_DMA_DUMP").is_some() {
                    engine.management[0]
                        .stop()
                        .map_err(|error| error.to_string())?;
                    let dump_result = (|| {
                        dump_l6_audio_tx_buffer(&engine.management[0], 14, &options.logs)?;
                        dump_l6_audio_tx_buffer(&engine.management[0], 12, &options.logs)
                    })();
                    let resume_result = engine.management[0]
                        .resume()
                        .map_err(|error| error.to_string());
                    dump_result?;
                    resume_result?;
                }
                let runtime_held = l6max_pad_runtime(&engine.management[0])?;
                let indicator_rows_held = indicator_samples
                    .last()
                    .map(|(_, rows, _)| *rows)
                    .unwrap_or_else(|| inputs.panel.snapshot().indicators);
                let pad_record_held = indicator_samples
                    .last()
                    .map(|(_, _, record)| *record)
                    .unwrap_or(pad_record_before);
                let applied = inputs.main.snapshot().applied;
                inputs.main.up(target).map_err(|error| error.to_string())?;
                wait_applied(&inputs.main, applied + 1, &display, &mut latest)?;
                let held_elapsed = indicator_samples
                    .last()
                    .map(|(elapsed, _, _)| *elapsed)
                    .unwrap_or_default()
                    + 20;
                let released_samples = if timing_only {
                    // Keep the guest running while measuring wall-clock audio
                    // pacing; QMP memory reads stop it and skew the ratio.
                    pump_for(&display, &mut latest, audio_post_play_settle()?)?;
                    Vec::new()
                } else {
                    sample_l6max_panel_rows(
                        inputs,
                        &engine.management[0],
                        pad_state_address,
                        &display,
                        &mut latest,
                        // Let one-shot samples longer than the key hold finish
                        // before QEMU exits and finalizes the backend stream.
                        audio_post_play_settle()?,
                    )?
                };
                indicator_samples.extend(
                    released_samples
                        .into_iter()
                        .map(|(elapsed, rows, record)| (held_elapsed + elapsed, rows, record)),
                );
                let (changed_led_indices, changed_rows, sampled_pad_records) =
                    summarize_l6max_panel_rows(
                        &options.logs,
                        target,
                        indicator_rows_before,
                        indicator_rows_held,
                        pad_record_before,
                        &indicator_samples,
                    )?;
                let runtime_after = l6max_pad_runtime(&engine.management[0])?;
                let pad_state_after = engine.management[0]
                    .read_memory(0, pad_state_address, 2)
                    .map_err(|error| error.to_string())?;
                let trace = fs::read_to_string(options.logs.join("engine.trace"))
                    .map_err(|error| error.to_string())?;
                let nonzero_tx = trace
                    .lines()
                    .filter(|line| line.starts_with("l6 audio TX samples sai=00384000"))
                    .skip(tx_lines_before)
                    .any(|line| {
                        line.split_whitespace()
                            .find_map(|field| field.strip_prefix("nonzero="))
                            .and_then(|count| count.parse::<u64>().ok())
                            .is_some_and(|count| count > 0)
                    });
                if timing_only {
                    let wall_seconds = playback_wall_start.elapsed().as_secs_f64();
                    let guest_ms = inputs
                        .panel
                        .snapshot()
                        .guest_ms
                        .wrapping_sub(playback_guest_start);
                    println!(
                        "L6max active playback timing: wall={wall_seconds:.3}s guest={:.3}s guest/wall={:.3}",
                        guest_ms as f64 / 1000.0,
                        guest_ms as f64 / 1000.0 / wall_seconds
                    );
                }
                let expected_pad_led = 27 + tested_pad as usize;
                if !timing_only && nonzero_tx && !changed_led_indices.contains(&expected_pad_led) {
                    return Err(format!(
                        "L6max target {target} produced SAI1 audio but did not change expected Pad {} LED {} (changed {changed_led_indices:?})",
                        tested_pad + 1,
                        expected_pad_led
                    ));
                }
                println!(
                    "L6max candidate target {target} / Pad {}: runtime before=[{runtime_before}], held=[{runtime_held}], after=[{runtime_after}], pad record before={pad_record_before:02x?} held={pad_record_held:02x?} after={pad_state_after:02x?} sampled={sampled_pad_records:02x?}; panel rows before={indicator_rows_before:06x?}, held={indicator_rows_held:06x?}, changed={changed_rows:06x?}; logical LEDs changed={changed_led_indices:?}; nonzero SAI1 after press={nonzero_tx}",
                    tested_pad + 1
                );
                results.push((target, tested_pad, nonzero_tx));
                if nonzero_tx && targets.len() > 1 {
                    println!(
                        "Stopping the sweep after the first audio onset; remaining candidates need fresh boots to avoid carryover."
                    );
                    break;
                }
            }
            if targets.len() == 1 && !results[0].2 {
                return Err(format!(
                    "L6max candidate target {} did not produce nonzero SAI1 samples after verified Editor assignment",
                    targets[0]
                ));
            }
            if results.iter().all(|(_, _, played)| !played) {
                return Err(format!(
                    "no L6max sound-pad candidate produced audio: {results:?}"
                ));
            }
            if results.len() < targets.len() {
                return Err(format!(
                    "candidate sweep stopped after playback began; tested {results:?}; untested targets: {:?}",
                    &targets[results.len()..]
                ));
            }
        }
        return Ok(());
    }
    if editor_pad_probe_only {
        if engine.model() != l6fw::Model::L6 {
            return Err("L6_EDITOR_PAD_PROBE_ONLY requires L6 firmware".into());
        }
        let usb = engine.usb.as_ref().ok_or("USB host is unavailable")?;
        probe_l6_editor(usb, &engine.management[0], inputs, &options.logs, true)?;
        println!("L6 Editor pad playback probe completed.");
        return Ok(());
    }
    if trace_button_leds
        && (!trace_only || l6_gpio_editor_setup)
        && engine.model() == l6fw::Model::L6
    {
        if let Some(usb) = &engine.usb {
            let editor_probe =
                probe_l6_editor(usb, &engine.management[0], inputs, &options.logs, true);
            if l6_gpio_editor_setup {
                editor_probe?;
                if let Some(socket) = gpio_event_gdb_socket.as_deref() {
                    let mut debugger = Gdb::connect(socket).map_err(|error| error.to_string())?;
                    let callback = trace_l6_assignment_callback(
                        &mut debugger,
                        &engine.management[0],
                        Duration::from_secs(3),
                    )?;
                    println!(
                        "L6 assignment completion callback reached after Editor assignment={callback}"
                    );
                }
                println!("Waiting for L6 pad assignment to settle before GPIO testing.");
                pump_for(&display, &mut latest, Duration::from_secs(3))?;
                if std::env::var_os("L6_REQUIRE_PAD_FILES").is_some() {
                    let target = std::env::var("L6_MAIN_GPIO_PROBE_ONLY").unwrap_or_default();
                    let pads = match target.as_str() {
                        "pad-1" => vec![0],
                        "pad-2" => vec![1],
                        "pad-3" => vec![2],
                        "pad-4" => vec![3],
                        "" | "all" => (0..4).collect(),
                        _ => Vec::new(),
                    };
                    let timeout_ms = std::env::var("L6_PAD_ASSIGNMENT_TIMEOUT_MS")
                        .ok()
                        .map(|value| {
                            value.parse::<u64>().map_err(|error| {
                                format!("invalid L6_PAD_ASSIGNMENT_TIMEOUT_MS: {error}")
                            })
                        })
                        .transpose()?
                        .unwrap_or(30_000);
                    if timeout_ms == 0 || timeout_ms > 120_000 {
                        return Err(
                            "L6_PAD_ASSIGNMENT_TIMEOUT_MS must be between 1 and 120000".into()
                        );
                    }
                    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                    loop {
                        let mut pending_pads = Vec::new();
                        for pad in &pads {
                            let pending = engine.management[0]
                                .read_memory(0, 0x8044_337f + pad, 1)
                                .map_err(|error| error.to_string())?[0];
                            if pending != 0 {
                                pending_pads.push((*pad, pending));
                            }
                        }
                        if pending_pads.is_empty() {
                            break;
                        }
                        if Instant::now() >= deadline {
                            let details = pending_pads
                                .iter()
                                .map(|(pad, flag)| format!("Pad {}={flag:#04x}", pad + 1))
                                .collect::<Vec<_>>()
                                .join(", ");
                            return Err(format!(
                                "L6 Editor assignments did not complete within {timeout_ms} ms ({details}); skipping physical-key validation"
                            ));
                        }
                        pump_for(&display, &mut latest, Duration::from_millis(100))?;
                    }
                }
            } else if let Err(error) = editor_probe {
                eprintln!("L6 Editor pad-state probe unavailable: {error}");
            }
        }
    }
    if std::env::var_os("L6MAX_RECORDER_PLAYBACK_PROBE").is_some() {
        if engine.model() != l6fw::Model::L6max {
            return Err("L6MAX_RECORDER_PLAYBACK_PROBE requires L6max firmware".into());
        }
        run_l6max_recorder_playback_probe(
            &inputs,
            &engine.management[0],
            &display,
            &mut latest,
            &options.logs,
            l6max_recorder_gdb_socket
                .as_deref()
                .ok_or("L6max recorder probe requires the GDB socket")?,
        )?;
        return Ok(());
    }
    if engine.model() == l6fw::Model::L6 {
        let main = &engine.management[0];
        fs::write(
            options.logs.join("l6-main-registers.txt"),
            main.registers(0).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let heap = main
            .read_memory(0, 0x801f901c, 0x20)
            .map_err(|error| error.to_string())?;
        fs::write(
            options.logs.join("l6-heap-state.txt"),
            format!("{heap:02x?}\n"),
        )
        .map_err(|error| error.to_string())?;
        run_l6_inputs(
            &inputs,
            &engine.management[0],
            &display,
            &mut latest,
            &options.logs,
            play_gpio_gdb_socket.as_deref(),
            gpio_event_gdb_socket.as_deref(),
        )?;
        if require_audio {
            verify_audio_trace(engine.model(), &options.logs)?;
        }
        return Ok(());
    }
    if sound_pad_probe.is_none() {
        for client in &engine.management {
            client.stop().map_err(|e| e.to_string())?;
        }
        pump_for(&display, &mut latest, Duration::from_millis(50))?;
        let before = latest.ok_or("firmware never published a completed display frame")?;
        save(&before, &options.logs.join("socket-input-before.png"))?;
        let trace_before =
            fs::read_to_string(options.logs.join("engine.trace")).map_err(|e| e.to_string())?;
        let presses_before = trace_before.matches("l6 panel USART1 TX 91\n").count();
        let releases_before = trace_before.matches("l6 panel USART1 TX 90\n").count();
        let initial = inputs.panel.snapshot().applied;
        let down1 = inputs.down(1, 50).map_err(|e| e.to_string())?;
        let up1 = inputs.up(1).map_err(|e| e.to_string())?;
        let down2 = inputs.down(1, 50).map_err(|e| e.to_string())?;
        let up2 = inputs.up(1).map_err(|e| e.to_string())?;
        wait_applied(&inputs.panel, initial + 1, &display, &mut latest)?;
        pump_for(&display, &mut latest, Duration::from_millis(700))?;
        if inputs.panel.snapshot().applied != initial + 1 {
            return Err("guest release progressed while paused".into());
        }
        println!(
            "Paused guest applied Down but did not advance release during 700 ms host wait; sequences {down1}/{up1}/{down2}/{up2}"
        );
        for client in &engine.management {
            client.resume().map_err(|e| e.to_string())?;
        }
        wait_applied(&inputs.panel, initial + 4, &display, &mut latest)?;
        // Keep the guest running while firmware scans both clicks and redraws
        // the LCD. The input acknowledgement alone does not mean the screen
        // has caught up yet.
        pump_for(&display, &mut latest, screen_settle_delay())?;
        for client in &engine.management {
            client.stop().map_err(|e| e.to_string())?;
        }
        pump_for(&display, &mut latest, Duration::from_millis(50))?;
        let after = latest.ok_or("display frame missing after input")?;
        save(&after, &options.logs.join("socket-input-after.png"))?;
        let trace_after =
            fs::read_to_string(options.logs.join("engine.trace")).map_err(|e| e.to_string())?;
        let presses = trace_after.matches("l6 panel USART1 TX 91\n").count() - presses_before;
        let releases = trace_after.matches("l6 panel USART1 TX 90\n").count() - releases_before;
        if (presses, releases) != (2, 2) {
            return Err(format!(
                "expected two press/release packets, got {presses}/{releases}"
            ));
        }
        if before == after {
            return Err("published display did not change after two Up clicks".into());
        }
        println!(
            "Two rapid 50 ms clicks generated exactly two press packets and two release packets; published display changed."
        );
        let applied = inputs.panel.snapshot().applied;
        let encoder_before = trace_after.matches("l6 panel USART1 TX a1\n").count();
        inputs.encoder(2, 1).map_err(|e| e.to_string())?;
        wait_applied(&inputs.panel, applied + 1, &display, &mut latest)?;
        for client in &engine.management {
            client.resume().map_err(|e| e.to_string())?;
        }
        pump_for(&display, &mut latest, Duration::from_millis(500))?;
        let trace =
            fs::read_to_string(options.logs.join("engine.trace")).map_err(|e| e.to_string())?;
        let encoders = trace.matches("l6 panel USART1 TX a1\n").count() - encoder_before;
        if encoders != 1 {
            return Err(format!("expected one encoder report, got {encoders}"));
        }
        println!(
            "Encoder GPIO scan emitted exactly one A1 report; completed frame publications: {}",
            inputs.main.snapshot().frame_ready
        );
    }
    let trace =
        fs::read_to_string(options.logs.join("engine.trace")).map_err(|error| error.to_string())?;
    let editor_presses_before = trace.matches("l6 panel USART1 TX 91\n").count();
    let editor_releases_before = trace.matches("l6 panel USART1 TX 90\n").count();
    let complete_first_boot =
        !skip_onboarding && (delayed_sd_image.is_some() || sound_pad_probe.is_some());
    let mut editor_steps = if skip_onboarding {
        Vec::new()
    } else if complete_first_boot {
        vec![
            (2, "move-to-year"),
            (3, "year-edit"),
            (1, "year-increment"),
            (3, "year-confirm"),
        ]
    } else {
        vec![(3, "edit"), (1, "up"), (3, "confirm"), (2, "down")]
    };
    if complete_first_boot {
        for field in ["month", "day", "hour", "minute"] {
            editor_steps.extend([
                (2, "next-field"),
                (3, "field-edit"),
                (1, "field-increment"),
                (3, "field-confirm"),
            ]);
            let last = editor_steps.len() - 1;
            editor_steps[last].1 = match field {
                "month" => "month-confirm",
                "day" => "day-confirm",
                "hour" => "hour-confirm",
                _ => "minute-confirm",
            };
        }
        editor_steps.extend([(2, "select-ok"), (3, "date-save")]);
        if delayed_sd_image.is_some() || sound_pad_probe.is_some() {
            editor_steps.push((3, "battery-confirm"));
        }
    }
    for (index, (button, label)) in editor_steps.iter().enumerate() {
        let applied = inputs.panel.snapshot().applied;
        inputs
            .down(*button, 50)
            .map_err(|error| error.to_string())?;
        inputs.up(*button).map_err(|error| error.to_string())?;
        wait_applied(&inputs.panel, applied + 2, &display, &mut latest)?;
        // Allow the firmware's key scan and LCD refresh to finish before the
        // setup-step screenshot is captured.
        pump_for(&display, &mut latest, screen_settle_delay())?;
        if let Some(frame) = latest {
            save(
                &frame,
                &options.logs.join(format!("setup-{index:02}-{label}.png")),
            )?;
        }
    }
    let editor_trace =
        fs::read_to_string(options.logs.join("engine.trace")).map_err(|error| error.to_string())?;
    let editor_presses =
        editor_trace.matches("l6 panel USART1 TX 91\n").count() - editor_presses_before;
    let editor_releases =
        editor_trace.matches("l6 panel USART1 TX 90\n").count() - editor_releases_before;
    if (editor_presses, editor_releases) != (editor_steps.len(), editor_steps.len()) {
        return Err(format!(
            "expected {} setup press/release packets, got {editor_presses}/{editor_releases}",
            editor_steps.len()
        ));
    }
    if let Some(image) = &delayed_sd_image {
        engine.management[0]
            .insert_sd(image)
            .map_err(|error| error.to_string())?;
        pump_for(&display, &mut latest, sd_insert_settle_delay()?)?;
        if let Some(frame) = latest {
            save(&frame, &options.logs.join("sd-card-inserted.png"))?;
        }
        if skip_onboarding {
            println!(
                "Skipped first-boot setup using persistent state, then inserted the WAV demo card after boot."
            );
        } else {
            println!("Completed first-boot setup without a card, then inserted the WAV demo card.");
        }
    } else if skip_onboarding {
        println!("Using the existing persistent RTC and settings; first-boot setup was skipped.");
    } else {
        println!("Date editor redrew for Confirm, Up, Confirm, and Down.");
    }

    if let Some(value) = sound_pad_probe {
        if engine.model() != l6fw::Model::L6max {
            return Err("L6_SOUND_PAD_PLAYBACK_PROBE currently requires L6max firmware".into());
        }
        let pad = value
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|pad| *pad < 4)
            .ok_or("L6_SOUND_PAD_PLAYBACK_PROBE must be a pad index from 0 to 3")?;
        if std::env::var_os("L6_AUDIO_SAMPLE_TRACE").is_none() {
            return Err("L6_SOUND_PAD_PLAYBACK_PROBE needs L6_AUDIO_SAMPLE_TRACE=1".into());
        }
        // Keep each playback probe isolated to one sound-pad target; channel-
        // mode controls are not part of this GPIO input map.
        let target = l6max_sound_pad_target()?;
        if !skip_onboarding {
            // Dismiss the "Done!" notice produced when the battery type is
            // saved before opening Menu.
            let applied = inputs.panel.snapshot().applied;
            let released = inputs.panel.snapshot().released;
            inputs
                .panel
                .tap(3, 120)
                .map_err(|error| error.to_string())?;
            wait_applied(&inputs.panel, applied + 1, &display, &mut latest)?;
            wait_released(&inputs.panel, released + 1)?;
            pump_for(&display, &mut latest, screen_settle_delay())?;
            if let Some(frame) = latest {
                save(&frame, &options.logs.join("battery-done-dismissed.png"))?;
            }
        }
        // Battery-type confirmation posts a transient "Done!" message. Let it
        // time out before sending Menu, or Confirm inputs are handled as Undo.
        pump_for(&display, &mut latest, Duration::from_secs(3))?;
        if let Some(frame) = latest {
            save(&frame, &options.logs.join("post-onboarding-ready.png"))?;
        }
        let applied = inputs.main.snapshot().applied;
        let released = inputs.main.snapshot().released;
        inputs.main.tap(0, 500).map_err(|error| error.to_string())?;
        wait_applied(&inputs.main, applied + 1, &display, &mut latest)?;
        // TAP is acknowledged when the press starts. Wait for its separate
        // release acknowledgement before treating the next LCD frame as the
        // result of the key press.
        wait_released(&inputs.main, released + 1)?;
        pump_for(&display, &mut latest, screen_settle_delay())?;
        if let Some(frame) = latest {
            save(&frame, &options.logs.join("sound-pad-menu.png"))?;
        }
        if std::env::var_os("L6_SOUND_PAD_SETUP_ONLY").is_some() {
            println!(
                "Captured the post-onboarding screen and opened the main menu; stopping before menu navigation."
            );
            return Ok(());
        }
        // Menu starts on Project. Two Down inputs select Sound Pad, then
        // Confirm enters Sound Pad and Sound Assign. Move down to the
        // requested pad before opening its file picker. The picker opens with
        // the first WAV selected, so confirm directly; an extra Down selects
        // "None Assigned" and clears the pad.
        // L6max panel targets 1/2 are Up/Down and target 3 is Confirm. MAIN
        // target 0 is the Menu key.
        let menu_buttons = [2, 2, 3, 3]
            .into_iter()
            .chain(std::iter::repeat_n(2, pad as usize))
            .chain([3, 3]);
        for (index, button) in menu_buttons.enumerate() {
            let applied = inputs.panel.snapshot().applied;
            let released = inputs.panel.snapshot().released;
            inputs
                .panel
                .tap(button, 120)
                .map_err(|error| error.to_string())?;
            wait_applied(&inputs.panel, applied + 1, &display, &mut latest)?;
            wait_released(&inputs.panel, released + 1)?;
            // The input socket acknowledges delivery to QEMU, not completion
            // of the firmware's GPIO scan and LCD redraw. After the release
            // acknowledgement, leave time for that path before saving the
            // frame or sending the next menu key.
            pump_for(&display, &mut latest, screen_settle_delay())?;
            if let Some(frame) = latest {
                save(
                    &frame,
                    &options
                        .logs
                        .join(format!("sound-pad-assign-{index:02}.png")),
                )?;
            }
        }
        let applied = inputs.main.snapshot().applied;
        let released = inputs.main.snapshot().released;
        inputs.main.tap(0, 500).map_err(|error| error.to_string())?;
        wait_applied(&inputs.main, applied + 1, &display, &mut latest)?;
        wait_released(&inputs.main, released + 1)?;
        pump_for(&display, &mut latest, Duration::from_secs(1))?;
        if let Some(frame) = latest {
            save(&frame, &options.logs.join("sound-pad-ready.png"))?;
        }
        if std::env::var_os("L6_SOUND_PAD_VERIFY_UI_ASSIGNMENT").is_some() {
            let usb = engine
                .usb
                .as_ref()
                .ok_or("USB is required to query the UI assignment")?;
            println!("Querying pad assignments after the screen-driven menu flow.");
            let probe = probe_l6_editor(usb, &engine.management[0], inputs, &options.logs, false)?;
            let assignment = probe
                .assignments
                .into_iter()
                .find_map(|(assigned_pad, replies)| (assigned_pad == pad as u8).then_some(replies))
                .unwrap_or_default();
            let is_assigned = assignment.iter().any(|reply| {
                reply.get(4..7) == Some(&[0x45, 0x02, pad as u8])
                    && reply.get(7..9) != Some(&[0x7f, 0x7f])
                    && reply.get(9..11) != Some(&[0, 0])
            });
            if !is_assigned {
                return Err(format!(
                    "screen-driven assignment left L6max pad {pad} unassigned: {assignment:02x?}"
                ));
            }
            println!("Screen-driven assignment verified for L6max pad {pad}.");
            println!("Completed the screen-driven assignment verification.");
            return Ok(());
        }
        let pad_state_address = 0x806293f4 + pad * 2;
        let pad_state_before = engine.management[0]
            .read_memory(0, pad_state_address, 2)
            .map_err(|error| error.to_string())?;
        let pad_record_before = [pad_state_before[0], pad_state_before[1]];
        let indicator_rows_before = inputs.panel.snapshot().indicators;
        let tx_sample_lines_before =
            trace_count(&options.logs, "l6 audio TX samples sai=00384000")?;
        let timing_only = std::env::var_os("L6_AUDIO_TIMING_TRACE").is_some();
        let playback_guest_start = inputs.panel.snapshot().guest_ms;
        let playback_wall_start = Instant::now();
        let applied = inputs.main.snapshot().applied;
        inputs
            .main
            .down(target, 500)
            .map_err(|error| error.to_string())?;
        wait_applied(&inputs.main, applied + 1, &display, &mut latest)?;
        if std::env::var_os("L6_AUDIO_DMA_DUMP").is_some() {
            // Let the panel scan and sample stream settle before pausing the
            // guest to inspect the active TCD and its source block.
            pump_for(&display, &mut latest, Duration::from_secs(1))?;
            let qmp = &engine.management[0];
            qmp.stop().map_err(|error| error.to_string())?;
            let dump_result = (|| {
                dump_l6_audio_tx_buffer(qmp, 14, &options.logs)?;
                dump_l6_audio_tx_buffer(qmp, 12, &options.logs)
            })();
            let resume_result = qmp.resume().map_err(|error| error.to_string());
            dump_result?;
            resume_result?;
        }
        let mut indicator_samples = if timing_only {
            // Keep the guest running during the pacing window. The regular
            // indicator probe reads firmware memory through QMP, which stops
            // the guest while it samples and skews host/guest timing.
            pump_for(&display, &mut latest, Duration::from_millis(250))?;
            Vec::new()
        } else {
            sample_l6max_panel_rows(
                inputs,
                &engine.management[0],
                pad_state_address,
                &display,
                &mut latest,
                Duration::from_millis(250),
            )?
        };
        let indicator_rows_held = indicator_samples
            .last()
            .map(|(_, rows, _)| *rows)
            .unwrap_or_else(|| inputs.panel.snapshot().indicators);
        let pad_record_held = indicator_samples
            .last()
            .map(|(_, _, record)| *record)
            .unwrap_or(pad_record_before);
        let applied = inputs.main.snapshot().applied;
        inputs.main.up(target).map_err(|error| error.to_string())?;
        wait_applied(&inputs.main, applied + 1, &display, &mut latest)?;
        let held_elapsed = indicator_samples
            .last()
            .map(|(elapsed, _, _)| *elapsed)
            .unwrap_or_default()
            + 20;
        let released_samples = if timing_only {
            pump_for(&display, &mut latest, audio_post_play_settle()?)?;
            Vec::new()
        } else {
            sample_l6max_panel_rows(
                inputs,
                &engine.management[0],
                pad_state_address,
                &display,
                &mut latest,
                audio_post_play_settle()?,
            )?
        };
        indicator_samples.extend(
            released_samples
                .into_iter()
                .map(|(elapsed, rows, record)| (held_elapsed + elapsed, rows, record)),
        );
        if std::env::var_os("L6_AUDIO_TIMING_TRACE").is_some() {
            let wall_seconds = playback_wall_start.elapsed().as_secs_f64();
            let guest_ms = inputs
                .panel
                .snapshot()
                .guest_ms
                .wrapping_sub(playback_guest_start);
            println!(
                "L6max active playback timing (guest left running): wall={wall_seconds:.3}s guest={:.3}s guest/wall={:.3}",
                guest_ms as f64 / 1000.0,
                guest_ms as f64 / 1000.0 / wall_seconds
            );
        }
        let (changed_led_indices, changed_rows, sampled_pad_records) = if timing_only {
            (Vec::new(), indicator_rows_before, Vec::new())
        } else {
            summarize_l6max_panel_rows(
                &options.logs,
                target,
                indicator_rows_before,
                indicator_rows_held,
                pad_record_before,
                &indicator_samples,
            )?
        };
        let pad_state_after = engine.management[0]
            .read_memory(0, pad_state_address, 2)
            .map_err(|error| error.to_string())?;
        if let Some(frame) = latest {
            save(&frame, &options.logs.join("sound-pad-playback-final.png"))?;
        }
        println!(
            "L6max pad {pad} candidate target {target} runtime record before={pad_state_before:02x?} held={pad_record_held:02x?} after={pad_state_after:02x?} sampled={sampled_pad_records:02x?}; panel rows before={indicator_rows_before:06x?}, held={indicator_rows_held:06x?}, changed={changed_rows:06x?}; logical LEDs changed={changed_led_indices:?}"
        );
        let trace = fs::read_to_string(options.logs.join("engine.trace"))
            .map_err(|error| error.to_string())?;
        let nonzero_tx = trace
            .lines()
            .filter(|line| line.starts_with("l6 audio TX samples sai=00384000"))
            .skip(tx_sample_lines_before)
            .any(|line| {
                line.split_whitespace()
                    .find_map(|field| field.strip_prefix("nonzero="))
                    .and_then(|count| count.parse::<u64>().ok())
                    .is_some_and(|count| count > 0)
            });
        if !nonzero_tx {
            return Err(format!(
                "L6max sound pad {pad} candidate target {target} did not produce nonzero SAI1 TX samples; verify the candidate mapping and WAV assignment"
            ));
        }
        if !timing_only {
            let expected_pad_led = 27 + pad as usize;
            if !changed_led_indices.contains(&expected_pad_led) {
                return Err(format!(
                    "L6max sound pad {pad} candidate target {target} produced SAI1 audio but did not change expected Pad {} LED {} (changed {changed_led_indices:?})",
                    pad + 1,
                    expected_pad_led
                ));
            }
        }
        println!("L6max sound pad {pad} produced nonzero SAI1 TX samples.");
    }
    for target in [0, 4, 5] {
        let applied = inputs.main.snapshot().applied;
        inputs
            .main
            .tap(target, 120)
            .map_err(|error| error.to_string())?;
        wait_applied(&inputs.main, applied + 1, &display, &mut latest)?;
    }
    println!("L6max MAIN GPIO input smoke accepted Menu, Play, and Record targets.");
    if require_audio {
        verify_audio_trace(engine.model(), &options.logs)?;
    }
    Ok(())
}
