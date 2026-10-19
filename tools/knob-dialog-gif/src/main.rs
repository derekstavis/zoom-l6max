//! Records a short looping GIF of the knob-dialog patch's value popup, by
//! driving the emulator's channel-1 level encoder and sampling the LCD
//! framebuffer, the same way `knob-dialog-smoke` does for its assertions.
//! Local-only: needs `ffmpeg` on PATH and a real firmware dir, never run in CI.
use l6max_host::{engine::Options, headless::Headless};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn capture(guest: &Headless, ms: u32, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let end = guest.guest_ms() + ms;
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last = None;
    while guest.guest_ms() < end {
        if let Some(frame) = guest.sample_display()? {
            last = Some(frame);
        }
        if Instant::now() > deadline {
            return Err("display timeout".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(frame) = guest.sample_display()? {
        last = Some(frame);
    }
    let raw = last.ok_or("no display frame")?;
    image::DynamicImage::ImageLuma8(image::GrayImage::from_fn(128, 64, |x, y| {
        image::Luma([
            if raw[((63 - y as usize) / 8) * 128 + x as usize] & (1 << ((63 - y as usize) % 8)) != 0
            {
                255
            } else {
                0
            },
        ])
    }))
    .resize(768, 384, image::imageops::FilterType::Nearest)
    .save(path)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut engine_args = Vec::new();
    let mut out = PathBuf::from("knob-dialog.gif");
    while let Some(arg) = args.next() {
        if arg == "--out" {
            out = PathBuf::from(args.next().ok_or("--out needs a path")?);
        } else {
            engine_args.push(arg);
        }
    }
    let options = Options::parse_args(engine_args)?;

    let frames_dir = options.logs.join("knob-dialog-gif-frames");
    fs::create_dir_all(&frames_dir)?;
    for entry in fs::read_dir(&frames_dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "png") {
            fs::remove_file(entry.path())?;
        }
    }

    let guest = Headless::start(&options)?;
    guest.recorder()?;

    let mut frame_index = 0u32;
    let mut shoot = |guest: &Headless, hold_ms: u32| -> Result<(), Box<dyn std::error::Error>> {
        let path = frames_dir.join(format!("frame-{frame_index:04}.png"));
        capture(guest, hold_ms, &path)?;
        frame_index += 1;
        Ok(())
    };

    // Select "level" on channel 1 and settle on the plain mixer screen. Shot
    // twice so the loop pauses there before the knob starts turning.
    guest.tap(35, 50, 450)?;
    shoot(&guest, 400)?;
    shoot(&guest, 400)?;

    // Positive encoder steps lower the displayed value here (see
    // `InputHub::knob`'s polarity note) — negative steps raise it.
    let inputs = guest.engine.inputs.as_ref().unwrap();
    for steps in [-4, -4, -4, -4, -3] {
        inputs.encoder(0, steps)?;
        shoot(&guest, 600)?;
    }
    // Hold the final value for a beat before it expires.
    shoot(&guest, 500)?;
    shoot(&guest, 500)?;
    guest.wait(2300)?;
    // Back to the plain screen once the popup auto-dismisses; held a bit
    // longer so the loop doesn't feel like it jumps straight back in.
    shoot(&guest, 500)?;
    shoot(&guest, 500)?;

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let status = Command::new("ffmpeg")
        .args(["-y", "-framerate", "3", "-i"])
        .arg(frames_dir.join("frame-%04d.png"))
        .args([
            "-vf",
            "split[s0][s1];[s0]palettegen=max_colors=2:reserve_transparent=0[p];[s1][p]paletteuse=dither=none",
            "-loop",
            "0",
        ])
        .arg(&out)
        .status()?;
    if !status.success() {
        return Err("ffmpeg failed to build the gif".into());
    }
    println!("wrote {} ({frame_index} frames)", out.display());
    Ok(())
}
