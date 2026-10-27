use crate::components::{layout::Layout, panel};
use gpui::{App, Context, HeadlessAppContext, Render, Window, div, prelude::*, px, rgb, size};
use gpui_platform::{current_headless_renderer, current_platform};
use std::sync::Arc;

struct PanelSnapshot {
    model: l6fw::Model,
    powered: bool,
}

impl Render for PanelSnapshot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let layout = Layout::for_model(self.model);
        let scale = super::components::layout::window_width(self.model) / layout.width;
        let mut indicators = [0; 8];
        if self.model == l6fw::Model::L6 {
            // Show the firmware's default Hall effect and LEVEL mode states
            // in the reference screenshot.
            indicators[0] = 1 << 12;
            // The channel strip starts in LEVEL mode.
            indicators[4] |= 1 << 8;
        }
        div().size_full().bg(rgb(0x000000)).child(panel::panel(
            &layout,
            scale,
            panel::PanelData {
                indicators,
                powered: self.powered,
                ..Default::default()
            },
            self.model,
            &super::components::panel::PanelInteractions::default(),
        ))
    }
}

pub fn render(args: &[String]) -> Result<(), String> {
    let model = match args.first().map(String::as_str) {
        Some("l6") => l6fw::Model::L6,
        Some("l6max") => l6fw::Model::L6max,
        _ => {
            return Err(
                "usage: --render-panel <l6|l6max> [--powered-off] --output <path.png>".into(),
            );
        }
    };
    let powered = !args.iter().any(|arg| arg == "--powered-off");
    let output = args
        .windows(2)
        .find(|pair| pair[0] == "--output")
        .map(|pair| pair[1].as_str())
        .ok_or("missing --output <path.png>")?;
    let layout = Layout::for_model(model);
    let device_width = super::components::layout::window_width(model);
    let device_height = device_width * layout.height / layout.width;
    let platform = current_platform(true);
    let text_system = platform.text_system();
    let mut app = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
        current_headless_renderer()
    });
    let handle = app
        .open_window(
            size(px(device_width), px(device_height)),
            |_, cx: &mut App| cx.new(|_| PanelSnapshot { model, powered }),
        )
        .map_err(|error| error.to_string())?;
    app.run_until_parked();
    let image = app
        .capture_screenshot(handle.into())
        .map_err(|error| error.to_string())?;
    image.save(output).map_err(|error| error.to_string())?;
    println!(
        "Saved {}x{} {} panel to {}",
        image.width(),
        image.height(),
        match model {
            l6fw::Model::L6 => "L6",
            l6fw::Model::L6max => "L6max",
        },
        output
    );
    Ok(())
}
