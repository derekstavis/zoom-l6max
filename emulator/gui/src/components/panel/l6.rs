//! L6 six-channel control surface rendering.
use super::super::layout::{
    COMBO_SOCKET_DIAMETER as SHARED_COMBO_SOCKET_DIAMETER, L6_FACEPLATE_SCALE, Layout, Region,
    TRS_SOCKET_DIAMETER as SHARED_TRS_SOCKET_DIAMETER,
};
use super::super::{
    controls,
    encoder::{EncoderLedLevel, EncoderWithLeds},
    knobs,
};
use super::shared::{
    at_local, label_local, label_local_sized, panel_identifier_local, section_header_local,
    section_header_local_sized,
};
use gpui::{Div, div, prelude::*, px, rgb};
use l6max_host::indicators;

// Scale manual-coordinate sizes so the shared physical sockets match L6max.
// Every 1/4-inch channel and output jack uses the same shared diameter.
const TRS_SOCKET_DIAMETER: f32 = SHARED_TRS_SOCKET_DIAMETER * L6_FACEPLATE_SCALE;
const COMBO_SOCKET_DIAMETER: f32 = SHARED_COMBO_SOCKET_DIAMETER * L6_FACEPLATE_SCALE;

/// Manual-derived L6 faceplate with positions transcribed from the product
/// reference. The L6 has no LCD; its channel mute and source buttons and
/// encoder hit regions are bound separately in the model-specific layout.
pub fn panel(
    layout: &Layout,
    scale: f32,
    rows: [u32; 8],
    indicator_levels: [[u8; 24]; 8],
    analog: [u16; 5],
    powered: bool,
    interactions: &super::PanelInteractions,
) -> Div {
    let s = scale;
    let button_at = |layout: &Layout,
                     id: u32,
                     scale: f32,
                     label: &str,
                     icon: Option<&str>,
                     led: Option<u32>| {
        button_at_with_interactions(layout, id, scale, label, icon, led, interactions)
    };
    let button_column =
        |layout: &Layout, buttons: &[(u32, &str, Option<&str>, Option<u32>)], scale: f32| {
            button_column_with_interactions(layout, buttons, scale, interactions)
        };
    // Normalize each L6 ring segment against its measured full-scale endpoint;
    // lower firmware PWM levels remain proportional to that segment's maximum.
    let measured_rings = indicators::ring_levels(indicator_levels);
    let rings: [[u8; 9]; 6] = std::array::from_fn(|ring| {
        std::array::from_fn(|segment| {
            normalize_ring_pwm(ring, segment, measured_rings[ring][segment])
        })
    });
    let [left_level, right_level] = indicators::l6_master_meter_levels(indicator_levels);
    let button_leds = indicators::l6_button_leds(rows);
    let sound_pad_leds = indicators::l6_sound_pad_leds(rows);
    let effect_leds = indicators::l6_effect_leds(rows);
    let mode_leds = indicators::l6_mode_leds(rows);
    let scene_leds = indicators::l6_scene_leds(rows);
    let mut panel = div()
        .relative()
        .size_full()
        .font_family("Helvetica Neue")
        .bg(rgb(0x000000))
        .child(at_local(
            Region::new(0.0, 0.0, layout.width, layout.height),
            s,
            div()
                .size_full()
                .rounded(px(31.0 * s))
                .border(px(2.0 * s))
                .border_color(rgb(0x111111))
                .bg(rgb(0x444548)),
        ));
    // Draw the colored fills first so the manual's fine divider strokes sit
    // above them, as they do on the printed panel.
    for center in [90.0, 196.0, 303.0, 410.0, 516.0, 622.0] {
        panel = panel.child(at_local(
            Region::new(center - 53.0, 458.0, 106.0, 108.0),
            s,
            div().size_full().bg(rgb(0x373286)),
        ));
        // The manual drawing outlines the six lower channel control fields
        // above the knobs and above the printed channel numbers.
        for y in [457.0, 565.0] {
            panel = panel.child(at_local(
                Region::new(center - 53.0, y, 106.0, 1.5),
                s,
                div().size_full().bg(rgb(0xd0d5d6)),
            ));
        }
    }
    panel = panel.child(at_local(
        Region::new(674.0, 18.0, 89.0, layout.height - 18.0),
        s,
        div().size_full().bg(rgb(0x38318f)),
    ));

    // Divider positions and endpoints are transcribed from the manual SVG.
    // The channel rules stop and resume around the jack area instead of
    // continuing as a uniform grid.
    for (x, y, height) in [
        (249.0, 18.0, 206.0),
        (356.0, 41.0, 183.0),
        (461.0, 41.0, 183.0),
        (569.0, 41.0, 183.0),
        (37.0, 254.0, 306.0),
        (143.0, 391.0, 169.0),
        (249.0, 316.0, 244.0),
        (356.0, 316.0, 244.0),
        (461.0, 316.0, 244.0),
        (569.0, 316.0, 244.0),
        (674.0, 18.0, layout.height - 18.0),
        (763.0, 18.0, layout.height - 18.0),
        (860.0, 17.0, 543.0),
        (959.0, 17.0, 543.0),
        (1057.0, 17.0, 543.0),
        (1156.0, 17.0, 543.0),
    ] {
        panel = panel.child(at_local(
            Region::new(x, y, 1.5, height),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }
    // The three right-hand section headers are boxed across their top edge.
    for (x, width) in [(860.0, 99.0), (959.0, 98.0), (1057.0, 99.0)] {
        panel = panel.child(at_local(
            Region::new(x, 17.0, width, 1.5),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }

    // Two combo inputs and four stereo line-input pairs.
    for (center, y) in [(181.7, 114.2), (98.1, 231.7)] {
        panel = panel.child(at_local(
            Region::centered(center, y, COMBO_SOCKET_DIAMETER, COMBO_SOCKET_DIAMETER),
            s,
            controls::socket(COMBO_SOCKET_DIAMETER * s, true),
        ));
    }
    for center in [303.0, 410.0, 516.0, 622.0] {
        for (y, side) in [(82.4, "L"), (178.4, "R")] {
            panel = panel.child(at_local(
                Region::centered(center, y, TRS_SOCKET_DIAMETER, TRS_SOCKET_DIAMETER),
                s,
                controls::socket(TRS_SOCKET_DIAMETER * s, false),
            ));
            let (label_x, label_width, label) =
                if side == "L" && (center == 516.0 || center == 622.0) {
                    (center - 48.0, 76.0, "L (MONO)")
                } else {
                    (center - 36.0, 24.0, side)
                };
            panel = panel.child(label_local(label_x, y + 32.0, label_width, label, s));
        }
    }
    panel = panel
        .child(label_local_sized(45.0, 9.0, 130.0, "POWER", s, 11.0))
        .child(at_local(
            layout.button(53),
            s,
            controls::power_with_press(42.0 * s, s, {
                let down = interactions.button_down.clone();
                move || down(53, false)
            }),
        ))
        .child(at_local(
            // The printing sits left of the upper combo socket; keep it clear
            // of the lower-left chassis screw.
            Region::new(28.0, 98.0, 96.0, 62.0),
            s,
            div()
                .size_full()
                .text_size(px(10.5 * s))
                .line_height(px(14.0 * s))
                .text_color(rgb(0xe1e6e9))
                .child("XLR: MIC\nTRS: LINE\n(+4 dBu BAL)"),
        ))
        .child(label_local_sized(
            354.0,
            18.0,
            250.0,
            "LINE (−10 dBu UNBAL)",
            s,
            9.5,
        ))
        .child(at_local(
            Region::new(300.0, 23.0, 82.0, 1.5),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ))
        .child(at_local(
            Region::new(541.0, 23.0, 82.0, 1.5),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));

    // The physical unit's central wordmark is a removable sticker; show the
    // emulator's model identifier in the same panel area.
    panel = panel.child(panel_identifier_local(
        Region::new(206.0, 246.0, 410.0, 38.0),
        "ZOOM L6 Emulator",
        s,
        25.0,
    ));
    for (x, y) in [(132.0, 42.0), (43.0, 154.0), (231.0, 154.0), (233.0, 231.0)] {
        panel = panel.child(at_local(
            Region::centered(x, y, 14.0, 14.0),
            s,
            controls::screw(14.0 * s),
        ));
    }
    panel = panel.child(button_at(
        layout,
        7,
        s,
        "48V",
        None,
        button_leds[11].then_some(0xef6d66),
    ));

    let channel_centers = [90.0, 196.0, 303.0, 410.0, 516.0, 622.0];
    for (index, center) in channel_centers.into_iter().enumerate() {
        panel = panel
            .child(label_local(center - 43.0, 331.0, 86.0, "SIGNAL", s))
            .child(at_local(
                Region::centered(center, 322.0, 11.0, 11.0),
                s,
                controls::led(11.0 * s, None),
            ))
            .child(at_local(
                layout.knobs[index],
                s,
                EncoderWithLeds::new(92.0 * s, EncoderLedLevel::Levels(rings[index]))
                    .with_input(interactions.encoder_input(index)),
            ))
            .child(label_local_sized(
                center - 45.0,
                572.0,
                90.0,
                if index < 2 {
                    (index + 1).to_string()
                } else {
                    format!("{}(ST)", index + 1)
                }
                .as_str(),
                s,
                13.0,
            ));
        let mute_target = [10, 13, 16, 19, 22, 25][index];
        panel = panel.child(button_at(
            layout,
            mute_target,
            s,
            "",
            Some("volume-off"),
            button_leds[index].then_some(0xef6d66),
        ));
        if (2..=5).contains(&index) {
            let (label, symbol, led) = match index {
                2 | 3 => ("MONO ×2", None, button_leds[index + 4]),
                4 => ("1/2", Some("usb"), button_leds[8]),
                _ => ("3/4", Some("usb"), button_leds[9]),
            };
            let target = [2, 3, 6, 47][index - 2];
            panel = panel.child(button_at(
                layout,
                target,
                s,
                label,
                symbol,
                led.then_some(0xf59a52),
            ));
        }
    }

    // L6's blue channel-strip selector occupies a narrow column after the six
    // channel controls; the model has two AUX sends and no SUB-MIX selector.
    panel = panel
        .child(label_local_sized(683.0, 37.0, 80.0, "EQ", s, 12.5))
        .child(at_local(
            Region::new(678.0, 264.0, 85.0, 31.0),
            s,
            div()
                .size_full()
                .font_family("Helvetica Neue")
                .text_size(px(10.5 * s))
                .line_height(px(12.5 * s))
                .text_color(rgb(0xe1e6e9))
                .text_center()
                .child("CHANNEL\nSTRIP"),
        ))
        .child(label_local_sized(680.0, 297.0, 86.0, "SEND", s, 11.5));
    panel = panel
        .child(button_column(
            layout,
            &[
                (8, "HIGH", None, mode_leds[0].then_some(0x60c873)),
                (11, "FREQ", None, mode_leds[1].then_some(0x60c873)),
                (14, "MID", None, mode_leds[2].then_some(0x60c873)),
                (17, "LOW", None, mode_leds[3].then_some(0x60c873)),
            ],
            s,
        ))
        .child(button_at(
            layout,
            20,
            s,
            "AUX1",
            None,
            mode_leds[4].then_some(0x60c873),
        ))
        .child(button_at(
            layout,
            23,
            s,
            "AUX2",
            None,
            mode_leds[5].then_some(0x60c873),
        ))
        .child(button_at(
            layout,
            26,
            s,
            "EFX",
            None,
            mode_leds[6].then_some(0x60c873),
        ))
        .child(button_at(
            layout,
            29,
            s,
            "PAN",
            None,
            mode_leds[7].then_some(0x60c873),
        ))
        .child(button_at(
            layout,
            28,
            s,
            "LEVEL",
            None,
            mode_leds[8].then_some(0x60c873),
        ));
    for (y, h) in [(84.0, 21.0), (133.0, 21.0), (182.0, 21.0)] {
        panel = panel.child(at_local(
            Region::centered(723.0, y + h / 2.0, 1.5, h),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }
    // FREQ and MID share the small printed bracket shown on the L6 faceplate.
    for (x, y, width, height) in [
        (684.0, 117.0, 1.5, 40.0),
        (684.0, 117.0, 12.0, 1.5),
        (684.0, 157.0, 12.0, 1.5),
    ] {
        panel = panel.child(at_local(
            Region::new(x, y, width, height),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }
    for (y, h) in [(346.0, 19.0), (393.0, 20.0), (441.0, 22.0), (491.0, 30.0)] {
        panel = panel.child(at_local(
            Region::centered(723.0, y + h / 2.0, 1.5, h),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }

    // MIDI, AUX SEND, MASTER and MONITOR sections mirror the device's right
    // side. The monitor area has recorder buttons, but no display.
    panel = panel
        .child(super::midi::midi_ports(
            s,
            super::midi::MidiLayout {
                title: Region::new(768.0, 19.0, 88.0, 20.0),
                title_font: 11.5,
                center_x: 811.0,
                input_y: 57.0,
                output_y: 127.0,
                label_x: 770.0,
                label_width: 56.0,
                input_label_y: 72.0,
                output_label_y: 144.0,
            },
        ))
        .child(section_header_local(862.0, 19.0, 99.0, 19.0, "AUX SEND", s))
        .child(section_header_local_sized(
            961.0, 19.0, 96.0, 19.0, "MASTER", s, 11.0,
        ))
        .child(section_header_local_sized(
            1059.0, 19.0, 97.0, 19.0, "MONITOR", s, 11.0,
        ))
        .child(at_local(
            Region::centered(811.0, 193.0, 39.0, 14.0),
            s,
            controls::usb_socket(39.0 * s, 14.0 * s),
        ))
        .child(at_local(
            Region::centered(811.0, 219.0, 19.5, 19.5),
            s,
            controls::icon("usb", 19.5 * s, 0xe1e6e9),
        ));

    for (center, y, label_text) in [(910.7, 82.4, "1"), (910.7, 178.4, "2")] {
        panel = panel
            .child(at_local(
                Region::centered(center, y, TRS_SOCKET_DIAMETER, TRS_SOCKET_DIAMETER),
                s,
                controls::socket(TRS_SOCKET_DIAMETER * s, false),
            ))
            .child(label_local(center - 25.0, y + 32.0, 18.0, label_text, s));
    }
    for (center, y, label_text) in [(1009.1, 82.4, "L"), (1009.1, 178.4, "R")] {
        panel = panel
            .child(at_local(
                Region::centered(center, y, TRS_SOCKET_DIAMETER, TRS_SOCKET_DIAMETER),
                s,
                controls::socket(TRS_SOCKET_DIAMETER * s, false),
            ))
            .child(label_local(center - 26.0, y + 32.0, 18.0, label_text, s));
    }
    panel = panel
        .child(at_local(
            Region::centered(1107.6, 82.4, TRS_SOCKET_DIAMETER, TRS_SOCKET_DIAMETER),
            s,
            controls::socket(TRS_SOCKET_DIAMETER * s, false),
        ))
        .child(at_local(
            Region::centered(1110.0, 173.0, 10.0, 10.0),
            s,
            controls::led(10.0 * s, powered.then_some(0xd8ed38)),
        ))
        .child(label_local(1081.0, 160.0, 58.0, "POWER", s))
        .child(label_local_sized(1084.0, 199.0, 66.0, "SCENE", s, 10.5));

    panel = panel.child(at_local(
        Region::new(974.0, 253.5, 58.0, 191.5),
        s,
        super::vu_meter::VuMeter::new(super::vu_meter::VuMeterProps {
            scale: s,
            scale_labels: &[
                super::vu_meter::ScaleLabel {
                    x: 0.0,
                    y: 18.5,
                    width: 18.0,
                    text: "0",
                    font: 9.5,
                },
                super::vu_meter::ScaleLabel {
                    x: 0.0,
                    y: 52.5,
                    width: 18.0,
                    text: "−6",
                    font: 9.5,
                },
                super::vu_meter::ScaleLabel {
                    x: 0.0,
                    y: 108.5,
                    width: 18.0,
                    text: "−20",
                    font: 9.5,
                },
                super::vu_meter::ScaleLabel {
                    x: 0.0,
                    y: 171.5,
                    width: 18.0,
                    text: "−48",
                    font: 9.5,
                },
            ],
            style: super::vu_meter::MeterStyle {
                // Measured from C1_illust.svg, then converted from its
                // 420.332 × 207.587 faceplate crop to L6 layout units.
                bezel: Region::new(20.5, 0.1, 30.9, 135.4),
                columns: 2,
                rows: 6,
                first_segment: Region::new(25.1, 8.0, 7.05, 16.8),
                column_step: 13.4,
                row_step: 21.35,
                appearance: super::vu_meter::SHARED_APPEARANCE,
                active_fill: 0xe7ee39,
            },
            left_level: Some(left_level),
            right_level: Some(right_level),
        }),
    ));
    // Record and transport keys sit inside a single recessed black surround.
    panel = panel.child(at_local(
        Region::centered(1106.5, 399.0, 90.0, 110.0),
        s,
        div().size_full().rounded(px(5.0 * s)).bg(rgb(0x111213)),
    ));
    panel = panel.child(at_local(
        Region::new(1109.25, 341.0, 1.5, 12.0),
        s,
        div().size_full().bg(rgb(0xd0d5d6)),
    ));
    for (y, height) in [(285.0, 20.0), (333.0, 20.0), (381.0, 20.0), (429.0, 51.0)] {
        panel = panel.child(at_local(
            Region::centered(811.0, y + height / 2.0, 1.5, height),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }
    panel = panel
        .child(button_column(
            layout,
            &[
                (49, "1", None, sound_pad_leds[0].then_some(0xef6d66)),
                (50, "2", None, sound_pad_leds[1].then_some(0xef6d66)),
                (51, "3", None, sound_pad_leds[2].then_some(0xef6d66)),
                (52, "4", None, sound_pad_leds[3].then_some(0xef6d66)),
            ],
            s,
        ))
        .child(button_at(layout, 48, s, "TAP", None, None));
    panel = panel.child(super::effects::effects_legend(
        s,
        [
            false,
            effect_leds[0],
            effect_leds[1],
            effect_leds[2],
            effect_leds[3],
            effect_leds[4],
        ],
        super::effects::EffectsLayout {
            led_x: 888.0,
            first_led_y: 312.0,
            led_step: 18.0,
            label_x: 900.0,
            label_y_offset: -8.0,
            label_width: 70.0,
            ai_noise_reduction: None,
        },
    ));
    for (x, y, height) in [(910.0, 381.0, 12.0), (910.0, 433.0, 23.0)] {
        panel = panel.child(at_local(
            Region::centered(x, y + height / 2.0, 1.5, height),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }
    panel = panel
        .child(button_in_region(
            Region::centered(904.0, 422.0, 54.0, 28.0),
            9,
            s,
            "SEL",
            None,
            None,
            interactions,
        ))
        .child(button_at(
            layout,
            12,
            s,
            "COMP",
            None,
            button_leds[10].then_some(0xf59a52),
        ))
        .child(button_column(
            layout,
            &[
                (15, "A", None, scene_leds[0].then_some(0xc9555b)),
                (18, "B", None, scene_leds[1].then_some(0xc9555b)),
                (21, "C", None, scene_leds[2].then_some(0xc9555b)),
            ],
            s,
        ))
        .child(button_at(
            layout,
            24,
            s,
            "",
            Some("record"),
            Some(if indicators::l6_record_led(rows) {
                0xef3646
            } else {
                0x542b30
            }),
        ))
        .child(label_local_sized(1064.0, 388.0, 92.0, "RECORDER", s, 10.5))
        .child(button_at(layout, 4, s, "", Some("play-stop"), None));
    for (y, h) in [(239.0, 23.0), (290.0, 23.0)] {
        panel = panel.child(at_local(
            Region::centered(1110.0, y + h / 2.0, 1.5, h),
            s,
            div().size_full().bg(rgb(0xd0d5d6)),
        ));
    }

    for (i, (title, color)) in [
        ("SOUND PAD", 0xbfc4c7),
        ("EFX RTN", 0x4030bb),
        ("MASTER", 0xee343d),
        ("MONITOR", 0x303235),
    ]
    .into_iter()
    .enumerate()
    {
        let region = layout.knobs[8 + i];
        panel = panel
            .child(at_local(
                region,
                s,
                knobs::knob_with_input(
                    region.width * s,
                    color,
                    analog[i] as u32,
                    interactions.encoder_input(8 + i),
                ),
            ))
            .child(label_local_sized(
                region.x + region.width / 2.0 - 46.0,
                572.0,
                92.0,
                title,
                s,
                10.5,
            ));
    }
    panel
}

// Rounded per-segment endpoint averages from two L6 1.10 captures with all
// six controls at 127. Normalize each physical ring segment against its
// observed full-load endpoint because the matrix pins report different duty.
const RING_FULL_SCALE: [[u8; 9]; 6] = [
    [60, 60, 61, 61, 62, 62, 62, 62, 61],
    [61, 60, 60, 60, 60, 60, 60, 61, 60],
    [60, 60, 61, 61, 62, 62, 62, 62, 61],
    [60, 60, 60, 60, 60, 60, 60, 61, 57],
    [62, 62, 62, 62, 61, 61, 60, 60, 57],
    [60, 60, 60, 60, 60, 61, 61, 62, 30],
];

fn normalize_ring_pwm(ring: usize, segment: usize, measured: u8) -> u8 {
    let full_scale = u16::from(RING_FULL_SCALE[ring][segment]);
    ((u16::from(measured) * 255 + full_scale / 2) / full_scale).min(255) as u8
}

#[cfg(test)]
mod pwm_tests {
    use super::{RING_FULL_SCALE, normalize_ring_pwm};

    #[test]
    fn l6_each_ring_segment_reaches_full_intensity_at_its_measured_endpoint() {
        for (ring, segments) in RING_FULL_SCALE.iter().enumerate() {
            for (segment, &full_scale) in segments.iter().enumerate() {
                assert_eq!(normalize_ring_pwm(ring, segment, full_scale), 255);
            }
        }
    }

    #[test]
    fn l6_per_segment_pwm_calibration_is_monotonic_and_clamps() {
        for ring in 0..RING_FULL_SCALE.len() {
            for segment in 0..RING_FULL_SCALE[ring].len() {
                let levels = (0..=255)
                    .map(|measured| normalize_ring_pwm(ring, segment, measured))
                    .collect::<Vec<_>>();
                assert_eq!(levels[0], 0);
                assert_eq!(levels[255], 255);
                assert!(levels.windows(2).all(|pair| pair[0] <= pair[1]));
            }
        }
    }
}

fn button_at_with_interactions(
    layout: &Layout,
    id: u32,
    scale: f32,
    label: &str,
    icon: Option<&str>,
    led: Option<u32>,
    interactions: &super::PanelInteractions,
) -> Div {
    button_in_region(layout.button(id), id, scale, label, icon, led, interactions)
}

fn button_in_region(
    region: Region,
    id: u32,
    scale: f32,
    label: &str,
    icon: Option<&str>,
    led: Option<u32>,
    interactions: &super::PanelInteractions,
) -> Div {
    at_local(
        region,
        scale,
        controls::button_with_events(
            region.width * scale,
            region.height * scale,
            scale,
            label,
            icon,
            led,
            interactions.button_events(id, false),
        ),
    )
}

/// Lay out regular, equally sized control groups in normal flex flow. Their
/// origin and gap come from the shared hit-region table, so visual buttons and
/// input targets keep the same geometry.
fn button_column_with_interactions(
    layout: &Layout,
    buttons: &[(u32, &str, Option<&str>, Option<u32>)],
    scale: f32,
    interactions: &super::PanelInteractions,
) -> Div {
    assert!(buttons.len() > 1);
    let first = layout.button(buttons[0].0);
    let second = layout.button(buttons[1].0);
    let gap = second.y - first.y - first.height;
    assert!(gap >= 0.0);
    for pair in buttons.windows(2) {
        let previous = layout.button(pair[0].0);
        let current = layout.button(pair[1].0);
        assert_eq!(
            (previous.x, previous.width, previous.height),
            (first.x, first.width, first.height)
        );
        assert!((current.y - previous.y - previous.height - gap).abs() < 0.01);
    }
    let last = layout.button(buttons.last().unwrap().0);
    let height = last.y + last.height - first.y;
    at_local(
        Region::new(first.x, first.y, first.width, height),
        scale,
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(gap * scale))
            .children(buttons.iter().map(|(id, label, icon, led)| {
                controls::button_with_events(
                    first.width * scale,
                    first.height * scale,
                    scale,
                    label,
                    *icon,
                    *led,
                    interactions.button_events(*id, false),
                )
            })),
    )
}
