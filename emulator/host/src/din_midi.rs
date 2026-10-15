//! The MIDI IN/OUT (DIN/TRS) jacks, bridged to a single named host MIDI port.
//!
//! Unlike the three USB MIDI cables (see [`crate::usb_midi`]), this is one
//! plain MIDI 1.0 byte stream over a UART, with no cable number and no USB
//! event-packet framing: bytes are forwarded as is, in both directions,
//! between the engine's inherited socket and the host port.
use crate::host_midi::{HostMidi, NativeMidi};
use std::{
    io::{self, Read, Write},
    marker::PhantomData,
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// The single port name; back ends append [`crate::host_midi::ENDPOINT_SUFFIX`].
pub const PORT_NAME: &str = "L6max MIDI In/Out Port";

/// Bytes the host port received become writes to the UART socket.
fn forward_host_to_uart<P: HostMidi>(
    ports: &mut P,
    mut write: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    for (port, bytes) in ports.read(32)? {
        if port == 0 {
            write(&bytes)?;
        }
    }
    Ok(())
}
/// Bytes read from the UART socket are delivered to the host port, as is.
fn forward_uart_to_host<P: HostMidi>(ports: &mut P, bytes: &[u8]) -> io::Result<()> {
    if !bytes.is_empty() {
        ports.write(0, bytes)?;
    }
    Ok(())
}

/// Worker that mirrors the device's DIN MIDI UART onto one host MIDI port.
/// On a platform without a back end it still drains the socket so the
/// guest's transmit side never blocks, and reports that through `status`.
pub struct DinMidiBridge<P: HostMidi = NativeMidi> {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<String>>,
    backend: PhantomData<fn() -> P>,
}
impl<P: HostMidi + 'static> DinMidiBridge<P> {
    /// `stream` is the host end of the pair whose other end is the guest's
    /// inherited `L6_MAIN_MIDI_UART_FD` socket.
    pub fn start(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("Connecting the MIDI IN/OUT port".to_owned()));
        let flag = stop.clone();
        let state = status.clone();
        let worker = thread::Builder::new()
            .name("l6-din-midi".into())
            .spawn(move || {
                if let Err(e) = Self::run(stream, &flag, &state) {
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    *state.lock().unwrap() = format!("MIDI IN/OUT port stopped: {e}");
                    eprintln!("DIN MIDI: {e}");
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
            status,
            backend: PhantomData,
        })
    }
    fn run(mut stream: UnixStream, stop: &AtomicBool, state: &Mutex<String>) -> io::Result<()> {
        let set = |text: String| *state.lock().unwrap() = text;
        let mut ports: Option<P> = None;
        match P::open(&[PORT_NAME]) {
            Ok(opened) => {
                ports = Some(opened);
                set("MIDI IN/OUT port connected".into());
            }
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                set(format!("Host MIDI unavailable: {e}"));
            }
            Err(e) => return Err(e),
        }
        let mut buffer = [0u8; 256];
        while !stop.load(Ordering::Acquire) {
            if let Some(native) = &mut ports {
                forward_host_to_uart(native, |chunk| stream.write_all(chunk))?;
            }
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(native) = &mut ports {
                        forward_uart_to_host(native, &buffer[..n])?;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            thread::sleep(Duration::from_millis(2));
        }
        drop(ports);
        Ok(())
    }
}
impl<P: HostMidi> Drop for DinMidiBridge<P> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
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
    fn host_bytes_become_uart_writes_on_port_zero_only() {
        let mut ports = Loopback::default();
        ports.incoming.push_back((0, vec![0x90, 0x40, 0x7f]));
        // A port this bridge does not own is ignored.
        ports.incoming.push_back((1, vec![0xb0, 1, 2]));
        let mut sent = Vec::new();
        forward_host_to_uart(&mut ports, |chunk| {
            sent.push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(sent, vec![vec![0x90, 0x40, 0x7f]]);
    }
    #[test]
    fn uart_bytes_reach_the_single_host_port_unframed() {
        let mut ports = Loopback::default();
        forward_uart_to_host(&mut ports, &[0x80, 0x40, 0x40]).unwrap();
        forward_uart_to_host(&mut ports, &[]).unwrap();
        assert_eq!(ports.outgoing, vec![(0, vec![0x80, 0x40, 0x40])]);
    }
}
