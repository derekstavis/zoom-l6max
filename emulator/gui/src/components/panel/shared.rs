//! Drawing helpers shared by the separate L6 and L6max panel views.
use super::super::layout::Region;
use gpui::{Div, IntoElement, div, prelude::*, px, rgb};

pub fn at(region: Region, s: f32, content: impl IntoElement) -> Div {
    at_local(region.local(), s, content)
}

pub fn at_local(region: Region, s: f32, content: impl IntoElement) -> Div {
    div()
        .absolute()
        .left(px(region.x * s))
        .top(px(region.y * s))
        .w(px(region.width * s))
        .h(px(region.height * s))
        .child(content)
}

/// Center a model identifier in a panel's printed nameplate area.
pub fn panel_identifier(region: Region, text: &str, s: f32, font: f32) -> Div {
    panel_identifier_at(at(region, s, div()), text, s, font)
}

/// Center a model identifier using panel-local coordinates.
pub fn panel_identifier_local(region: Region, text: &str, s: f32, font: f32) -> Div {
    panel_identifier_at(at_local(region, s, div()), text, s, font)
}

fn panel_identifier_at(container: Div, text: &str, s: f32, font: f32) -> Div {
    container.child(
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .font_family("Helvetica Neue")
            .font_weight(gpui::FontWeight::BOLD)
            .text_size(px(font * s))
            .text_color(rgb(0xe1e6e9))
            .child(text.to_owned()),
    )
}

pub fn label(x: f32, y: f32, w: f32, text: &str, s: f32) -> Div {
    label_sized(x, y, w, text, s, 8.0)
}

pub fn label_sized(x: f32, y: f32, w: f32, text: &str, s: f32, font: f32) -> Div {
    at(
        Region::new(x, y, w, 20.0),
        s,
        div()
            .relative()
            .size_full()
            .font_family("Helvetica Neue")
            .line_height(px(14.0 * s))
            .text_center()
            .text_size(px(font * s))
            .text_color(rgb(0xe1e6e9))
            .child(text.to_owned()),
    )
}

pub fn line(x: f32, y: f32, w: f32, h: f32, s: f32) -> Div {
    at(
        Region::new(x, y, w, h),
        s,
        div().size_full().bg(rgb(0xc4cbcd)),
    )
}

pub fn label_local(x: f32, y: f32, w: f32, text: &str, s: f32) -> Div {
    label_local_sized(x, y, w, text, s, 9.5)
}

pub fn label_local_sized(x: f32, y: f32, w: f32, text: &str, s: f32, font: f32) -> Div {
    at_local(
        Region::new(x, y, w, 20.0),
        s,
        div()
            .relative()
            .size_full()
            .font_family("Helvetica Neue")
            .line_height(px(14.0 * s))
            .text_center()
            .text_size(px(font * s))
            .text_color(rgb(0xe1e6e9))
            .child(text.to_owned()),
    )
}

/// Light printed section tab in L6-local faceplate coordinates.
pub fn section_header_local(x: f32, y: f32, w: f32, h: f32, text: &str, s: f32) -> Div {
    section_header_local_sized(x, y, w, h, text, s, 11.0)
}

pub fn section_header_local_sized(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    text: &str,
    s: f32,
    font: f32,
) -> Div {
    at_local(
        Region::new(x, y, w, h),
        s,
        section_header_content_sized(text, s, font),
    )
}

/// Light printed section tab using faceplate (global) coordinates.
pub fn section_header(x: f32, y: f32, w: f32, h: f32, text: &str, s: f32) -> Div {
    at(Region::new(x, y, w, h), s, section_header_content(text, s))
}

fn section_header_content(text: &str, s: f32) -> Div {
    section_header_content_sized(text, s, 9.0)
}

fn section_header_content_sized(text: &str, s: f32, font: f32) -> Div {
    div()
        .size_full()
        .font_family("Helvetica Neue")
        .bg(rgb(0xd0dce0))
        .text_color(rgb(0x292d30))
        .text_size(px(font * s))
        .text_center()
        .child(text.to_owned())
}
