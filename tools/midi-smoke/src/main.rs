//! Native CoreMIDI -> USB DMA -> firmware -> GPIO indicator round trip.
#[cfg(target_os = "macos")]
fn run() -> Result<(), String> {
    use coremidi::{Client, Destinations, PacketBuffer, Source};
    use l6max_host::{
        display::PublishedDisplay,
        engine::{Engine, Options},
        indicators,
    };
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    let mut options = Options::parse()?;
    options.usb_enabled = true;
    options.host_midi = true;
    let engine = Engine::start(&options)?;
    let inputs = engine.inputs.as_ref().unwrap();
    let display = PublishedDisplay::new(
        engine.display_memory.as_ref().unwrap().map(),
        inputs.main.clone(),
    )
    .map_err(|e| e.to_string())?;
    let until = |ms: u32| -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs(120);
        while inputs.panel.snapshot().guest_ms < ms {
            display.sample().map_err(|e| e.to_string())?;
            if Instant::now() > end {
                return Err("guest deadline".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    };
    until(40000)?;
    inputs.tap(3, 50).map_err(|e| e.to_string())?;
    until(42000)?;
    inputs.tap(3, 50).map_err(|e| e.to_string())?;
    until(47000)?;
    println!("{}", engine.midi.as_ref().unwrap().status.lock().unwrap());
    let name = "L6max Mixer Control Port (Emulator)";
    let source = Source::from_name(name).ok_or("missing native MIDI source")?;
    let destination = Destinations
        .into_iter()
        .find(|d| d.name().as_deref() == Some(name))
        .ok_or("missing native MIDI destination")?;
    let client =
        Client::new("L6max MIDI integration check").map_err(|s| format!("CoreMIDI {s}"))?;
    let (tx, rx) = mpsc::channel();
    #[allow(deprecated)]
    let input = client
        .input_port("integration input", move |packets| {
            for p in packets.iter() {
                let _ = tx.send(p.data().to_vec());
            }
        })
        .map_err(|s| format!("CoreMIDI {s}"))?;
    input
        .connect_source(&source)
        .map_err(|s| format!("CoreMIDI {s}"))?;
    let output = client
        .output_port("integration output")
        .map_err(|s| format!("CoreMIDI {s}"))?;
    inputs.encoder(0, -4).map_err(|e| e.to_string())?;
    let end = Instant::now() + Duration::from_secs(15);
    let mut received = false;
    while Instant::now() < end {
        display.sample().map_err(|e| e.to_string())?;
        if let Ok(data) = rx.try_recv() {
            println!("Host MIDI IN: {data:02x?}");
            if data.len() == 3 && data[0..2] == [0xb0, 0x51] {
                received = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    if !received {
        return Err("knob CC did not reach native MIDI source".into());
    }
    let before = indicators::rings(inputs.panel.snapshot().indicators)[0];
    output
        .send(&destination, &PacketBuffer::new(0, &[0xb0, 0x51, 100]))
        .map_err(|s| format!("CoreMIDI {s}"))?;
    until(inputs.panel.snapshot().guest_ms + 3000)?;
    let after = indicators::rings(inputs.panel.snapshot().indicators)[0];
    println!("Native MIDI OUT CC81=100: firmware GPIO ring {before:#x} -> {after:#x}");
    if before == after {
        return Err("host CC did not alter firmware indicator output".into());
    }
    input
        .disconnect_source(&source)
        .map_err(|s| format!("CoreMIDI {s}"))?;
    let close_started = Instant::now();
    drop(engine);
    if close_started.elapsed() > Duration::from_secs(2) {
        return Err("native MIDI shutdown exceeded two seconds".into());
    }
    if Source::from_name(name).is_some()
        || Destinations
            .into_iter()
            .any(|d| d.name().as_deref() == Some(name))
    {
        return Err("native MIDI endpoints remained after engine shutdown".into());
    }
    println!("Native MIDI endpoints disposed on engine shutdown");
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn run() -> Result<(), String> {
    Err("CoreMIDI check requires macOS".into())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("midi-smoke: {e}");
        std::process::exit(1);
    }
}
