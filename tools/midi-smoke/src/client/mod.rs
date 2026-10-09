//! How the smoke check talks to the emulated mixer-control port.
//!
//! The scenario in `main.rs` is written once against [`MidiClient`]. Operating
//! system clients exercise the host MIDI endpoints end to end; the direct client
//! speaks USB MIDI to the emulated device and needs no host MIDI system.
use l6max_host::host_midi::endpoint_name;
use std::time::{Duration, Instant};

/// Cable carrying mixer control.
pub const MIXER_CABLE: usize = 1;

pub fn mixer_endpoint() -> String {
    endpoint_name(MIXER_CABLE)
}

pub trait MidiClient {
    /// Send raw MIDI bytes to the device's mixer-control port.
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// The next 3-byte Control Change from the device, if one is waiting.
    fn poll(&mut self) -> Result<Option<[u8; 3]>, String>;
    /// Drop host-side subscriptions so the device's endpoints can disappear
    /// when the emulator shuts down.
    fn release(&mut self) {}
    /// Whether the device's endpoint is still visible to host applications.
    /// `None` when the client does not go through a host MIDI system.
    fn endpoint_present(&mut self) -> Option<bool> {
        None
    }
}

/// Poll until `accept` returns true or `limit` passes; `idle` runs between polls
/// (the scenario samples the display there). Returns whether `accept` succeeded.
pub fn wait_for(
    client: &mut dyn MidiClient,
    limit: Duration,
    mut idle: impl FnMut() -> Result<(), String>,
    mut accept: impl FnMut(&[u8; 3]) -> bool,
) -> Result<bool, String> {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        idle()?;
        match client.poll()? {
            Some(message) => {
                println!("Device MIDI OUT: {message:02x?}");
                if accept(&message) {
                    return Ok(true);
                }
            }
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
mod coremidi_client;
#[cfg(target_os = "macos")]
pub use coremidi_client::connect_os;
#[cfg(target_os = "linux")]
mod alsa_client;
#[cfg(target_os = "linux")]
pub use alsa_client::connect_os;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn connect_os() -> Result<Box<dyn MidiClient>, String> {
    Err("no host MIDI client for this operating system; use L6_MIDI_DIRECT=1".into())
}

mod direct;
pub use direct::DirectClient;
