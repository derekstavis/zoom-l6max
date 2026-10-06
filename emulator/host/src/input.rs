//! Socket input transport. QEMU owns the returned engine endpoint; the UI owns
//! EngineInput. Dropping the final handle shuts down its reply reader.
use crate::protocol::{self, Message};
use std::{
    collections::VecDeque,
    io,
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Default)]
pub struct InputStatus {
    pub connected: bool,
    pub powered_off: bool,
    pub sent: u64,
    pub applied: u64,
    pub released: u64,
    pub rejected: u64,
    pub frame_ready: u64,
    pub frame_guest_ms: u32,
    pub indicator_updates: u64,
    pub indicators: [u32; 8],
    pub main_gpio: [u32; 5],
    pub adc_samples: [u16; 5],
    pub adc_updates: u64,
    pub guest_ms: u32,
    pub last_ack: Option<Message>,
    pub error: Option<String>,
}

struct Inner {
    writer: Mutex<UnixStream>,
    sequence: AtomicU32,
    status: Arc<Mutex<InputStatus>>,
    frames: Arc<Mutex<FrameQueue>>,
}
#[derive(Default)]
struct FrameQueue {
    ready: VecDeque<Message>,
    owned: [Option<u32>; 2],
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(writer) = self.writer.lock() {
            let _ = writer.shutdown(Shutdown::Both);
        }
    }
}

#[derive(Clone)]
pub struct EngineInput(Arc<Inner>);
impl EngineInput {
    pub fn pair() -> io::Result<(Self, UnixStream)> {
        let (host, engine) = UnixStream::pair()?;
        Ok((Self::new(host)?, engine))
    }
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        // Bound a blocked engine's effect on the UI and retire the channel on
        // timeout: a partial write cannot safely be retried as a new message.
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        let mut reader = stream.try_clone()?;
        let status = Arc::new(Mutex::new(InputStatus {
            connected: true,
            ..Default::default()
        }));
        let reader_status = status.clone();
        let frames = Arc::new(Mutex::new(FrameQueue::default()));
        let reader_frames = frames.clone();
        thread::Builder::new()
            .name("qemu-input-replies".into())
            .spawn(move || {
                loop {
                    match Message::read(&mut reader) {
                        Ok(message) if message.kind == protocol::POWER_OFF => {
                            reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .powered_off = true;
                        }
                        Ok(message) if message.kind == protocol::CLOCK => {
                            reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .guest_ms = message.duration_ms;
                        }
                        Ok(message) if message.kind == protocol::ADC_SAMPLE => {
                            let mut state = reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            state.adc_samples[(message.target - 3) as usize] = message.value as u16;
                            state.adc_updates += 1;
                        }
                        Ok(message) if message.kind == protocol::MAIN_GPIO => {
                            let mut state = reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            state.main_gpio[message.target as usize] = message.value as u32;
                            state.indicator_updates += 1;
                        }
                        Ok(message) if message.kind == protocol::INDICATORS => {
                            let mut state = reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            state.indicators[message.target as usize] = message.value as u32;
                            state.indicator_updates += 1;
                        }
                        Ok(message) if message.kind == protocol::FRAME_READY => {
                            let mut queue = reader_frames
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            let slot = message.target as usize;
                            if queue.ready.len() == 2 || queue.owned[slot].is_some() {
                                drop(queue);
                                let mut state = reader_status
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner());
                                state.connected = false;
                                state.error = Some("engine reused an owned display slot".into());
                                let _ = reader.shutdown(Shutdown::Both);
                                break;
                            }
                            queue.owned[slot] = Some(message.sequence);
                            queue.ready.push_back(message);
                            drop(queue);
                            let mut state = reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            state.frame_ready += 1;
                            state.frame_guest_ms = message.duration_ms;
                        }
                        Ok(message)
                            if matches!(
                                message.kind,
                                protocol::APPLIED | protocol::RELEASED | protocol::ERROR
                            ) =>
                        {
                            let mut state = reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            match message.kind {
                                protocol::APPLIED => state.applied += 1,
                                protocol::RELEASED => state.released += 1,
                                _ => state.rejected += 1,
                            }
                            state.last_ack = Some(message);
                        }
                        result => {
                            let mut state = reader_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            state.connected = false;
                            state.error = Some(match result {
                                Err(error) => error.to_string(),
                                Ok(_) => {
                                    "engine sent a command instead of an acknowledgement".into()
                                }
                            });
                            let _ = reader.shutdown(Shutdown::Both);
                            break;
                        }
                    }
                }
            })?;
        Ok(Self(Arc::new(Inner {
            writer: Mutex::new(stream),
            sequence: AtomicU32::new(0),
            status,
            frames,
        })))
    }
    pub fn snapshot(&self) -> InputStatus {
        self.0
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
    /// Drain publication notices while retaining slot ownership until consumed.
    pub fn take_frames(&self) -> Vec<Message> {
        self.0
            .frames
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .ready
            .drain(..)
            .collect()
    }
    pub fn consume_frame(&self, frame: Message) -> io::Result<()> {
        frame.validate()?;
        if frame.kind != protocol::FRAME_READY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected frame publication",
            ));
        }
        let mut writer = self
            .0
            .writer
            .lock()
            .map_err(|_| io::Error::other("input writer lock poisoned"))?;
        let mut frames = self
            .0
            .frames
            .lock()
            .map_err(|_| io::Error::other("frame queue lock poisoned"))?;
        let slot = frame.target as usize;
        if frames.owned[slot] != Some(frame.sequence) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame slot is not owned by this generation",
            ));
        }
        if !self.snapshot().connected {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "engine input disconnected",
            ));
        }
        let acknowledgement = Message {
            kind: protocol::FRAME_CONSUMED,
            value: 0,
            duration_ms: 0,
            ..frame
        };
        if let Err(error) = acknowledgement.write(&mut *writer) {
            let _ = writer.shutdown(Shutdown::Both);
            let mut status = self
                .0
                .status
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            status.connected = false;
            status.error = Some(error.to_string());
            return Err(error);
        }
        frames.owned[slot] = None;
        Ok(())
    }
    fn send(&self, kind: u32, target: u32, value: i32, duration_ms: u32) -> io::Result<u32> {
        // Hold the writer lock across sequence allocation and the complete
        // write so concurrent callers preserve sequence order on the stream.
        let mut writer = self
            .0
            .writer
            .lock()
            .map_err(|_| io::Error::other("input writer lock poisoned"))?;
        let sequence = loop {
            let next = self
                .0
                .sequence
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            if next != 0 {
                break next;
            }
        };
        let message = Message {
            kind,
            sequence,
            target,
            value,
            duration_ms,
        };
        message.validate()?;
        if !self.snapshot().connected {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "engine input disconnected",
            ));
        }
        if let Err(error) = message.write(&mut *writer) {
            let _ = writer.shutdown(Shutdown::Both);
            let mut state = self
                .0
                .status
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.connected = false;
            state.error = Some(error.to_string());
            return Err(error);
        }
        self.0
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sent += 1;
        Ok(sequence)
    }
    pub fn down(&self, button: u32, duration_ms: u32) -> io::Result<u32> {
        self.send(protocol::DOWN, button, 1, duration_ms)
    }
    pub fn up(&self, button: u32) -> io::Result<u32> {
        self.send(protocol::UP, button, 0, 0)
    }
    pub fn tap(&self, button: u32, duration_ms: u32) -> io::Result<u32> {
        self.send(protocol::TAP, button, 1, duration_ms)
    }
    pub fn encoder(&self, index: u32, steps: i32) -> io::Result<u32> {
        self.send(protocol::ENCODER_STEP, index, steps, 0)
    }
    pub fn adc(&self, channel: u32, value: u16) -> io::Result<u32> {
        self.send(protocol::ADC_SET, channel, value as i32, 0)
    }
}

#[derive(Clone)]
pub struct InputHub {
    pub main: EngineInput,
    pub panel: EngineInput,
    analog: Arc<Mutex<[u16; 5]>>,
}
impl InputHub {
    pub fn new(main: EngineInput, panel: EngineInput) -> Self {
        Self {
            main,
            panel,
            analog: Arc::new(Mutex::new([512; 5])),
        }
    }
    fn button_engine(&self, button: u32) -> &EngineInput {
        if matches!(button, 0 | 4 | 5 | 48..=53) {
            &self.main
        } else {
            &self.panel
        }
    }
    pub fn down(&self, button: u32, duration_ms: u32) -> io::Result<u32> {
        self.button_engine(button).down(button, duration_ms)
    }
    pub fn up(&self, button: u32) -> io::Result<u32> {
        self.button_engine(button).up(button)
    }
    pub fn tap(&self, button: u32, duration_ms: u32) -> io::Result<u32> {
        self.button_engine(button).tap(button, duration_ms)
    }
    pub fn encoder(&self, index: u32, steps: i32) -> io::Result<u32> {
        self.panel.encoder(index, steps)
    }
    pub fn analog_positions(&self) -> [u16; 5] {
        *self.analog.lock().unwrap()
    }
    pub fn restore_analog_positions(&self, values: [u16; 5]) -> io::Result<()> {
        for (channel, value) in crate::controls::ANALOG_CHANNELS.into_iter().zip(values) {
            self.main.adc(channel, value)?;
        }
        *self.analog.lock().unwrap() = values;
        Ok(())
    }
    /// Gesture steps raise the physical knob position; encoder polarity differs.
    pub fn knob(&self, index: usize, steps: i32) -> io::Result<u32> {
        if index < 8 {
            return self.encoder(index as u32, steps.saturating_neg());
        }
        let knob = index
            .checked_sub(8)
            .filter(|i| *i < 5)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid knob"))?;
        let mut positions = self.analog.lock().unwrap();
        let value = (positions[knob] as i64
            + steps as i64 * crate::controls::ANALOG_COUNTS_PER_STEP)
            .clamp(0, 1023) as u16;
        let sequence = self
            .main
            .adc(crate::controls::ANALOG_CHANNELS[knob], value)?;
        positions[knob] = value;
        Ok(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn analog_gestures_route_to_main_and_stop_at_physical_endpoints() {
        let (main, mut wire) = EngineInput::pair().unwrap();
        let (panel, _panel_wire) = EngineInput::pair().unwrap();
        let hub = InputHub::new(main, panel);
        hub.knob(8, i32::MAX).unwrap();
        let high = Message::read(&mut wire).unwrap();
        assert_eq!(
            (high.kind, high.target, high.value),
            (protocol::ADC_SET, 5, 1023)
        );
        hub.knob(8, i32::MIN).unwrap();
        assert_eq!(Message::read(&mut wire).unwrap().value, 0);
        hub.knob(10, 4).unwrap();
        let master = Message::read(&mut wire).unwrap();
        assert_eq!((master.target, master.value), (4, 640));
        assert_eq!(hub.analog_positions(), [0, 512, 640, 512, 512]);
        assert!(hub.knob(13, 1).is_err());
        hub.down(53, 50).unwrap();
        let power = Message::read(&mut wire).unwrap();
        assert_eq!((power.kind, power.target), (protocol::DOWN, 53));
    }

    #[test]
    fn indicator_events_coalesce_without_affecting_input_acknowledgements() {
        let (host, mut engine) = EngineInput::pair().unwrap();
        for sequence in 1..=256 {
            Message {
                kind: protocol::INDICATORS,
                sequence,
                target: 3,
                value: sequence as i32,
                duration_ms: sequence * 50,
            }
            .write(&mut engine)
            .unwrap();
        }
        Message {
            kind: protocol::CLOCK,
            sequence: 257,
            target: 0,
            value: 0,
            duration_ms: 12345,
        }
        .write(&mut engine)
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while host.snapshot().indicator_updates < 256 || host.snapshot().guest_ms != 12345 {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        let state = host.snapshot();
        assert_eq!(state.indicators, [0, 0, 0, 256, 0, 0, 0, 0]);
        assert_eq!(state.applied, 0);
        assert_eq!(state.guest_ms, 12345);
        assert_eq!(state.last_ack, None);
        assert!(host.take_frames().is_empty());
    }
    #[test]
    fn routes_commands_and_receives_fragmented_acknowledgements() {
        let (main, mut main_engine) = EngineInput::pair().unwrap();
        let (panel, mut panel_engine) = EngineInput::pair().unwrap();
        let hub = InputHub::new(main, panel);
        let menu_seq = hub.tap(0, 500).unwrap();
        assert_eq!(Message::read(&mut main_engine).unwrap().sequence, menu_seq);
        let up_seq = hub.down(1, 500).unwrap();
        let command = Message::read(&mut panel_engine).unwrap();
        assert_eq!(
            (command.kind, command.sequence, command.target),
            (protocol::DOWN, up_seq, 1)
        );
        hub.encoder(7, -2).unwrap();
        assert_eq!(Message::read(&mut panel_engine).unwrap().value, -2);
        let reply = Message {
            kind: protocol::APPLIED,
            ..command
        };
        use std::io::Write;
        for chunk in reply.encode().unwrap().chunks(3) {
            panel_engine.write_all(chunk).unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while hub.panel.snapshot().applied == 0 {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(hub.panel.snapshot().last_ack, Some(reply));
        drop(panel_engine);
        while hub.panel.snapshot().connected {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(hub.up(1).unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }
}
