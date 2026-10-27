//! Reusable latching switch component.
use gpui::{App, IntoElement, MouseButton, RenderOnce, div, prelude::*, px, rgb};
use std::sync::Arc;

#[derive(IntoElement)]
pub struct Switch {
    pub width: f32,
    pub height: f32,
    pub on: bool,
    pub on_toggle: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Switch {
    pub fn new(width: f32, height: f32, on: bool) -> Self {
        Self {
            width,
            height,
            on,
            on_toggle: None,
        }
    }

    pub fn on_toggle(mut self, handler: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_toggle = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            width: w,
            height: h,
            on,
            on_toggle,
        } = self;
        let short = w.min(h);
        let cap = short * 0.76;
        let inset = short * 0.12;
        let switch = div()
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
            );
        if let Some(on_toggle) = on_toggle {
            switch.on_mouse_down(MouseButton::Left, move |_, _, _| on_toggle())
        } else {
            switch
        }
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
}
