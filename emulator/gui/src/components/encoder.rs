//! Reusable encoder with a firmware-driven LED ring.
use gpui::{
    App, Bounds, CursorStyle, IntoElement, MouseButton, PathBuilder, Pixels, Point, RenderOnce,
    ScrollDelta, Window, canvas, div, point, prelude::*, px, rgb,
};
use std::sync::Arc;

/// Encoder indication can be supplied as a single ring position or an exact
/// firmware-decoded segment mask (which also represents startup animations).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // The positional prop is part of the reusable component API.
pub enum EncoderLedLevel {
    Position(Option<u8>),
    Segments(u16),
}

#[derive(IntoElement)]
pub struct EncoderWithLeds {
    pub size: f32,
    pub level: EncoderLedLevel,
    pub input: Option<EncoderInput>,
}

#[derive(Clone, Default)]
pub struct EncoderInput {
    pub drag_start: Option<Arc<dyn Fn(f32, f32) + Send + Sync>>,
    pub scroll: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    pub drag_end: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl EncoderWithLeds {
    pub fn new(size: f32, level: EncoderLedLevel) -> Self {
        Self {
            size,
            level,
            input: None,
        }
    }

    pub fn with_input(mut self, input: EncoderInput) -> Self {
        self.input = Some(input);
        self
    }

    fn mask(&self) -> u16 {
        match self.level {
            EncoderLedLevel::Position(Some(position)) if position < 9 => 1 << position,
            EncoderLedLevel::Position(_) => 0,
            EncoderLedLevel::Segments(mask) => mask & 0x1ff,
        }
    }
}

impl RenderOnce for EncoderWithLeds {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let size = self.size;
        let mask = self.mask();
        let mut encoder = div()
            .size(px(size))
            .flex_shrink_0()
            .cursor(CursorStyle::OpenHand)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        let (center, diameter) = geometry(bounds);
                        circle(window, center, diameter * 0.49, 0x171616);
                        circle(window, center, diameter * 0.475, 0x343638);
                        for segment in 0..9 {
                            annular_segment(
                                window,
                                center,
                                diameter * 0.395,
                                diameter * 0.468,
                                136.0 + segment as f32 * 30.0,
                                164.0 + segment as f32 * 30.0,
                                if mask & (1 << segment) != 0 {
                                    0xf59a52
                                } else {
                                    0x56595d
                                },
                            );
                        }
                        circle(window, center, diameter * 0.375, 0x151618);
                        circle(window, center, diameter * 0.350, 0x37393c);
                        circle(window, center, diameter * 0.294, 0x282a2c);
                        circle(window, center, diameter * 0.279, 0x494b4e);
                    },
                )
                .size_full(),
            );
        if let Some(input) = self.input {
            if let Some(on_start) = input.drag_start {
                encoder = encoder.on_mouse_down(MouseButton::Left, move |event, _, _| {
                    on_start(event.position.x.as_f32(), event.position.y.as_f32());
                });
            }
            if let Some(on_scroll) = input.scroll {
                encoder = encoder.on_scroll_wheel(move |event, _, cx| {
                    let delta = match event.delta {
                        ScrollDelta::Lines(p) => {
                            if p.y.abs() >= p.x.abs() {
                                p.y
                            } else {
                                p.x
                            }
                        }
                        ScrollDelta::Pixels(p) => {
                            if p.y.abs() >= p.x.abs() {
                                p.y.as_f32() / 24.0
                            } else {
                                p.x.as_f32() / 24.0
                            }
                        }
                    };
                    on_scroll(delta);
                    cx.stop_propagation();
                });
            }
            if let Some(on_end) = input.drag_end {
                let on_out = on_end.clone();
                encoder = encoder
                    .on_mouse_up(MouseButton::Left, move |_, _, _| on_end())
                    .on_mouse_up_out(MouseButton::Left, move |_, _, _| on_out());
            }
        }
        encoder
    }
}

#[cfg(test)]
mod tests {
    use super::{EncoderLedLevel, EncoderWithLeds};

    #[test]
    fn encoder_level_selects_one_led_and_preserves_firmware_masks() {
        assert_eq!(
            EncoderWithLeds::new(20.0, EncoderLedLevel::Position(Some(4))).mask(),
            1 << 4
        );
        assert_eq!(
            EncoderWithLeds::new(20.0, EncoderLedLevel::Position(Some(9))).mask(),
            0
        );
        assert_eq!(
            EncoderWithLeds::new(20.0, EncoderLedLevel::Segments(0x12f)).mask(),
            0x12f
        );
        assert_eq!(
            EncoderWithLeds::new(20.0, EncoderLedLevel::Segments(0xffff)).mask(),
            0x1ff
        );
    }
}

pub fn geometry(bounds: Bounds<Pixels>) -> (Point<Pixels>, f32) {
    (
        point(
            bounds.origin.x + bounds.size.width * 0.5,
            bounds.origin.y + bounds.size.height * 0.5,
        ),
        bounds.size.width.as_f32().min(bounds.size.height.as_f32()),
    )
}

fn offset(center: Point<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(center.x + px(x), center.y + px(y))
}

fn polar(center: Point<Pixels>, radius: f32, degrees: f32) -> Point<Pixels> {
    let radians = degrees.to_radians();
    offset(center, radius * radians.cos(), radius * radians.sin())
}

fn circle(window: &mut Window, center: Point<Pixels>, radius: f32, color: u32) {
    if radius <= 0.0 {
        return;
    }
    let mut builder = PathBuilder::fill();
    builder.move_to(offset(center, radius, 0.0));
    builder.arc_to(
        point(px(radius), px(radius)),
        px(0.0),
        false,
        true,
        offset(center, -radius, 0.0),
    );
    builder.arc_to(
        point(px(radius), px(radius)),
        px(0.0),
        false,
        true,
        offset(center, radius, 0.0),
    );
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(color));
    }
}

fn annular_segment(
    window: &mut Window,
    center: Point<Pixels>,
    inner: f32,
    outer: f32,
    start: f32,
    end: f32,
    color: u32,
) {
    if outer <= 0.0 {
        return;
    }
    let mut builder = PathBuilder::fill();
    builder.move_to(polar(center, outer, start));
    builder.arc_to(
        point(px(outer), px(outer)),
        px(0.0),
        false,
        true,
        polar(center, outer, end),
    );
    builder.line_to(polar(center, inner, end));
    builder.arc_to(
        point(px(inner), px(inner)),
        px(0.0),
        false,
        false,
        polar(center, inner, start),
    );
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(color));
    }
}
