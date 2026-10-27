//! Native host MIDI clients used by scenarios that exercise emulator endpoints.
use std::time::Duration;

pub trait Client {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn poll(&mut self) -> Result<Option<[u8; 3]>, String>;
    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() == 3 && bytes[0] & 0xf0 == 0xb0 {
            self.send(bytes)
        } else {
            Err("host MIDI backend does not support raw MIDI byte streams".into())
        }
    }
    fn poll_bytes(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.poll()
            .map(|message| message.map(|message| message.to_vec()))
    }
    fn release(&mut self) {}
    fn endpoint_present(&mut self) -> Option<bool> {
        None
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Client;
    use coremidi::{
        Client as CoreMidi, Destination, Destinations, InputPort, OutputPort, PacketBuffer, Source,
    };
    use std::sync::mpsc::{self, Receiver};

    pub struct NativeClient {
        _client: CoreMidi,
        input: InputPort,
        output: OutputPort,
        source: Source,
        destination: Destination,
        received: Receiver<Vec<u8>>,
        endpoint: String,
    }

    fn status(value: i32) -> String {
        format!("CoreMIDI {value}")
    }

    fn destination_present(name: &str) -> bool {
        Destinations
            .into_iter()
            .any(|destination| destination.name().as_deref() == Some(name))
    }

    impl NativeClient {
        pub fn connect(name: &str) -> Result<Self, String> {
            let source = Source::from_name(name).ok_or("missing native MIDI source")?;
            let destination = Destinations
                .into_iter()
                .find(|candidate| candidate.name().as_deref() == Some(name))
                .ok_or("missing native MIDI destination")?;
            let client = CoreMidi::new("L6 firmware test driver").map_err(status)?;
            let (tx, received) = mpsc::channel();
            #[allow(deprecated)]
            let input = client
                .input_port("smoke input", move |packets| {
                    for packet in packets.iter() {
                        let _ = tx.send(packet.data().to_vec());
                    }
                })
                .map_err(status)?;
            input.connect_source(&source).map_err(status)?;
            let output = client.output_port("smoke output").map_err(status)?;
            Ok(Self {
                _client: client,
                input,
                output,
                source,
                destination,
                received,
                endpoint: name.to_owned(),
            })
        }
    }

    impl Client for NativeClient {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.output
                .send(&self.destination, &PacketBuffer::new(0, bytes))
                .map_err(status)
        }
        fn poll(&mut self) -> Result<Option<[u8; 3]>, String> {
            while let Ok(bytes) = self.received.try_recv() {
                if let [status @ 0xb0..=0xbf, first, second] = bytes[..] {
                    return Ok(Some([status, first, second]));
                }
            }
            Ok(None)
        }
        fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.output
                .send(&self.destination, &PacketBuffer::new(0, bytes))
                .map_err(status)
        }
        fn poll_bytes(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.received.try_recv().ok())
        }
        fn release(&mut self) {
            let _ = self.input.disconnect_source(&self.source);
        }
        fn endpoint_present(&mut self) -> Option<bool> {
            Some(Source::from_name(&self.endpoint).is_some() || destination_present(&self.endpoint))
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Client;
    use alsa::seq::{
        Addr, ClientIter, EvCtrl, Event, EventType, PortCap, PortIter, PortSubscribe, PortType, Seq,
    };
    use std::ffi::CString;

    pub struct NativeClient {
        seq: Seq,
        mine: Addr,
        target: Addr,
        endpoint: String,
        decoder: alsa::seq::MidiEvent,
    }

    fn status(error: alsa::Error) -> String {
        format!("ALSA {error}")
    }

    fn find_port(seq: &Seq, name: &str) -> Option<Addr> {
        ClientIter::new(seq).find_map(|client| {
            PortIter::new(seq, client.get_client()).find_map(|port| {
                (port.get_name().ok().as_deref() == Some(name)).then(|| port.addr())
            })
        })
    }

    impl NativeClient {
        pub fn connect(name: &str) -> Result<Self, String> {
            let seq = Seq::open(None, None, true).map_err(status)?;
            seq.set_client_name(&CString::new("L6 firmware test driver").unwrap())
                .map_err(status)?;
            let port = seq
                .create_simple_port(
                    &CString::new("smoke port").unwrap(),
                    PortCap::READ | PortCap::WRITE | PortCap::SUBS_READ | PortCap::SUBS_WRITE,
                    PortType::MIDI_GENERIC | PortType::APPLICATION,
                )
                .map_err(status)?;
            let mine = Addr {
                client: seq.client_id().map_err(status)?,
                port,
            };
            let target = find_port(&seq, name).ok_or("missing native MIDI port")?;
            for (sender, destination) in [(target, mine), (mine, target)] {
                let subscription = PortSubscribe::empty().map_err(status)?;
                subscription.set_sender(sender);
                subscription.set_dest(destination);
                seq.subscribe_port(&subscription).map_err(status)?;
            }
            Ok(Self {
                seq,
                mine,
                target,
                endpoint: name.to_owned(),
                decoder: alsa::seq::MidiEvent::new(16_384).map_err(status)?,
            })
        }
    }

    impl Client for NativeClient {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            if let [status_byte, parameter, value] = *bytes {
                if status_byte & 0xf0 != 0xb0 {
                    return Err("ALSA host MIDI client supports Control Change messages".into());
                }
                let mut event = Event::new(
                    EventType::Controller,
                    &EvCtrl {
                        channel: status_byte & 0x0f,
                        param: u32::from(parameter),
                        value: i32::from(value),
                    },
                );
                event.set_source(self.mine.port);
                event.set_dest(self.target);
                event.set_direct();
                self.seq.event_output_direct(&mut event).map_err(status)?;
                return Ok(());
            }
            Err("ALSA host MIDI client supports 3-byte Control Change messages".into())
        }
        fn poll(&mut self) -> Result<Option<[u8; 3]>, String> {
            let mut input = self.seq.input();
            while let Ok(event) = input.event_input() {
                if let Some(control) = event.get_data::<EvCtrl>() {
                    return Ok(Some([
                        0xb0 | control.channel,
                        control.param as u8,
                        control.value as u8,
                    ]));
                }
            }
            Ok(None)
        }
        fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
            let mut remaining = bytes;
            let mut encoder = alsa::seq::MidiEvent::new(16_384).map_err(status)?;
            while !remaining.is_empty() {
                let (used, event) = encoder.encode(remaining).map_err(status)?;
                if let Some(mut event) = event {
                    event.set_source(self.mine.port);
                    event.set_dest(self.target);
                    event.set_direct();
                    self.seq.event_output_direct(&mut event).map_err(status)?;
                }
                if used == 0 {
                    return Err("ALSA MIDI encoder made no progress".into());
                }
                remaining = &remaining[used..];
            }
            Ok(())
        }
        fn poll_bytes(&mut self) -> Result<Option<Vec<u8>>, String> {
            let mut input = self.seq.input();
            let mut event = match input.event_input() {
                Ok(event) => event,
                Err(error) if error.errno() == libc::EAGAIN => return Ok(None),
                Err(error) => return Err(status(error)),
            };
            let mut bytes = vec![0; 16_384];
            let length = self
                .decoder
                .decode(&mut bytes, &mut event)
                .map_err(status)?;
            bytes.truncate(length);
            Ok((!bytes.is_empty()).then_some(bytes))
        }
        fn endpoint_present(&mut self) -> Option<bool> {
            Some(find_port(&self.seq, &self.endpoint).is_some())
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::Client;

    pub struct NativeClient;
    impl NativeClient {
        pub fn connect(_name: &str) -> Result<Self, String> {
            Err("host MIDI smoke requires CoreMIDI or ALSA".into())
        }
    }
    impl Client for NativeClient {
        fn send(&mut self, _bytes: &[u8]) -> Result<(), String> {
            Err("host MIDI smoke requires CoreMIDI or ALSA".into())
        }
        fn poll(&mut self) -> Result<Option<[u8; 3]>, String> {
            Err("host MIDI smoke requires CoreMIDI or ALSA".into())
        }
    }
}

pub fn connect(name: &str) -> Result<Box<dyn Client>, String> {
    Ok(Box::new(platform::NativeClient::connect(name)?))
}

pub struct DirectUsbClient {
    usb: std::sync::Arc<l6max_host::usb::UsbHost>,
    endpoints: l6max_host::usb::Endpoints,
    encoder: l6max_host::usb_midi::Encoder,
    pending: std::collections::VecDeque<[u8; 3]>,
    cable: u8,
}

impl DirectUsbClient {
    pub fn connect(
        usb: std::sync::Arc<l6max_host::usb::UsbHost>,
        cable: u8,
    ) -> Result<Self, String> {
        usb.connect(true).map_err(|error| error.to_string())?;
        let endpoints = usb
            .enumerate()
            .map_err(|error| error.to_string())?
            .midi
            .ok_or("USB device has no MIDI interface")?;
        Ok(Self {
            usb,
            endpoints,
            encoder: l6max_host::usb_midi::Encoder::default(),
            pending: std::collections::VecDeque::new(),
            cable,
        })
    }
}

impl Client for DirectUsbClient {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let wire = self.encoder.push(self.cable, bytes);
        self.usb
            .send(self.endpoints.output, &wire)
            .map_err(|error| error.to_string())
    }

    fn poll(&mut self) -> Result<Option<[u8; 3]>, String> {
        if self.pending.is_empty() {
            match self.usb.try_receive(self.endpoints.input, 16_384) {
                Ok(bytes) => {
                    for (cable, data) in
                        l6max_host::usb_midi::decode(&bytes).map_err(|error| error.to_string())?
                    {
                        if cable == 1 && data.len() == 3 && data[0] & 0xf0 == 0xb0 {
                            self.pending.push_back([data[0], data[1], data[2]]);
                        }
                    }
                }
                Err(error) if error.raw_os_error() == Some(libc::EAGAIN) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(self.pending.pop_front())
    }
}

pub fn wait_for(
    client: &mut dyn Client,
    timeout: Duration,
    mut idle: impl FnMut() -> Result<(), String>,
    mut accept: impl FnMut([u8; 3]) -> bool,
) -> Result<bool, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        idle()?;
        if let Some(message) = client.poll()? {
            if accept(message) {
                return Ok(true);
            }
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(false)
}
