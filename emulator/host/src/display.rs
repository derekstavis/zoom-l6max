//! Display frames published by QEMU into two shared-memory slots. QEMU retains
//! each completed slot until FRAME_CONSUMED; the UI copies only the newest one.
use crate::{input::EngineInput, protocol};
use memmap2::Mmap;
use std::{io, sync::Arc};

pub const FRAME_BYTES: usize = 1024;
pub const DISPLAY_BYTES: usize = FRAME_BYTES * 2;

pub struct PublishedDisplay {
    memory: Arc<Mmap>,
    input: EngineInput,
}
impl PublishedDisplay {
    pub fn new(memory: Arc<Mmap>, input: EngineInput) -> io::Result<Self> {
        if memory.len() < DISPLAY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "display segment is smaller than two frames",
            ));
        }
        Ok(Self { memory, input })
    }
    pub fn sample(&self) -> io::Result<Option<[u8; FRAME_BYTES]>> {
        let frames = self.input.take_frames();
        let Some(newest) = frames.last() else {
            let status = self.input.snapshot();
            if !status.connected {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    status
                        .error
                        .unwrap_or_else(|| "display engine disconnected".into()),
                ));
            }
            return Ok(None);
        };
        newest.validate()?;
        if newest.kind != protocol::FRAME_READY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid display publication",
            ));
        }
        let start = newest.target as usize * FRAME_BYTES;
        let mut result = [0; FRAME_BYTES];
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        result.copy_from_slice(&self.memory[start..start + FRAME_BYTES]);
        // Release every drained slot, including obsolete frames. The newest
        // frame must be copied completely before its ownership is returned.
        for frame in frames {
            self.input.consume_frame(frame)?;
        }
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{protocol::Message, shared_memory::SharedRam};
    use memmap2::MmapOptions;
    use std::{
        thread,
        time::{Duration, Instant},
    };
    fn wait_for(input: &EngineInput, count: u64) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while input.snapshot().frame_ready < count {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn copies_newest_published_slot_and_releases_both_generations() {
        let ram = SharedRam::new(DISPLAY_BYTES).unwrap();
        // SAFETY: this peer writes only before publishing; ownership then
        // belongs to PublishedDisplay until acknowledgement.
        let mut guest = unsafe { MmapOptions::new().map_mut(ram.file()).unwrap() };
        guest[..FRAME_BYTES].fill(0x12);
        guest[FRAME_BYTES..].fill(0x34);
        let (input, mut peer) = EngineInput::pair().unwrap();
        let display = PublishedDisplay::new(ram.map(), input.clone()).unwrap();
        for (sequence, target) in [(17, 0), (18, 1)] {
            Message {
                kind: protocol::FRAME_READY,
                sequence,
                target,
                value: 255,
                duration_ms: 73_000,
            }
            .write(&mut peer)
            .unwrap();
        }
        wait_for(&input, 2);
        assert_eq!(display.sample().unwrap(), Some([0x34; FRAME_BYTES]));
        for (sequence, target) in [(17, 0), (18, 1)] {
            let consumed = Message::read(&mut peer).unwrap();
            assert_eq!(
                (consumed.kind, consumed.sequence, consumed.target),
                (protocol::FRAME_CONSUMED, sequence, target)
            );
        }
        assert_eq!(input.snapshot().applied, 0);
        assert_eq!(input.snapshot().sent, 0);
        assert_eq!(display.sample().unwrap(), None);
    }
    #[test]
    fn rejects_republication_before_consumption_even_after_notice_drain() {
        let (input, mut peer) = EngineInput::pair().unwrap();
        let publication = Message {
            kind: protocol::FRAME_READY,
            sequence: 1,
            target: 0,
            value: 255,
            duration_ms: 0,
        };
        publication.write(&mut peer).unwrap();
        wait_for(&input, 1);
        assert_eq!(input.take_frames(), vec![publication]);
        Message {
            sequence: 2,
            ..publication
        }
        .write(&mut peer)
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while input.snapshot().connected {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            input
                .snapshot()
                .error
                .unwrap()
                .contains("owned display slot")
        );
    }
    #[test]
    fn rejects_out_of_range_display_slot() {
        let invalid = Message {
            kind: protocol::FRAME_READY,
            sequence: 1,
            target: 2,
            value: 255,
            duration_ms: 0,
        };
        assert!(invalid.encode().is_err());
        let (input, mut peer) = EngineInput::pair().unwrap();
        use std::io::Write;
        let mut wire = Message {
            target: 1,
            ..invalid
        }
        .encode()
        .unwrap();
        wire[12..16].copy_from_slice(&2u32.to_le_bytes());
        peer.write_all(&wire).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while input.snapshot().connected {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            input
                .snapshot()
                .error
                .unwrap()
                .contains("invalid input message")
        );
    }
}
