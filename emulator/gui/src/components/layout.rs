//! Shared physical dimensions. Rendering and input use these same regions.
use l6max_host::controls::{CONTROLS, Control};
pub const CHANNEL_WIDTH: f32 = 76.0;
pub const KNOB_Y: f32 = 470.0;
pub const MODE_X: f32 = 698.0;
pub const BODY: Region = Region {
    x: 52.0,
    y: 104.0,
    width: 1095.0,
    height: 423.0,
};
pub const RECORDER: Region = Region {
    x: 962.0,
    y: 276.0,
    width: 158.0,
    height: 160.0,
};
pub const LCD_GLASS: Region = Region {
    x: 995.0,
    y: 297.0,
    width: 94.0,
    height: 49.0,
};
pub const GLOBAL_CENTERS: [f32; 5] = [796.0, 866.0, 936.0, 1006.0, 1076.0];
pub const SWITCH_LONG: f32 = 28.0;
pub const SWITCH_SHORT: f32 = 18.0;
pub const WINDOW_WIDTH: f32 = 1200.0;
pub const TRS_SOCKET_DIAMETER: f32 = 49.0;
pub const COMBO_SOCKET_DIAMETER: f32 = 80.0;
const L6_OUTPUT_KNOB_CENTERS: [f32; 4] = [812.0, 910.0, 1008.0, 1108.0];
pub const L6_FACEPLATE_SCALE: f32 = (WINDOW_WIDTH / BODY.width) * (284.0 / 223.0);
pub fn window_width(model: l6fw::Model) -> f32 {
    match model {
        l6fw::Model::L6 => WINDOW_WIDTH * (223.0 / 284.0),
        l6fw::Model::L6max => WINDOW_WIDTH,
    }
}
pub fn channel_x(channel: usize) -> f32 {
    88.0 + CHANNEL_WIDTH * channel as f32
}
pub fn interactive_knob_indices(model: l6fw::Model) -> Vec<usize> {
    match model {
        l6fw::Model::L6 => (0..6).chain(8..12).collect(),
        l6fw::Model::L6max => (0..13).collect(),
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Region {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn centered(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self::new(x - width / 2.0, y - height / 2.0, width, height)
    }
    pub fn local(self) -> Self {
        Self::new(self.x - BODY.x, self.y - BODY.y, self.width, self.height)
    }
}
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub buttons: Vec<(Control, Region)>,
    pub knobs: [Region; 13],
    pub lcd: Region,
}
impl Layout {
    pub fn new() -> Self {
        let mut regions = [Region::new(0.0, 0.0, 0.0, 0.0); 54];
        let mute = [22, 27, 37, 42, 29, 34, 39, 44];
        for ch in 0..8 {
            let x = channel_x(ch) + CHANNEL_WIDTH / 2.0;
            regions[mute[ch]] = Region::centered(x, 408.0, 38.0, 20.0);
        }
        // Hi-Z belongs to each of the first two inputs; phantom power is
        // shared across each pair. These are not one button per channel.
        for (slot, id) in [7, 17, 12].into_iter().enumerate() {
            regions[id] =
                Region::centered(channel_x(0) + 24.0 + 54.0 * slot as f32, 371.0, 38.0, 20.0);
        }
        regions[32] = Region::centered(channel_x(3), 371.0, 38.0, 20.0);
        for (id, ch) in [(9, 4), (14, 5), (19, 6), (24, 7)] {
            regions[id] = Region::centered(channel_x(ch) + CHANNEL_WIDTH / 2.0, 371.0, 38.0, 20.0);
        }
        for (id, ch) in [(8, 4), (13, 5), (18, 6), (23, 7)] {
            regions[id] = Region::centered(
                channel_x(ch) + CHANNEL_WIDTH / 2.0,
                281.0,
                SWITCH_LONG,
                SWITCH_SHORT,
            );
        }
        for (id, y) in [
            (28, 152.0),
            (33, 188.0),
            (38, 225.0),
            (43, 262.0),
            (10, 299.0),
            (15, 335.0),
            (20, 372.0),
            (25, 408.0),
            (30, 449.0),
            (35, 490.0),
        ] {
            regions[id] = Region::centered(MODE_X + 31.0, y, 38.0, 20.0);
        }
        for (id, y) in [(49, 298.0), (50, 337.0), (51, 376.0), (52, 408.0)] {
            regions[id] = Region::centered(796.0, y, 38.0, 20.0);
        }
        regions[48] = Region::centered(866.0, 298.0, 38.0, 20.0);
        regions[40] = Region::centered(866.0, 408.0, 38.0, 20.0);
        regions[45] = Region::centered(936.0, 408.0, 38.0, 20.0);
        for (i, id) in [11, 16, 21, 26].into_iter().enumerate() {
            regions[id] = Region::centered(
                988.0 + 36.0 * (i % 2) as f32,
                232.0 + 28.0 * (i / 2) as f32,
                26.0,
                18.0,
            );
        }
        regions[47] = Region::centered(1076.0, 244.0, SWITCH_SHORT, SWITCH_LONG);
        for id in 0..4 {
            regions[id] = Region::centered(983.0 + 39.0 * id as f32, 372.0, 24.0, 16.0);
        }
        for id in 4..7 {
            regions[id] = Region::centered(989.0 + 52.0 * (id - 4) as f32, 408.0, 39.0, 20.0);
        }
        regions[53] = Region::centered(92.0, 142.0, 30.0, 30.0);
        Self {
            width: BODY.width,
            height: BODY.height,
            buttons: CONTROLS
                .iter()
                .map(|c| (*c, regions[c.id as usize].local()))
                .collect(),
            knobs: std::array::from_fn(|i| {
                if i < 8 {
                    Region::centered(channel_x(i) + CHANNEL_WIDTH / 2.0, KNOB_Y, 66.0, 66.0).local()
                } else {
                    Region::centered(GLOBAL_CENTERS[i - 8], KNOB_Y, 50.0, 50.0).local()
                }
            }),
            lcd: Region::new(999.0, 301.0, 86.0, 43.0).local(),
        }
    }
    pub fn for_model(model: l6fw::Model) -> Self {
        let mut layout = Self::new();
        if model == l6fw::Model::L6 {
            // The L6 faceplate follows the manual drawing's 419.3:206.8 body,
            // rather than the taller L6max display/recorder proportions.
            layout.width = 1200.0;
            layout.height = 592.0;
            layout.knobs = std::array::from_fn(|i| {
                if i < 6 {
                    Region::centered(
                        [90.0, 196.0, 303.0, 410.0, 516.0, 622.0][i],
                        510.0,
                        92.0,
                        92.0,
                    )
                } else if (8..12).contains(&i) {
                    Region::centered(L6_OUTPUT_KNOB_CENTERS[i - 8], 510.0, 70.0, 70.0)
                } else {
                    Region::new(0.0, 0.0, 0.0, 0.0)
                }
            });
            if let Some((_, power)) = layout
                .buttons
                .iter_mut()
                .find(|(control, _)| control.id == 53)
            {
                *power = Region::centered(56.0, 57.0, 42.0, 42.0);
            }
            layout.lcd = Region::new(0.0, 0.0, 0.0, 0.0);
            // These six panel target IDs were exercised against L6 firmware:
            // each produces its channel's standard mute CC on the MIDI jack.
            // Keep the L6 panel's target-to-faceplate map separate from L6max.
            layout.buttons = [
                (7, "phantom-48v", 143.0),
                (10, "ch1-mute", 90.0),
                (13, "ch2-mute", 196.0),
                (16, "ch3-mute", 303.0),
                (19, "ch4-mute", 410.0),
                (22, "ch5-mute", 516.0),
                (25, "ch6-mute", 622.0),
                (2, "ch3-mono", 303.0),
                (3, "ch4-mono", 410.0),
                (6, "ch5-usb-1-2", 516.0),
                (47, "ch6-usb-3-4", 622.0),
            ]
            .into_iter()
            .map(|(id, name, x)| {
                (
                    Control {
                        id,
                        name,
                        switch: false,
                    },
                    Region::centered(
                        x,
                        if (10..=25).contains(&id) {
                            421.0
                        } else {
                            373.0
                        },
                        54.0,
                        28.0,
                    ),
                )
            })
            .chain([
                (
                    Control {
                        id: 9,
                        name: "efx-type",
                        switch: false,
                    },
                    Region::centered(904.0, 422.0, 54.0, 28.0),
                ),
                (
                    Control {
                        id: 12,
                        name: "comp-master",
                        switch: false,
                    },
                    Region::centered(1015.0, 422.0, 60.0, 28.0),
                ),
            ])
            .chain(
                [
                    (8, "l6-mode-eq-high", 70.0),
                    (11, "l6-mode-freq", 119.0),
                    (14, "l6-mode-mid", 168.0),
                    (17, "l6-mode-low", 217.0),
                    (20, "l6-mode-aux1", 332.0),
                    (23, "l6-mode-aux2", 379.0),
                    (26, "l6-mode-efx", 427.0),
                    (29, "l6-mode-pan", 477.0),
                    (28, "l6-mode-level", 535.0),
                ]
                .into_iter()
                .map(|(id, name, y)| {
                    (
                        Control {
                            id,
                            name,
                            switch: false,
                        },
                        Region::centered(723.0, y, 54.0, 28.0),
                    )
                }),
            )
            .chain(
                [
                    (15, "scene-a", 225.0),
                    (18, "scene-b", 276.0),
                    (21, "scene-c", 327.0),
                ]
                .into_iter()
                .map(|(id, name, y)| {
                    (
                        Control {
                            id,
                            name,
                            switch: false,
                        },
                        Region::centered(1110.0, y, 48.0, 28.0),
                    )
                }),
            )
            .chain(std::iter::once((
                Control {
                    id: 24,
                    name: "rec-record",
                    switch: false,
                },
                Region::centered(1110.0, 378.0, 56.0, 28.0),
            )))
            .chain(
                [(4, 1110.0, 428.0), (48, 904.0, 270.0)]
                    .into_iter()
                    .map(|(id, x, y)| {
                        (
                            *CONTROLS.iter().find(|control| control.id == id).unwrap(),
                            Region::centered(x, y, 56.0, 28.0),
                        )
                    }),
            )
            .chain((0..4).map(|index| {
                (
                    *CONTROLS
                        .iter()
                        .find(|control| control.id == 49 + index)
                        .unwrap(),
                    Region::centered(811.0, 271.0 + 48.0 * index as f32, 44.0, 28.0),
                )
            }))
            .chain(std::iter::once((
                *CONTROLS.iter().find(|control| control.id == 53).unwrap(),
                Region::centered(56.0, 57.0, 42.0, 42.0),
            )))
            .collect();
        }
        layout
    }
    pub fn button(&self, id: u32) -> Region {
        self.buttons
            .iter()
            .find(|(c, _)| c.id == id)
            .expect("physical control ID")
            .1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hardware_controls_fit_the_panel_and_share_physical_dimensions() {
        let l = Layout::new();
        let mut ids = std::collections::HashSet::new();
        for (control, r) in &l.buttons {
            assert!(ids.insert(control.id));
            assert!(r.width > 0.0 && r.height > 0.0 && r.x >= 0.0 && r.y >= 0.0);
            assert!(r.x + r.width <= l.width && r.y + r.height <= l.height);
            if control.switch {
                assert_eq!(
                    (r.width.min(r.height), r.width.max(r.height)),
                    (SWITCH_SHORT, SWITCH_LONG)
                );
            }
        }
        assert_eq!(ids.len(), CONTROLS.len());
        for r in l.knobs {
            assert_eq!(r.y + r.height / 2.0, KNOB_Y - BODY.y);
        }
        for pair in l.knobs[..8].windows(2) {
            assert_eq!(pair[1].x - pair[0].x, CHANNEL_WIDTH);
        }
        for i in 0..l.buttons.len() {
            for j in i + 1..l.buttons.len() {
                let a = l.buttons[i].1;
                let b = l.buttons[j].1;
                assert!(
                    a.x + a.width <= b.x
                        || b.x + b.width <= a.x
                        || a.y + a.height <= b.y
                        || b.y + b.height <= a.y,
                    "overlapping controls {} {}",
                    l.buttons[i].0.id,
                    l.buttons[j].0.id
                );
            }
        }
        assert_eq!(l.lcd.width / l.lcd.height, 2.0);
    }

    #[test]
    fn l6_knob_hit_regions_match_the_six_drawn_encoder_centers() {
        let layout = Layout::for_model(l6fw::Model::L6);
        let centers = [90.0, 196.0, 303.0, 410.0, 516.0, 622.0];
        for channel in 0..6 {
            let knob = layout.knobs[channel];
            assert_eq!(knob.x + knob.width / 2.0, centers[channel]);
            assert_eq!(knob.y + knob.height / 2.0, 510.0);
            assert_eq!((knob.width, knob.height), (92.0, 92.0));
        }
        assert!(
            layout.knobs[6..8]
                .iter()
                .all(|knob| knob.width == 0.0 && knob.height == 0.0)
        );
        assert_eq!(layout.knobs[12].width, 0.0);
        for (index, x) in L6_OUTPUT_KNOB_CENTERS.into_iter().enumerate() {
            let knob = layout.knobs[8 + index];
            assert_eq!(knob.x + knob.width / 2.0, x);
            assert_eq!(knob.y + knob.height / 2.0, 510.0);
            assert_eq!((knob.width, knob.height), (70.0, 70.0));
        }
        assert_eq!(
            interactive_knob_indices(l6fw::Model::L6),
            [0, 1, 2, 3, 4, 5, 8, 9, 10, 11]
        );
        assert_eq!(interactive_knob_indices(l6fw::Model::L6max).len(), 13);
        assert_eq!((layout.width, layout.height), (1200.0, 592.0));
        assert_eq!(layout.lcd, Region::new(0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn l6_mute_hits_use_verified_panel_targets_and_drawn_button_centers() {
        let layout = Layout::for_model(l6fw::Model::L6);
        let expected = [
            (10, 90.0),
            (13, 196.0),
            (16, 303.0),
            (19, 410.0),
            (22, 516.0),
            (25, 622.0),
        ];
        for (target, x) in expected {
            let (control, region) = layout
                .buttons
                .iter()
                .find(|(control, _)| control.id == target)
                .expect("verified L6 mute target");
            assert!(control.name.ends_with("-mute"));
            assert_eq!(region.x + region.width / 2.0, x);
            assert_eq!(region.y + region.height / 2.0, 421.0);
            assert_eq!((region.width, region.height), (54.0, 28.0));
        }
        for (target, x) in [(2, 303.0), (3, 410.0), (6, 516.0), (47, 622.0)] {
            let (control, region) = layout
                .buttons
                .iter()
                .find(|(control, _)| control.id == target)
                .expect("verified L6 source target");
            assert!(control.name.contains("mono") || control.name.contains("usb"));
            assert_eq!(region.x + region.width / 2.0, x);
            assert_eq!(region.y + region.height / 2.0, 373.0);
        }
        let phantom = layout
            .buttons
            .iter()
            .find(|(control, _)| control.id == 7)
            .expect("verified L6 phantom power target")
            .1;
        assert_eq!(
            (
                phantom.x + phantom.width / 2.0,
                phantom.y + phantom.height / 2.0
            ),
            (143.0, 373.0)
        );
        for (target, y) in [
            (8, 70.0),
            (11, 119.0),
            (14, 168.0),
            (17, 217.0),
            (20, 332.0),
            (23, 379.0),
            (26, 427.0),
            (29, 477.0),
            (28, 535.0),
        ] {
            let region = layout.button(target);
            assert_eq!(region.x + region.width / 2.0, 723.0);
            assert_eq!(region.y + region.height / 2.0, y);
        }
    }

    #[test]
    fn l6_scene_hits_match_the_three_drawn_scene_buttons() {
        let layout = Layout::for_model(l6fw::Model::L6);
        for (target, label, y) in [
            (15, "scene-a", 225.0),
            (18, "scene-b", 276.0),
            (21, "scene-c", 327.0),
        ] {
            let (control, region) = layout
                .buttons
                .iter()
                .find(|(control, _)| control.id == target)
                .expect("verified L6 scene target");
            assert_eq!(control.name, label);
            assert_eq!(region.x + region.width / 2.0, 1110.0);
            assert_eq!(region.y + region.height / 2.0, y);
        }
    }

    #[test]
    fn l6_record_hit_matches_the_drawn_record_button() {
        let layout = Layout::for_model(l6fw::Model::L6);
        let (control, region) = layout
            .buttons
            .iter()
            .find(|(control, _)| control.id == 24)
            .expect("verified L6 record target");
        assert_eq!(control.name, "rec-record");
        assert_eq!(region.x + region.width / 2.0, 1110.0);
        assert_eq!(region.y + region.height / 2.0, 378.0);
    }

    #[test]
    fn l6_gpio_button_hits_match_play_tap_and_sound_pads() {
        let layout = Layout::for_model(l6fw::Model::L6);
        let expected = [
            (4, 1110.0, 428.0),
            (48, 904.0, 270.0),
            (49, 811.0, 271.0),
            (50, 811.0, 319.0),
            (51, 811.0, 367.0),
            (52, 811.0, 415.0),
        ];
        for (target, x, y) in expected {
            let (_, region) = layout
                .buttons
                .iter()
                .find(|(control, _)| control.id == target)
                .expect("verified L6 main-GPIO hit target");
            assert_eq!(region.x + region.width / 2.0, x);
            assert_eq!(region.y + region.height / 2.0, y);
        }
    }

    #[test]
    fn model_windows_keep_hardware_scale_and_body_proportions() {
        let l6 = Layout::for_model(l6fw::Model::L6);
        let l6max = Layout::for_model(l6fw::Model::L6max);
        let l6_scale = window_width(l6fw::Model::L6) / l6.width;
        let l6max_scale = window_width(l6fw::Model::L6max) / l6max.width;
        // Both faceplates use the same physical socket sizes after applying
        // the L6 panel-coordinate scale and their model-specific window scale.
        for diameter in [TRS_SOCKET_DIAMETER, COMBO_SOCKET_DIAMETER] {
            assert!(
                (diameter * L6_FACEPLATE_SCALE * l6_scale - diameter * l6max_scale).abs() < 0.01
            );
        }
        assert_eq!(window_width(l6fw::Model::L6) / WINDOW_WIDTH, 223.0 / 284.0);
    }
}
