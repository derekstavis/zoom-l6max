//! Native device composition; firmware transport stays outside visual components.
use super::{
    controls, knobs,
    layout::{
        BODY, CHANNEL_WIDTH, GLOBAL_CENTERS, LCD_GLASS, Layout, MODE_X, RECORDER, Region, channel_x,
    },
};
use gpui::{Div, IntoElement, div, prelude::*, px, rgb};
use l6max_host::indicators;

const SCREWS: [(f32, f32); 8] = [
    (147.0, 136.0),
    (87.0, 218.0),
    (220.0, 234.0),
    (309.0, 136.0),
    (249.0, 218.0),
    (382.0, 234.0),
    (160.0, 314.0),
    (322.0, 314.0),
];

fn at(region: Region, s: f32, content: impl IntoElement) -> Div {
    at_local(region.local(), s, content)
}
fn at_local(region: Region, s: f32, content: impl IntoElement) -> Div {
    div()
        .absolute()
        .left(px(region.x * s))
        .top(px(region.y * s))
        .w(px(region.width * s))
        .h(px(region.height * s))
        .child(content)
}
fn label(x: f32, y: f32, w: f32, text: &str, s: f32) -> Div {
    label_sized(x, y, w, text, s, 8.0)
}
fn label_sized(x: f32, y: f32, w: f32, text: &str, s: f32, font: f32) -> Div {
    at(
        Region::new(x, y, w, 20.0),
        s,
        div()
            .relative()
            .size_full()
            .font_family("Helvetica Neue")
            .line_height(px(14.0 * s))
            .text_center()
            .text_size(px(font * s))
            .text_color(rgb(0xe1e6e9))
            .child(text.to_owned()),
    )
}
fn line(x: f32, y: f32, w: f32, h: f32, s: f32) -> Div {
    at(
        Region::new(x, y, w, h),
        s,
        div().size_full().bg(rgb(0xc4cbcd)),
    )
}
// Reserve clear space around physical caps and screw heads along printed rules.
fn channel_divider(x: f32, y: f32, height: f32, s: f32, layout: &Layout) -> Div {
    let mut gaps = Vec::new();
    let mut reserve = |r: Region, horizontal_clearance: f32| {
        let clearance = 4.0;
        if x + 1.5 > r.x - horizontal_clearance && x < r.x + r.width + horizontal_clearance {
            let start = (r.y - clearance).max(y);
            let end = (r.y + r.height + clearance).min(y + height);
            if end > start {
                gaps.push((start, end));
            }
        }
    };
    for (sx, sy) in SCREWS {
        reserve(Region::centered(sx, sy, 14.0, 14.0), 4.0);
    }
    for (_, r) in &layout.buttons {
        reserve(
            Region::new(r.x + BODY.x, r.y + BODY.y, r.width, r.height),
            1.0,
        );
    }
    // Project each circular socket onto the divider; keep clearance from its rim.
    for (cx, cy) in [
        (180.0, 184.0),
        (128.0, 270.0),
        (342.0, 184.0),
        (290.0, 270.0),
    ] {
        let dx = x + 0.75 - cx;
        let radius: f32 = 44.0;
        if dx.abs() < radius {
            let reach = (radius * radius - dx * dx).sqrt();
            let start = (cy - reach).max(y);
            let end = (cy + reach).min(y + height);
            if end > start {
                gaps.push((start, end));
            }
        }
    }
    gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut rules = div().absolute().inset_0();
    let mut cursor = y;
    for (start, end) in gaps {
        if start - cursor >= 3.0 {
            rules = rules.child(line(x, cursor, 1.5, start - cursor, s));
        }
        cursor = cursor.max(end);
    }
    if y + height - cursor >= 3.0 {
        rules = rules.child(line(x, cursor, 1.5, y + height - cursor, s));
    }
    rules
}

fn face(id: u32) -> (&'static str, Option<&'static str>, Option<(usize, u32)>) {
    match id {
        0 => ("", None, None),
        1 => ("", None, None),
        2 => ("", None, None),
        3 => ("", None, None),
        4 => ("", Some("play-stop"), None),
        5 => ("", Some("record"), Some((31, 0xef6d66))),
        6 => ("BOUNCE", None, None),
        7 => ("Hi-Z", None, Some((8, 0xf59a52))),
        12 => ("Hi-Z", None, Some((9, 0xf59a52))),
        17 => ("48V", None, Some((11, 0xef6d66))),
        32 => ("48V", None, Some((12, 0xef6d66))),
        22 | 27 | 37 | 42 | 29 | 34 | 39 | 44 => (
            "",
            Some("volume-off"),
            Some((
                [22, 27, 37, 42, 29, 34, 39, 44]
                    .iter()
                    .position(|x| *x == id)
                    .unwrap(),
                0xef6d66,
            )),
        ),
        9 => ("MONO\n×2", None, Some((13, 0xf59a52))),
        14 => ("MONO\n×2", None, Some((14, 0xf59a52))),
        19 => ("1/2", Some("usb"), Some((15, 0xf59a52))),
        24 => ("3/4", Some("usb"), Some((16, 0xf59a52))),
        28 => ("HIGH", None, Some((17, 0xf59a52))),
        33 => ("FREQ", None, Some((18, 0xf59a52))),
        38 => ("MID", None, Some((19, 0xf59a52))),
        43 => ("LOW", None, Some((20, 0xf59a52))),
        10 => ("AUX1", None, Some((21, 0xf59a52))),
        15 => ("AUX2", None, Some((22, 0xf59a52))),
        20 => ("EFX", None, Some((23, 0xf59a52))),
        25 => ("SUB-MIX", None, Some((24, 0xf59a52))),
        30 => ("PAN", None, Some((25, 0xf59a52))),
        35 => ("LEVEL", None, Some((26, 0xf59a52))),
        40 => ("SEL", None, None),
        45 => ("COMP", None, Some((10, 0xf59a52))),
        11 => ("A", None, Some((40, 0xef6d66))),
        16 => ("B", None, Some((41, 0xef6d66))),
        21 => ("C", None, Some((42, 0xef6d66))),
        26 => ("D", None, Some((43, 0xef6d66))),
        48 => ("TAP", None, Some((33, 0xf59a52))),
        49 => ("1", None, Some((27, 0xef6d66))),
        50 => ("2", None, Some((28, 0xef6d66))),
        51 => ("3", None, Some((29, 0xef6d66))),
        52 => ("4", None, Some((30, 0xef6d66))),
        53 => ("", Some("power"), None),
        _ => ("", None, None),
    }
}
pub fn panel(
    layout: &Layout,
    scale: f32,
    rows: [u32; 8],
    switches: [bool; 54],
    analog: [u16; 5],
) -> Div {
    let s = scale;
    let leds = indicators::leds(rows);
    let rings = indicators::rings(rows);
    let mut panel = div()
        .relative()
        .size_full()
        .font_family("Helvetica Neue")
        .bg(rgb(0x000000));
    panel = panel.child(at(
        BODY,
        s,
        div()
            .size_full()
            .rounded(px(31.0 * s))
            .border(px(2.0 * s))
            .border_color(rgb(0x111111))
            .bg(rgb(0x444548)),
    ));
    for (x, y) in SCREWS {
        panel = panel.child(at(
            Region::centered(x, y, 14.0, 14.0),
            s,
            controls::screw(14.0 * s),
        ));
    }
    // Eight identical strips share socket rows, button rows, and encoder baseline.
    for ch in 0..8 {
        let x = channel_x(ch);
        panel = panel
            .child(at(
                Region::new(x, 431.0, CHANNEL_WIDTH, 78.0),
                s,
                div().size_full().bg(rgb(0x39358b)),
            ))
            .child(channel_divider(
                x,
                if ch <= 4 { 118.0 } else { 139.0 },
                if ch < 4 {
                    [0.0, 27.0, 91.0, 10.0][ch]
                } else {
                    if ch == 4 { 180.0 } else { 159.0 }
                },
                s,
                layout,
            ))
            .child(channel_divider(
                x,
                if ch == 2 {
                    237.0
                } else if ch < 4 {
                    325.0
                } else {
                    338.0
                },
                if ch == 2 {
                    272.0
                } else if ch < 4 {
                    184.0
                } else {
                    171.0
                },
                s,
                layout,
            ))
            .child(label(x, 350.0, CHANNEL_WIDTH, "SIGNAL", s))
            .child(at(
                Region::centered(x + CHANNEL_WIDTH / 2.0, 342.0, 8.0, 8.0),
                s,
                controls::led(
                    8.0 * s,
                    if leds[44 + ch] {
                        Some(0xef4040)
                    } else if leds[52 + ch] {
                        Some(0x76db52)
                    } else {
                        None
                    },
                ),
            ))
            .child(label_sized(
                x,
                511.0,
                CHANNEL_WIDTH,
                &if ch < 4 {
                    format!("{}", ch + 1)
                } else {
                    format!("{}(ST)", ch + 1)
                },
                s,
                12.0,
            ));
        if ch < 4 {
            let cx = if ch % 2 == 0 { 180.0 } else { 128.0 } + 162.0 * (ch / 2) as f32;
            let cy = if ch % 2 == 0 { 184.0 } else { 270.0 };
            panel = panel.child(at(
                Region::centered(cx, cy, 80.0, 80.0),
                s,
                controls::socket(80.0 * s, true),
            ));
        } else {
            for y in [163.0, 230.0] {
                panel = panel.child(at(
                    Region::centered(x + CHANNEL_WIDTH / 2.0, y, 49.0, 49.0),
                    s,
                    controls::socket(49.0 * s, false),
                ));
            }
            panel = panel
                .child(label(
                    x,
                    194.0,
                    if ch >= 6 { 40.0 } else { 16.0 },
                    if ch >= 6 { "L(MONO)" } else { "L" },
                    s,
                ))
                .child(label(x, 261.0, 16.0, "R", s));
            for (dx, w, text) in [(2.0, 22.0, "0dB"), (50.0, 25.0, "-20dB")] {
                panel = panel.child(at(
                    Region::new(x + dx, 277.0, w, 10.0),
                    s,
                    div()
                        .size_full()
                        .text_center()
                        .text_size(px(6.0 * s))
                        .text_color(rgb(0xe1e6e9))
                        .child(text),
                ));
            }
        }
    }
    for (x, y, height) in [(164.0, 235.0, 14.0), (316.0, 235.0, 14.0)] {
        panel = panel.child(channel_divider(x, y, height, s, layout));
    }
    panel = panel
        .child(label_sized(70.0, 115.0, 44.0, "POWER", s, 10.0))
        .child(at(
            Region::new(74.0, 174.0, 60.0, 35.0),
            s,
            div()
                .text_size(px(7.0 * s))
                .line_height(px(9.0 * s))
                .text_color(rgb(0xe1e6e9))
                .child("XLR:MIC\nTRS:LINE\n(+4dBu BAL)"),
        ))
        .child(at(
            Region::new(342.0, 306.0, 123.0, 23.0),
            s,
            div()
                .text_size(px(16.0 * s))
                .text_color(rgb(0xd5e2e9))
                .child("EMULATOR"),
        ))
        .child(at(
            Region::new(471.0, 303.0, 108.0, 28.0),
            s,
            div()
                .text_size(px(20.0 * s))
                .text_color(rgb(0xd5e2e9))
                .child("Mixer"),
        ))
        .child(at(
            Region::new(582.0, 299.0, 112.0, 30.0),
            s,
            div()
                .text_size(px(25.0 * s))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(0xeeeeee))
                .child("L6max"),
        ))
        .child(label(496.0, 117.0, 104.0, "LINE (-10dBu UNBAL)", s))
        .child(line(431.0, 124.0, 58.0, 1.0, s))
        .child(line(606.0, 124.0, 53.0, 1.0, s))
        .child(at(
            Region::new(MODE_X, 119.0, 62.0, 390.0),
            s,
            div().size_full().bg(rgb(0x39358b)),
        ))
        .child(label_sized(MODE_X, 130.0, 62.0, "EQ", s, 10.0))
        .child(label_sized(MODE_X, 277.0, 62.0, "SEND", s, 10.0));
    for x in [MODE_X, 760.0, 832.0, 902.0, 972.0, 1120.0] {
        panel = panel.child(line(x, 119.0, 1.5, 390.0, s));
    }
    // The wide SUB-MIX legend extends left of its column boundary.
    panel = panel
        .child(line(1042.0, 119.0, 1.5, 144.0, s))
        .child(line(1042.0, 282.0, 1.5, 227.0, s));
    panel = panel.child(label_sized(764.0, 125.0, 64.0, "MIDI", s, 10.0));
    for (y, text) in [(148.0, "IN"), (193.0, "OUT")] {
        panel = panel
            .child(at(
                Region::centered(796.0, y, 14.0, 14.0),
                s,
                div()
                    .size(px(14.0 * s))
                    .rounded_full()
                    .border(px(s))
                    .border_color(rgb(0x080808))
                    .bg(rgb(0x151515)),
            ))
            .child(label(767.0, y - 5.0, 18.0, text, s));
    }
    panel = panel
        .child(label(764.0, 247.0, 64.0, "AUDIO/MIDI", s))
        .child(at(
            Region::new(849.0, 275.0, 34.0, 9.0),
            s,
            div()
                .size_full()
                .bg(rgb(0xd0dce0))
                .text_color(rgb(0x293138))
                .text_size(px(7.0 * s))
                .line_height(px(9.0 * s))
                .text_center()
                .child("LEARN"),
        ));
    for (x, title) in [
        (866.0, "AUX SEND"),
        (936.0, "MASTER"),
        (1006.0, "MONITOR"),
        (1076.0, "SUB-OUT"),
    ] {
        panel = panel.child(at(
            Region::new(x - 33.0, 119.0, 66.0, 14.0),
            s,
            div()
                .size_full()
                .bg(rgb(0xd0dce0))
                .text_color(rgb(0x292d30))
                .text_size(px(9.0 * s))
                .text_center()
                .child(title),
        ));

        panel = panel.child(at(
            Region::centered(x, 160.0, 49.0, 49.0),
            s,
            controls::socket(49.0 * s, false),
        ));
        if x < 972.0 {
            panel = panel.child(at(
                Region::centered(x, 230.0, 49.0, 49.0),
                s,
                controls::socket(49.0 * s, false),
            ));
        }
    }
    panel = panel
        .child(label_sized(974.0, 207.0, 64.0, "SCENE", s, 10.0))
        .child(label_sized(1046.0, 206.0, 60.0, "MASTER", s, 10.0))
        .child(label_sized(1046.0, 263.0, 60.0, "SUB-MIX", s, 10.0));
    panel = panel
        .child(at(
            Region::new(849.0, 316.0, 44.0, 16.0),
            s,
            div()
                .size_full()
                .bg(rgb(0xd0dce0))
                .pl(px(s))
                .text_left()
                .text_size(px(7.5 * s))
                .line_height(px(8.0 * s))
                .text_color(rgb(0x293138))
                .child("AI Noise\nReduction"),
        ))
        .child(at(
            Region::centered(843.0, 322.0, 6.0, 6.0),
            s,
            controls::led(6.0 * s, leds[39].then_some(0x76db52)),
        ));
    for (i, name) in ["Hall", "Room", "Spring", "Delay", "Echo"]
        .iter()
        .enumerate()
    {
        let y = 344.0 + 11.0 * i as f32;
        panel = panel
            .child(at(
                Region::centered(843.0, y, 6.0, 6.0),
                s,
                controls::led(6.0 * s, leds[34 + i].then_some(0x76db52)),
            ))
            .child(at(
                Region::new(850.0, y - 5.0, 42.0, 14.0),
                s,
                div()
                    .size_full()
                    .text_left()
                    .text_size(px(8.0 * s))
                    .line_height(px(14.0 * s))
                    .text_color(rgb(0xe1e6e9))
                    .child(*name),
            ));
    }
    panel = panel
        .child(at(
            RECORDER,
            s,
            div().size_full().rounded(px(5.0 * s)).bg(rgb(0x090a0b)),
        ))
        .child(label_sized(969.0, 421.0, 145.0, "RECORDER", s, 10.0));
    panel = panel.child(at(LCD_GLASS, s, div().size_full().bg(rgb(0x111417))));
    for (i, name) in ["menu", "chevron-up", "chevron-down", "check"]
        .iter()
        .enumerate()
    {
        let r = layout.button(i as u32);
        let x = r.x + r.width / 2.0;
        panel = panel.child(at_local(
            Region::centered(x, 356.0 - BODY.y, 14.0, 8.0),
            s,
            controls::legend(14.0 * s, 8.0 * s, name),
        ));
        if i == 0 || i == 3 {
            panel = panel.child(at_local(
                Region::centered(x, 386.0 - BODY.y, 32.0, 12.0),
                s,
                div()
                    .size_full()
                    .text_center()
                    .text_size(px(8.0 * s))
                    .text_color(rgb(0xe1e6e9))
                    .child(if i == 0 { "MENU" } else { "UNDO" }),
            ));
        } else {
            panel = panel.child(at_local(
                Region::centered(x, 385.0 - BODY.y, 15.0, 10.0),
                s,
                controls::transport_legend(12.0 * s, i == 2, 0xe1e6e9),
            ));
        }
    }
    // Six paired segments and their calibrated printed scale. Unmapped
    // meter outputs stay unlit; they are never synthesized from knob position.
    panel = panel.child(at(
        Region::new(925.0, 285.0, 24.0, 103.0),
        s,
        div()
            .size_full()
            .rounded(px(2.0 * s))
            .border(px(s))
            .border_color(rgb(0x202225))
            .bg(rgb(0x36393c)),
    ));
    for row in 0..6 {
        for col in 0..2 {
            panel = panel.child(at(
                Region::new(
                    929.0 + col as f32 * 10.0,
                    290.0 + row as f32 * 17.0,
                    6.0,
                    11.0,
                ),
                s,
                div()
                    .size_full()
                    .rounded(px(s))
                    .border(px(s))
                    .border_color(rgb(0x202225))
                    .bg(rgb(0x424549)),
            ));
        }
    }
    for (y, text) in [(290.0, "0"), (307.0, "-6"), (375.0, "-48")] {
        panel = panel.child(label(904.0, y, 18.0, text, s));
    }
    for x in [984.0, 1054.0] {
        panel = panel.child(at(
            Region::new(x, 193.0, 13.0, 13.0),
            s,
            controls::icon("headphones", 13.0 * s, 0xe1e6e9),
        ));
    }
    panel = panel
        .child(at(
            Region::centered(796.0, 226.0, 28.0, 10.0),
            s,
            controls::usb_socket(28.0 * s, 10.0 * s),
        ))
        .child(at(
            Region::centered(796.0, 209.0, 14.0, 14.0),
            s,
            controls::icon("usb", 14.0 * s, 0xe1e6e9),
        ))
        .child(at(
            Region::centered(796.0, 266.0, 17.0, 13.0),
            s,
            controls::icon("laptop-minimal", 17.0 * s, 0xe1e6e9),
        ));
    for (x, top, bottom) in [(839.0, "1", "2"), (909.0, "L", "R")] {
        panel = panel
            .child(label(x, 191.0, 14.0, top, s))
            .child(label(x, 257.0, 14.0, bottom, s));
    }
    for (first, second) in [
        (28, 33),
        (33, 38),
        (38, 43),
        (10, 15),
        (15, 20),
        (20, 25),
        (49, 50),
        (50, 51),
        (51, 52),
    ] {
        let a = layout.button(first);
        let b = layout.button(second);
        let gap = b.y - a.y - a.height;
        let height = (gap - 6.0).clamp(0.0, 12.0);
        panel = panel.child(at_local(
            Region::new(
                a.x + a.width / 2.0,
                a.y + a.height + (gap - height) / 2.0,
                1.5,
                height,
            ),
            s,
            div().size_full().bg(rgb(0xc7c7c7)),
        ));
    }
    panel = panel
        .child(line(705.0, 188.0, 1.5, 37.0, s))
        .child(line(705.0, 188.0, 5.0, 1.5, s))
        .child(line(705.0, 225.0, 5.0, 1.5, s))
        .child(line(1100.0, 392.0, 1.5, 5.0, s))
        .child(line(809.0, 261.0, 3.0, 8.0, s))
        .child(line(796.0, 165.0, 1.5, 15.0, s))
        .child(line(796.0, 419.0, 1.5, 23.0, s));
    panel = panel
        .child(line(866.0, 283.0, 1.5, 3.0, s))
        .child(line(866.0, 392.0, 1.5, 4.0, s))
        .child(line(866.0, 419.0, 1.5, 23.0, s));
    for y in [221.0, 266.0] {
        panel = panel.child(at(
            Region::centered(1076.0, y, 3.5, 3.5),
            s,
            div().size_full().rounded_full().bg(rgb(0xe1e6e9)),
        ));
    }
    for (i, x) in GLOBAL_CENTERS.into_iter().enumerate() {
        for dx in [-15.0, 15.0] {
            panel = panel.child(at(
                Region::centered(x + dx, 495.0, 3.0, 3.0),
                s,
                div().size_full().rounded_full().bg(rgb(0xd6d6d6)),
            ));
        }
        if i == 1 {
            panel = panel.child(label(x - 30.0, 497.0, 24.0, "OFF", s));
        }
    }
    for (x, y, text) in [
        (915.0, 438.0, "-20"),
        (958.0, 438.0, "0"),
        (913.0, 497.0, "-∞"),
        (957.0, 497.0, "+20"),
    ] {
        panel = panel.child(label(x - 11.0, y, 22.0, text, s));
    }
    for (x, y) in [
        (915.0, 450.0),
        (958.0, 450.0),
        (908.0, 477.0),
        (964.0, 477.0),
        (936.0, 441.0),
    ] {
        panel = panel.child(at(
            Region::centered(x, y, 3.0, 3.0),
            s,
            div().size_full().rounded_full().bg(rgb(0xd6d6d6)),
        ));
    }
    for (control, region) in &layout.buttons {
        let (text, icon, indicator) = face(control.id);
        let visual = if control.id == 53 {
            controls::power(region.width * s, s)
        } else if control.switch {
            controls::switch(
                region.width * s,
                region.height * s,
                switches[control.id as usize],
            )
        } else {
            controls::button(
                region.width * s,
                region.height * s,
                s,
                text,
                icon,
                indicator.and_then(|(i, c)| leds[i].then_some(c)),
            )
        };
        panel = panel.child(at_local(*region, s, visual));
    }
    for (i, region) in layout.knobs.iter().enumerate() {
        let visual = if i < 8 {
            knobs::encoder(region.width * s, rings[i])
        } else {
            knobs::knob(
                region.width * s,
                [0xbfc4c7, 0x4030bb, 0xee343d, 0x303235, 0x303235][i - 8],
                analog[i - 8] as u32,
            )
        };
        panel = panel.child(at_local(*region, s, visual));
    }
    for (x, text) in [
        (796.0, "SOUND PAD"),
        (866.0, "EFX RTN"),
        (936.0, "MASTER"),
        (1006.0, "MONITOR"),
        (1076.0, "SUB-OUT"),
    ] {
        panel = panel.child(label_sized(x - 34.0, 511.0, 68.0, text, s, 10.0));
    }
    panel
}
