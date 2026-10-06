//! Native panel controls. Firmware input handlers belong to the panel hit zones.
use gpui::{
    Div, PathBuilder, Pixels, Point, Svg, Window, canvas, div, point, prelude::*, px, rgb, svg,
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

/// Dimensions and scale are supplied by the shared panel layout.
pub fn button(
    w: f32,
    h: f32,
    scale: f32,
    label: &str,
    symbol: Option<&str>,
    lit: Option<u32>,
) -> Div {
    let face = if symbol == Some("record") {
        0xd5d8dc
    } else {
        lit.unwrap_or(0xd5d8dc)
    };
    let control = div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(w))
        .h(px(h))
        .flex_shrink_0()
        .rounded(px(3.0 * scale))
        .border(px(scale))
        .border_color(rgb(0x555a60))
        .bg(rgb(0x72777d))
        .child(
            div()
                .absolute()
                .left(px(scale))
                .right(px(scale))
                .top(px(scale))
                .bottom(px(2.0 * scale))
                .rounded(px(2.0 * scale))
                .bg(rgb(face)),
        );
    // Face and content are siblings in the same positioned paint order. A
    // normal flow text node otherwise paints beneath the absolute face.
    let mut content = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .font_family("Helvetica Neue")
        .text_size(px(if label.len() > 5 {
            7.0 * scale
        } else {
            9.0 * scale
        }))
        .text_color(rgb(0x282b30))
        .line_height(px(if label.contains('\n') {
            7.0 * scale
        } else {
            10.0 * scale
        }))
        .text_center();
    content = match symbol {
        Some("play-stop") => content
            .gap(px(2.0 * scale))
            .child(icon("play", h * 0.52, 0x282b30))
            .child(div().text_size(px(11.0 * scale)).child("/"))
            .child(icon("square", h * 0.46, 0x282b30)),
        Some("record") => content.child(
            div()
                .w(px(h * 0.43))
                .h(px(h * 0.43))
                .rounded_full()
                .bg(rgb(lit.unwrap_or(0xcd1930))),
        ),
        Some("usb") if !label.is_empty() => content
            .flex_col()
            .child(icon("usb", h * 0.47, 0x282b30))
            .child(
                div()
                    .text_size(px(7.0 * scale))
                    .line_height(px(7.0 * scale))
                    .child(label.to_owned()),
            ),
        Some(name) => content.child(icon(name, (h * 0.7).min(w * 0.65), 0x282b30)),
        None => content.child(label.to_owned()),
    };
    control.child(content)
}

/// Circular power key with the same public icon as the panel legends.
pub fn power(size: f32, scale: f32) -> Div {
    div()
        .relative()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .rounded_full()
        .border(px(scale))
        .border_color(rgb(0x202326))
        .bg(rgb(0x898d91))
        .child(
            div()
                .absolute()
                .inset(px(2.0 * scale))
                .rounded_full()
                .bg(rgb(0xb8bdc1)),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(icon("power", size * 0.6, 0x565b5f)),
        )
}

/// Common physical switch geometry, with a cap translated inside its track.
pub fn switch(w: f32, h: f32, on: bool) -> Div {
    let short = w.min(h);
    let cap = short * 0.76;
    let inset = short * 0.12;
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_shrink_0()
        .rounded(px(short * 0.5))
        .bg(rgb(0x25272a))
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(short * 0.5))
                .border(px(short * 0.08))
                .border_color(rgb(0x141619)),
        )
        .child(
            div()
                .absolute()
                .top(px(if h > w && on { h - cap - inset } else { inset }))
                .left(px(if w >= h && on { w - cap - inset } else { inset }))
                .w(px(cap))
                .h(px(cap))
                .rounded_full()
                .border(px(short * 0.04))
                .border_color(rgb(0x73777b))
                .bg(rgb(0x424549)),
        )
}

/// TRS or XLR/TRS combo socket, sharing the same concentric construction.
pub fn socket(size: f32, combo: bool) -> Div {
    if combo {
        let mut face = div().relative().size(px(size)).flex_shrink_0().child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let d = bounds.size.width.as_f32().min(bounds.size.height.as_f32());
                    let c = point(bounds.origin.x + px(d * 0.5), bounds.origin.y + px(d * 0.5));
                    // Molded flange, recessed insert and keyed combo opening.
                    for (radius, color) in [
                        (0.495, 0x141516),
                        (0.470, 0x393b3d),
                        (0.445, 0x111213),
                        (0.397, 0x080909),
                        (0.365, 0x454648),
                        (0.342, 0x3a3c3e),
                        (0.145, 0x050606),
                    ] {
                        circle(window, c, d * radius, color);
                    }
                    for (dx, dy) in [(-0.165, 0.0), (0.165, 0.0), (0.0, 0.165)] {
                        circle(
                            window,
                            point(c.x + px(d * dx), c.y + px(d * dy)),
                            d * 0.069,
                            0x020303,
                        );
                    }
                    let mut notch = PathBuilder::fill();
                    for (i, (x, y)) in [
                        (0.44, 0.024),
                        (0.56, 0.024),
                        (0.56, 0.09),
                        (0.53, 0.115),
                        (0.47, 0.115),
                        (0.44, 0.09),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let p = point(bounds.origin.x + px(d * x), bounds.origin.y + px(d * y));
                        if i == 0 {
                            notch.move_to(p);
                        } else {
                            notch.line_to(p);
                        }
                    }
                    notch.close();
                    if let Ok(path) = notch.build() {
                        window.paint_path(path, rgb(0x080909));
                    }
                },
            )
            .size_full(),
        );
        for (x, y, number) in [(0.27, 0.25, "2"), (0.27, 0.61, "3"), (0.70, 0.52, "1")] {
            face = face.child(
                div()
                    .absolute()
                    .left(px(size * x))
                    .top(px(size * y))
                    .text_size(px(size * 0.09))
                    .line_height(px(size * 0.11))
                    .text_color(rgb(0x616367))
                    .child(number),
            );
        }
        // Raised molded badge above the three contact cavities.
        face = face.child(
            div()
                .absolute()
                .left(px(size * 0.465))
                .top(px(size * 0.225))
                .size(px(size * 0.07))
                .rounded_full()
                .border(px(size * 0.009))
                .border_color(rgb(0x626468)),
        );
        return face;
    }
    div().size(px(size)).flex_shrink_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let d = bounds.size.width.as_f32().min(bounds.size.height.as_f32());
                let center = point(bounds.origin.x + px(d * 0.5), bounds.origin.y + px(d * 0.5));
                circle(window, center, d * 0.49, 0x16191b);
                polygon(window, center, d * 0.48, 6, 0.0, 0xb5babd);
                polygon(window, center, d * 0.435, 6, 0.0, 0x777c80);
                circle(window, center, d * 0.40, 0xd4d8db);
                circle(window, center, d * 0.365, 0x363a3d);
                circle(window, center, d * 0.335, 0x969c9f);
                circle(window, center, d * 0.295, 0x2b2d30);
                circle(window, center, d * 0.26, 0x5b6064);
                circle(window, center, d * 0.225, 0x090a0b);
            },
        )
        .size_full(),
    )
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

/// USB receptacle, distinct from the printed USB function icon.
pub fn usb_socket(w: f32, h: f32) -> Div {
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .rounded(px(h * 0.30))
        .border(px(h * 0.10))
        .border_color(rgb(0x080808))
        .bg(rgb(0x121212))
        .child(
            div()
                .absolute()
                .left(px(w * 0.20))
                .right(px(w * 0.20))
                .top(px(h * 0.37))
                .h(px(h * 0.20))
                .bg(rgb(0x303030)),
        )
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

fn polygon(
    window: &mut Window,
    center: Point<Pixels>,
    radius: f32,
    sides: usize,
    angle: f32,
    color: u32,
) {
    let mut path = PathBuilder::fill();
    for side in 0..sides {
        let a = angle + side as f32 / sides as f32 * std::f32::consts::TAU;
        let p = point(
            center.x + px(radius * a.cos()),
            center.y + px(radius * a.sin()),
        );
        if side == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color));
    }
}

/// Color is the decoded hardware output; None is the unlit lens.
pub fn led(size: f32, color: Option<u32>) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .rounded_full()
        .border(px((size * 0.13).max(0.5)))
        .border_color(rgb(0x27292b))
        .bg(rgb(color.unwrap_or(0x55594b)))
}
