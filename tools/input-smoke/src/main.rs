//! Live integration check: QMP lifecycle, paused guest deadlines, discrete
//! button packets, visible selection changes, and encoder input.
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
    input::EngineInput,
};

use std::{
    fs::{self},
    path::Path,
    thread,
    time::{Duration, Instant},
};

type Frame = [u8; 1024];
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
fn run() -> Result<(), String> {
    let options = Options::parse()?;
    let engine = Engine::start(&options)?;
    let inputs = engine.inputs.as_ref().ok_or("QEMU is required")?;
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
    pump_for(&display, &mut latest, Duration::from_secs(42))?;
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
    pump_for(&display, &mut latest, Duration::from_millis(700))?;
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
    let trace = fs::read_to_string(options.logs.join("engine.trace")).map_err(|e| e.to_string())?;
    let encoders = trace.matches("l6 panel USART1 TX a1\n").count() - encoder_before;
    if encoders != 1 {
        return Err(format!("expected one encoder report, got {encoders}"));
    }
    println!(
        "Encoder GPIO scan emitted exactly one A1 report; completed frame publications: {}",
        inputs.main.snapshot().frame_ready
    );
    let editor_presses_before = trace.matches("l6 panel USART1 TX 91\n").count();
    let editor_releases_before = trace.matches("l6 panel USART1 TX 90\n").count();
    // Exercise the editor through actual firmware packets: enter the selected
    // field, change it with Up, confirm it, then navigate with Down.
    for (button, label) in [(3, "edit"), (1, "up"), (3, "confirm"), (2, "down")] {
        let previous = latest.ok_or("display frame missing before date edit")?;
        let applied = inputs.panel.snapshot().applied;
        inputs.down(button, 50).map_err(|error| error.to_string())?;
        inputs.up(button).map_err(|error| error.to_string())?;
        wait_applied(&inputs.panel, applied + 2, &display, &mut latest)?;
        pump_for(&display, &mut latest, Duration::from_millis(200))?;
        let current = latest.ok_or("display frame missing during date edit")?;
        save(&current, &options.logs.join(format!("date-{label}.png")))?;
        if previous == current {
            return Err(format!("date editor did not redraw after {label}"));
        }
    }
    let editor_trace =
        fs::read_to_string(options.logs.join("engine.trace")).map_err(|error| error.to_string())?;
    let editor_presses =
        editor_trace.matches("l6 panel USART1 TX 91\n").count() - editor_presses_before;
    let editor_releases =
        editor_trace.matches("l6 panel USART1 TX 90\n").count() - editor_releases_before;
    if (editor_presses, editor_releases) != (4, 4) {
        return Err(format!(
            "expected four editor press/release packets, got {editor_presses}/{editor_releases}"
        ));
    }
    println!("Date editor redrew for Confirm, Up, Confirm, and Down through published frames.");
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("input-smoke:{error}");
        std::process::exit(1);
    }
}
