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
pub fn channel_x(channel: usize) -> f32 {
    88.0 + CHANNEL_WIDTH * channel as f32
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
}
