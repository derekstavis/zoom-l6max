//! Native panel controls. Firmware input handlers belong to the panel hit zones.
use gpui::{
    Div, IntoElement, PathBuilder, Pixels, Point, Svg, Window, canvas, div, point, prelude::*, px,
    rgb, svg,
};

/// Embedded public icon assets; GPUI caches each SVG mask by its byte hash.
pub fn icon(name: &str, size: f32, color: u32) -> Svg {
    let bytes: &[u8] = match name {
        "power" => include_bytes!("../../assets/icons/power.svg"),
        "volume-off" => include_bytes!("../../assets/icons/phosphor/speaker-simple-slash-fill.svg"),
        "check" => include_bytes!("../../assets/icons/phosphor/check-bold.svg"),
        "menu" => include_bytes!("../../assets/icons/menu.svg"),
        "chevron-up" => include_bytes!("../../assets/icons/phosphor/caret-up-fill.svg"),
        "chevron-down" => include_bytes!("../../assets/icons/phosphor/caret-down-fill.svg"),
        "play" => include_bytes!("../../assets/icons/phosphor/play-fill.svg"),
        "square" => include_bytes!("../../assets/icons/phosphor/stop-fill.svg"),
        "skip-back" => include_bytes!("../../assets/icons/phosphor/rewind-fill.svg"),
        "skip-forward" => include_bytes!("../../assets/icons/phosphor/fast-forward-fill.svg"),
        "usb" => include_bytes!("../../assets/icons/usb.svg"),
        "headphones" => include_bytes!("../../assets/icons/headphones.svg"),
        "mic" => include_bytes!("../../assets/icons/mic.svg"),
        "circle" => include_bytes!("../../assets/icons/circle.svg"),
        "laptop-minimal" => include_bytes!("../../assets/icons/laptop-minimal.svg"),
        _ => panic!("Unknown panel icon: {name}"),
    };
    svg()
        .data(bytes)
        .w(px(size))
        .h(px(size))
        .text_color(rgb(color))
}

pub fn button_with_events(
    w: f32,
    h: f32,
    scale: f32,
    label: &str,
    symbol: Option<&str>,
    lit: Option<u32>,
    events: super::button::ButtonEvents,
) -> impl gpui::IntoElement {
    super::button::Button::new(w, h, scale, label, symbol, lit).with_events(events)
}

pub fn power_with_press(
    size: f32,
    scale: f32,
    on_press: impl Fn() + Send + Sync + 'static,
) -> impl gpui::IntoElement {
    super::power_button::PowerButton::new(size, scale).on_press(on_press)
}

/// The engine keeps a press down for its requested duration, so a release only
/// shortens the hold when a shorter one was requested.
pub fn power_with_events(
    size: f32,
    scale: f32,
    on_press: impl Fn() + Send + Sync + 'static,
    on_release: impl Fn() + Send + Sync + 'static,
) -> impl gpui::IntoElement {
    super::power_button::PowerButton::new(size, scale)
        .on_press(on_press)
        .on_release(on_release)
}

pub fn switch_with_toggle(
    w: f32,
    h: f32,
    on: bool,
    on_toggle: impl Fn() + Send + Sync + 'static,
) -> impl gpui::IntoElement {
    super::switch::Switch::new(w, h, on).on_toggle(on_toggle)
}

/// Compatibility facade for the model-specific jack components.
pub fn socket(size: f32, combo: bool) -> impl gpui::IntoElement {
    if combo {
        super::xlr_jack::XlrJack::new(size).into_any_element()
    } else {
        super::trs_jack::TrsJack::new(size).into_any_element()
    }
}

/// Recessed Phillips fastener, shared by all eight panel mounting locations.
pub fn screw(size: f32) -> Div {
    div().size(px(size)).flex_shrink_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let d = bounds.size.width.as_f32().min(bounds.size.height.as_f32());
                let c = point(bounds.origin.x + px(d * 0.5), bounds.origin.y + px(d * 0.5));
                circle(window, c, d * 0.49, 0x121416);
                circle(window, c, d * 0.40, 0x63686c);
                circle(window, c, d * 0.33, 0x292c2f);
                circle(window, c, d * 0.26, 0x383c3f);
                let mut recess = PathBuilder::stroke(px(d * 0.14));
                recess.move_to(point(c.x - px(d * 0.21), c.y));
                recess.line_to(point(c.x + px(d * 0.21), c.y));
                recess.move_to(point(c.x, c.y - px(d * 0.21)));
                recess.line_to(point(c.x, c.y + px(d * 0.21)));
                if let Ok(path) = recess.build() {
                    window.paint_path(path, rgb(0x08090a));
                }
            },
        )
        .size_full(),
    )
}

/// The pale printed patches above recorder navigation keys.
pub fn legend(w: f32, h: f32, name: &str) -> Div {
    div()
        .w(px(w))
        .h(px(h))
        .rounded(px(h * 0.08))
        .bg(rgb(0xd3dadf))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(name, h * 0.88, 0x35393d))
}

/// Track navigation combines public double triangles with the printed end bar.
pub fn transport_legend(size: f32, forward: bool, color: u32) -> Div {
    let bar = div().w(px(size * 0.065)).h(px(size * 0.52)).bg(rgb(color));
    let triangles = icon(
        if forward { "skip-forward" } else { "skip-back" },
        size,
        color,
    );
    let content = div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(size * 0.025))
        .h(px(size))
        .flex_shrink_0();
    if forward {
        content.child(triangles).child(bar)
    } else {
        content.child(bar).child(triangles)
    }
}

/// Compatibility facade for the USB-C receptacle component.
pub fn usb_socket(w: f32, h: f32) -> impl gpui::IntoElement {
    super::usb_c_port::UsbCPort::new(w, h)
}

/// Compatibility facade for the five-pin MIDI jack component.
pub fn midi_jack(size: f32, scale: f32) -> impl gpui::IntoElement {
    super::midi_jack::MidiJack::new(size, scale)
}

fn circle(window: &mut Window, center: Point<Pixels>, radius: f32, color: u32) {
    let mut path = PathBuilder::fill();
    path.move_to(point(center.x + px(radius), center.y));
    path.arc_to(
        point(px(radius), px(radius)),
        px(0.0),
        false,
        true,
        point(center.x - px(radius), center.y),
    );
    path.arc_to(
        point(px(radius), px(radius)),
        px(0.0),
        false,
        true,
        point(center.x + px(radius), center.y),
    );
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color));
    }
}

/// Compatibility facade; decoded LED state is supplied as component props.
pub fn led(size: f32, color: Option<u32>) -> impl gpui::IntoElement {
    super::indicator_led::IndicatorLed::new(size, color)
}
