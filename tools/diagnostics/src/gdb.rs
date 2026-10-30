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
        self.receive_packet()
    }
    fn receive_packet(&mut self) -> io::Result<String> {
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
    /// Interrupt a running target and return its GDB stop reply.
    pub fn interrupt(&mut self) -> io::Result<String> {
        self.0.write_all(&[3])?;
        self.receive_packet()
    }
    /// Resume the selected target until a breakpoint/exception, allowing a
    /// longer wait for firmware boot or a deliberately injected call.
    pub fn continue_until_stop(&mut self, timeout: Duration) -> io::Result<String> {
        self.0.set_read_timeout(Some(timeout))?;
        let result = self.request("c");
        self.0.set_read_timeout(Some(Duration::from_secs(5)))?;
        result
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

    pub fn read(&mut self, address: u32, length: usize) -> io::Result<Vec<u8>> {
        let response = self.request(&format!("m{address:x},{length:x}"))?;
        if response.len() != length * 2 {
            return Err(io::Error::other(format!(
                "GDB read returned {} bytes, expected {length}",
                response.len() / 2
            )));
        }
        (0..length)
            .map(|index| {
                u8::from_str_radix(&response[index * 2..index * 2 + 2], 16)
                    .map_err(io::Error::other)
            })
            .collect()
    }

    /// Call a Thumb routine on the selected QEMU CPU, using a temporary
    /// return trap just below the saved stack pointer. The target must be
    /// stopped by the caller before invoking this method.
    pub fn call_thumb(
        &mut self,
        address: u32,
        arguments: &[u32],
        timeout: Duration,
    ) -> io::Result<String> {
        if address & 1 != 0 || !(1..=5).contains(&arguments.len()) {
            return Err(io::Error::other(
                "Thumb call needs an even address and 1..5 arguments",
            ));
        }
        let saved = (0..16)
            .map(|index| self.read_register(index))
            .collect::<io::Result<Vec<_>>>()
            .map_err(|error| io::Error::other(format!("read caller registers: {error}")))?;
        let old_sp = saved[13];
        let in_internal_ram = (0x2000_0100..0x2030_0000).contains(&old_sp);
        let in_external_ram = (0x8020_0000..0x8420_0000).contains(&old_sp);
        if !in_internal_ram && !in_external_ram {
            return Err(io::Error::other(format!(
                "unexpected main RAM stack pointer {old_sp:#010x}"
            )));
        }
        let return_address = old_sp - 2;
        self.breakpoint(return_address, true).map_err(|error| {
            io::Error::other(format!("set firmware return breakpoint: {error}"))
        })?;
        let call_sp = (old_sp - 32) & !7;
        let result = (|| {
            if let Some(argument) = arguments.get(4) {
                self.write(call_sp, &argument.to_le_bytes())?;
            }
            self.register(13, call_sp)?;
            for index in 0..4 {
                self.register(index as u32, arguments.get(index).copied().unwrap_or(0))?;
            }
            self.register(14, return_address | 1)?;
            self.register(15, address | 1)?;
            self.continue_until_stop(timeout).map_err(|error| {
                io::Error::other(format!("wait for firmware return breakpoint: {error}"))
            })
        })();

        // The caller stops QEMU after the trap and before restoring this
        // context, so the scratch instruction cannot race the guest.
        let stopped_pc = if result.is_ok() {
            self.read_register(15).ok()
        } else {
            None
        };
        let restore_result = (|| {
            self.breakpoint(return_address, false)?;
            for index in 0..13 {
                self.register(index, saved[index as usize])?;
            }
            self.register(13, saved[13])?;
            self.register(14, saved[14])?;
            self.register(15, saved[15] | 1)?;
            Ok::<(), io::Error>(())
        })();
        let stop = result?;
        restore_result?;
        if let Some(pc) = stopped_pc {
            if pc < return_address || pc > return_address + 4 {
                return Err(io::Error::other(format!(
                    "firmware routine stopped at {pc:#010x}, outside its return trap"
                )));
            }
        }
        Ok(stop)
    }
}
