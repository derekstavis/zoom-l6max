//! Capture stock GUI paths through GPIO button input; no host drawing or writes.
use l6max_host::{engine::Options, headless::Headless};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
fn pump(guest: &Headless, end: u32, frame: &mut Option<[u8; 1024]>) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while guest.guest_ms() < end {
        if let Some(raw) = guest.display.sample()? {
            *frame = Some(raw);
        }
        if Instant::now() > deadline {
            return Err(std::io::Error::other("guest clock stalled"));
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(raw) = guest.display.sample()? {
        *frame = Some(raw);
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut keys = String::new();
    let mut rest = Vec::new();
    while let Some(a) = args.next() {
        if a == "--keys" {
            keys = args
                .next()
                .ok_or("--keys needs comma-separated GPIO IDs")?
                .into_string()
                .map_err(|_| "UTF-8 keys required")?;
        } else {
            rest.push(a);
        }
    }
    let options = Options::parse_args(rest)?;
    if options.state_dir.is_some() {
        return Err("requires --volatile to preserve device state".into());
    }
    let guest = Headless::start(&options)?;
    let mut frame = None;
    pump(&guest, 40000, &mut frame)?;
    for settle in [2000, 5000] {
        guest.engine.inputs.as_ref().unwrap().tap(3, 50)?;
        pump(&guest, guest.guest_ms() + settle, &mut frame)?;
    }
    let mut rows = Vec::new();
    for (i, key) in std::iter::once(None)
        .chain(
            keys.split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.parse::<u32>().map(Some))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .enumerate()
    {
        if let Some(key) = key {
            guest.engine.inputs.as_ref().unwrap().tap(key, 50)?;
        }
        pump(&guest, guest.guest_ms() + 1000, &mut frame)?;
        if let Some(raw) = frame {
            image::GrayImage::from_fn(128, 64, |x, y| {
                image::Luma([
                    if raw[((63 - y as usize) / 8) * 128 + x as usize]
                        & (1 << ((63 - y as usize) % 8))
                        != 0
                    {
                        255
                    } else {
                        0
                    },
                ])
            })
            .save(options.logs.join(format!("{i:02}.png")))?;
        }
        let window = u32::from_le_bytes(
            guest.engine.management[0]
                .read_memory(0, 0x80202610, 4)?
                .try_into()
                .unwrap(),
        );
        println!("{i:02} key {key:?}: window {window:08x}");
        rows.push(serde_json::json!({"step":i,"key":key,"guest_ms":guest.guest_ms(),"window":format!("{window:08x}")}));
    }
    fs::write(
        options.logs.join("gui-probe.json"),
        serde_json::to_string_pretty(&rows)?,
    )?;
    Ok(())
}
