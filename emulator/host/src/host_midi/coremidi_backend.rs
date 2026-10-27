//! CoreMIDI virtual sources and destinations.
use super::{
    HostMidi, MANUFACTURER_NAME, client_name_for_ports, endpoint_name, model_name_for_ports,
};
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
    fn open(names: &[&str]) -> io::Result<Self> {
        let (tx, received) = mpsc::sync_channel::<(u8, Vec<u8>)>(128);
        let client = Client::new(client_name_for_ports(names)).map_err(error)?;
        let model = model_name_for_ports(names);
        let mut destinations = Vec::new();
        let mut sources = Vec::new();
        for (cable, &name) in names.iter().enumerate() {
            let name = endpoint_name(name);
            let sender = tx.clone();
            #[allow(deprecated)]
            let destination = client
                .virtual_destination(&name, move |packets| {
                    for p in packets.iter() {
                        let _ = sender.try_send((cable as u8, p.data().to_vec()));
                    }
                })
                .map_err(error)?;
            let _ = destination.set_property_string("model", model);
            let _ = destination.set_property_string("manufacturer", MANUFACTURER_NAME);
            destinations.push(destination);
            let source = client.virtual_source(&name).map_err(error)?;
            let _ = source.set_property_string("model", model);
            let _ = source.set_property_string("manufacturer", MANUFACTURER_NAME);
            sources.push(source);
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
