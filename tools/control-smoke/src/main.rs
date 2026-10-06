//! Live GPIO checks of controls and multiplexed LEDs.
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
};
use std::{
    cell::RefCell,
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn save(raw: &[u8; 1024], path: &Path) -> Result<(), image::ImageError> {
    image::GrayImage::from_fn(128, 64, |x, y| {
        image::Luma([
            if raw[((63 - y as usize) / 8) * 128 + x as usize] >> ((63 - y as usize) % 8) & 1 != 0 {
                255
            } else {
                0
            },
        ])
    })
    .save(path)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse()?;
    let engine = Engine::start(&options)?;
    let inputs = engine.inputs.as_ref().ok_or("QEMU required")?;
    let display = PublishedDisplay::new(
        engine.display_memory.as_ref().unwrap().map(),
        inputs.main.clone(),
    )?;
    let latest = RefCell::new(None);
    let pump = |seconds: f64| -> Result<(), Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < end {
            if let Some(frame) = display.sample()? {
                *latest.borrow_mut() = Some(frame);
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    };
    println!("Waiting for completed display frames...");
    let deadline = Instant::now() + Duration::from_secs(180);
    while inputs.main.snapshot().frame_guest_ms < 12_000 {
        pump(0.1)?;
        if Instant::now() > deadline {
            return Err("display boot timeout".into());
        }
    }
    pump(3.0)?;
    println!("boot {:?}", inputs.panel.snapshot());
    save(
        latest.borrow().as_ref().unwrap(),
        &options.logs.join("boot.png"),
    )?;
    for i in 0..2 {
        inputs.down(3, 50)?;
        inputs.up(3)?;
        pump(2.0)?;
        save(
            latest.borrow().as_ref().unwrap(),
            &options.logs.join(format!("setup-{i}.png")),
        )?;
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    while inputs.main.snapshot().frame_guest_ms < 18_000 {
        pump(0.1)?;
        if Instant::now() > deadline {
            return Err("setup timeout".into());
        }
    }
    pump(8.0)?;
    let leds = || l6max_host::indicators::leds(inputs.panel.snapshot().indicators);
    println!(
        "initial LEDs: {:?}",
        leds()
            .iter()
            .enumerate()
            .filter_map(|(i, on)| on.then_some(i))
            .collect::<Vec<_>>()
    );
    let mut tested = std::collections::HashSet::new();
    for control in l6max_host::controls::CONTROLS
        .iter()
        .filter(|c| c.name.starts_with("ch") && c.name.ends_with("mute"))
        .chain(
            l6max_host::controls::CONTROLS
                .iter()
                .filter(|c| c.name.starts_with("mode-")),
        )
        .chain(
            l6max_host::controls::CONTROLS
                .iter()
                .filter(|c| matches!(c.name, "efx-select" | "compressor" | "tap-button")),
        )
    {
        tested.insert(control.id);
        let before = leds();
        inputs.down(control.id, 50)?;
        inputs.up(control.id)?;
        pump(2.0)?;
        let after = leds();
        let delta = (0..72)
            .filter(|&i| before[i] != after[i])
            .map(|i| (i, after[i]))
            .collect::<Vec<_>>();
        println!("{} id={} LEDs {:?}", control.name, control.id, delta);
        if control.name.ends_with("mute") {
            assert!(
                after[(control.name.as_bytes()[2] - b'1') as usize],
                "mute LED absent"
            );
        }
    }
    let packets = |id: u32, pressed: bool| -> Option<String> {
        let (row, col) = match id {
            1 => (5, 5),
            2 => (4, 5),
            3 => (7, 5),
            6 => (6, 5),
            7..=46 => ((id - 7) / 5, (id - 7) % 5 + 1),
            47 => (0, 0),
            _ => return None,
        };
        Some(format!(
            "l6 panel USART1 TX {:02x}\nl6 panel USART1 TX {col:02x}\nl6 panel USART1 TX {row:02x}\n",
            if pressed != (id == 47) { 0x91 } else { 0x90 }
        ))
    };
    for control in l6max_host::controls::CONTROLS
        .iter()
        .filter(|c| !tested.contains(&c.id))
    {
        let before = std::fs::read_to_string(options.logs.join("engine.trace"))?;
        inputs.down(control.id, if control.switch { 0 } else { 50 })?;
        if control.switch {
            pump(0.5)?;
        }
        inputs.up(control.id)?;
        pump(2.0)?;
        if let Some(on) = packets(control.id, true) {
            let off = packets(control.id, false).unwrap();
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let trace = std::fs::read_to_string(options.logs.join("engine.trace"))?;
                if trace.matches(&off).count() > before.matches(&off).count()
                    && trace.matches(&on).count() > before.matches(&on).count()
                {
                    break;
                }
                if Instant::now() > deadline {
                    return Err(format!("{} firmware edge timeout", control.name).into());
                }
                pump(0.1)?;
            }
            let after = std::fs::read_to_string(options.logs.join("engine.trace"))?;
            let off = packets(control.id, false).unwrap();
            assert_eq!(
                after.matches(&on).count() - before.matches(&on).count(),
                1,
                "{} press",
                control.name
            );
            assert_eq!(
                after.matches(&off).count() - before.matches(&off).count(),
                1,
                "{} release",
                control.name
            );
        }
        println!(
            "{}: GPIO press/release, main output banks {:08x?}",
            control.name,
            inputs.main.snapshot().main_gpio
        );
    }
    std::fs::write(
        options.logs.join("indicators.json"),
        serde_json::to_vec(
            &serde_json::json!({"panel": inputs.panel.snapshot().indicators, "main": inputs.main.snapshot().main_gpio}),
        )?,
    )?;
    assert_eq!(inputs.panel.snapshot().rejected, 0);
    assert_eq!(inputs.main.snapshot().rejected, 0);
    Ok(())
}
