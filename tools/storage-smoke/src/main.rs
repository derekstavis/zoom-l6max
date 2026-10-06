//! SD card detection, removable-media IRQs, and filesystem startup through firmware.
use l6max_host::{engine::Options, headless::Headless};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn save(raw: &[u8; 1024], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    image::GrayImage::from_fn(128, 64, |x, y| {
        image::Luma([
            if raw[((63 - y as usize) / 8) * 128 + x as usize] & (1 << ((63 - y as usize) % 8)) != 0
            {
                255
            } else {
                0
            },
        ])
    })
    .save(path)?;
    Ok(())
}
fn until(
    h: &Headless,
    frame: &mut Option<[u8; 1024]>,
    ms: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while h.guest_ms() < ms {
        if let Some(f) = h.display.sample()? {
            *frame = Some(f);
        }
        if Instant::now() > deadline {
            return Err("SD guest deadline".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(f) = h.display.sample()? {
        *frame = Some(f);
    }
    Ok(())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::parse()?;
    options.trace_sd = true;
    options.host_midi = false;
    let h = Headless::start(&options)?;
    let inputs = h.engine.inputs.as_ref().unwrap();
    let mut frame = None;
    until(&h, &mut frame, 40000)?;
    inputs.tap(3, 50)?;
    until(&h, &mut frame, 42000)?;
    inputs.tap(3, 50)?;
    until(&h, &mut frame, 47000)?;
    let home = frame.ok_or("no display frame")?;
    save(&home, &options.logs.join("storage-home.png"))?;
    if let Some(image) = &options.sd_image {
        h.engine.management[0].eject_sd()?;
        until(&h, &mut frame, h.guest_ms() + 3000)?;
        let removed = frame.unwrap();
        save(&removed, &options.logs.join("storage-ejected.png"))?;
        if removed == home {
            return Err("SD removal did not alter firmware display".into());
        }
        h.engine.management[0].insert_sd(image)?;
        until(&h, &mut frame, h.guest_ms() + 5000)?;
        let inserted = frame.unwrap();
        save(&inserted, &options.logs.join("storage-reinserted.png"))?;
        if inserted != home {
            return Err("SD reinsertion did not restore recorder display".into());
        }
    }
    let trace = fs::read_to_string(options.logs.join("engine.trace"))?;
    let commands = trace
        .lines()
        .filter(|s| s.starts_with("sdhci_send_command"))
        .collect::<Vec<_>>();
    if options.sd_image.is_some() {
        for cmd in [
            "CMD00", "CMD08", "CMD02", "CMD03", "CMD09", "CMD07", "CMD17",
        ] {
            if !commands.iter().any(|s| s.contains(cmd)) {
                return Err(format!("missing SD command {cmd}").into());
            }
        }
        if !trace.contains("l6 SD card detect=0") || !trace.contains("l6 SD card detect=1") {
            return Err("missing SD card-detect edges".into());
        }
    } else if !commands.is_empty() {
        return Err("card commands issued without a card".into());
    }
    println!(
        "SD present={}: {} firmware commands; detection and display checks passed",
        options.sd_image.is_some(),
        commands.len()
    );
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("storage-smoke: {e}");
        std::process::exit(1);
    }
}
