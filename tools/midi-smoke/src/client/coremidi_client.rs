//! CoreMIDI client connected to the emulator's mixer-control endpoints.
use super::{MidiClient, mixer_endpoint};
use coremidi::{Client, Destination, Destinations, InputPort, OutputPort, PacketBuffer, Source};
use std::sync::mpsc::{self, Receiver};

struct CoreMidiClient {
    _client: Client,
    input: InputPort,
    output: OutputPort,
    source: Source,
    destination: Destination,
    received: Receiver<Vec<u8>>,
}
fn status(s: i32) -> String {
    format!("CoreMIDI {s}")
}
fn destination_present(name: &str) -> bool {
    Destinations
        .into_iter()
        .any(|d| d.name().as_deref() == Some(name))
}
pub fn connect_os() -> Result<Box<dyn MidiClient>, String> {
    let name = mixer_endpoint();
    let source = Source::from_name(&name).ok_or("missing native MIDI source")?;
    let destination = Destinations
        .into_iter()
        .find(|d| d.name().as_deref() == Some(name.as_str()))
        .ok_or("missing native MIDI destination")?;
    let client = Client::new("L6max MIDI integration check").map_err(status)?;
    let (tx, received) = mpsc::channel();
    #[allow(deprecated)]
    let input = client
        .input_port("integration input", move |packets| {
            for p in packets.iter() {
                let _ = tx.send(p.data().to_vec());
            }
        })
        .map_err(status)?;
    input.connect_source(&source).map_err(status)?;
    let output = client.output_port("integration output").map_err(status)?;
    Ok(Box::new(CoreMidiClient {
        _client: client,
        input,
        output,
        source,
        destination,
        received,
    }))
}
impl MidiClient for CoreMidiClient {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.output
            .send(&self.destination, &PacketBuffer::new(0, bytes))
            .map_err(status)
    }
    fn poll(&mut self) -> Result<Option<[u8; 3]>, String> {
        while let Ok(data) = self.received.try_recv() {
            if let [status @ 0xb0..=0xbf, a, b] = data[..] {
                return Ok(Some([status, a, b]));
            }
        }
        Ok(None)
    }
    fn release(&mut self) {
        let _ = self.input.disconnect_source(&self.source);
    }
    fn endpoint_present(&mut self) -> Option<bool> {
        let name = mixer_endpoint();
        Some(Source::from_name(&name).is_some() || destination_present(&name))
    }
}
