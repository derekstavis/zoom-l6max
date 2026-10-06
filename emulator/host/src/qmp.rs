//! Typed QMP management over an inherited socket, using the upstream QAPI crate.
//! Device input and display events use the separate runtime protocol.
use qapi::{Command, Qmp, Stream, qmp};
use std::{
    io::{self, BufRead, BufReader, Read},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::Mutex,
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    pub status: String,
}

/// Apply a deadline to the entire request, including intervening QMP events.
/// A socket read timeout alone would restart the timeout after every event.
struct DeadlineReader {
    socket: UnixStream,
    deadline: Instant,
}

impl Read for DeadlineReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "QMP request deadline exceeded")
            })?;
        self.socket.set_read_timeout(Some(remaining))?;
        self.socket.read(bytes)
    }
}

type Management = Qmp<Stream<BufReader<DeadlineReader>, UnixStream>>;

pub struct QmpClient {
    management: Mutex<Management>,
}

impl QmpClient {
    /// The other end of `stream` is inherited by QEMU's socket chardev. Requests
    /// are serialized because the synchronous QAPI client consumes replies in
    /// order; QAPI queues asynchronous events encountered before each reply.
    pub fn connect(stream: UnixStream) -> io::Result<Self> {
        stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
        let reader = DeadlineReader {
            socket: stream.try_clone()?,
            deadline: Instant::now() + REQUEST_TIMEOUT,
        };
        let mut management = Qmp::new(Stream::new(BufReader::new(reader), stream));
        // qapi::read_capabilities currently panics on greeting EOF. Avoid that
        // by checking for an initial byte without consuming the greeting.
        if management.inner_mut().get_mut_read().fill_buf()?.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "QMP disconnected before greeting",
            ));
        }
        management.handshake().map_err(io::Error::from)?;
        Ok(Self {
            management: Mutex::new(management),
        })
    }

    fn execute<C: Command>(&self, command: &C) -> io::Result<C::Ok> {
        let mut management = self.management.lock().unwrap();
        management.inner_mut().get_mut_read().get_mut().deadline = Instant::now() + REQUEST_TIMEOUT;
        let result = management.execute(command);
        if matches!(result, Err(qapi::ExecuteError::Io(_))) {
            // A timed out response might arrive later. Closing prevents a later
            // request from accidentally consuming that response as its own.
            let _ = management.inner().get_ref_write().shutdown(Shutdown::Both);
        }
        result.map_err(io::Error::from)
    }

    pub fn query_status(&self) -> io::Result<Status> {
        let result = self.execute(&qmp::query_status {})?;
        let status = serde_json::to_value(result.status)?;
        Ok(Status {
            running: result.running,
            status: status
                .as_str()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "QMP status is not a name")
                })?
                .to_owned(),
        })
    }

    pub fn stop(&self) -> io::Result<()> {
        self.execute(&qmp::stop {}).map(|_| ())
    }

    pub fn resume(&self) -> io::Result<()> {
        self.execute(&qmp::cont {}).map(|_| ())
    }

    pub fn quit(&self) -> io::Result<()> {
        self.execute(&qmp::quit {}).map(|_| ())
    }
    /// Remove the attached SD medium through QEMU's block layer and SDBus.
    #[allow(deprecated)]
    pub fn eject_sd(&self) -> io::Result<()> {
        self.execute(&qmp::eject {
            device: Some("l6-sd".into()),
            id: None,
            force: Some(true),
        })
        .map(|_| ())
    }
    #[allow(deprecated)]
    pub fn insert_sd(&self, image: &std::path::Path) -> io::Result<()> {
        let filename = image
            .canonicalize()?
            .to_str()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "SD image path is not UTF-8")
            })?
            .to_owned();
        self.execute(&qmp::blockdev_change_medium {
            device: Some("l6-sd".into()),
            id: None,
            filename,
            format: Some("raw".into()),
            force: Some(true),
            read_only_mode: None,
        })
        .map(|_| ())
    }

    /// Diagnostic register snapshot from one CPU, using QAPI's HMP bridge.
    pub fn registers(&self, cpu: i64) -> io::Result<String> {
        self.execute(&qmp::human_monitor_command {
            command_line: "info registers".into(),
            cpu_index: Some(cpu),
        })
    }

    /// Read a bounded physical-memory region for firmware diagnostics.
    pub fn read_memory(&self, cpu: i64, address: u32, size: usize) -> io::Result<Vec<u8>> {
        if size == 0 || size > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "memory diagnostic size must be 1..4096",
            ));
        }
        let reply = self.execute(&qmp::human_monitor_command {
            command_line: format!("xp /{size}bx 0x{address:x}"),
            cpu_index: Some(cpu),
        })?;
        let bytes = reply
            .lines()
            .filter_map(|line| line.split_once(':').map(|(_, data)| data))
            .flat_map(str::split_whitespace)
            .map(|word| u8::from_str_radix(word.trim_start_matches("0x"), 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() != size {
            return Err(io::Error::new(io::ErrorKind::InvalidData, reply));
        }
        Ok(bytes)
    }

    /// Querying status also pumps pending asynchronous events through QAPI.
    /// Call at the supervisor's health tick; display events use runtime IPC.
    pub fn poll_events(&self) -> io::Result<Vec<qmp::Event>> {
        self.query_status()?;
        Ok(self.drain_events())
    }

    pub fn drain_events(&self) -> Vec<qmp::Event> {
        self.management.lock().unwrap().events().collect()
    }
}

impl Drop for QmpClient {
    fn drop(&mut self) {
        if let Ok(management) = self.management.lock() {
            let _ = management.inner().get_ref_write().shutdown(Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, thread};

    fn greeting(stream: &mut UnixStream) {
        stream.write_all(b"{\"QMP\":{\"version\":{\"qemu\":{\"major\":10,\"minor\":0,\"micro\":0},\"package\":\"\"},\"capabilities\":[]}}\r\n").unwrap();
    }

    fn expect_command(reader: &mut BufReader<UnixStream>, name: &str) {
        let mut command = String::new();
        reader.read_line(&mut command).unwrap();
        let command: serde_json::Value = serde_json::from_str(&command).unwrap();
        assert_eq!(command["execute"], name);
    }

    #[test]
    fn typed_management_preserves_events_interleaved_with_replies() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(peer.try_clone().unwrap());
            greeting(&mut peer);
            expect_command(&mut reader, "qmp_capabilities");
            peer.write_all(b"{\"return\":{}}\r\n").unwrap();
            expect_command(&mut reader, "stop");
            peer.write_all(b"{\"event\":\"STOP\",\"timestamp\":{\"seconds\":1,\"microseconds\":2}}\r\n{\"return\":{}}\r\n").unwrap();
            expect_command(&mut reader, "query-status");
            peer.write_all(
                b"{\"return\":{\"running\":false,\"singlestep\":false,\"status\":\"paused\"}}\r\n",
            )
            .unwrap();
            expect_command(&mut reader, "cont");
            peer.write_all(b"{\"event\":\"RESUME\",\"timestamp\":{\"seconds\":1,\"microseconds\":3}}\r\n{\"return\":{}}\r\n").unwrap();
            expect_command(&mut reader, "quit");
            peer.write_all(b"{\"return\":{}}\r\n").unwrap();
        });
        let client = QmpClient::connect(client).unwrap();
        client.stop().unwrap();
        assert_eq!(
            client.query_status().unwrap(),
            Status {
                running: false,
                status: "paused".into()
            }
        );
        let events = client.drain_events();
        assert_eq!(events.len(), 1);
        assert_eq!(serde_json::to_value(&events[0]).unwrap()["event"], "STOP");
        client.resume().unwrap();
        assert_eq!(client.drain_events().len(), 1);
        client.quit().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn greeting_eof_is_an_error_instead_of_qapi_panic() {
        let (client, peer) = UnixStream::pair().unwrap();
        drop(peer);
        // macOS may report ConnectionReset before a read reaches EOF.
        assert!(QmpClient::connect(client).is_err());
    }
}
