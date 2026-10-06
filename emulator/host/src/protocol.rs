//! Fixed-width wire protocol shared with the L6max QEMU input device.
//!
//! A stream may split a message across reads. Every word is little endian and
//! every message is exactly 24 bytes; malformed messages close the channel.
use std::io::{self, Read, Write};

pub const MAGIC: u32 = 0x4c36_4950;
pub const MESSAGE_BYTES: usize = 24;
pub const MAX_DURATION_MS: u32 = 60_000;
pub const DOWN: u32 = 1;
pub const UP: u32 = 2;
pub const TAP: u32 = 3;
pub const ENCODER_STEP: u32 = 4;
pub const FRAME_CONSUMED: u32 = 5;
pub const ADC_SET: u32 = 6;
pub const APPLIED: u32 = 0x8000_0001;
pub const RELEASED: u32 = 0x8000_0002;
pub const ERROR: u32 = 0x8000_0003;
pub const FRAME_READY: u32 = 0x8000_0004;
/// GPIO-latched matrix row; value is a 24-column bit mask.
pub const INDICATORS: u32 = 0x8000_0005;
/// Main MCU GPIO1..5 output latch values, target 0..4.
pub const MAIN_GPIO: u32 = 0x8000_0006;
/// A firmware read of an analog knob's ADC result, target physical channel 3..7.
pub const ADC_SAMPLE: u32 = 0x8000_0007;
/// Periodic virtual-clock notification; independent of changed display pixels.
pub const CLOCK: u32 = 0x8000_0008;
/// Board power-hold output dropped after firmware shutdown.
pub const POWER_OFF: u32 = 0x8000_0009;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub kind: u32,
    pub sequence: u32,
    pub target: u32,
    pub value: i32,
    /// Command duration, or low 32 bits of guest virtual milliseconds in replies.
    pub duration_ms: u32,
}

impl Message {
    pub fn validate(&self) -> io::Result<()> {
        let valid = self.sequence != 0
            && match self.kind {
                DOWN => {
                    self.target < crate::controls::INPUT_COUNT as u32
                        && self.value == 1
                        && self.duration_ms <= MAX_DURATION_MS
                }
                UP => {
                    self.target < crate::controls::INPUT_COUNT as u32
                        && self.value == 0
                        && self.duration_ms == 0
                }
                TAP => {
                    self.target < crate::controls::INPUT_COUNT as u32
                        && self.value == 1
                        && (1..=MAX_DURATION_MS).contains(&self.duration_ms)
                }
                ENCODER_STEP => self.target < 8 && self.value != 0 && self.duration_ms == 0,
                ADC_SET => {
                    (3..=7).contains(&self.target)
                        && (0..=1023).contains(&self.value)
                        && self.duration_ms == 0
                }
                APPLIED | RELEASED => self.target < crate::controls::INPUT_COUNT as u32,
                ERROR => self.target < crate::controls::INPUT_COUNT as u32 && self.value < 0,
                FRAME_CONSUMED => self.target < 2 && self.value == 0 && self.duration_ms == 0,
                FRAME_READY => self.target < 2 && (0..=255).contains(&self.value),
                MAIN_GPIO => self.target < 5,
                CLOCK | POWER_OFF => self.target == 0 && self.value == 0,
                ADC_SAMPLE => (3..=7).contains(&self.target) && (0..=1023).contains(&self.value),
                INDICATORS => self.target < 8 && (0..=0xffffff).contains(&self.value),
                _ => false,
            };
        if valid {
            Ok(())
        } else {
            Err(invalid("invalid input message fields"))
        }
    }

    pub fn encode(self) -> io::Result<[u8; MESSAGE_BYTES]> {
        self.validate()?;
        let mut bytes = [0; MESSAGE_BYTES];
        for (index, word) in [
            MAGIC,
            self.kind,
            self.sequence,
            self.target,
            self.value as u32,
            self.duration_ms,
        ]
        .into_iter()
        .enumerate()
        {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: [u8; MESSAGE_BYTES]) -> io::Result<Self> {
        let word =
            |index: usize| u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap());
        if word(0) != MAGIC {
            return Err(invalid("invalid input message magic"));
        }
        let message = Self {
            kind: word(1),
            sequence: word(2),
            target: word(3),
            value: word(4) as i32,
            duration_ms: word(5),
        };
        message.validate()?;
        Ok(message)
    }

    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0; MESSAGE_BYTES];
        reader.read_exact(&mut bytes)?;
        Self::decode(bytes)
    }

    pub fn write(self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&self.encode()?)
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    struct Fragmented(Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let len = bytes.len().min(3);
            self.0.read(&mut bytes[..len])
        }
    }
    #[test]
    fn reads_fragmented_messages_without_losing_boundaries() {
        let commands = [
            Message {
                kind: TAP,
                sequence: 17,
                target: 1,
                value: 1,
                duration_ms: 500,
            },
            Message {
                kind: ENCODER_STEP,
                sequence: 18,
                target: 7,
                value: -13,
                duration_ms: 0,
            },
        ];
        let mut wire = Vec::new();
        for command in commands {
            command.write(&mut wire).unwrap();
        }
        let mut reader = Fragmented(Cursor::new(wire));
        for command in commands {
            assert_eq!(Message::read(&mut reader).unwrap(), command);
        }
        assert_eq!(
            Message::read(&mut reader).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
    #[test]
    fn reply_guest_timestamp_uses_the_full_word() {
        for kind in [APPLIED, RELEASED, ERROR] {
            let message = Message {
                kind,
                sequence: 3,
                target: 1,
                value: if kind == ERROR { -22 } else { 0 },
                duration_ms: u32::MAX,
            };
            assert_eq!(Message::decode(message.encode().unwrap()).unwrap(), message);
        }
    }
    #[test]
    fn rejects_corruption_and_invalid_commands() {
        let base = Message {
            kind: TAP,
            sequence: 1,
            target: 1,
            value: 1,
            duration_ms: 500,
        };
        let mut wire = base.encode().unwrap();
        wire[0] ^= 1;
        assert!(Message::decode(wire).is_err());
        for message in [
            Message {
                sequence: 0,
                ..base
            },
            Message { kind: 23, ..base },
            Message { target: 54, ..base },
            Message {
                duration_ms: 0,
                ..base
            },
            Message {
                duration_ms: 60_001,
                ..base
            },
        ] {
            assert!(message.encode().is_err());
        }
    }

    #[test]
    fn adc_commands_reject_board_id_channels_and_out_of_range_samples() {
        let base = Message {
            kind: ADC_SET,
            sequence: 1,
            target: 3,
            value: 1023,
            duration_ms: 0,
        };
        assert_eq!(Message::decode(base.encode().unwrap()).unwrap(), base);
        for invalid in [
            Message { target: 8, ..base },
            Message { target: 13, ..base },
            Message { target: 2, ..base },
            Message { value: -1, ..base },
            Message {
                value: 1024,
                ..base
            },
            Message {
                duration_ms: 1,
                ..base
            },
        ] {
            assert!(invalid.encode().is_err());
        }
    }

    #[test]
    fn main_gpio_reply_preserves_high_output_bits() {
        let message = Message {
            kind: MAIN_GPIO,
            sequence: 1,
            target: 4,
            value: 0x80000001u32 as i32,
            duration_ms: u32::MAX,
        };
        assert_eq!(Message::decode(message.encode().unwrap()).unwrap(), message);
        assert!(
            Message {
                target: 5,
                ..message
            }
            .encode()
            .is_err()
        );
    }

    #[test]
    fn indicator_surface_preserves_all_columns_and_rejects_invalid_rows() {
        let message = Message {
            kind: INDICATORS,
            sequence: 1,
            target: 7,
            value: 0xffffff,
            duration_ms: u32::MAX,
        };
        assert_eq!(Message::decode(message.encode().unwrap()).unwrap(), message);
        assert!(
            Message {
                target: 8,
                ..message
            }
            .encode()
            .is_err()
        );
        assert!(
            Message {
                value: 0x1000000,
                ..message
            }
            .encode()
            .is_err()
        );
        assert!(
            Message {
                value: -1,
                ..message
            }
            .encode()
            .is_err()
        );
    }
}
