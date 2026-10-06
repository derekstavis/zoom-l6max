//! Firmware GPIO shutdown, stopped virtual time, and restart with persistent state.
use l6max_host::{engine::Options, headless::Headless};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::parse()?;
    if options.state_dir.is_some() {
        return Err("requires --volatile to protect device state".into());
    }
    let fixture =
        Fixture(std::env::temp_dir().join(format!("l6-power-smoke-{}", std::process::id())));
    fs::create_dir(&fixture.0)?;
    options.state_dir = Some(fixture.0.clone());
    let guest = Headless::start(&options)?;
    guest.recorder()?;
    guest.tap(53, 100, 1000)?;
    assert!(
        !guest
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .main
            .snapshot()
            .powered_off,
        "short press cut power"
    );
    guest.engine.inputs.as_ref().unwrap().tap(53, 3500)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !guest
        .engine
        .inputs
        .as_ref()
        .unwrap()
        .main
        .snapshot()
        .powered_off
    {
        guest.display.sample()?;
        if Instant::now() > deadline {
            return Err("firmware did not release power hold".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let status = guest.engine.management[0].query_status()?;
    assert_eq!(status.status, "shutdown");
    thread::sleep(Duration::from_millis(100));
    let stopped = guest.guest_ms();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        guest.guest_ms(),
        stopped,
        "clock advanced while powered off"
    );
    println!(
        "PASS: short press ignored; long press reached firmware GPIO power cut; both chips stopped"
    );
    drop(guest);
    options.logs = options.logs.join("restart");
    let restarted = Headless::start(&options)?;
    restarted.until(40000)?;
    let mode = u32::from_le_bytes(
        restarted.engine.management[0]
            .read_memory(0, 0x80462d9c, 4)?
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        mode, 0,
        "Date/Time settings did not persist across power cycle"
    );
    assert!(
        !restarted
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .main
            .snapshot()
            .powered_off
    );
    println!("PASS: restart ran both firmware images and retained Date/Time setup");
    Ok(())
}
