//! USB MIDI 1.0 packet framing and native host endpoints.
use std::io;

const LENGTHS: [usize; 16] = [0, 0, 2, 3, 3, 1, 2, 3, 3, 3, 3, 3, 2, 2, 3, 1];
pub fn decode(bytes: &[u8]) -> io::Result<Vec<(u8, Vec<u8>)>> {
    if bytes.len() % 4 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated USB MIDI packet",
        ));
    }
    let mut messages = Vec::new();
    for p in bytes.chunks_exact(4) {
        let n = LENGTHS[(p[0] & 15) as usize];
        if n != 0 {
            messages.push((p[0] >> 4, p[1..1 + n].to_vec()));
        }
    }
    Ok(messages)
}
#[derive(Default)]
pub struct Encoder {
    running: u8,
    sysex: bool,
    pending: Vec<u8>,
}
impl Encoder {
    /// Preserve running status and SysEx across CoreMIDI packet boundaries.
    pub fn push(&mut self, cable: u8, bytes: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let emit = |output: &mut Vec<u8>, cin: u8, data: &[u8]| {
            let mut packet = [0; 4];
            packet[0] = (cable << 4) | cin;
            packet[1..1 + data.len()].copy_from_slice(data);
            output.extend_from_slice(&packet);
        };
        for &b in bytes {
            if b >= 0xf8 {
                emit(&mut output, 15, &[b]);
                continue;
            }
            if self.sysex {
                if b == 0xf7 || b < 0x80 {
                    self.pending.push(b);
                    if b == 0xf7 {
                        emit(&mut output, 4 + self.pending.len() as u8, &self.pending);
                        self.pending.clear();
                        self.sysex = false;
                    } else if self.pending.len() == 3 {
                        emit(&mut output, 4, &self.pending);
                        self.pending.clear();
                    }
                    continue;
                }
                self.sysex = false;
                self.pending.clear();
            }
            if b & 0x80 != 0 {
                self.pending.clear();
                if b < 0xf0 {
                    self.running = b;
                    self.pending.push(b);
                } else {
                    self.running = 0;
                    match b {
                        0xf0 => {
                            self.sysex = true;
                            self.pending.push(b);
                        }
                        0xf1..=0xf3 => self.pending.push(b),
                        0xf6 | 0xf7 => emit(&mut output, 5, &[b]),
                        _ => {}
                    }
                }
            } else {
                if self.pending.is_empty() && self.running != 0 {
                    self.pending.push(self.running);
                }
                if !self.pending.is_empty() {
                    self.pending.push(b);
                }
            }
            if !self.sysex && !self.pending.is_empty() {
                let status = self.pending[0];
                let (cin, length) = match status {
                    0x80..=0xbf | 0xe0..=0xef => (status >> 4, 3),
                    0xc0..=0xdf => (status >> 4, 2),
                    0xf1 | 0xf3 => (2, 2),
                    0xf2 => (3, 3),
                    _ => (0, 0),
                };
                if length != 0 && self.pending.len() == length {
                    emit(&mut output, cin, &self.pending);
                    self.pending.clear();
                }
            }
        }
        output
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use crate::usb::UsbHost;
    use coremidi::{Client, PacketBuffer, VirtualDestination, VirtualSource};
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread::{self, JoinHandle},
        time::Duration,
    };

    pub struct MidiBridge {
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
        pub status: Arc<Mutex<String>>,
        usb: Arc<UsbHost>,
    }
    // Field order disposes endpoints before the owning CoreMIDI client on
    // shutdown, reconnect and every error path.
    struct NativePorts {
        _destinations: Vec<VirtualDestination>,
        sources: Vec<VirtualSource>,
        _client: Client,
    }
    impl MidiBridge {
        pub fn start(usb: Arc<UsbHost>) -> io::Result<Self> {
            let stop = Arc::new(AtomicBool::new(false));
            let status = Arc::new(Mutex::new("Waiting for firmware USB startup".to_owned()));
            let flag = stop.clone();
            let state = status.clone();
            let shutdown_usb = usb.clone();
            let worker = thread::Builder::new()
                .name("l6-usb-midi".into())
                .spawn(move || {
                    let run = || -> io::Result<()> {
                        let (tx, rx) = mpsc::sync_channel::<(u8, Vec<u8>)>(128);
                        let mut ports: Option<NativePorts> = None;
                        let mut encoders: [Encoder; 3] =
                            std::array::from_fn(|_| Encoder::default());
                        let mut endpoints = None;
                        while !flag.load(Ordering::Acquire) {
                            if endpoints.is_none() {
                                match usb.enumerate() {
                                    Ok(config) => {
                                        endpoints = config.midi;
                                        if endpoints.is_none() {
                                            *state.lock().unwrap() =
                                                "USB File Transfer mode; MIDI unavailable".into();
                                            ports = None;
                                        } else if ports.is_none() {
                                            let error = |s| {
                                                io::Error::other(format!("CoreMIDI status {s}"))
                                            };
                                            let client =
                                                Client::new("L6max Emulator").map_err(error)?;
                                            let mut destinations = Vec::new();
                                            let mut sources = Vec::new();
                                            for (cable, name) in [
                                                "L6max MIDI I/O Port",
                                                "L6max Mixer Control Port",
                                                "for L6 Editor Port",
                                            ]
                                            .iter()
                                            .enumerate()
                                            {
                                                let name = format!("{name} (Emulator)");
                                                let sender = tx.clone();
                                                #[allow(deprecated)]
                                                let dest = client
                                                    .virtual_destination(&name, move |packets| {
                                                        for p in packets.iter() {
                                                            let _ = sender.try_send((
                                                                cable as u8,
                                                                p.data().to_vec(),
                                                            ));
                                                        }
                                                    })
                                                    .map_err(error)?;
                                                destinations.push(dest);
                                                sources.push(
                                                    client.virtual_source(&name).map_err(error)?,
                                                );
                                            }
                                            ports = Some(NativePorts {
                                                _destinations: destinations,
                                                sources,
                                                _client: client,
                                            });
                                            *state.lock().unwrap() =
                                                "Three native MIDI ports connected".into();
                                        }
                                    }
                                    Err(e)
                                        if e.raw_os_error() == Some(libc::ENODEV)
                                            || e.raw_os_error() == Some(libc::EAGAIN)
                                            || e.kind() == io::ErrorKind::InvalidData =>
                                    {
                                        *state.lock().unwrap() =
                                            format!("Waiting for firmware USB: {e}");
                                    }
                                    Err(e) => return Err(e),
                                }
                                if endpoints.is_none() {
                                    thread::sleep(Duration::from_millis(250));
                                    continue;
                                }
                            }
                            let pair = endpoints.unwrap();
                            for _ in 0..32 {
                                let Ok((cable, bytes)) = rx.try_recv() else {
                                    break;
                                };
                                let data = encoders[cable as usize].push(cable, &bytes);
                                for chunk in data.chunks(512) {
                                    usb.send(pair.output, chunk)?;
                                }
                            }
                            match usb.try_receive(pair.input, 16384) {
                                Ok(bytes) => {
                                    for (cable, data) in decode(&bytes)? {
                                        if let Some(ports) = &ports {
                                            if let Some(source) = ports.sources.get(cable as usize)
                                            {
                                                source
                                                    .received(&PacketBuffer::new(0, &data))
                                                    .map_err(|s| {
                                                        io::Error::other(format!(
                                                            "CoreMIDI receive status {s}"
                                                        ))
                                                    })?;
                                            }
                                        }
                                    }
                                }
                                Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => {}
                                Err(e) if e.raw_os_error() == Some(libc::ENODEV) => {
                                    endpoints = None;
                                    ports = None;
                                }
                                Err(e) => return Err(e),
                            }
                            thread::sleep(Duration::from_millis(2));
                        }
                        drop(ports);
                        Ok(())
                    };
                    if let Err(e) = run() {
                        if flag.load(Ordering::Acquire) {
                            return;
                        }
                        *state.lock().unwrap() = format!("USB MIDI stopped: {e}");
                        eprintln!("USB MIDI: {e}");
                    }
                })?;
            Ok(Self {
                stop,
                worker: Some(worker),
                status,
                usb: shutdown_usb,
            })
        }
    }
    impl Drop for MidiBridge {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            self.usb.shutdown();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub use native::MidiBridge;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn running_status_and_realtime_survive_packet_boundaries() {
        let mut e = Encoder::default();
        assert_eq!(e.push(1, &[0xb0, 81]), Vec::<u8>::new());
        assert_eq!(
            e.push(1, &[0xf8, 32, 81, 64]),
            [0x1f, 0xf8, 0, 0, 0x1b, 0xb0, 81, 32, 0x1b, 0xb0, 81, 64]
        );
    }
    #[test]
    fn sysex_is_segmented_and_preserves_cable() {
        let mut e = Encoder::default();
        let mut bytes = e.push(2, &[0xf0, 0x52]);
        bytes.extend(e.push(2, &[0, 1, 2, 0xf7]));
        assert_eq!(bytes, [0x24, 0xf0, 0x52, 0, 0x27, 1, 2, 0xf7]);
        let recovered = decode(&bytes)
            .unwrap()
            .into_iter()
            .flat_map(|(_, p)| p)
            .collect::<Vec<_>>();
        assert_eq!(recovered, [0xf0, 0x52, 0, 1, 2, 0xf7]);
        assert!(decode(&[0x1b, 0xb0]).is_err());
    }
}
