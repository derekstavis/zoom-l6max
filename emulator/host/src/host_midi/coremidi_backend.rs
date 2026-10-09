//! CoreMIDI virtual sources and destinations.
use super::{HostMidi, endpoint_name};
use coremidi::{Client, PacketBuffer, VirtualDestination, VirtualSource};
use std::{
    io,
    sync::mpsc::{self, Receiver},
};

// Field order disposes endpoints before the owning CoreMIDI client on
// shutdown, reconnect and every error path.
pub struct CoreMidiPorts {
    received: Receiver<(u8, Vec<u8>)>,
    sources: Vec<VirtualSource>,
    _destinations: Vec<VirtualDestination>,
    _client: Client,
}
fn error(status: i32) -> io::Error {
    io::Error::other(format!("CoreMIDI status {status}"))
}
impl HostMidi for CoreMidiPorts {
    fn open() -> io::Result<Self> {
        let (tx, received) = mpsc::sync_channel::<(u8, Vec<u8>)>(128);
        let client = Client::new("L6max Emulator").map_err(error)?;
        let mut destinations = Vec::new();
        let mut sources = Vec::new();
        for cable in 0..super::PORT_NAMES.len() {
            let name = endpoint_name(cable);
            let sender = tx.clone();
            #[allow(deprecated)]
            let destination = client
                .virtual_destination(&name, move |packets| {
                    for p in packets.iter() {
                        let _ = sender.try_send((cable as u8, p.data().to_vec()));
                    }
                })
                .map_err(error)?;
            destinations.push(destination);
            sources.push(client.virtual_source(&name).map_err(error)?);
        }
        Ok(Self {
            received,
            sources,
            _destinations: destinations,
            _client: client,
        })
    }
    fn read(&mut self, limit: usize) -> io::Result<Vec<(u8, Vec<u8>)>> {
        Ok(self.received.try_iter().take(limit).collect())
    }
    fn write(&mut self, cable: u8, data: &[u8]) -> io::Result<()> {
        if let Some(source) = self.sources.get(usize::from(cable)) {
            source
                .received(&PacketBuffer::new(0, data))
                .map_err(|status| io::Error::other(format!("CoreMIDI receive status {status}")))?;
        }
        Ok(())
    }
}
