//! Power hit testing through GPUI events without touching the system pointer.
use crate::NativePanel;
use gpui::{App, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, Window, point, px};
use std::time::{Duration, Instant};
pub struct PowerSmoke {
    stage: u8,
    deadline: Instant,
    next_action: Instant,
}
impl PowerSmoke {
    pub fn new() -> Self {
        Self {
            stage: 0,
            deadline: Instant::now() + Duration::from_secs(300),
            next_action: Instant::now(),
        }
    }
    pub fn tick(
        &mut self,
        panel: &mut NativePanel,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<bool, String> {
        if Instant::now() > self.deadline {
            return Err(format!("native Power timeout at stage {}", self.stage));
        }
        let guest_ms = panel
            .controls
            .as_ref()
            .map(|c| c.panel.snapshot().guest_ms)
            .unwrap_or(0);
        let event = |id, down, window: &mut Window, cx: &mut App| {
            let r = panel
                .layout
                .buttons
                .iter()
                .find(|(c, _)| c.id == id)
                .unwrap()
                .1;
            let size = window.viewport_size();
            let scale = (size.width.as_f32() / panel.layout.width)
                .min(size.height.as_f32() / panel.layout.height);
            let position = point(
                px((size.width.as_f32() - panel.layout.width * scale) / 2.
                    + (r.x + r.width / 2.) * scale),
                px((size.height.as_f32() - panel.layout.height * scale) / 2.
                    + (r.y + r.height / 2.) * scale),
            );
            if down {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            } else {
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            }
        };
        match self.stage {
            0 if guest_ms >= 40000 => {
                event(3, true, window, cx);
                event(3, false, window, cx);
                self.stage = 1;
            }
            1 if guest_ms >= 42000 => {
                event(3, true, window, cx);
                event(3, false, window, cx);
                self.stage = 2;
            }
            2 if guest_ms >= 47000 => {
                event(53, true, window, cx);
                self.stage = 3;
            }
            3 if guest_ms >= 51000 => {
                event(53, false, window, cx);
                self.stage = 4;
            }
            4 if panel.powered_off => {
                if panel.controls.is_some()
                    || panel.display.is_some()
                    || panel._qemu.is_some()
                    || panel.last_frame.iter().any(|&b| b != 0)
                {
                    return Err("off state retained guest/display/input".into());
                }
                println!("PASS: native Power hold shut down QEMU and blanked display/LEDs");
                self.stage = 5;
                self.next_action = Instant::now() + Duration::from_millis(300);
            }
            5 if Instant::now() >= self.next_action => {
                event(53, true, window, cx);
                event(53, false, window, cx);
                self.stage = 6;
            }
            6 if guest_ms >= 40000 => {
                if panel.powered_off || panel.last_frame.iter().all(|&b| b == 0) {
                    return Err("Power-on did not restore firmware display".into());
                }
                println!(
                    "PASS: native Power click restarted both chips and restored live display; system pointer untouched"
                );
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
}
