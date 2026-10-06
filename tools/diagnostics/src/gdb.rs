//! Minimal GDB remote client for all-stop QEMU firmware diagnostics.
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};
pub struct Gdb(UnixStream);
impl Gdb {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let socket = UnixStream::connect(path)?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut client = Self(socket);
        client.request("Hg1")?; // Main MCU, not panel CPU.
        Ok(client)
    }
    pub fn request(&mut self, command: &str) -> io::Result<String> {
        let checksum = command.bytes().fold(0u8, u8::wrapping_add);
        self.0
            .write_all(format!("${command}#{checksum:02x}").as_bytes())?;
        let mut byte = [0];
        loop {
            self.0.read_exact(&mut byte)?;
            if byte[0] == b'$' {
                break;
            }
        }
        let mut encoded = Vec::new();
        loop {
            self.0.read_exact(&mut byte)?;
            if byte[0] == b'#' {
                break;
            }
            encoded.push(byte[0]);
        }
        let mut checksum_bytes = [0; 2];
        self.0.read_exact(&mut checksum_bytes)?;
        let expected = u8::from_str_radix(std::str::from_utf8(&checksum_bytes).unwrap(), 16)
            .map_err(io::Error::other)?;
        if encoded.iter().copied().fold(0u8, u8::wrapping_add) != expected {
            return Err(io::Error::other("GDB checksum mismatch"));
        }
        self.0.write_all(b"+")?;
        let mut data = Vec::new();
        let mut i = 0;
        while i < encoded.len() {
            match encoded[i] {
                b'}' => {
                    i += 1;
                    data.push(encoded[i] ^ 0x20);
                }
                b'*' => {
                    i += 1;
                    let previous = *data
                        .last()
                        .ok_or_else(|| io::Error::other("bad GDB repeat"))?;
                    data.extend(std::iter::repeat_n(previous, (encoded[i] - 29) as usize));
                }
                b => data.push(b),
            }
            i += 1;
        }
        String::from_utf8(data).map_err(io::Error::other)
    }
    fn ok(&mut self, command: &str) -> io::Result<()> {
        let response = self.request(command)?;
        if response != "OK" {
            return Err(io::Error::other(format!("{command}: {response}")));
        }
        Ok(())
    }
    pub fn breakpoint(&mut self, address: u32, enable: bool) -> io::Result<()> {
        self.ok(&format!(
            "{}0,{address:x},2",
            if enable { "Z" } else { "z" }
        ))
    }
    pub fn register(&mut self, index: u32, value: u32) -> io::Result<()> {
        let hex = value
            .to_le_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        self.ok(&format!("P{index:x}={hex}"))
    }
    pub fn read_register(&mut self, index: u32) -> io::Result<u32> {
        let value = self.request(&format!("p{index:x}"))?;
        if value.len() != 8 {
            return Err(io::Error::other(format!(
                "unexpected ARM register: {value}"
            )));
        }
        let mut bytes = [0; 4];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&value[2 * i..2 * i + 2], 16).map_err(io::Error::other)?;
        }
        Ok(u32::from_le_bytes(bytes))
    }
    pub fn write(&mut self, address: u32, data: &[u8]) -> io::Result<()> {
        let hex = data.iter().map(|b| format!("{b:02x}")).collect::<String>();
        self.ok(&format!("M{address:x},{:x}:{hex}", data.len()))
    }
}
