//! Observe the real recorder polling callback's effect on a live knob popup.
use l6max_diagnostics::gdb;
use l6max_host::{engine::Options, headless::Headless};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::parse_args(std::env::args_os().skip(1))?;
    if options.state_dir.is_some() {
        return Err("requires --volatile".into());
    }
    let socket = PathBuf::from(format!(
        "/private/tmp/l6-trigger-{}.sock",
        std::process::id()
    ));
    options.gdb_socket = Some(socket.clone());
    let h = Headless::start(&options)?;
    h.recorder()?;
    let q = &h.engine.management[0];
    let inputs = h.engine.inputs.as_ref().unwrap();
    let word = |address| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(u32::from_le_bytes(
            q.read_memory(0, address, 4)?.try_into().unwrap(),
        ))
    };
    let original_bitmap = word(0x80207458)?;
    h.tap(35, 50, 150)?;
    inputs.encoder(0, -1)?;
    h.wait(350)?;
    let snapshot = || -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(serde_json::json!({
  "bitmap":word(0x80207458)?,"busy":q.read_memory(0,0x8020a2b0,1)?[0],
  "notification_active":q.read_memory(0,0x8020a2c0,1)?[0],
  "elapsed":u16::from_le_bytes(q.read_memory(0,0x8020a2c4,2)?.try_into().unwrap()),
  "recorder_state_a":word(0x8020a9f0)?,"recorder_state_b":word(0x8020a9f4)?}))
    };
    let before = snapshot()?;
    assert_ne!(word(0x80207458)?, original_bitmap, "no initial popup");
    assert_eq!(before["busy"], 0);
    q.stop()?;
    let mut g = gdb::Gdb::connect(&socket)?;
    g.breakpoint(0x800064c0, true)?;
    q.resume()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while q.query_status()?.running {
        h.sample_display()?;
        if Instant::now() > deadline {
            return Err("tick breakpoint timeout".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    g.breakpoint(0x800064c0, false)?;
    // One callback invocation is replaced before its prologue. LR and the task
    // stack remain intact. No dirty flag or recorder state is fabricated.
    g.register(0, 0)?;
    g.register(15, 0x8004da20)?;
    q.resume()?;
    h.wait(300)?;
    let after = snapshot()?;
    let report = serde_json::json!({"trigger":"original recorder periodic callback 0x8004da20","original_bitmap":original_bitmap,"before":before,"after":after});
    fs::write(
        options.logs.join("popup-trigger.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
