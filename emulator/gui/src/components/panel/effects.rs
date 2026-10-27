//! Shared effects LED and legend group.
use super::super::{controls, layout::Region};
use super::shared::at_local;
use gpui::{Div, div, prelude::*, px, rgb};

#[derive(Clone, Copy)]
pub struct EffectsLayout {
    pub led_x: f32,
    pub first_led_y: f32,
    pub led_step: f32,
    pub label_x: f32,
    pub label_y_offset: f32,
    pub label_width: f32,
    pub ai_noise_reduction: Option<(Region, f32, f32)>,
}

pub fn effects_legend(scale: f32, leds: [bool; 6], layout: EffectsLayout) -> Div {
    let mut group = div().absolute();
    if let Some((ai_label, ai_led_x, ai_led_y)) = layout.ai_noise_reduction {
        group = group
            .child(at_local(
                ai_label,
                scale,
                div()
                    .size_full()
                    .pl(px(scale))
                    .text_left()
                    .text_size(px(7.5 * scale))
                    .line_height(px(8.0 * scale))
                    .text_color(rgb(0x293138))
                    .bg(rgb(0xd0dce0))
                    .child("AI Noise\nReduction"),
            ))
            .child(at_local(
                Region::centered(ai_led_x, ai_led_y, 6.0, 6.0),
                scale,
                controls::led(6.0 * scale, leds[0].then_some(0x76db52)),
            ));
    }

    for (index, name) in ["Hall", "Room", "Spring", "Delay", "Echo"]
        .iter()
        .enumerate()
    {
        let y = layout.first_led_y + layout.led_step * index as f32;
        group = group
            .child(at_local(
                Region::centered(layout.led_x, y, 6.0, 6.0),
                scale,
                controls::led(6.0 * scale, leds[index + 1].then_some(0x76db52)),
            ))
            .child(at_local(
                Region::new(
                    layout.label_x,
                    y + layout.label_y_offset,
                    layout.label_width,
                    14.0,
                ),
                scale,
                div()
                    .size_full()
                    .text_left()
                    .text_size(px(8.0 * scale))
                    .line_height(px(14.0 * scale))
                    .text_color(rgb(0xe1e6e9))
                    .child(*name),
            ));
    }
    group
}
