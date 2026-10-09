//! USB MIDI 1.0 packet framing and the bridge between the emulated device and
//! the host operating system's MIDI, through [`HostMidi`].
use crate::{
    host_midi::{HostMidi, NativeMidi},
    usb::UsbHost,
};
use std::{
    io,
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub use crate::host_midi::PORT_NAMES;

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
    /// Preserve running status and SysEx across host MIDI packet boundaries.
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

/// Host messages as `(cable, bytes)` become USB MIDI packets, sent in chunks the
/// device's bulk endpoint accepts.
fn forward_host_to_usb<P: HostMidi>(
    ports: &mut P,
    encoders: &mut [Encoder; 3],
    mut send: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    for (cable, bytes) in ports.read(32)? {
        let Some(encoder) = encoders.get_mut(usize::from(cable)) else {
            continue;
        };
        for chunk in encoder.push(cable, &bytes).chunks(512) {
            send(chunk)?;
        }
    }
    Ok(())
}
/// Device packets are unframed and delivered to the endpoint of their cable.
fn forward_usb_to_host<P: HostMidi>(ports: &mut P, packets: &[u8]) -> io::Result<()> {
    for (cable, data) in decode(packets)? {
        if usize::from(cable) < PORT_NAMES.len() {
            ports.write(cable, &data)?;
        }
    }
    Ok(())
}

/// Worker that mirrors the device's three USB MIDI cables onto host endpoints.
/// The endpoints exist only while the firmware exposes the MIDI interface. On a
/// platform without a back end it still drains the device so its queue never
/// stalls, and reports that through `status`.
pub struct MidiBridge<P: HostMidi = NativeMidi> {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<String>>,
    usb: Arc<UsbHost>,
    backend: PhantomData<fn() -> P>,
}
impl<P: HostMidi + 'static> MidiBridge<P> {
    pub fn start(usb: Arc<UsbHost>) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("Waiting for firmware USB startup".to_owned()));
        let flag = stop.clone();
        let state = status.clone();
        let worker_usb = usb.clone();
        let worker = thread::Builder::new()
            .name("l6-usb-midi".into())
            .spawn(move || {
                if let Err(e) = Self::run(&worker_usb, &flag, &state) {
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
            usb,
            backend: PhantomData,
        })
    }
    fn run(usb: &UsbHost, stop: &AtomicBool, state: &Mutex<String>) -> io::Result<()> {
        let set = |text: String| *state.lock().unwrap() = text;
        let mut ports: Option<P> = None;
        let mut unsupported = false;
        let mut encoders: [Encoder; 3] = std::array::from_fn(|_| Encoder::default());
        let mut endpoints = None;
        while !stop.load(Ordering::Acquire) {
            if endpoints.is_none() {
                match usb.enumerate() {
                    Ok(config) => {
                        endpoints = config.midi;
                        if endpoints.is_none() {
                            set("USB File Transfer mode; MIDI unavailable".into());
                            ports = None;
                        } else if ports.is_none() && !unsupported {
                            match P::open() {
                                Ok(opened) => {
                                    ports = Some(opened);
                                    set("Three native MIDI ports connected".into());
                                }
                                Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                                    unsupported = true;
                                    set(format!("Host MIDI unavailable: {e}"));
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                    Err(e)
                        if e.raw_os_error() == Some(libc::ENODEV)
                            || e.raw_os_error() == Some(libc::EAGAIN)
                            || e.kind() == io::ErrorKind::InvalidData =>
                    {
                        set(format!("Waiting for firmware USB: {e}"));
                    }
                    Err(e) => return Err(e),
                }
                if endpoints.is_none() {
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
            let pair = endpoints.unwrap();
            if let Some(native) = &mut ports {
                forward_host_to_usb(native, &mut encoders, |chunk| usb.send(pair.output, chunk))?;
            }
            match usb.try_receive(pair.input, 16384) {
                Ok(bytes) => match &mut ports {
                    Some(native) => forward_usb_to_host(native, &bytes)?,
                    // Still validate framing so a corrupt stream is reported.
                    None => drop(decode(&bytes)?),
                },
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
    }
}
impl<P: HostMidi> Drop for MidiBridge<P> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.usb.shutdown();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_midi::testing::Loopback;
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
    #[test]
    fn host_messages_are_framed_per_cable_and_chunked() {
        let mut ports = Loopback::default();
        ports.incoming.push_back((1, vec![0xb0, 0x51, 100]));
        ports
            .incoming
            .push_back((2, vec![0xf0, 0x7e, 0, 6, 1, 0xf7]));
        // Messages for a cable the bridge does not know are dropped.
        ports.incoming.push_back((7, vec![0xb0, 1, 2]));
        let mut encoders: [Encoder; 3] = std::array::from_fn(|_| Encoder::default());
        let mut sent = Vec::new();
        forward_host_to_usb(&mut ports, &mut encoders, |chunk| {
            sent.push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(sent[0], [0x1b, 0xb0, 0x51, 100]);
        assert_eq!(sent[1], [0x24, 0xf0, 0x7e, 0, 0x27, 6, 1, 0xf7]);
        assert_eq!(sent.len(), 2);
    }
    #[test]
    fn long_sysex_is_split_at_the_endpoint_limit() {
        let mut ports = Loopback::default();
        let mut sysex = vec![0xf0];
        sysex.extend((0..500).map(|i| (i % 100) as u8));
        sysex.push(0xf7);
        ports.incoming.push_back((2, sysex.clone()));
        let mut encoders: [Encoder; 3] = std::array::from_fn(|_| Encoder::default());
        let mut chunks = Vec::new();
        forward_host_to_usb(&mut ports, &mut encoders, |chunk| {
            chunks.push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.len() <= 512 && c.len() % 4 == 0));
        let wire = chunks.concat();
        let recovered = decode(&wire)
            .unwrap()
            .into_iter()
            .flat_map(|(_, bytes)| bytes)
            .collect::<Vec<_>>();
        assert_eq!(recovered, sysex);
    }
    #[test]
    fn device_packets_reach_only_known_cables() {
        let mut ports = Loopback::default();
        let packets = [
            0x1b, 0xb0, 0x51, 100, // cable 1 control change
            0x2b, 0xb0, 0x01, 2, // cable 2
            0x9b, 0xb0, 0x01, 3, // cable 9 does not exist
        ];
        forward_usb_to_host(&mut ports, &packets).unwrap();
        assert_eq!(
            ports.outgoing,
            [(1, vec![0xb0, 0x51, 100]), (2, vec![0xb0, 0x01, 2])]
        );
        assert!(forward_usb_to_host(&mut ports, &packets[..3]).is_err());
    }
}
