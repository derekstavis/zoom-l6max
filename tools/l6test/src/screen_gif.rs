//! Capture the emulated L6max LCD as an animated, looping GIF.
use gif::{Encoder, Frame, Repeat};
use l6fw::Model;
use l6max_host::{engine::Options, headless::Headless};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

const WIDTH: u16 = 512;
const HEIGHT: u16 = 256;
const SCALE: usize = 4;

#[derive(Clone, Copy)]
enum EventKind {
    Tap { target: u32, hold_ms: u32 },
    Encoder { channel: u32, steps: i32 },
}

#[derive(Clone, Copy)]
struct Event {
    at_ms: u32,
    kind: EventKind,
}

fn parse_values(value: &str, flag: &str) -> Result<Vec<u32>, String> {
    value
        .split(':')
        .map(|part| {
            part.parse::<u32>()
                .map_err(|_| format!("{flag} contains an invalid number: {value}"))
        })
        .collect()
}

fn parse_tap(value: &str) -> Result<Event, String> {
    let fields = parse_values(value, "--tap")?;
    if fields.len() != 3 || fields[0] >= 54 || fields[2] == 0 || fields[2] > 60_000 {
        return Err("--tap expects TARGET:AT_MS:HOLD_MS (target 0..53, hold 1..60000)".into());
    }
    Ok(Event {
        at_ms: fields[1],
        kind: EventKind::Tap {
            target: fields[0],
            hold_ms: fields[2],
        },
    })
}

fn parse_encoder(value: &str) -> Result<Event, String> {
    let fields = value.split(':').collect::<Vec<_>>();
    if fields.len() != 3 {
        return Err("--encoder expects CHANNEL:AT_MS:STEPS".into());
    }
    let channel = fields[0]
        .parse::<u32>()
        .map_err(|_| "--encoder channel must be 1..8")?;
    let at_ms = fields[1]
        .parse::<u32>()
        .map_err(|_| "--encoder time must be a nonnegative integer")?;
    let steps = fields[2]
        .parse::<i32>()
        .map_err(|_| "--encoder steps must be a signed integer")?;
    if !(1..=8).contains(&channel) || steps == 0 || steps.unsigned_abs() > 1000 {
        return Err("--encoder expects channel 1..8 and steps -1000..1000 excluding zero".into());
    }
    Ok(Event {
        at_ms,
        kind: EventKind::Encoder { channel, steps },
    })
}

fn indexed_screen(raw: &[u8; 1024]) -> Vec<u8> {
    let mut pixels = vec![0; usize::from(WIDTH) * usize::from(HEIGHT)];
    for y in 0..64usize {
        for x in 0..128usize {
            let source_y = 63 - y;
            let bit = (raw[(source_y / 8) * 128 + x] >> (source_y % 8)) & 1;
            if bit == 0 {
                continue;
            }
            for dy in 0..SCALE {
                let offset = ((y * SCALE + dy) * usize::from(WIDTH)) + x * SCALE;
                pixels[offset..offset + SCALE].fill(1);
            }
        }
    }
    pixels
}

pub fn run(arguments: Vec<OsString>) -> Result<(), String> {
    let mut output = None;
    let mut duration_ms = 12_000u32;
    let mut fps = 5u32;
    let mut events = Vec::new();
    let mut engine_args = Vec::new();
    let mut isolated_state = false;
    let mut args = arguments.into_iter();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--out") => {
                output = Some(PathBuf::from(args.next().ok_or("--out needs a path")?));
            }
            Some("--duration-ms") => {
                duration_ms = args
                    .next()
                    .ok_or("--duration-ms needs an integer")?
                    .to_str()
                    .ok_or("duration must be UTF-8")?
                    .parse()
                    .map_err(|_| "invalid --duration-ms")?;
            }
            Some("--fps") => {
                fps = args
                    .next()
                    .ok_or("--fps needs an integer")?
                    .to_str()
                    .ok_or("fps must be UTF-8")?
                    .parse()
                    .map_err(|_| "invalid --fps")?;
            }
            Some("--tap") => {
                let value = args.next().ok_or("--tap needs TARGET:AT_MS:HOLD_MS")?;
                events.push(parse_tap(value.to_str().ok_or("--tap must be UTF-8")?)?);
            }
            Some("--encoder") => {
                let value = args.next().ok_or("--encoder needs CHANNEL:AT_MS:STEPS")?;
                events.push(parse_encoder(
                    value.to_str().ok_or("--encoder must be UTF-8")?,
                )?);
            }
            Some("--help" | "-h") => {
                println!(
                    "Usage: l6test capture screen --out FILE.gif [--duration-ms N] [--fps N] [--tap TARGET:AT_MS:HOLD_MS]... [--encoder CHANNEL:AT_MS:STEPS]... --volatile [engine options]\n\nRecords the L6max LCD as an animated GIF. Scripted events are relative to the start of capture."
                );
                return Ok(());
            }
            _ => {
                isolated_state |= argument == "--volatile" || argument == "--state-dir";
                engine_args.push(argument);
            }
        }
    }
    let output = output.ok_or("--out FILE.gif is required")?;
    if output.extension().and_then(|ext| ext.to_str()) != Some("gif") {
        return Err("--out path must end in .gif".into());
    }
    if !(1000..=60_000).contains(&duration_ms) || !(1..=15).contains(&fps) {
        return Err("duration must be 1000..60000 ms and fps must be 1..15".into());
    }
    if !isolated_state {
        return Err("specify --volatile or an isolated --state-dir".into());
    }
    if events.iter().any(|event| event.at_ms >= duration_ms) {
        return Err("scripted event time must be less than --duration-ms".into());
    }
    events.sort_by_key(|event| event.at_ms);

    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if output.exists() {
        return Err(format!("refusing to overwrite {}", output.display()));
    }
    let options = Options::parse_args(engine_args).map_err(|error| error.to_string())?;
    let guest = Headless::start(&options)?;
    if guest.engine.model() != Model::L6max {
        return Err("screen GIF capture requires L6max firmware".into());
    }
    guest.recorder().map_err(|error| error.to_string())?;

    let temp = output.with_extension(format!("gif.tmp-{}", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    let result: Result<u32, String> = (|| {
        let mut encoder = Encoder::new(file, WIDTH, HEIGHT, &[0, 0, 0, 255, 255, 255])
            .map_err(|error| error.to_string())?;
        encoder
            .set_repeat(Repeat::Infinite)
            .map_err(|error| error.to_string())?;
        let interval_ms = (1000 / fps).max(1);
        let start_guest_ms = guest.guest_ms();
        let start_wall = Instant::now();
        let mut next_frame_ms = 0u32;
        let mut event_index = 0usize;
        let mut frame_count = 0u32;
        loop {
            guest.sample_display().map_err(|error| error.to_string())?;
            let elapsed_ms = guest.guest_ms().wrapping_sub(start_guest_ms);
            while event_index < events.len() && events[event_index].at_ms <= elapsed_ms {
                match events[event_index].kind {
                    EventKind::Tap { target, hold_ms } => guest
                        .engine
                        .inputs
                        .as_ref()
                        .ok_or("QEMU input unavailable")?
                        .tap(target, hold_ms)
                        .map_err(|error| error.to_string())?,
                    EventKind::Encoder { channel, steps } => guest
                        .engine
                        .inputs
                        .as_ref()
                        .ok_or("QEMU input unavailable")?
                        .encoder(channel - 1, steps)
                        .map_err(|error| error.to_string())?,
                };
                event_index += 1;
            }
            if elapsed_ms >= next_frame_ms {
                if let Some(raw) = guest.sample_display().map_err(|error| error.to_string())? {
                    let mut frame =
                        Frame::from_indexed_pixels(WIDTH, HEIGHT, indexed_screen(&raw), None);
                    frame.delay = (100 / fps).max(1) as u16;
                    encoder
                        .write_frame(&frame)
                        .map_err(|error| error.to_string())?;
                    frame_count += 1;
                }
                next_frame_ms = next_frame_ms.saturating_add(interval_ms);
            }
            if elapsed_ms >= duration_ms {
                break;
            }
            if start_wall.elapsed() > Duration::from_secs(120) {
                return Err("guest clock stalled during screen capture".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        if frame_count == 0 {
            return Err("no L6max display frames were captured".into());
        }
        encoder
            .into_inner()
            .map_err(|error| error.to_string())?
            .sync_all()
            .map_err(|error| error.to_string())?;
        fs::hard_link(&temp, &output).map_err(|error| error.to_string())?;
        Ok(frame_count)
    })();
    let _ = fs::remove_file(&temp);
    let frame_count = result?;
    println!("Wrote {} ({frame_count} frames)", output.display());
    Ok(())
}
