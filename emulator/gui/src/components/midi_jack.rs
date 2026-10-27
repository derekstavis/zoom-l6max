//! Small five-pin MIDI jack component.
use gpui::{App, IntoElement, RenderOnce, Window, div, prelude::*, px, rgb};

#[derive(IntoElement)]
pub struct MidiJack {
    pub diameter: f32,
    pub border_width: f32,
}

impl MidiJack {
    pub fn new(diameter: f32, border_width: f32) -> Self {
        Self {
            diameter,
            border_width,
        }
    }
}

impl RenderOnce for MidiJack {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .size(px(self.diameter))
            .flex_shrink_0()
            .rounded_full()
            .border(px(self.border_width))
            .border_color(rgb(0x080808))
            .bg(rgb(0x151515))
    }
}
