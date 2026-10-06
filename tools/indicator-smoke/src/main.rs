//! Live firmware check of the GPIO indicator surface and encoder feedback.
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
    indicators::rings,
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
    let reports = || -> Result<usize, std::io::Error> {
        Ok(std::fs::read_to_string(options.logs.join("engine.trace"))?
            .matches("l6 panel USART1 TX a1\n")
            .count())
    };
    let wait_reports = |expected| -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(60);
        while reports()? < expected {
            pump(0.25)?;
            if Instant::now() > deadline {
                return Err("encoder firmware report timeout".into());
            }
        }
        pump(1.0)?;
        assert_eq!(reports()?, expected, "duplicate encoder reports");
        Ok(())
    };
    let initial_reports = reports()?;
    let original = rings(inputs.panel.snapshot().indicators);
    for channel in 0..8 {
        let before = rings(inputs.panel.snapshot().indicators);
        inputs.encoder(channel, -12)?;
        wait_reports(initial_reports + (channel as usize + 1) * 12)?;
        println!(
            "encoder {channel}: {before:06x?} -> {:06x?}",
            rings(inputs.panel.snapshot().indicators)
        );
        let after = rings(inputs.panel.snapshot().indicators);
        assert_ne!(
            before[channel as usize], after[channel as usize],
            "encoder {channel} did not update its ring"
        );
        for other in 0..8 {
            if other != channel as usize {
                assert_eq!(
                    before[other], after[other],
                    "encoder {channel} changed ring {other}"
                );
            }
        }
    }
    save(
        latest.borrow().as_ref().unwrap(),
        &options.logs.join("after.png"),
    )?;
    let before_reverse = rings(inputs.panel.snapshot().indicators);
    inputs.encoder(7, 12)?;
    wait_reports(initial_reports + 108)?;
    let reversed = rings(inputs.panel.snapshot().indicators);
    assert_eq!(
        reversed[7], original[7],
        "reverse turn did not restore ring"
    );
    assert_eq!(&reversed[..7], &before_reverse[..7]);
    assert_eq!(inputs.panel.snapshot().rejected, 0);
    println!(
        "All eight rings changed independently; reverse turn restored ring 8; 108 exact firmware encoder reports."
    );
    Ok(())
}
