//! Host operating-system MIDI back ends behind one interface.
//!
//! A back end exposes the emulated device as three duplex MIDI endpoints (one per
//! USB MIDI cable) to other applications. It knows nothing about USB framing: it
//! reads raw MIDI bytes sent by host applications and writes raw MIDI bytes from
//! the device. [`crate::usb_midi::MidiBridge`] owns the framing and the worker.
//!
//! Adding a platform means implementing [`HostMidi`] in one file and selecting it
//! below; nothing else in the host crate changes.
use std::io;

/// Endpoint names for USB MIDI cables 0, 1 and 2, as the firmware's ports are
/// named. Back ends append [`ENDPOINT_SUFFIX`].
pub const PORT_NAMES: [&str; 3] = [
    "L6max MIDI I/O Port",
    "L6max Mixer Control Port",
    "for L6 Editor Port",
];
pub const ENDPOINT_SUFFIX: &str = " (Emulator)";

/// Name under which cable `cable` appears to host applications.
pub fn endpoint_name(cable: usize) -> String {
    format!("{}{ENDPOINT_SUFFIX}", PORT_NAMES[cable])
}

pub trait HostMidi: Sized {
    /// Create the three endpoints. Dropping the value must remove them, before
    /// the owning client or session, so applications see them disappear.
    /// Returns [`io::ErrorKind::Unsupported`] when the platform has no back end.
    fn open() -> io::Result<Self>;
    /// Drain up to `limit` messages that host applications sent to the device,
    /// as `(cable, raw MIDI bytes)`. Must not block.
    fn read(&mut self, limit: usize) -> io::Result<Vec<(u8, Vec<u8>)>>;
    /// Deliver raw MIDI bytes from the device on `cable` to its subscribers.
    /// Unknown cables are ignored.
    fn write(&mut self, cable: u8, data: &[u8]) -> io::Result<()>;
}

#[cfg(target_os = "macos")]
mod coremidi_backend;
#[cfg(target_os = "macos")]
pub use coremidi_backend::CoreMidiPorts as NativeMidi;

#[cfg(target_os = "linux")]
mod alsa_backend;
#[cfg(target_os = "linux")]
pub use alsa_backend::AlsaPorts as NativeMidi;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod unavailable;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub use unavailable::Unavailable as NativeMidi;

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::collections::VecDeque;

    /// In-memory back end: `incoming` is what host applications "sent",
    /// `outgoing` records what the device delivered.
    #[derive(Default)]
    pub struct Loopback {
        pub incoming: VecDeque<(u8, Vec<u8>)>,
        pub outgoing: Vec<(u8, Vec<u8>)>,
    }
    impl HostMidi for Loopback {
        fn open() -> io::Result<Self> {
            Ok(Self::default())
        }
        fn read(&mut self, limit: usize) -> io::Result<Vec<(u8, Vec<u8>)>> {
            let count = limit.min(self.incoming.len());
            Ok(self.incoming.drain(..count).collect())
        }
        fn write(&mut self, cable: u8, data: &[u8]) -> io::Result<()> {
            if usize::from(cable) < PORT_NAMES.len() {
                self.outgoing.push((cable, data.to_vec()));
            }
            Ok(())
        }
    }
}
