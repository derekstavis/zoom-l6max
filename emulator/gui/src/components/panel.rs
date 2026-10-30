//! Model-selected panel renderer; model-specific surfaces stay in separate modules.
mod effects;
mod l6;
mod l6max;
mod midi;
pub(super) mod shared;
pub use super::vu_meter;

use super::layout::Layout;
use super::{button::ButtonEvents, encoder::EncoderInput};
use gpui::Div;
use std::sync::Arc;

pub use l6max_host::panel::PanelData;

#[derive(Clone)]
pub struct PanelInteractions {
    pub button_down: Arc<dyn Fn(u32, bool) + Send + Sync>,
    pub button_up: Arc<dyn Fn(u32) + Send + Sync>,
    pub switch_toggle: Arc<dyn Fn(u32) + Send + Sync>,
    pub encoder_drag_start: Arc<dyn Fn(usize, f32, f32) + Send + Sync>,
    pub encoder_drag_move: Arc<dyn Fn(usize, bool, f32, f32) + Send + Sync>,
    pub encoder_scroll: Arc<dyn Fn(usize, f32) + Send + Sync>,
    pub encoder_drag_end: Arc<dyn Fn(usize) + Send + Sync>,
}

impl Default for PanelInteractions {
    fn default() -> Self {
        Self {
            button_down: Arc::new(|_, _| {}),
            button_up: Arc::new(|_| {}),
            switch_toggle: Arc::new(|_| {}),
            encoder_drag_start: Arc::new(|_, _, _| {}),
            encoder_drag_move: Arc::new(|_, _, _, _| {}),
            encoder_scroll: Arc::new(|_, _| {}),
            encoder_drag_end: Arc::new(|_| {}),
        }
    }
}

impl PanelInteractions {
    pub fn button_events(&self, id: u32, is_switch: bool) -> ButtonEvents {
        let down = self.button_down.clone();
        let up = self.button_up.clone();
        ButtonEvents {
            press: Some(Arc::new(move || down(id, is_switch))),
            release: (!is_switch).then(|| Arc::new(move || up(id)) as Arc<dyn Fn() + Send + Sync>),
        }
    }

    pub fn switch_toggle(&self, id: u32) -> Arc<dyn Fn() + Send + Sync> {
        let toggle = self.switch_toggle.clone();
        Arc::new(move || toggle(id))
    }

    pub fn encoder_input(&self, channel: usize) -> EncoderInput {
        let start = self.encoder_drag_start.clone();
        let scroll = self.encoder_scroll.clone();
        let end = self.encoder_drag_end.clone();
        EncoderInput {
            drag_start: Some(Arc::new(move |x, y| start(channel, x, y))),
            scroll: Some(Arc::new(move |delta| scroll(channel, delta))),
            drag_end: Some(Arc::new(move || end(channel))),
        }
    }
}

pub fn panel(
    layout: &Layout,
    scale: f32,
    data: PanelData,
    model: l6fw::Model,
    interactions: &PanelInteractions,
) -> Div {
    let indicator_levels = if data
        .indicator_levels
        .iter()
        .flatten()
        .all(|&level| level == 0)
    {
        std::array::from_fn(|row| {
            std::array::from_fn(|bit| {
                if data.indicators[row] & (1 << bit) != 0 {
                    255
                } else {
                    0
                }
            })
        })
    } else {
        data.indicator_levels
    };
    match model {
        l6fw::Model::L6 => l6::panel(
            layout,
            scale,
            data.indicators,
            indicator_levels,
            data.analog,
            data.powered,
            interactions,
        ),
        l6fw::Model::L6max => l6max::panel(
            layout,
            scale,
            data.indicators,
            indicator_levels,
            data.switches,
            data.analog,
            interactions,
        ),
    }
}
