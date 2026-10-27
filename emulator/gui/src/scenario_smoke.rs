//! GPUI adapter only: scenarios and assertions live in smoke-core.
use crate::components::{
    layout::{Layout, Region},
    panel::PanelData,
};
use gpui::{
    App, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta,
    ScrollWheelEvent, Window, point, px,
};
use l6max_host::input::InputHub;
use smoke_core::{Command, Gesture, Progress, Runner};
use std::sync::atomic::AtomicU32;

pub fn tick(
    runner: &mut Runner,
    layout: &Layout,
    inputs: Option<&InputHub>,
    data: PanelData,
    press_duration: &AtomicU32,
    window: &mut Window,
    cx: &mut App,
) -> Result<bool, String> {
    let progress = match inputs {
        Some(inputs) => runner.tick(data, inputs)?,
        None => runner.tick_without_input(data)?,
    };
    match progress {
        Progress::Device(_) => Err("device operation requires the headless backend".into()),
        Progress::Complete => Ok(true),
        Progress::Waiting => Ok(false),
        Progress::Dispatch(command) => {
            if let Err(error) = dispatch(command, layout, data, press_duration, window, cx) {
                runner.finish("FAIL", Some(&error))?;
                return Err(error);
            }
            Ok(false)
        }
    }
}
fn dispatch(
    command: Command,
    layout: &Layout,
    data: PanelData,
    press_duration: &AtomicU32,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    let size = window.viewport_size();
    let scale = (size.width.as_f32() / layout.width).min(size.height.as_f32() / layout.height);
    let center = |r: Region| {
        point(
            px((size.width.as_f32() - layout.width * scale) / 2.0 + (r.x + r.width / 2.0) * scale),
            px((size.height.as_f32() - layout.height * scale) / 2.0
                + (r.y + r.height / 2.0) * scale),
        )
    };
    if let Command::Switch { target, enabled } = command {
        if data.switches[target as usize] == enabled {
            return Err(
                "switch is already in requested position; fixture or sequence mismatch".into(),
            );
        }
    }
    let (region, gesture, hold, release) = match command {
        Command::Press { target } | Command::Switch { target, .. } => (
            layout
                .buttons
                .iter()
                .find(|(c, _)| c.id == target)
                .ok_or_else(|| format!("control target {target} has no hit region"))?
                .1,
            None,
            None,
            false,
        ),
        Command::HoldDown {
            target,
            duration_ms,
        } => (
            layout
                .buttons
                .iter()
                .find(|(c, _)| c.id == target)
                .ok_or_else(|| format!("control target {target} has no hit region"))?
                .1,
            None,
            Some(duration_ms),
            false,
        ),
        Command::Release { target } => (
            layout
                .buttons
                .iter()
                .find(|(c, _)| c.id == target)
                .ok_or_else(|| format!("control target {target} has no hit region"))?
                .1,
            None,
            None,
            true,
        ),
        Command::Encoder {
            channel,
            steps,
            gesture,
        } => (
            layout.knobs[channel - 1],
            Some((steps, gesture)),
            None,
            false,
        ),
        Command::Analog {
            channel,
            steps,
            gesture,
        } => (
            layout.knobs[7 + channel],
            Some((steps, gesture)),
            None,
            false,
        ),
    };
    if region.width <= 0.0 || region.height <= 0.0 {
        return Err("control has an empty hit region".into());
    }
    let position = center(region);
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            ..Default::default()
        }),
        cx,
    );
    if release {
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                position,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        );
        return Ok(());
    }
    if let Some(duration_ms) = hold {
        press_duration.store(duration_ms, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some((steps, Gesture::Scroll)) = gesture {
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Lines(point(0.0, steps as f32)),
                ..Default::default()
            }),
            cx,
        );
        return Ok(());
    }
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            position,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    );
    if hold.is_some() {
        return Ok(());
    }
    let mut end = position;
    if let Some((steps, kind)) = gesture {
        let distance = px(8.0 * scale * steps as f32);
        end = match kind {
            Gesture::DragHorizontal => point(position.x + distance, position.y),
            _ => point(position.x, position.y - distance),
        };
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: end,
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        );
    }
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            position: end,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantic_targets_match_the_actual_model_hit_regions() {
        for model in [l6fw::Model::L6, l6fw::Model::L6max] {
            let layout = Layout::for_model(model);
            for channel in 1..=if model == l6fw::Model::L6 { 6 } else { 8 } {
                let target =
                    smoke_core::control_target(model, &format!("channel.{channel}.mute")).unwrap();
                let (control, region) =
                    layout.buttons.iter().find(|(c, _)| c.id == target).unwrap();
                assert_eq!(control.name, format!("ch{channel}-mute"));
                assert!(region.width > 0.0 && region.height > 0.0);
            }
            let target = smoke_core::control_target(model, "mode.level").unwrap();
            let (control, _) = layout.buttons.iter().find(|(c, _)| c.id == target).unwrap();
            assert!(control.name.ends_with("mode-level"));
        }
    }
}
