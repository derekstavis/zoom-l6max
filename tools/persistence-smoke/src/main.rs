//! Restart validation through GPIO input and firmware-owned device storage.
use l6max_host::{engine::Options, headless::Headless};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn wait(
    guest: &Headless,
    target: u32,
    directory: &Path,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(150);
    let mut last = None;
    while guest.guest_ms() < target {
        if let Some(frame) = guest.display.sample()? {
            last = Some(frame);
        }
        if Instant::now() > deadline {
            return Err("guest deadline".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(frame) = guest.display.sample()? {
        last = Some(frame);
    }
    if let Some(raw) = last {
        image::GrayImage::from_fn(128, 64, |x, y| {
            image::Luma([
                if raw[((63 - y as usize) / 8) * 128 + x as usize] & (1 << ((63 - y as usize) % 8))
                    != 0
                {
                    255
                } else {
                    0
                },
            ])
        })
        .save(directory.join(format!("{label}.png")))?;
    }
    Ok(())
}
fn tap(
    guest: &Headless,
    button: u32,
    logs: &Path,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    guest.engine.inputs.as_ref().unwrap().tap(button, 50)?;
    wait(guest, guest.guest_ms() + 1050, logs, label)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut resume = false;
    let arguments = std::env::args_os()
        .skip(1)
        .filter(|arg| {
            if arg == "--resume" {
                resume = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    let mut options = Options::parse_args(arguments)?;
    let state = options.state_dir.clone().ok_or("requires --state-dir")?;
    if !resume && state.join("panel-rtc.bin").exists() {
        return Err("use a fresh isolated --state-dir".into());
    }
    let logs = options.logs.clone();
    options.logs = logs.join("first");
    if !resume {
        let guest = Headless::start(&options)?;
        wait(&guest, 1500, &options.logs, "loading")?;
        wait(&guest, 40000, &options.logs, "date-initial")?;
        for (button, label) in [
            (2, "year-select"),
            (3, "year-edit"),
            (1, "year-increment"),
            (3, "year-confirm"),
        ] {
            tap(&guest, button, &options.logs, label)?;
        }
        for n in 0..5 {
            tap(&guest, 2, &options.logs, &format!("next-{n}"))?;
            if n < 4 {
                tap(&guest, 3, &options.logs, &format!("field-{n}-edit"))?;
                tap(&guest, 1, &options.logs, &format!("field-{n}-increment"))?;
                tap(&guest, 3, &options.logs, &format!("field-{n}-confirm"))?;
            }
        }
        tap(&guest, 3, &options.logs, "date-save")?;
        tap(&guest, 3, &options.logs, "battery-confirm")?;
        wait(&guest, guest.guest_ms() + 3000, &options.logs, "home")?;
    }
    let saved = fs::read(state.join("panel-rtc.bin"))?;
    let panel = fs::read(state.join("panel-flash.bin"))?;
    if &panel[0x3ffc..0x4000] != b"0100" {
        return Err("panel firmware version stamp did not persist".into());
    }
    let nor = fs::read(state.join("main-nor.bin"))?;
    let bank = [0x1f9000usize, 0x1fc000]
        .into_iter()
        .find(|&base| {
            (1..=3).contains(&u32::from_le_bytes(nor[base..base + 4].try_into().unwrap()))
        })
        .ok_or("firmware did not commit a settings bank")?;
    if &nor[0x1f7ffc..0x1f8000] != b"0110" || &nor[0x1f6ffc..0x1f7000] != b"0100" {
        return Err("installed version records do not match the package".into());
    }
    if saved[6] != 0x26 {
        return Err(format!(
            "date editor did not save 2026: RTC bytes {:02x?}",
            &saved[..8]
        )
        .into());
    }
    let calendar = u32::from_le_bytes(saved[4..8].try_into().unwrap());
    let time = u32::from_le_bytes(saved[..4].try_into().unwrap());
    if !resume && (calendar & 0x00ff1f3f != 0x00260202 || time & 0x003f7f00 != 0x00010100) {
        return Err("date editor did not save all five edited fields".into());
    }
    options.logs = logs.join("restart");
    {
        let guest = Headless::start(&options)?;
        wait(&guest, 40000, &options.logs, "boot")?;
        let restored = fs::read(state.join("panel-rtc.bin"))?;
        if restored[4..8] != saved[4..8] {
            return Err("calendar overwritten during restart".into());
        }
        if u32::from_le_bytes(restored[..4].try_into().unwrap()) & 0x003f7f00 != time & 0x003f7f00 {
            return Err("hour or minute overwritten during restart".into());
        }
        let trace = fs::read_to_string(options.logs.join("engine.trace"))?;
        if trace.contains("l6 panel ROM write") {
            return Err("panel unexpectedly reprogrammed on restart".into());
        }
        if !trace.contains(&format!("addr={:08x} ipcr1=000017e0", bank + 4)) {
            return Err("firmware did not reload the committed settings bank".into());
        }
        // Navigate through physical GPIO inputs, without using the system pointer.
        let request = trace
            .lines()
            .find_map(|line| {
                line.strip_prefix("l6 main RTC request at ")
                    .and_then(|rest| rest.split_whitespace().next()?.parse::<u32>().ok())
            })
            .ok_or("RTC request trace missing")?;
        let response = trace
            .lines()
            .find_map(|line| {
                line.strip_prefix("l6 panel RTC response start at ")
                    .and_then(|rest| rest.split_whitespace().next()?.parse::<u32>().ok())
            })
            .ok_or("RTC response trace missing")?;
        if response < request || response - request >= 2000 {
            return Err("panel RTC reply missed the firmware's startup timeout".into());
        }
        guest.engine.inputs.as_ref().unwrap().tap(0, 500)?;
        wait(&guest, guest.guest_ms() + 1500, &options.logs, "menu")?;
        for n in 0..7 {
            tap(&guest, 2, &options.logs, &format!("menu-{n}"))?;
        }
        tap(&guest, 3, &options.logs, "system")?;
        for n in 0..5 {
            tap(&guest, 2, &options.logs, &format!("system-{n}"))?;
        }
        tap(&guest, 3, &options.logs, "firmware-information")?;
    }
    println!(
        "Firmware saved and restored RTC calendar and NOR settings; panel flash survived without reprogramming."
    );
    Ok(())
}
