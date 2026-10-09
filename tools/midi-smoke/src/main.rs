//! Host MIDI -> USB MIDI DMA -> firmware -> GPIO indicator round trip.
//!
//! Uses the operating system's MIDI (CoreMIDI on macOS, the ALSA sequencer on
//! Linux) unless `L6_MIDI_DIRECT=1`, which speaks USB MIDI straight to the
//! emulated device and needs no host MIDI system or `/dev/snd/seq` access.
//! `L6_MIDI_SYNC=1` also exercises the midi-sync patch: CC 127 must be answered
//! with 80 Control Changes (needs a patched image).
mod client;

use client::{DirectClient, MidiClient, wait_for};
use l6max_host::{engine::Options, headless::Headless, indicators};
use std::time::{Duration, Instant};

const KNOB_CC: [u8; 2] = [0xb0, 0x51];

fn run() -> Result<(), String> {
    let direct = std::env::var_os("L6_MIDI_DIRECT").is_some();
    let sync = std::env::var_os("L6_MIDI_SYNC").is_some();
    let mut options = Options::parse()?;
    options.usb_enabled = true;
    options.host_midi = !direct;
    let guest = Headless::start(&options)?;
    guest.recorder().map_err(|e| e.to_string())?;
    guest.until(47000).map_err(|e| e.to_string())?;
    let inputs = guest.engine.inputs.as_ref().unwrap();
    let mut client: Box<dyn MidiClient> = if direct {
        Box::new(DirectClient::connect(guest.engine.usb.clone().unwrap())?)
    } else {
        println!(
            "{}",
            guest.engine.midi.as_ref().unwrap().status.lock().unwrap()
        );
        client::connect_os()?
    };
    let idle = || guest.sample_display().map(drop).map_err(|e| e.to_string());

    inputs.encoder(0, -4).map_err(|e| e.to_string())?;
    if !wait_for(&mut *client, Duration::from_secs(15), idle, |m| {
        m[..2] == KNOB_CC
    })? {
        return Err("knob CC did not reach the host".into());
    }
    let ring = || indicators::rings(inputs.panel.snapshot().indicators)[0];
    let before = ring();
    client.send(&[0xb0, 0x51, 100])?;
    guest.wait(3000).map_err(|e| e.to_string())?;
    let after = ring();
    println!("Host MIDI OUT CC81=100: firmware GPIO ring {before:#x} -> {after:#x}");
    if before == after {
        return Err("host CC did not alter firmware indicator output".into());
    }
    if sync {
        client.send(&[0xb0, 127, 127])?;
        let mut replies = 0;
        wait_for(&mut *client, Duration::from_secs(20), idle, |_| {
            replies += 1;
            replies == 80
        })?;
        println!("midi-sync: {replies} Control Change replies to CC 127");
        if replies != 80 {
            return Err(format!("expected 80 CC replies, saw {replies}"));
        }
    }
    client.release();
    let Headless { engine, .. } = guest;
    let closing = Instant::now();
    drop(engine);
    if closing.elapsed() > Duration::from_secs(2) {
        return Err("host MIDI shutdown exceeded two seconds".into());
    }
    if client.endpoint_present() == Some(true) {
        return Err("host MIDI endpoints remained after engine shutdown".into());
    }
    println!("Host MIDI endpoints disposed on engine shutdown");
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("midi-smoke: {e}");
        std::process::exit(1);
    }
}
