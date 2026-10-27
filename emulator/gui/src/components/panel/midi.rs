//! Shared MIDI input/output jack group for the L6 and L6max panels.
use super::super::{controls, layout::Region};
use super::shared::{at_local, label_local_sized};
use gpui::{Div, div, prelude::*};

#[derive(Clone, Copy)]
pub struct MidiLayout {
    pub title: Region,
    pub title_font: f32,
    pub center_x: f32,
    pub input_y: f32,
    pub output_y: f32,
    pub label_x: f32,
    pub label_width: f32,
    pub input_label_y: f32,
    pub output_label_y: f32,
}

pub fn midi_ports(scale: f32, layout: MidiLayout) -> Div {
    div()
        .absolute()
        .inset_0()
        .child(label_local_sized(
            layout.title.x,
            layout.title.y,
            layout.title.width,
            "MIDI",
            scale,
            layout.title_font,
        ))
        .child(label_local_sized(
            layout.label_x,
            layout.input_label_y,
            layout.label_width,
            "IN",
            scale,
            8.0,
        ))
        .child(label_local_sized(
            layout.label_x,
            layout.output_label_y,
            layout.label_width,
            "OUT",
            scale,
            8.0,
        ))
        .child(at_local(
            Region::centered(layout.center_x, layout.input_y, 14.0, 14.0),
            scale,
            controls::midi_jack(14.0 * scale, scale),
        ))
        .child(at_local(
            Region::centered(layout.center_x, layout.output_y, 14.0, 14.0),
            scale,
            controls::midi_jack(14.0 * scale, scale),
        ))
}
