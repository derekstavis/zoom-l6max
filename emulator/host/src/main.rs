//! Run the device host without creating a graphical application.
use l6max_host::{engine::Options, headless::Headless};
use std::{
    ffi::OsString,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine_args = Vec::<OsString>::new();
    let mut seconds = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--seconds" {
            let value = args.next().ok_or("--seconds requires a guest duration")?;
            let value: u32 = value.to_str().ok_or("invalid duration")?.parse()?;
            seconds = Some(value.checked_mul(1000).ok_or("duration is too large")?);
        } else if arg == "--help" || arg == "-h" {
            println!(
                "Usage: l6max-host [--seconds GUEST_SECONDS] [engine options]\nRuns until SIGINT/SIGTERM, power-off, or the guest duration expires."
            );
            engine_args.push(arg);
        } else {
            engine_args.push(arg);
        }
    }
    let options = Options::parse_args(engine_args)?;
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
    let host = Headless::start(&options)?;
    let inputs = host.engine.inputs.as_ref().unwrap();
    eprintln!("Device host started; logs: {}", options.logs.display());
    let mut last_guest_ms = host.guest_ms();
    let mut progressed_at = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        let guest_ms = host.guest_ms();
        if seconds.is_some_and(|end| guest_ms >= end) || inputs.main.snapshot().powered_off {
            break;
        }
        host.sample_display()?;
        if guest_ms != last_guest_ms {
            last_guest_ms = guest_ms;
            progressed_at = Instant::now();
        } else if progressed_at.elapsed() > Duration::from_secs(120) {
            return Err("guest stopped advancing; inspect QEMU logs".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    eprintln!("Device host stopped at {} guest ms", host.guest_ms());
    // Engine's Drop shuts down and reaps QEMU, including when interrupted.
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("l6max-host: {error}");
        std::process::exit(1);
    }
}
