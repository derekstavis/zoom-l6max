//! Shared GPUI component for the L6 and L6max paired-segment level meters.
use super::{
    layout::Region,
    panel::shared::{at_local, label_local_sized},
};
use gpui::{App, Div, RenderOnce, div, prelude::*, px, rgb};

#[derive(Clone, Copy)]
pub struct ScaleLabel {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub text: &'static str,
    pub font: f32,
}

/// Faceplate-derived meter geometry and model-specific segment styling.
pub struct MeterStyle {
    pub bezel: Region,
    pub columns: usize,
    pub rows: usize,
    pub first_segment: Region,
    pub column_step: f32,
    pub row_step: f32,
    pub appearance: MeterAppearance,
    pub active_fill: u32,
}

/// Shared L6/L6max meter finish. Their faceplate geometry and live LED state
/// differ, while the rendered bezel and segment style stays identical.
#[derive(Clone, Copy)]
pub struct MeterAppearance {
    pub bezel_radius: f32,
    pub bezel_fill: u32,
    pub bezel_border: Option<(f32, u32)>,
    pub segment_radius: f32,
    pub segment_fill: u32,
    pub segment_border: Option<(f32, u32)>,
}

pub const SHARED_APPEARANCE: MeterAppearance = MeterAppearance {
    bezel_radius: 2.0,
    bezel_fill: 0x36393c,
    bezel_border: Some((1.0, 0x202225)),
    segment_radius: 1.0,
    segment_fill: 0x424549,
    segment_border: Some((1.0, 0x202225)),
};

/// Component props. Levels count illuminated segments from the bottom (0–6);
/// `None` leaves that side unlit while its source is unavailable.
pub struct VuMeterProps {
    pub scale: f32,
    pub style: MeterStyle,
    pub scale_labels: &'static [ScaleLabel],
    pub left_level: Option<u8>,
    pub right_level: Option<u8>,
}

#[derive(IntoElement)]
pub struct VuMeter(pub VuMeterProps);

impl VuMeter {
    pub fn new(props: VuMeterProps) -> Self {
        Self(props)
    }
}

impl RenderOnce for VuMeter {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        paired_meter(self.0)
    }
}

fn paired_meter(props: VuMeterProps) -> Div {
    let VuMeterProps {
        scale,
        style,
        scale_labels,
        left_level,
        right_level,
    } = props;
    let mut meter = div().relative().size_full();
    meter = meter.child(at_local(
        style.bezel,
        scale,
        styled_box(
            scale,
            style.appearance.bezel_radius,
            style.appearance.bezel_fill,
            style.appearance.bezel_border,
        ),
    ));

    for row in 0..style.rows {
        for column in 0..style.columns {
            let x = style.first_segment.x + style.column_step * column as f32;
            let y = style.first_segment.y + style.row_step * row as f32;
            let mut segment = styled_box(
                scale,
                style.appearance.segment_radius,
                if level_lights_segment(
                    if column == 0 { left_level } else { right_level },
                    row,
                    style.rows,
                ) {
                    style.active_fill
                } else {
                    style.appearance.segment_fill
                },
                style.appearance.segment_border,
            );
            segment = segment.size_full();
            meter = meter.child(at_local(
                Region::new(x, y, style.first_segment.width, style.first_segment.height),
                scale,
                segment,
            ));
        }
    }

    for mark in scale_labels {
        meter = meter.child(label_local_sized(
            mark.x, mark.y, mark.width, mark.text, scale, mark.font,
        ));
    }
    meter
}

fn level_lights_segment(level: Option<u8>, row: usize, rows: usize) -> bool {
    level.is_some_and(|level| row >= rows.saturating_sub(usize::from(level.min(rows as u8))))
}

#[cfg(test)]
mod tests {
    use super::level_lights_segment;

    #[test]
    fn vu_levels_light_segments_from_the_bottom_and_clamp_to_meter_range() {
        let rows = 6;
        assert!((0..rows).all(|row| !level_lights_segment(None, row, rows)));
        assert!((0..rows).all(|row| !level_lights_segment(Some(0), row, rows)));
        assert_eq!(
            (0..rows)
                .filter(|row| level_lights_segment(Some(2), *row, rows))
                .collect::<Vec<_>>(),
            [4, 5]
        );
        assert!((0..rows).all(|row| level_lights_segment(Some(9), row, rows)));
    }
}

fn styled_box(scale: f32, radius: f32, fill: u32, border: Option<(f32, u32)>) -> Div {
    let mut box_view = div().size_full().rounded(px(radius * scale)).bg(rgb(fill));
    if let Some((width, color)) = border {
        box_view = box_view.border(px(width * scale)).border_color(rgb(color));
    }
    box_view
}
