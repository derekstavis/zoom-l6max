//! A single panel indicator LED with decoded on/off state.
use gpui::{App, IntoElement, RenderOnce, Window, div, prelude::*, px, rgb};

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
            .bg(rgb(self.color.unwrap_or(0x55594b)))
    }
}
