use super::HostMidi;
use std::io;

/// Placeholder for platforms without a host MIDI back end.
pub struct Unavailable;
impl HostMidi for Unavailable {
    fn open() -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no host MIDI back end for this operating system",
        ))
    }
    fn read(&mut self, _: usize) -> io::Result<Vec<(u8, Vec<u8>)>> {
        Ok(Vec::new())
    }
    fn write(&mut self, _: u8, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
}
