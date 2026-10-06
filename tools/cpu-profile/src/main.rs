//! Compare idle CPU consumption and guest clock progression after firmware boot.
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
    input::EngineInput,
};
use std::{
    collections::BTreeMap,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn cpu_seconds(pid: u32) -> Result<f64, String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "time="])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("ps could not read QEMU CPU time".into());
    }
    let time = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    time.trim().split(':').try_fold(0.0, |total, part| {
        part.parse::<f64>()
            .map(|part| total * 60.0 + part)
            .map_err(|e| e.to_string())
    })
}
fn pump(display: &PublishedDisplay, duration: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < duration {
        display.sample().map_err(|e| e.to_string())?;
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
fn guest_stamp(input: &EngineInput, display: &PublishedDisplay) -> Result<u32, String> {
    // A single encoder command supplies a guest-time acknowledgment without
    // pressing or holding a button. The benchmark consumes display updates.
    let applied = input.snapshot().applied;
    input.encoder(2, 1).map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        pump(display, Duration::from_millis(5))?;
        let status = input.snapshot();
        if status.applied > applied {
            return status
                .last_ack
                .map(|ack| ack.duration_ms)
                .ok_or("missing acknowledgment".into());
        }
        if start.elapsed() > Duration::from_secs(3) {
            return Err("guest clock probe timed out".into());
        }
    }
}
fn run() -> Result<(), String> {
    let options = Options::parse()?;
    let engine = Engine::start(&options)?;
    let inputs = engine.inputs.as_ref().ok_or("QEMU is required")?;
    let display = PublishedDisplay::new(
        engine.display_memory.as_ref().unwrap().map(),
        inputs.main.clone(),
    )
    .map_err(|e| e.to_string())?;
    println!("Booting for 45 seconds; timing mode {:?}", options.timing);
    pump(&display, Duration::from_secs(45))?;
    let guest_before = guest_stamp(&inputs.panel, &display)?;
    let cpu_before = cpu_seconds(engine.process_id().unwrap())?;
    let start = Instant::now();
    let mut pcs = [BTreeMap::new(), BTreeMap::new()];
    while start.elapsed() < Duration::from_secs(20) {
        for (cpu, counts) in pcs.iter_mut().enumerate() {
            let regs = engine.management[0]
                .registers(cpu as i64)
                .map_err(|e| e.to_string())?;
            let pc = regs
                .split_whitespace()
                .find_map(|word| word.strip_prefix("R15="))
                .ok_or("PC missing")?;
            *counts.entry(pc.to_owned()).or_insert(0u32) += 1;
        }
        pump(&display, Duration::from_millis(157))?;
    }
    let cpu_after = cpu_seconds(engine.process_id().unwrap())?;
    let guest_after = guest_stamp(&inputs.panel, &display)?;
    let wall = start.elapsed().as_secs_f64();
    let guest = guest_after.wrapping_sub(guest_before) as f64 / 1000.0;
    println!(
        "QEMU CPU {:.1}% (one core = 100%); wall {wall:.3}s; guest {guest:.3}s; ratio {:.3}",
        100.0 * (cpu_after - cpu_before) / wall,
        guest / wall
    );
    for (cpu, counts) in pcs.iter().enumerate() {
        println!("CPU {cpu} sampled PCs: {counts:?}");
    }
    if !(0.95..=1.05).contains(&(guest / wall)) {
        return Err("guest clock differs from wall time by more than 5%".into());
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("cpu-profile: {error}");
        std::process::exit(1);
    }
}
