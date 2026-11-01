//! Production panel snapshot and retained display feed, shared by GUI and tests.
use crate::{display::PublishedDisplay, engine::Engine, input::InputHub};
use std::{cell::Cell, sync::Arc};

/// Backend state consumed by the panel components. Keeping this snapshot
/// separate from GPUI lets interaction smoke tests check the exact values the
/// renderer receives without requiring a window or a GPU frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelData {
    pub indicators: [u32; 8],
    pub indicator_levels: [[u8; 24]; 8],
    pub switches: [bool; 54],
    pub analog: [u16; 5],
    pub powered: bool,
    /// Packed 128x64 monochrome display bytes in the firmware's native layout.
    pub display: [u8; 1024],
}

impl PanelData {
    /// Compact 64x32 ASCII preview for smoke-test diagnostics.
    pub fn display_ascii(&self) -> String {
        let mut preview = String::with_capacity(64 * 33);
        for y in 0..32 {
            for x in 0..64 {
                let lit = (0..2).any(|dy| {
                    (0..2).any(|dx| {
                        let px = x * 2 + dx;
                        let py = y * 2 + dy;
                        let index = ((63 - py) / 8) * 128 + px;
                        (self.display[index] >> ((63 - py) % 8)) & 1 != 0
                    })
                });
                preview.push(if lit { '#' } else { ' ' });
            }
            if y != 31 {
                preview.push('\n');
            }
        }
        preview
    }
}

impl Default for PanelData {
    fn default() -> Self {
        Self {
            indicators: [0; 8],
            indicator_levels: [[0; 24]; 8],
            switches: [false; 54],
            analog: [0; 5],
            powered: false,
            display: [0; 1024],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PanelData;
    use super::PanelFeed;
    use crate::{
        display::PublishedDisplay,
        input::{EngineInput, InputHub},
        protocol::{self, Message},
        shared_memory::SharedRam,
    };
    use memmap2::MmapOptions;
    use std::{
        cell::Cell,
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn feed_retains_static_pixels_while_new_indicator_state_continues() {
        let memory = SharedRam::new(2048).unwrap();
        // SAFETY: the simulated guest writes before publishing ownership.
        let mut writer = unsafe { MmapOptions::new().map_mut(memory.file()).unwrap() };
        writer[..1024].fill(0x55);
        let (main, mut main_peer) = EngineInput::pair().unwrap();
        let (panel, mut panel_peer) = EngineInput::pair().unwrap();
        let inputs = Arc::new(InputHub::new(
            main.clone(),
            panel.clone(),
            l6fw::Model::L6max,
        ));
        let feed = PanelFeed {
            display: PublishedDisplay::new(memory.map(), main.clone()).unwrap(),
            inputs,
            frame: Cell::new([0; 1024]),
        };
        Message {
            kind: protocol::FRAME_READY,
            sequence: 1,
            target: 0,
            value: 255,
            duration_ms: 100,
        }
        .write(&mut main_peer)
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while main.snapshot().frame_ready == 0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(feed.sample().unwrap(), Some([0x55; 1024]));
        assert_eq!(
            Message::read(&mut main_peer).unwrap().kind,
            protocol::FRAME_CONSUMED
        );
        Message {
            kind: protocol::INDICATORS,
            sequence: 2,
            target: 0,
            value: 0x20,
            duration_ms: 200,
        }
        .write(&mut panel_peer)
        .unwrap();
        while panel.snapshot().indicator_updates == 0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(feed.sample().unwrap(), None);
        let data = feed.data([false; 54], true);
        assert_eq!(data.display, [0x55; 1024]);
        assert_eq!(data.indicators[0], 0x20);
        assert_eq!(data.analog, [512; 5]);
    }

    #[test]
    fn ascii_display_preview_uses_firmware_page_bit_order() {
        let mut data = PanelData::default();
        data.display[7 * 128] = 0x80;
        let preview = data.display_ascii();
        let lines: Vec<_> = preview.lines().collect();
        assert_eq!(lines.len(), 32);
        assert!(lines[0].starts_with('#'));
        assert!(lines[1..].iter().all(|line| line.chars().all(|c| c == ' ')));
    }
}

/// Owns display-slot consumption and keeps the last completed frame.
/// A static display is valid: frame publication is not a heartbeat.
pub struct PanelFeed {
    display: PublishedDisplay,
    inputs: Arc<InputHub>,
    frame: Cell<[u8; 1024]>,
}
impl PanelFeed {
    pub fn new(engine: &Engine) -> Result<Self, String> {
        let inputs = engine.inputs.clone().ok_or("QEMU input unavailable")?;
        let memory = engine
            .display_memory
            .as_ref()
            .ok_or("display memory unavailable")?;
        Ok(Self {
            display: PublishedDisplay::new(memory.map(), inputs.main.clone())
                .map_err(|e| e.to_string())?,
            inputs,
            frame: Cell::new([0; 1024]),
        })
    }
    pub fn sample(&self) -> std::io::Result<Option<[u8; 1024]>> {
        let changed = self.display.sample()?;
        if let Some(frame) = changed {
            self.frame.set(frame);
        }
        Ok(changed)
    }
    pub fn data(&self, switches: [bool; 54], powered: bool) -> PanelData {
        let panel = self.inputs.panel.snapshot();
        PanelData {
            indicators: panel.indicators,
            indicator_levels: panel.indicator_levels,
            switches,
            analog: self.inputs.analog_positions(),
            powered,
            display: self.frame.get(),
        }
    }
}
