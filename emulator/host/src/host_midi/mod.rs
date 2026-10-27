//! Host operating-system MIDI back ends behind one interface.
//!
//! A back end exposes a list of duplex MIDI endpoints to other applications,
//! one per port name it is given. It knows nothing about USB framing: it
//! reads raw MIDI bytes sent by host applications and writes raw MIDI bytes
//! from the device. [`crate::usb_midi::MidiBridge`] owns USB cable framing
//! and its worker for the three USB MIDI cables; [`crate::din_midi`] opens a
//! single-port instance for the MIDI IN/OUT (DIN/TRS) jacks, with no framing
//! at all since that port carries one plain MIDI byte stream.
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
pub const L6_DIN_PORT_NAME: &str = "L6 MIDI In/Out Port";
pub const L6MAX_DIN_PORT_NAME: &str = "L6max MIDI In/Out Port";
pub const L6_PORT_NAMES: [&str; 3] = [
    "L6 MIDI I/O Port",
    "L6 Mixer Control Port",
    "for L6 Editor Port",
];
pub const ENDPOINT_SUFFIX: &str = " (Emulator)";
pub const L6_CLIENT_NAME: &str = "L6 Emulator";
pub const L6MAX_CLIENT_NAME: &str = "L6max Emulator";

pub fn port_names_for_model(model: l6fw::Model) -> &'static [&'static str; 3] {
    match model {
        l6fw::Model::L6 => &L6_PORT_NAMES,
        l6fw::Model::L6max => &PORT_NAMES,
    }
}

pub fn din_port_name_for_model(model: l6fw::Model) -> &'static str {
    match model {
        l6fw::Model::L6 => L6_DIN_PORT_NAME,
        l6fw::Model::L6max => L6MAX_DIN_PORT_NAME,
    }
}

/// `names` is either one of the two 3-entry USB port arrays or a single-entry
/// DIN port array; both shapes carry the model in their first/only name.
fn is_l6(names: &[&str]) -> bool {
    names.first() == Some(&L6_PORT_NAMES[0]) || names.first() == Some(&L6_DIN_PORT_NAME)
}

pub fn client_name_for_ports(names: &[&str]) -> &'static str {
    if is_l6(names) {
        L6_CLIENT_NAME
    } else {
        L6MAX_CLIENT_NAME
    }
}

/// Model name real hardware would report in its `kMIDIPropertyModel`. The
/// official editor builds its device name from an endpoint's model and name
/// properties, so a back end that never sets `model` is invisible to it even
/// though generic CoreMIDI clients (ours included) only need `name`.
pub fn model_name_for_ports(names: &[&str]) -> &'static str {
    if is_l6(names) { "L6" } else { "L6max" }
}

pub const MANUFACTURER_NAME: &str = "ZOOM Corporation";

/// Name under which a port called `name` appears to host applications.
pub fn endpoint_name(name: &str) -> String {
    format!("{name}{ENDPOINT_SUFFIX}")
}

pub trait HostMidi: Sized {
    /// Create one endpoint per entry of `names`, in order, as port indices
    /// 0.. Dropping the value must remove them, before the owning client or
    /// session, so applications see them disappear. Returns
    /// [`io::ErrorKind::Unsupported`] when the platform has no back end.
    fn open(names: &[&str]) -> io::Result<Self>;
    /// Drain up to `limit` messages that host applications sent to the device,
    /// as `(port index, raw MIDI bytes)`. Must not block.
    fn read(&mut self, limit: usize) -> io::Result<Vec<(u8, Vec<u8>)>>;
    /// Deliver raw MIDI bytes from the device on `port` to its subscribers.
    /// An unknown port index is ignored.
    fn write(&mut self, port: u8, data: &[u8]) -> io::Result<()>;
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

    #[test]
    fn midi_port_names_follow_the_detected_firmware_model() {
        assert_eq!(port_names_for_model(l6fw::Model::L6), &L6_PORT_NAMES);
        assert_eq!(port_names_for_model(l6fw::Model::L6max), &PORT_NAMES);
        assert_eq!(din_port_name_for_model(l6fw::Model::L6), L6_DIN_PORT_NAME);
        assert_eq!(
            din_port_name_for_model(l6fw::Model::L6max),
            L6MAX_DIN_PORT_NAME
        );
        assert_eq!(
            client_name_for_ports(port_names_for_model(l6fw::Model::L6)),
            L6_CLIENT_NAME
        );
        assert_eq!(
            client_name_for_ports(port_names_for_model(l6fw::Model::L6max)),
            L6MAX_CLIENT_NAME
        );
        assert_eq!(client_name_for_ports(&[L6_DIN_PORT_NAME]), L6_CLIENT_NAME);
        assert_eq!(
            client_name_for_ports(&[L6MAX_DIN_PORT_NAME]),
            L6MAX_CLIENT_NAME
        );
        assert_eq!(model_name_for_ports(&[L6_DIN_PORT_NAME]), "L6");
        assert_eq!(model_name_for_ports(&[L6MAX_DIN_PORT_NAME]), "L6max");
    }

    /// In-memory back end: `incoming` is what host applications "sent",
    /// `outgoing` records what the device delivered.
    #[derive(Default)]
    pub struct Loopback {
        pub incoming: VecDeque<(u8, Vec<u8>)>,
        pub outgoing: Vec<(u8, Vec<u8>)>,
    }
    impl HostMidi for Loopback {
        fn open(_names: &[&str]) -> io::Result<Self> {
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
