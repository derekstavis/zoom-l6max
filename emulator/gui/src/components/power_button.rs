//! Reusable circular hardware power button.
use gpui::{App, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px, rgb};
use std::sync::Arc;

#[derive(IntoElement)]
pub struct PowerButton {
    pub diameter: f32,
    pub scale: f32,
    pub on_press: Option<Arc<dyn Fn() + Send + Sync>>,
    pub on_release: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl PowerButton {
    pub fn new(diameter: f32, scale: f32) -> Self {
        Self {
            diameter,
            scale,
            on_press: None,
            on_release: None,
        }
    }

    pub fn on_press(mut self, handler: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_press = Some(Arc::new(handler));
        self
    }

    pub fn on_release(mut self, handler: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_release = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for PowerButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let size = self.diameter;
        let scale = self.scale;
        let button = div()
            .relative()
            .w(px(size))
            .h(px(size))
            .flex_shrink_0()
            .rounded_full()
            .border(px(scale))
            .border_color(rgb(0x202326))
            .bg(rgb(0x898d91))
            .child(
                div()
                    .absolute()
                    .inset(px(2.0 * scale))
                    .rounded_full()
                    .bg(rgb(0xb8bdc1)),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(super::controls::icon("power", size * 0.6, 0x565b5f)),
            );
        let button = if let Some(on_press) = self.on_press {
            button
                .cursor(gpui::CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Left, move |_, _, _| on_press())
        } else {
            button
        };
        if let Some(on_release) = self.on_release {
            let release_out = on_release.clone();
            button
                .on_mouse_up(MouseButton::Left, move |_, _, _| on_release())
                .on_mouse_up_out(MouseButton::Left, move |_, _, _| release_out())
        } else {
            button
        }
    }
}
