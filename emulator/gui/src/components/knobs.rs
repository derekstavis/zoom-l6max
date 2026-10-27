//! Native knobs. Firmware state supplies the ring bits and ADC position;
//! gesture handling belongs to the panel's input layer.
use gpui::{
    Bounds, Div, PathBuilder, Pixels, Point, Window, canvas, div, point, prelude::*, px, rgb,
};

pub fn knob_with_input(
    size: f32,
    color: u32,
    value: u32,
    input: super::encoder::EncoderInput,
) -> Div {
    let mut knob = knob(size, color, value).cursor(gpui::CursorStyle::OpenHand);
    if let Some(on_start) = input.drag_start {
        knob = knob.on_mouse_down(gpui::MouseButton::Left, move |event, _, _| {
            on_start(event.position.x.as_f32(), event.position.y.as_f32());
        });
    }
    if let Some(on_scroll) = input.scroll {
        knob = knob.on_scroll_wheel(move |event, _, cx| {
            let delta = match event.delta {
                gpui::ScrollDelta::Lines(p) => {
                    if p.y.abs() >= p.x.abs() {
                        p.y
                    } else {
                        p.x
                    }
                }
                gpui::ScrollDelta::Pixels(p) => {
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
        knob = knob
            .on_mouse_up(gpui::MouseButton::Left, move |_, _, _| on_end())
            .on_mouse_up_out(gpui::MouseButton::Left, move |_, _, _| on_out());
    }
    knob
}

/// A colored analog knob. ADC values are clamped to the physical 0..1023 range.
/// The pointer travels 270 degrees, with the midpoint pointing straight up.
pub fn knob(size: f32, color: u32, value: u32) -> Div {
    div().size(px(size)).flex_shrink_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let (center, diameter) = geometry(bounds);
                circle(window, center, diameter * 0.465, 0x171719);
                circle(window, center, diameter * 0.433, mix(color, 0x000000, 0.48));
                circle(window, center, diameter * 0.400, mix(color, 0xffffff, 0.14));
                circle(window, center, diameter * 0.385, color);
                let angle = pointer_angle(value);
                let start = polar(center, diameter * 0.17, angle);
                let end = polar(center, diameter * 0.35, angle);
                let marker = mix(color, 0x000000, 0.62);
                // The molded marker is a recessed rounded slot, including on
                // the dark monitor/sub-out caps; it is not a luminous pointer.
                capsule(
                    window,
                    start,
                    end,
                    diameter * 0.072,
                    mix(color, 0xffffff, 0.08),
                );
                capsule(window, start, end, diameter * 0.055, marker);
            },
        )
        .size_full(),
    )
}

fn geometry(bounds: Bounds<Pixels>) -> (Point<Pixels>, f32) {
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

fn pointer_angle(value: u32) -> f32 {
    -225.0 + value.min(1023) as f32 / 1023.0 * 270.0
}

fn mix(color: u32, other: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let a = ((color >> shift) & 255u32) as f32;
        let b = ((other >> shift) & 255u32) as f32;
        ((a + (b - a) * amount).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

fn capsule(window: &mut Window, start: Point<Pixels>, end: Point<Pixels>, width: f32, color: u32) {
    if width <= 0.0 {
        return;
    }
    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(start);
    builder.line_to(end);
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgb(color));
    }
    circle(window, start, width * 0.5, color);
    circle(window, end, width * 0.5, color);
}
