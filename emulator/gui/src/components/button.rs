//! Reusable physical button component.
use super::controls::icon;
use gpui::{App, MouseButton, RenderOnce, div, prelude::*, px, rgb};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct ButtonEvents {
    pub press: Option<Arc<dyn Fn() + Send + Sync>>,
    pub release: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Props for a panel button. `size` is supplied by the parent layout, while
/// `scale` keeps borders, labels, and icons in step with the panel's scale.
#[derive(IntoElement)]
pub struct Button {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub label: String,
    pub symbol: Option<String>,
    pub lit_color: Option<u32>,
    pub on_press: Option<Arc<dyn Fn() + Send + Sync>>,
    pub on_release: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Button {
    pub fn new(
        width: f32,
        height: f32,
        scale: f32,
        label: impl Into<String>,
        symbol: Option<&str>,
        lit_color: Option<u32>,
    ) -> Self {
        Self {
            width,
            height,
            scale,
            label: label.into(),
            symbol: symbol.map(str::to_owned),
            lit_color,
            on_press: None,
            on_release: None,
        }
    }

    pub fn with_events(mut self, events: ButtonEvents) -> Self {
        self.on_press = events.press;
        self.on_release = events.release;
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            width: w,
            height: h,
            scale,
            label,
            symbol,
            lit_color,
            on_press,
            on_release,
        } = self;
        let face = if symbol.as_deref() == Some("record") {
            0xd5d8dc
        } else {
            lit_color.unwrap_or(0xd5d8dc)
        };
        let control = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w(px(w))
            .h(px(h))
            .flex_shrink_0()
            .rounded(px(3.0 * scale))
            .border(px(scale))
            .border_color(rgb(0x555a60))
            .bg(rgb(0x72777d))
            .child(
                div()
                    .absolute()
                    .left(px(scale))
                    .right(px(scale))
                    .top(px(scale))
                    .bottom(px(2.0 * scale))
                    .rounded(px(2.0 * scale))
                    .bg(rgb(face)),
            );
        let mut content = div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .font_family("Helvetica Neue")
            .text_size(px(if label.len() > 5 {
                7.0 * scale
            } else {
                9.0 * scale
            }))
            .text_color(rgb(0x282b30))
            .line_height(px(if label.contains('\n') {
                7.0 * scale
            } else {
                10.0 * scale
            }))
            .text_center();
        content = match symbol.as_deref() {
            Some("play-stop") => content
                .gap(px(2.0 * scale))
                .child(icon("play", h * 0.52, 0x282b30))
                .child(div().text_size(px(11.0 * scale)).child("/"))
                .child(icon("square", h * 0.46, 0x282b30)),
            Some("record") => content.child(
                div()
                    .w(px(h * 0.43))
                    .h(px(h * 0.43))
                    .rounded_full()
                    .bg(rgb(lit_color.unwrap_or(0xcd1930))),
            ),
            Some("usb") if !label.is_empty() => content
                .flex_col()
                .child(icon("usb", h * 0.47, 0x282b30))
                .child(
                    div()
                        .text_size(px(7.0 * scale))
                        .line_height(px(7.0 * scale))
                        .child(label),
                ),
            Some(name) => content.child(icon(name, (h * 0.7).min(w * 0.65), 0x282b30)),
            None => content.child(label),
        };
        let mut button = control
            .cursor(gpui::CursorStyle::PointingHand)
            .hover(|style| style.bg(gpui::rgba(0xffe6a455)));
        if let Some(on_press) = on_press {
            button = button.on_mouse_down(MouseButton::Left, move |_, _, _| on_press());
        }
        if let Some(on_release) = on_release {
            let release_out = on_release.clone();
            button = button
                .on_mouse_up(MouseButton::Left, move |_, _, _| on_release())
                .on_mouse_up_out(MouseButton::Left, move |_, _, _| release_out());
        }
        button.child(content)
    }
}
