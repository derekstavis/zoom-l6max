//! ALSA sequencer client with three duplex ports.
use super::{HostMidi, PORT_NAMES, endpoint_name};
use alsa::seq::{MidiEvent, PortCap, PortType, Seq};
use std::{ffi::CString, io};

const BUFFER: usize = 16384;

/// Dropping the sequencer handle deletes its client and ports.
pub struct AlsaPorts {
    seq: Seq,
    ports: Vec<i32>,
    decoder: MidiEvent,
    encoders: Vec<MidiEvent>,
}
fn error(e: alsa::Error) -> io::Error {
    io::Error::other(format!("ALSA sequencer: {e}"))
}
impl HostMidi for AlsaPorts {
    fn open() -> io::Result<Self> {
        // Non-blocking so the worker can interleave USB polling.
        let seq = Seq::open(None, None, true).map_err(error)?;
        seq.set_client_name(&CString::new("L6max Emulator").unwrap())
            .map_err(error)?;
        let mut ports = Vec::new();
        for cable in 0..PORT_NAMES.len() {
            let name = CString::new(endpoint_name(cable)).unwrap();
            ports.push(
                seq.create_simple_port(
                    &name,
                    PortCap::READ | PortCap::WRITE | PortCap::SUBS_READ | PortCap::SUBS_WRITE,
                    PortType::MIDI_GENERIC | PortType::APPLICATION,
                )
                .map_err(error)?,
            );
        }
        Ok(Self {
            seq,
            ports,
            decoder: MidiEvent::new(BUFFER as u32).map_err(error)?,
            encoders: (0..PORT_NAMES.len())
                .map(|_| MidiEvent::new(BUFFER as u32).map_err(error))
                .collect::<io::Result<_>>()?,
        })
    }
    fn read(&mut self, limit: usize) -> io::Result<Vec<(u8, Vec<u8>)>> {
        let mut messages = Vec::new();
        let mut input = self.seq.input();
        while messages.len() < limit {
            let mut event = match input.event_input() {
                Ok(event) => event,
                Err(e) if e.errno() == libc::EAGAIN => break,
                // A dropped event is recoverable; the buffer overran.
                Err(e) if e.errno() == libc::ENOSPC => continue,
                Err(e) => return Err(error(e)),
            };
            let Some(cable) = self
                .ports
                .iter()
                .position(|&port| port == event.get_dest().port)
            else {
                continue;
            };
            let mut bytes = vec![0; BUFFER];
            // Events without a MIDI byte form (client and port announcements)
            // report an error and are ignored.
            if let Ok(length) = self.decoder.decode(&mut bytes, &mut event) {
                bytes.truncate(length);
                if !bytes.is_empty() {
                    messages.push((cable as u8, bytes));
                }
            }
        }
        Ok(messages)
    }
    fn write(&mut self, cable: u8, mut data: &[u8]) -> io::Result<()> {
        let Some(&port) = self.ports.get(usize::from(cable)) else {
            return Ok(());
        };
        while !data.is_empty() {
            let (used, event) = self.encoders[usize::from(cable)]
                .encode(data)
                .map_err(error)?;
            if let Some(mut event) = event {
                event.set_source(port);
                event.set_subs();
                event.set_direct();
                self.seq.event_output_direct(&mut event).map_err(error)?;
            }
            if used == 0 {
                break;
            }
            data = &data[used..];
        }
        Ok(())
    }
}
