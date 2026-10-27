//! Reusable USB-C receptacle component.
use gpui::{App, IntoElement, RenderOnce, Window, div, prelude::*, px, rgb};

#[derive(IntoElement)]
pub struct UsbCPort {
    pub width: f32,
    pub height: f32,
}

impl UsbCPort {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

impl RenderOnce for UsbCPort {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (w, h) = (self.width, self.height);
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
}
