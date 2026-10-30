use crate::components::{layout::Layout, panel};
use gpui::{
    App, Context, HeadlessAppContext, ImageSource, Render, RenderImage, Window, div, img,
    prelude::*, px, rgb, size,
};
use gpui_platform::{current_headless_renderer, current_platform};
use image::{Frame, ImageBuffer, Rgba};
use l6max_host::panel::PanelData;
use serde::Deserialize;
use smallvec::SmallVec;
use std::{fs, path::Path, sync::Arc};

struct PanelSnapshot {
    model: l6fw::Model,
    data: PanelData,
    frame: Arc<RenderImage>,
}

impl Render for PanelSnapshot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let layout = Layout::for_model(self.model);
        let scale = super::components::layout::window_width(self.model) / layout.width;
        let lcd = layout.lcd;
        let lcd_scale = (lcd.width / 128.0).min(lcd.height / 64.0);
        let lcd_width = 128.0 * lcd_scale;
        let lcd_height = 64.0 * lcd_scale;
        div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .bg(rgb(0x000000))
            .child(
                div()
                    .relative()
                    .w(px(layout.width * scale))
                    .h(px(layout.height * scale))
                    .flex_shrink_0()
                    .child(panel::panel(
                        &layout,
                        scale,
                        self.data,
                        self.model,
                        &super::components::panel::PanelInteractions::default(),
                    ))
                    .when(self.model == l6fw::Model::L6max, |element| {
                        element.child(
                            div()
                                .absolute()
                                .left(px(lcd.x * scale))
                                .top(px(lcd.y * scale))
                                .w(px(lcd.width * scale))
                                .h(px(lcd.height * scale))
                                .overflow_hidden()
                                .bg(rgb(0x050708))
                                .child(
                                    img(ImageSource::Render(self.frame.clone()))
                                        .absolute()
                                        .left(px((lcd.width - lcd_width) * 0.5 * scale))
                                        .top(px((lcd.height - lcd_height) * 0.5 * scale))
                                        .w(px(lcd_width * scale))
                                        .h(px(lcd_height * scale))
                                        .object_fit(gpui::ObjectFit::Contain),
                                ),
                        )
                    }),
            )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    indicators: Vec<u32>,
    indicator_levels: Vec<Vec<u8>>,
    switches: Vec<bool>,
    analog: Vec<u16>,
    powered: bool,
    display: Vec<u8>,
}

pub fn render(args: &[String]) -> Result<(), String> {
    let model = match args.first().map(String::as_str) {
        Some("l6") => l6fw::Model::L6,
        Some("l6max") => l6fw::Model::L6max,
        _ => {
            return Err(
                "usage: --render-panel <l6|l6max> [--data panel.json] [--powered-off] --output <path.png>".into(),
            );
        }
    };
    let powered = !args.iter().any(|arg| arg == "--powered-off");
    let output = option(args, "--output").ok_or("missing --output <path.png>")?;
    if Path::new(output).exists() {
        return Err(format!("refusing to overwrite {output}"));
    }
    let data = if let Some(path) = option(args, "--data") {
        let snapshot: Snapshot =
            serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("invalid panel snapshot: {error}"))?;
        let indicators = snapshot
            .indicators
            .try_into()
            .map_err(|_| "panel snapshot must contain eight indicator rows")?;
        let indicator_levels = snapshot
            .indicator_levels
            .into_iter()
            .map(|row| {
                row.try_into()
                    .map_err(|_| "indicator level rows must contain 24 values")
            })
            .collect::<Result<Vec<[u8; 24]>, _>>()?
            .try_into()
            .map_err(|_| "panel snapshot must contain eight indicator level rows")?;
        let switches = snapshot
            .switches
            .try_into()
            .map_err(|_| "panel snapshot must contain 54 switches")?;
        let analog = snapshot
            .analog
            .try_into()
            .map_err(|_| "panel snapshot must contain five analog values")?;
        let display = snapshot
            .display
            .try_into()
            .map_err(|_| "panel snapshot display must contain 1024 bytes")?;
        PanelData {
            indicators,
            indicator_levels,
            switches,
            analog,
            powered: snapshot.powered,
            display,
        }
    } else {
        let mut indicators = [0; 8];
        if model == l6fw::Model::L6 {
            // Show the default Hall effect and LEVEL mode states in the demo.
            indicators[0] = 1 << 12;
            indicators[4] |= 1 << 8;
        }
        PanelData {
            indicators,
            indicator_levels: [[0; 24]; 8],
            switches: [false; 54],
            analog: [512; 5],
            powered,
            display: [0; 1024],
        }
    };
    let lcd = ImageBuffer::from_fn(128, 64, |x, y| {
        let index = ((63 - y as usize) / 8) * 128 + x as usize;
        let bit = (data.display[index] >> ((63 - y as usize) % 8)) & 1;
        let [r, g, b] = if bit != 0 {
            [172, 207, 224]
        } else {
            [8, 12, 15]
        };
        Rgba([b, g, r, 255])
    });
    let frame = Arc::new(RenderImage::new(SmallVec::from_elem(Frame::new(lcd), 1)));
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
            |_, cx: &mut App| cx.new(|_| PanelSnapshot { model, data, frame }),
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

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}
