//! Deterministic main-MCU breakpoints and lock contention, without host mouse input.
use l6max_diagnostics::gdb;
use l6max_host::{engine::Options, headless::Headless};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn word(h: &Headless, address: u32) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(u32::from_le_bytes(
        h.engine.management[0]
            .read_memory(0, address, 4)?
            .try_into()
            .unwrap(),
    ))
}
fn stopped(h: &Headless) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while h.engine.management[0].query_status()?.running {
        h.display.sample()?;
        if Instant::now() > deadline {
            return Err("breakpoint was not reached".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
fn symbol(report: &serde_json::Value, name: &str) -> u32 {
    report["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["symbol"] == name)
        .unwrap()["target"]
        .as_u64()
        .unwrap() as u32
}
fn inject_at_tick(
    h: &Headless,
    g: &mut gdb::Gdb,
    target: u32,
    arg: u32,
) -> Result<Option<[u8; 1024]>, Box<dyn std::error::Error>> {
    let q = &h.engine.management[0];
    q.stop()?;
    g.breakpoint(0x800064c0, true)?;
    q.resume()?;
    stopped(h)?;
    g.breakpoint(0x800064c0, false)?;
    // Replaces one void callback invocation with a stock operation in the same
    // task and ABI. Its existing LR returns to the callback dispatcher.
    g.register(0, arg)?;
    g.register(15, target)?;
    q.resume()?;
    let end = h.guest_ms() + 300;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut frame = None;
    while h.guest_ms() < end {
        if let Some(f) = h.display.sample()? {
            frame = Some(f);
        }
        if Instant::now() > deadline {
            return Err("injected operation stalled".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(f) = h.display.sample()? {
        frame = Some(f);
    }
    Ok(frame)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::parse_args(std::env::args_os().skip(1))?;
    if options.state_dir.is_some() {
        return Err("requires --volatile".into());
    }
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(options.firmware_dir.join("patch.json"))?)?;
    let socket = PathBuf::from(format!(
        "/private/tmp/l6-popup-gdb-{}.sock",
        std::process::id()
    ));
    options.gdb_socket = Some(socket.clone());
    let h = Headless::start(&options)?;
    h.recorder()?;
    let q = &h.engine.management[0];
    let inputs = h.engine.inputs.as_ref().unwrap();
    q.stop()?;
    let mut g = gdb::Gdb::connect(&socket)?;
    let tagged = word(&h, 0x8020a2a0)?;
    assert_eq!(tagged & 1, 1, "sidecar initialization failed");
    let context = tagged & !1;
    let semaphore = word(&h, context)?;
    assert_eq!(word(&h, semaphore + 60)?, 1);
    assert_eq!(word(&h, semaphore + 64)?, 0);
    let resources = q.read_memory(0, 0x802059b8, 12)?;
    let bitmap = word(&h, 0x80207458)?;
    let heap_before = word(&h, 0x8020ab1c)?;
    q.resume()?;
    h.tap(35, 50, 150)?;
    inputs.encoder(0, -1)?;
    h.wait(350)?;
    assert_ne!(word(&h, 0x80207458)?, bitmap, "no popup");
    let popup_frame = h.sample_display()?.ok_or("no initial popup frame")?;
    // Invoke the actual recorder polling callback while the busy flag is 0.
    // Its stock hide call is a no-op; neither expiry nor popup ownership should change.
    assert_eq!(q.read_memory(0, 0x8020a2b0, 1)?[0], 0);
    let allocation = word(&h, 0x80207458)?;
    let elapsed_before = u16::from_le_bytes(q.read_memory(0, 0x8020a2c4, 2)?.try_into().unwrap());
    let refreshed = inject_at_tick(&h, &mut g, 0x8004da20, 0)?
        // LCD publication deduplicates identical pixels. An unchanged poll
        // may have no new frame; the last completed transfer stays visible.
        .or(h.sample_display()?)
        .ok_or("no recorder display frame")?;
    assert_eq!(
        word(&h, 0x80207458)?,
        allocation,
        "idle busy-hide detached popup"
    );
    assert_eq!(q.read_memory(0, 0x8020a2c0, 1)?[0], 1);
    let elapsed_after = u16::from_le_bytes(q.read_memory(0, 0x8020a2c4, 2)?.try_into().unwrap());
    assert!(
        elapsed_after >= elapsed_before && elapsed_after < 20,
        "poll changed expiry timing"
    );
    for y in 9..57usize {
        for x in 9..119usize {
            let index = ((63 - y) / 8) * 128 + x;
            let mask = 1 << ((63 - y) % 8);
            assert_eq!(
                refreshed[index] & mask,
                popup_frame[index] & mask,
                "recorder poll erased popup at ({x}, {y})"
            );
        }
    }
    println!("Recorder polling: idle busy-hide preserves popup pixels, allocation and expiry.");
    // Replace our popup with an ordinary stock Done. The stock draw must see
    // original resources even though it happens inside the replacement call.
    let frame = inject_at_tick(&h, &mut g, symbol(&report, "notification_show_hook"), 15)?;
    assert_eq!(q.read_memory(0, 0x802059b8, 12)?, resources);
    assert_eq!(word(&h, 0x80207458)?, bitmap);
    assert_eq!(word(&h, 0x8020ab1c)?, heap_before);
    assert_eq!(q.read_memory(0, 0x8020a2c0, 1)?[0], 1, "stock Done missing");
    fs::write(
        options.logs.join("stock-done.lcd"),
        frame.ok_or("no Done frame")?,
    )?;
    h.wait(2400)?;
    assert_eq!(q.read_memory(0, 0x8020a2c0, 1)?[0], 0);
    println!(
        "Stock Done replacement: original bindings and heap restored before successor render."
    );
    // Arrange preemption precisely at the expiry pop boundary, while the
    // notification callback owns the lock. New encoder events then race with
    // cleanup through the real GPIO/UART path.
    inputs.encoder(0, -1)?;
    h.wait(350)?;
    q.stop()?;
    g.breakpoint(symbol(&report, "notification_pop_hook"), true)?;
    q.resume()?;
    stopped(&h)?;
    assert_ne!(word(&h, 0x80207458)?, bitmap);
    assert_ne!(word(&h, context + 8)?, 0, "expiry did not own the lock");
    let before = q.read_memory(0, 0x804600a4, 1)?[0];
    inputs.encoder(0, -3)?;
    g.breakpoint(symbol(&report, "notification_pop_hook"), false)?;
    q.resume()?;
    h.wait(700)?;
    let after = q.read_memory(0, 0x804600a4, 1)?[0];
    println!(
        "Expiry mixer: {before} -> {after}; mode {}, copied raw {}",
        word(&h, context + 16)?,
        word(&h, context + 20)?
    );
    assert_ne!(after, before, "mixer did not process expiry-boundary input");
    assert_eq!(
        word(&h, context + 20)?,
        u32::from(after),
        "copied popup value differs from applied mixer value"
    );
    h.wait(2400)?;
    assert_eq!(word(&h, 0x80207458)?, bitmap);
    assert_eq!(q.read_memory(0, 0x802059b8, 12)?, resources);
    assert_eq!(word(&h, 0x8020ab1c)?, heap_before);
    println!(
        "Forced expiry boundary: new mixer changes apply; popup returns to stock resources without leaking."
    );
    // The sixth distinct message calls pop from inside a locked show.
    inject_at_tick(&h, &mut g, symbol(&report, "notification_show_hook"), 16)?;
    let value_before = q.read_memory(0, 0x804600a4, 1)?[0];
    inputs.encoder(0, -1)?;
    h.wait(300)?;
    assert_ne!(q.read_memory(0, 0x804600a4, 1)?[0], value_before);
    assert_eq!(
        word(&h, 0x80207458)?,
        bitmap,
        "knob overrode a stock latched message"
    );
    for message in [2, 3, 4, 5, 6] {
        inject_at_tick(
            &h,
            &mut g,
            symbol(&report, "notification_show_hook"),
            message,
        )?;
    }
    let slot = q.read_memory(0, 0x8020a2c2, 1)?[0] as u32;
    assert_eq!(
        q.read_memory(0, 0x8020a2a4 + 2 * slot, 2)?,
        2u16.to_le_bytes(),
        "full queue did not pop its oldest message"
    );
    assert_eq!(
        word(&h, context + 8)?,
        0,
        "nested show/pop leaked lock depth"
    );
    inject_at_tick(&h, &mut g, symbol(&report, "busy_show_hook"), 0)?;
    assert_eq!(q.read_memory(0, 0x8020a2c8, 1)?[0], 1);
    let paused = q.read_memory(0, 0x8020a2c4, 2)?;
    h.wait(700)?;
    assert_eq!(
        q.read_memory(0, 0x8020a2c4, 2)?,
        paused,
        "busy overlay advanced notification expiry"
    );
    inject_at_tick(&h, &mut g, symbol(&report, "busy_hide_hook"), 0)?;
    assert_eq!(q.read_memory(0, 0x8020a2c8, 1)?[0], 0);
    inject_at_tick(&h, &mut g, symbol(&report, "overlays_clear_hook"), 0)?;
    assert_eq!(q.read_memory(0, 0x8020a2c0, 1)?[0], 0);
    assert_eq!(q.read_memory(0, 0x8020a2a4, 10)?, vec![0; 10]);
    assert_eq!(word(&h, context + 8)?, 0);
    println!("Nested full-queue pop, stock priority, busy pause/resume and clear passed.");
    // Force the actual popup allocator call to return null, without modifying
    // the heap. Mixer application has already completed before this callback.
    q.stop()?;
    g.breakpoint(0x80085478, true)?;
    let value_before = q.read_memory(0, 0x804600a4, 1)?[0];
    q.resume()?;
    inputs.encoder(0, -1)?;
    stopped(&h)?;
    assert_eq!(
        g.read_register(0)?,
        1788,
        "unexpected allocation at failure injection"
    );
    let return_pc = g.read_register(14)? & !1;
    g.breakpoint(0x80085478, false)?;
    g.register(0, 0)?;
    g.register(15, return_pc)?;
    q.resume()?;
    h.wait(400)?;
    assert_ne!(q.read_memory(0, 0x804600a4, 1)?[0], value_before);
    assert_eq!(word(&h, 0x80207458)?, bitmap);
    assert_eq!(word(&h, 0x8020ab1c)?, heap_before);
    assert_eq!(word(&h, context + 8)?, 0);
    println!("Forced popup allocation failure: mixer applied, heap and stock bindings unchanged.");
    // Hold the notification semaphore for the rest of this disposable VM.
    // A foreign owner is injected while all CPUs are stopped; the GUI task
    // blocks in the actual FreeRTOS take. No test fields are added to firmware.
    q.stop()?;
    assert_eq!(word(&h, context + 8)?, 0);
    g.write(context + 4, &0xdeadbeefu32.to_le_bytes())?;
    g.write(context + 8, &1u32.to_le_bytes())?;
    g.write(semaphore + 56, &0u32.to_le_bytes())?;
    let before = q.read_memory(0, 0x804600a4, 1)?[0];
    g.breakpoint(0x80053b28, true)?;
    q.resume()?;
    let start = h.guest_ms();
    inputs.encoder(0, -6)?;
    for _ in 0..6 {
        stopped(&h)?;
        // Count actual original mixer invocations while the GUI is blocked.
        // Step its first instruction so the next stop is the next invocation.
        g.breakpoint(0x80053b28, false)?;
        g.request("s")?;
        g.breakpoint(0x80053b28, true)?;
        q.resume()?;
    }
    q.stop()?;
    g.breakpoint(0x80053b28, false)?;
    q.resume()?;
    h.wait(1200)?;
    let after = q.read_memory(0, 0x804600a4, 1)?[0];
    println!(
        "Contention mixer: {before} -> {after}; mode {}, copied raw {}",
        word(&h, context + 16)?,
        word(&h, context + 20)?
    );
    assert_ne!(after, before, "popup lock blocked mixer input");
    assert_eq!(word(&h, context + 20)?, u32::from(after));
    assert_eq!(
        word(&h, context + 12)?,
        1,
        "latest optional value was not queued"
    );
    assert_eq!(
        word(&h, 0x80207458)?,
        bitmap,
        "blocked GUI changed popup resources"
    );
    assert_eq!(
        word(&h, semaphore + 36)?,
        1,
        "GUI did not block on the actual semaphore wait list"
    );
    let reports = fs::read_to_string(options.logs.join("engine.trace"))?
        .matches("l6 panel USART1 TX a1\n")
        .count();
    assert_eq!(reports, 13, "missing or duplicate GPIO encoder reports");
    println!(
        "Held lock: six original mixer calls observed; test phase {} guest ms, mixer {} -> {}; GUI waits, mixer does not.",
        h.guest_ms() - start,
        before,
        after
    );
    fs::write(
        options.logs.join("popup-concurrency.json"),
        serde_json::to_string_pretty(
            &serde_json::json!({"stock_replacement":true,"expiry_boundary":true,"held_lock":true,"full_queue":true,"busy_pause_resume":true,"popup_allocation_failure":true,"encoder_reports":reports,"mixer_before":before,"mixer_after":after,"context":context,"semaphore":semaphore}),
        )?,
    )?;
    drop(h);
    let _ = fs::remove_file(socket);
    Ok(())
}
