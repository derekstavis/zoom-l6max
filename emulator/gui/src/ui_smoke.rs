//! In-process GPUI events exercise hit testing without touching the OS pointer.
use crate::components::layout::{Layout, Region};
use gpui::{
    App, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta,
    ScrollWheelEvent, Window, point, px,
};
use l6max_host::{
    indicators::{leds, rings},
    input::InputHub,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub struct UiSmoke {
    logs: PathBuf,
    deadline: Instant,
    stage: u8,
    frame_time: u32,
    original: [u32; 8],
}
impl UiSmoke {
    pub fn new(logs: PathBuf) -> Self {
        Self {
            logs,
            deadline: Instant::now() + Duration::from_secs(180),
            stage: 0,
            frame_time: 0,
            original: [0; 8],
        }
    }
    pub fn tick(
        &mut self,
        layout: &Layout,
        inputs: &InputHub,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<bool, String> {
        if self.stage == 9 {
            return Ok(Instant::now() > self.deadline);
        }
        if Instant::now() > self.deadline {
            return Err(format!("UI test timeout at stage {}", self.stage));
        }
        let main = inputs.main.snapshot();
        let panel = inputs.panel.snapshot();
        if panel.rejected != 0 || main.rejected != 0 {
            return Err("input rejected".into());
        }
        let size = window.viewport_size();
        let scale = (size.width.as_f32() / layout.width).min(size.height.as_f32() / layout.height);
        let center = |r: Region| {
            point(
                px((size.width.as_f32() - layout.width * scale) / 2.
                    + (r.x + r.width / 2.) * scale),
                px((size.height.as_f32() - layout.height * scale) / 2.
                    + (r.y + r.height / 2.) * scale),
            )
        };
        let click = |position, window: &mut Window, cx: &mut App| {
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
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        };
        match self.stage {
            0 if panel.guest_ms >= 18_000 => {
                self.frame_time = panel.guest_ms;
                click(center(layout.buttons[3].1), window, cx);
                self.stage = 1;
            }
            1 if panel.applied >= 2 && panel.guest_ms > self.frame_time + 500 => {
                click(center(layout.buttons[3].1), window, cx);
                self.stage = 2;
            }
            2 if panel.guest_ms > self.frame_time + 1500
                && panel.applied >= 4
                && rings(panel.indicators).iter().all(|ring| *ring != 0) =>
            {
                self.original = rings(panel.indicators);
                for channel in 0..2 {
                    let start = center(layout.knobs[channel]);
                    let end = if channel == 0 {
                        point(start.x, start.y - px(96. * scale))
                    } else {
                        point(start.x + px(96. * scale), start.y)
                    };
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position: start,
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            position: start,
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position: end,
                            pressed_button: Some(MouseButton::Left),
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            position: end,
                            ..Default::default()
                        }),
                        cx,
                    );
                }
                let p = center(layout.knobs[2]);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: p,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: p,
                        delta: ScrollDelta::Lines(point(0., 12.)),
                        ..Default::default()
                    }),
                    cx,
                );
                self.stage = 3;
            }
            3 if rings(panel.indicators)[..3] == [31; 3] => {
                if rings(panel.indicators)[3..] != self.original[3..] {
                    return Err("gesture changed an unrelated ring".into());
                }
                let p = center(layout.knobs[2]);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: p,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: p,
                        delta: ScrollDelta::Lines(point(0., -12.)),
                        ..Default::default()
                    }),
                    cx,
                );
                self.stage = 4;
            }
            4 if rings(panel.indicators)[2] == self.original[2] => {
                let trace = std::fs::read_to_string(self.logs.join("engine.trace"))
                    .map_err(|e| e.to_string())?;
                let encoder = trace.matches("l6 panel USART1 TX a1\n").count();
                if encoder < 48 {
                    return Ok(false);
                }
                if encoder != 48 || trace.matches("l6 panel USART1 TX 91\n").count() != 2 {
                    return Err("unexpected firmware input count".into());
                }
                if rings(panel.indicators)[..2] != [31; 2] {
                    return Err("reverse scroll changed another ring".into());
                }
                let region = layout
                    .buttons
                    .iter()
                    .find(|(c, _)| c.name == "ch1-mute")
                    .unwrap()
                    .1;
                click(center(region), window, cx);
                self.stage = 5;
            }
            5 if leds(panel.indicators)[0] => {
                let region = layout
                    .buttons
                    .iter()
                    .find(|(c, _)| c.name == "mode-eq-high")
                    .unwrap()
                    .1;
                click(center(region), window, cx);
                self.frame_time = panel.guest_ms;
                self.stage = 6;
            }
            6 if leds(panel.indicators)[17]
                && !leds(panel.indicators)[26]
                && panel.guest_ms > self.frame_time + 250 =>
            {
                let region = layout
                    .buttons
                    .iter()
                    .find(|(c, _)| c.name == "ch5-pad-switch")
                    .unwrap()
                    .1;
                click(center(region), window, cx);
                self.frame_time = panel.guest_ms;
                self.stage = 7;
            }
            7 if panel.guest_ms > self.frame_time + 250 => {
                let trace = std::fs::read_to_string(self.logs.join("engine.trace"))
                    .map_err(|e| e.to_string())?;
                let closed =
                    "l6 panel USART1 TX 91\nl6 panel USART1 TX 02\nl6 panel USART1 TX 00\n";
                let open = "l6 panel USART1 TX 90\nl6 panel USART1 TX 02\nl6 panel USART1 TX 00\n";
                // UART trace publication may lag the guest clock notification.
                // Wait for the debounced edge, then retain exact edge-count checks.
                if trace.matches(closed).count() == 0 && !trace.contains(open) {
                    return Ok(false);
                }
                if trace.matches(closed).count() != 1 || trace.contains(open) {
                    return Err(
                        "switch did not retain its physical GPIO level after mouse up".into(),
                    );
                }
                let region = layout
                    .buttons
                    .iter()
                    .find(|(c, _)| c.name == "ch5-pad-switch")
                    .unwrap()
                    .1;
                click(center(region), window, cx);
                self.stage = 8;
            }
            8 => {
                let trace = std::fs::read_to_string(self.logs.join("engine.trace"))
                    .map_err(|e| e.to_string())?;
                let open = "l6 panel USART1 TX 90\nl6 panel USART1 TX 02\nl6 panel USART1 TX 00\n";
                if !trace.contains(open) {
                    return Ok(false);
                }
                if trace.matches(open).count() != 1
                    || trace.matches("l6 panel USART1 TX 91\n").count() != 5
                {
                    return Err("duplicate control or switch firmware events".into());
                }
                for i in 0..5 {
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position: center(layout.knobs[8 + i]),
                            delta: ScrollDelta::Lines(point(0.0, (4 + 2 * i) as f32)),
                            ..Default::default()
                        }),
                        cx,
                    );
                }
                click(
                    center(layout.buttons.iter().find(|(c, _)| c.id == 53).unwrap().1),
                    window,
                    cx,
                );
                self.stage = 10;
            }
            10 | 11 | 12 => {
                let mut expected = std::array::from_fn::<_, 5, _>(|i| {
                    512 + (4 + 2 * i) as u16 * l6max_host::controls::ANALOG_COUNTS_PER_STEP as u16
                });
                if self.stage >= 11 {
                    for value in &mut expected {
                        *value -= 64;
                    }
                }
                if self.stage == 12 {
                    expected[0] += 192;
                    expected[2] -= 128;
                }
                if inputs.analog_positions() != expected {
                    return Err(format!(
                        "analog gesture positions {:?}, expected {expected:?}",
                        inputs.analog_positions()
                    ));
                }
                if l6max_host::controls::ANALOG_CHANNELS
                    .iter()
                    .enumerate()
                    .any(|(i, channel)| main.adc_samples[(*channel - 3) as usize] != expected[i])
                {
                    return Ok(false);
                }
                if self.stage == 10 {
                    let trace = std::fs::read_to_string(self.logs.join("engine.trace"))
                        .map_err(|e| e.to_string())?;
                    if !trace.contains("l6 main power input=1")
                        || trace.matches("l6 main power input=0").count() < 2
                    {
                        return Ok(false);
                    }
                    if trace.matches("l6 main power input=1").count() != 1 {
                        return Err("duplicate Power GPIO closure".into());
                    }
                    for i in 0..5 {
                        window.dispatch_event(
                            PlatformInput::ScrollWheel(ScrollWheelEvent {
                                position: center(layout.knobs[8 + i]),
                                delta: ScrollDelta::Lines(point(0.0, -2.0)),
                                ..Default::default()
                            }),
                            cx,
                        );
                    }
                    self.stage = 11;
                    return Ok(false);
                }
                if self.stage == 11 {
                    for (index, dx, dy) in [(8, 0.0, -48.0), (10, -32.0, 0.0)] {
                        let start = center(layout.knobs[index]);
                        let end = point(start.x + px(dx * scale), start.y + px(dy * scale));
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position: start,
                                ..Default::default()
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseDown(MouseDownEvent {
                                position: start,
                                click_count: 1,
                                ..Default::default()
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position: end,
                                pressed_button: Some(MouseButton::Left),
                                ..Default::default()
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseUp(MouseUpEvent {
                                position: end,
                                click_count: 1,
                                ..Default::default()
                            }),
                            cx,
                        );
                    }
                    self.stage = 12;
                    return Ok(false);
                }
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(0.0), px(0.0)),
                        ..Default::default()
                    }),
                    cx,
                );
                println!(
                    "UI input passed: navigation, channel encoders, LEDs, persistent switch, Power GPIO edges, five ADC knobs in both directions and analog drags. System pointer untouched."
                );
                if std::env::var_os("L6_UI_SMOKE_HOLD").is_some() {
                    self.stage = 9;
                    self.deadline = Instant::now() + Duration::from_secs(600);
                    return Ok(false);
                }
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
}
