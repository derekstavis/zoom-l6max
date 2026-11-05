//! A single panel indicator LED with decoded on/off state.
use gpui::{App, IntoElement, RenderOnce, Window, div, prelude::*, px, rgb};

/// Unlit lamp color.
pub const OFF: u32 = 0x55594b;

#[derive(IntoElement)]
pub struct IndicatorLed {
    pub diameter: f32,
    pub color: Option<u32>,
}

impl IndicatorLed {
    pub fn new(diameter: f32, color: Option<u32>) -> Self {
        Self { diameter, color }
    }
}

impl RenderOnce for IndicatorLed {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .w(px(self.diameter))
            .h(px(self.diameter))
            .flex_shrink_0()
            .rounded_full()
            .border(px((self.diameter * 0.13).max(0.5)))
            .border_color(rgb(0x27292b))
            .bg(rgb(self.color.unwrap_or(OFF)))
    }
}

/// Mix `off` toward `on` by `level` (0..=255) in linear light, so a lamp's
/// apparent brightness follows its PWM duty.
pub fn blend(off: u32, on: u32, level: u8) -> u32 {
    fn srgb_to_linear(channel: u8) -> f64 {
        let channel = f64::from(channel) / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    fn linear_to_srgb(channel: f64) -> u8 {
        let channel = if channel <= 0.0031308 {
            channel * 12.92
        } else {
            1.055 * channel.powf(1.0 / 2.4) - 0.055
        };
        (channel.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    }

    let amount = f64::from(level) / 255.0;
    let mix = |shift: u32| {
        let off = srgb_to_linear((off >> shift) as u8);
        let on = srgb_to_linear((on >> shift) as u8);
        u32::from(linear_to_srgb(off + (on - off) * amount)) << shift
    };
    mix(16) | mix(8) | mix(0)
}

/// A lamp lit at `level`, or `None` when its duty is zero.
pub fn lit(on: u32, level: u8) -> Option<u32> {
    (level != 0).then(|| blend(OFF, on, level))
}

#[cfg(test)]
mod tests {
    use super::{OFF, blend, lit};

    #[test]
    fn blend_reaches_both_endpoints_and_brightens_monotonically() {
        let on = 0x76db52;
        assert_eq!(blend(OFF, on, 0), OFF);
        assert_eq!(blend(OFF, on, 255), on);
        let green = |level| (blend(OFF, on, level) >> 8) & 0xff;
        assert!((0..255).all(|level| green(level) <= green(level + 1)));
        assert_eq!(lit(on, 0), None);
        assert_eq!(lit(on, 255), Some(on));
    }
}
