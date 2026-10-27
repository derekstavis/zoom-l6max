//! Host tokens for the emulated ChipIdea device, over an inherited socket.
//! Firmware owns descriptors, class handlers and endpoint buffers.
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
const MAGIC: u32 = 0x4c365553;
const MAX: usize = 16384;

/// Adds a description, except for the transient errors callers retry on
/// (ENODEV while the guest's USB controller is still coming up, EAGAIN on a
/// socket timeout): `io::Error::new` always clears `raw_os_error()` even
/// when passed the same `ErrorKind`, so wrapping one of these here would
/// make every retry-on-`raw_os_error()` check above this call stop matching
/// and treat a startup race as a fatal error instead of retrying it.
fn context(error: io::Error, operation: &str) -> io::Error {
    match error.raw_os_error() {
        Some(libc::ENODEV) | Some(libc::EAGAIN) => error,
        _ => io::Error::new(error.kind(), format!("{operation}: {error}")),
    }
}
pub struct UsbHost {
    socket: Mutex<UnixStream>,
    sequence: Mutex<u32>,
    control_lock: Mutex<()>,
    shutdown_socket: UnixStream,
}
impl UsbHost {
    pub fn new(socket: UnixStream) -> io::Result<Self> {
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        socket.set_write_timeout(Some(Duration::from_secs(3)))?;
        Ok(Self {
            shutdown_socket: socket.try_clone()?,
            socket: Mutex::new(socket),
            sequence: Mutex::new(0),
            control_lock: Mutex::new(()),
        })
    }
    pub fn shutdown(&self) {
        let _ = self.shutdown_socket.shutdown(Shutdown::Both);
    }
    fn request(&self, kind: u32, endpoint: u32, length: usize, data: &[u8]) -> io::Result<Vec<u8>> {
        if length > MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "USB transfer exceeds 16 KiB",
            ));
        }
        let mut sock = self.socket.lock().unwrap();
        let mut seq = self.sequence.lock().unwrap();
        *seq = seq.wrapping_add(1).max(1);
        let wire = (|| -> io::Result<(i32, Vec<u8>)> {
            for word in [MAGIC, kind, *seq, endpoint, length as u32] {
                sock.write_all(&word.to_le_bytes())?;
            }
            sock.write_all(data)?;
            let mut header = [0u8; 20];
            sock.read_exact(&mut header)?;
            let word = |i: usize| u32::from_le_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
            if word(0) != MAGIC
                || word(1) != 0x80000000
                || word(2) != *seq
                || word(4) as usize > MAX
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid USB reply",
                ));
            }
            let mut bytes = vec![0; word(4) as usize];
            sock.read_exact(&mut bytes)?;
            Ok((word(3) as i32, bytes))
        })();
        match wire {
            Ok((0, bytes)) => Ok(bytes),
            Ok((status, _)) => Err(io::Error::from_raw_os_error(-status)),
            Err(error) => {
                let _ = sock.shutdown(Shutdown::Both);
                Err(error)
            }
        }
    }
    pub fn connect(&self, connected: bool) -> io::Result<()> {
        self.request(1, connected as u32, 0, &[]).map(|_| ())
    }
    fn token(&self, kind: u32, endpoint: u8, length: usize, data: &[u8]) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.request(kind, endpoint as u32, length, data) {
                Err(e) if e.raw_os_error() == Some(libc::EAGAIN) && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(2))
                }
                result => return result,
            }
        }
    }
    pub fn receive(&self, endpoint: u8, length: usize) -> io::Result<Vec<u8>> {
        self.token(3, endpoint | 0x80, length, &[])
    }
    /// One IN transaction. An unprimed endpoint returns EAGAIN without waiting.
    pub fn try_receive(&self, endpoint: u8, length: usize) -> io::Result<Vec<u8>> {
        self.request(3, (endpoint | 0x80) as u32, length, &[])
    }
    pub fn send(&self, endpoint: u8, data: &[u8]) -> io::Result<()> {
        self.token(4, endpoint & 15, data.len(), data).map(|_| ())
    }
    pub fn control(&self, setup: [u8; 8]) -> io::Result<Vec<u8>> {
        let _control = self.control_lock.lock().unwrap();
        self.request(2, 0, 8, &setup)
            .map_err(|e| context(e, "USB SETUP packet"))?;
        self.control_data_stage(setup)
    }

    fn control_data_stage(&self, setup: [u8; 8]) -> io::Result<Vec<u8>> {
        let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
        if setup[0] & 0x80 != 0 {
            let data = self
                .receive(0, MAX)
                .map_err(|error| context(error, "USB control IN data stage"))?;
            if data.len() > length {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control response exceeds wLength",
                ));
            }
            self.send(0, &[])
                .map_err(|error| context(error, "USB control IN status stage"))?;
            Ok(data)
        } else if length == 0 {
            self.receive(0, MAX)
                .map_err(|error| context(error, "USB control OUT status stage"))
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "control OUT data stage",
            ))
        }
    }

    fn retry_control_token(
        &self,
        endpoint: u8,
        input: bool,
        data: &[u8],
        deadline: Instant,
    ) -> io::Result<Vec<u8>> {
        loop {
            let result = if input {
                self.try_receive(endpoint, MAX)
            } else {
                self.request(4, (endpoint & 15) as u32, data.len(), data)
            };
            match result {
                Err(error)
                    if error.raw_os_error() == Some(libc::EAGAIN) && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(2));
                }
                result => return result,
            }
        }
    }

    fn control_data_stage_raw(&self, setup: [u8; 8], deadline: Instant) -> io::Result<Vec<u8>> {
        let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
        if setup[0] & 0x80 != 0 {
            let data = self.retry_control_token(0, true, &[], deadline)?;
            if data.len() > length {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control response exceeds wLength",
                ));
            }
            self.retry_control_token(0, false, &[], deadline)?;
            Ok(data)
        } else if length == 0 {
            self.retry_control_token(0, true, &[], deadline)
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "control OUT data stage",
            ))
        }
    }

    fn control_once_raw(&self, setup: [u8; 8], deadline: Instant) -> io::Result<Vec<u8>> {
        self.request(2, 0, 8, &setup)?;
        self.control_data_stage_raw(setup, deadline)
    }

    fn control_with_retry(&self, setup: [u8; 8]) -> io::Result<Vec<u8>> {
        let _control = self.control_lock.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match self.control_once_raw(setup, deadline) {
                Err(error)
                    if matches!(
                        error.raw_os_error(),
                        Some(libc::ENODEV) | Some(libc::EAGAIN)
                    ) && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                result => return result,
            }
        }
    }

    pub fn enumerate(&self) -> io::Result<Configuration> {
        self.enumerate_with_descriptors()
            .map(|(_, _, config)| config)
    }

    /// Enumerate through the production retry path, retaining wire evidence.
    pub fn enumerate_with_descriptors(&self) -> io::Result<(Vec<u8>, Vec<u8>, Configuration)> {
        self.connect(true)
            .map_err(|e| context(e, "connect USB device"))?;
        // Give the guest USB task time to handle bus reset before SETUP.
        thread::sleep(Duration::from_millis(500));
        let device = self.device_descriptor()?;
        if device.len() != 18 || device[..2] != [18, 1] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid device descriptor {device:02x?}"),
            ));
        }
        let bytes = self
            .control_with_retry([0x80, 6, 0, 2, 0, 0, 0xff, 0x3f])
            .map_err(|e| context(e, "get USB configuration descriptor"))?;
        let config = Configuration::parse(&bytes)?;
        self.control_with_retry([0, 9, config.value, 0, 0, 0, 0, 0])
            .map_err(|e| context(e, "set USB configuration"))?;
        Ok((device, bytes, config))
    }

    fn device_descriptor(&self) -> io::Result<Vec<u8>> {
        let setup = [0x80, 6, 0, 1, 0, 0, 18, 0];
        let deadline = Instant::now() + Duration::from_secs(30);
        let _control = self.control_lock.lock().unwrap();
        loop {
            match self.request(2, 0, 8, &setup) {
                Ok(_) => {
                    match self.control_data_stage_raw(setup, deadline) {
                        Ok(descriptor) if descriptor.len() == 18 => return Ok(descriptor),
                        Ok(descriptor) if descriptor.len() < 18 && Instant::now() < deadline => {
                            // Some firmware revisions prime EP0 with a short
                            // initial descriptor fragment while USB startup is
                            // settling. Restart the control transfer and wait
                            // for the complete fixed-size device descriptor.
                            thread::sleep(Duration::from_millis(25));
                        }
                        Ok(descriptor) => {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                format!("short USB device descriptor: {} bytes", descriptor.len()),
                            ));
                        }
                        Err(error)
                            if matches!(
                                error.raw_os_error(),
                                Some(libc::ENODEV) | Some(libc::EAGAIN)
                            ) && Instant::now() < deadline =>
                        {
                            // The setup packet can be accepted before the
                            // firmware primes EP0's IN dTD. Retry the whole
                            // control transfer until the device controller is
                            // ready, as well as retrying a rejected SETUP.
                            thread::sleep(Duration::from_millis(25));
                        }
                        Err(error) => {
                            return Err(context(error, "get USB device descriptor"));
                        }
                    }
                }
                Err(error)
                    if matches!(
                        error.raw_os_error(),
                        Some(libc::ENODEV) | Some(libc::EAGAIN)
                    ) && Instant::now() < deadline =>
                {
                    // The controller can reject SETUP until the firmware has
                    // enabled its device role after VBUS.
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(context(error, "get USB device descriptor")),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoints {
    pub interface: u8,
    pub input: u8,
    pub output: u8,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub midi: Option<Endpoints>,
    pub storage: Option<Endpoints>,
}
impl Configuration {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "malformed USB configuration");
        if bytes.len() < 9
            || bytes[..2] != [9, 2]
            || u16::from_le_bytes([bytes[2], bytes[3]]) as usize != bytes.len()
        {
            return Err(bad());
        }
        let mut result = Self {
            value: bytes[5],
            midi: None,
            storage: None,
        };
        let mut class = 0;
        let mut pair = Endpoints {
            interface: 0,
            input: 0,
            output: 0,
        };
        let mut offset = 0;
        while offset < bytes.len() {
            let length = bytes[offset] as usize;
            if length < 2 || offset + length > bytes.len() {
                return Err(bad());
            }
            let d = &bytes[offset..offset + length];
            if d[1] == 4 {
                if length < 9 {
                    return Err(bad());
                }
                class = if d[3] != 0 {
                    0
                } else if d[5..7] == [1, 3] {
                    1
                } else if d[5..8] == [8, 6, 0x50] {
                    2
                } else {
                    0
                };
                pair = Endpoints {
                    interface: d[2],
                    input: 0,
                    output: 0,
                };
            } else if d[1] == 5 && class != 0 {
                if length < 7 {
                    return Err(bad());
                }
                if d[3] & 3 == 2 {
                    if d[2] & 0x80 != 0 {
                        pair.input = d[2] & 15;
                    } else {
                        pair.output = d[2] & 15;
                    }
                    if pair.input != 0 && pair.output != 0 {
                        if class == 1 {
                            result.midi = Some(pair);
                        } else {
                            result.storage = Some(pair);
                        }
                    }
                }
            }
            offset += length;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configuration() -> Vec<u8> {
        let mut d = vec![9, 2, 0, 0, 2, 1, 0, 0xc0, 4];
        d.extend([
            9, 4, 4, 0, 2, 1, 3, 0, 0, 7, 5, 3, 2, 0, 2, 0, 7, 5, 0x84, 2, 0, 2, 0,
        ]);
        d.extend([
            9, 4, 5, 0, 2, 8, 6, 0x50, 0, 7, 5, 0x85, 2, 0, 2, 0, 7, 5, 6, 2, 0, 2, 0,
        ]);
        let length = d.len() as u16;
        d[2..4].copy_from_slice(&length.to_le_bytes());
        d
    }
    #[test]
    fn endpoints_are_read_from_the_firmware_configuration() {
        let c = Configuration::parse(&configuration()).unwrap();
        assert_eq!(
            c.midi,
            Some(Endpoints {
                interface: 4,
                input: 4,
                output: 3
            })
        );
        assert_eq!(
            c.storage,
            Some(Endpoints {
                interface: 5,
                input: 5,
                output: 6
            })
        );
        let mut bad = configuration();
        bad[9] = 0;
        assert!(Configuration::parse(&bad).is_err());
        assert!(Configuration::parse(&configuration()[..12]).is_err());
    }
    #[test]
    fn device_descriptor_waits_for_guest_usb_controller_enable() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            for status in [-libc::ENODEV, 0] {
                let mut request = [0; 28];
                peer.read_exact(&mut request).unwrap();
                assert_eq!(u32::from_le_bytes(request[4..8].try_into().unwrap()), 2);
                assert_eq!(&request[20..], &[0x80, 6, 0, 1, 0, 0, 18, 0]);
                let sequence = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let mut reply = Vec::new();
                for word in [MAGIC, 0x80000000, sequence, status as u32, 0] {
                    reply.extend(word.to_le_bytes());
                }
                peer.write_all(&reply).unwrap();
            }
            for (kind, endpoint, data) in [(3, 0x80, vec![0; 18]), (4, 0, vec![])] {
                let mut request = [0; 20];
                peer.read_exact(&mut request).unwrap();
                assert_eq!(u32::from_le_bytes(request[4..8].try_into().unwrap()), kind);
                assert_eq!(
                    u32::from_le_bytes(request[12..16].try_into().unwrap()),
                    endpoint
                );
                let sequence = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let mut reply = Vec::new();
                for word in [MAGIC, 0x80000000, sequence, 0, data.len() as u32] {
                    reply.extend(word.to_le_bytes());
                }
                reply.extend(data);
                peer.write_all(&reply).unwrap();
            }
        });
        let usb = UsbHost::new(socket).unwrap();
        assert_eq!(usb.device_descriptor().unwrap(), [0; 18]);
        server.join().unwrap();
    }
    #[test]
    fn device_descriptor_retries_when_ep0_is_not_primed_after_setup() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let mut receive_reply = |kind: u32, endpoint: u32, status: i32, data: &[u8]| {
                let mut request = [0; 28];
                let request_len = if kind == 2 { 28 } else { 20 };
                peer.read_exact(&mut request[..request_len]).unwrap();
                assert_eq!(u32::from_le_bytes(request[4..8].try_into().unwrap()), kind);
                assert_eq!(
                    u32::from_le_bytes(request[12..16].try_into().unwrap()),
                    endpoint
                );
                let sequence = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let mut reply = Vec::new();
                for word in [
                    MAGIC,
                    0x80000000,
                    sequence,
                    status as u32,
                    data.len() as u32,
                ] {
                    reply.extend(word.to_le_bytes());
                }
                reply.extend(data);
                peer.write_all(&reply).unwrap();
            };

            // SETUP succeeds while the device is still enabling its USB
            // controller, so the following EP0 IN transaction returns ENODEV.
            receive_reply(2, 0, 0, &[]);
            receive_reply(3, 0x80, -libc::ENODEV, &[]);
            // The first post-startup attempt can return only the two-byte
            // descriptor header; retry the full control transfer once more.
            receive_reply(2, 0, 0, &[]);
            receive_reply(3, 0x80, 0, &[18, 1]);
            receive_reply(4, 0, 0, &[]);
            // The next SETUP receives the complete device descriptor.
            receive_reply(2, 0, 0, &[]);
            receive_reply(
                3,
                0x80,
                0,
                &[18, 1, 0, 2, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            );
            receive_reply(4, 0, 0, &[]);
        });
        let usb = UsbHost::new(socket).unwrap();
        assert_eq!(
            usb.device_descriptor().unwrap(),
            [18, 1, 0, 2, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
        );
        server.join().unwrap();
    }
    #[test]
    fn enumeration_control_retries_transient_status_stage_eagain() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            for (kind, status) in [(2, 0), (3, -libc::EAGAIN), (3, 0)] {
                let request_len = if kind == 2 { 28 } else { 20 };
                let mut request = vec![0; request_len];
                peer.read_exact(&mut request).unwrap();
                assert_eq!(u32::from_le_bytes(request[4..8].try_into().unwrap()), kind);
                if kind == 3 {
                    assert_eq!(
                        u32::from_le_bytes(request[12..16].try_into().unwrap()),
                        0x80
                    );
                }
                let sequence = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let mut reply = Vec::new();
                for word in [MAGIC, 0x80000000, sequence, status as u32, 0] {
                    reply.extend(word.to_le_bytes());
                }
                peer.write_all(&reply).unwrap();
            }
        });
        let usb = UsbHost::new(socket).unwrap();
        assert!(
            usb.control_with_retry([0, 9, 1, 0, 0, 0, 0, 0])
                .unwrap()
                .is_empty()
        );
        server.join().unwrap();
    }
    #[test]
    fn fragmented_replies_and_usb_errors_preserve_stream_order() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            for (status, data) in [(-libc::EAGAIN, vec![]), (0, vec![0x1b, 0xb0, 81, 32])] {
                let mut request = [0; 20];
                peer.read_exact(&mut request).unwrap();
                assert_eq!(u32::from_le_bytes(request[4..8].try_into().unwrap()), 3);
                let sequence = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let mut reply = Vec::new();
                for word in [
                    MAGIC,
                    0x80000000,
                    sequence,
                    status as u32,
                    data.len() as u32,
                ] {
                    reply.extend(word.to_le_bytes());
                }
                reply.extend(data);
                for fragment in reply.chunks(3) {
                    peer.write_all(fragment).unwrap();
                }
            }
        });
        let usb = UsbHost::new(socket).unwrap();
        assert_eq!(
            usb.try_receive(4, 512).unwrap_err().raw_os_error(),
            Some(libc::EAGAIN)
        );
        assert_eq!(usb.try_receive(4, 512).unwrap(), [0x1b, 0xb0, 81, 32]);
        server.join().unwrap();
    }
    #[test]
    fn corrupt_reply_closes_the_channel() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let mut request = [0; 20];
            peer.read_exact(&mut request).unwrap();
            peer.write_all(&[0; 20]).unwrap();
            let mut b = [0];
            assert_eq!(peer.read(&mut b).unwrap(), 0);
        });
        let usb = UsbHost::new(socket).unwrap();
        assert_eq!(
            usb.connect(true).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        server.join().unwrap();
    }
}
