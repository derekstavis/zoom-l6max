//! The window that assigns WAV files to input channels and controls their playback.
use crate::NativePanel;
use gpui::{
    Context, CursorStyle, Div, Entity, MouseButton, Render, Stateful, Window, div, prelude::*, px,
    rgb,
};
use l6max_host::{controls::audio_input_channels, protocol};
use std::time::Duration;

pub const WIDTH: f32 = 560.0;
const PADDING: f32 = 16.0;
const BAR_WIDTH: f32 = WIDTH - 2.0 * PADDING;

pub struct AudioWindow {
    panel: Entity<NativePanel>,
}

impl AudioWindow {
    pub fn new(panel: Entity<NativePanel>, cx: &mut Context<Self>) -> Self {
        // Position and conversion progress change without any input to this window.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self { panel }
    }
}

fn button(id: impl Into<gpui::ElementId>, label: &str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .rounded_md()
        .px_3()
        .py_1()
        .border_1()
        .border_color(rgb(0x34405c))
        .bg(rgb(if enabled { 0x2a3550 } else { 0x171d2c }))
        .text_color(rgb(if enabled { 0xe8eef8 } else { 0x6a7388 }))
        .when(enabled, |element| element.cursor(CursorStyle::PointingHand))
        .child(label.to_owned())
}

fn bar(fraction: f32, color: u32) -> Div {
    div()
        .w(px(BAR_WIDTH))
        .h(px(8.0))
        .rounded_sm()
        .bg(rgb(0x171d2c))
        .child(
            div()
                .h_full()
                .rounded_sm()
                .w(px(BAR_WIDTH * fraction.clamp(0.0, 1.0)))
                .bg(rgb(color)),
        )
}

fn time(milliseconds: u32) -> String {
    let seconds = milliseconds / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl Render for AudioWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = self.panel.read(cx);
        let model = panel.model;
        let progress = panel.audio_progress();
        let converting = panel.audio_conversion.is_some();
        let patch = panel.audio_patch.clone();
        let error = panel.audio_error.clone();
        let length_ms = panel.audio_length_ms;
        let (position_ms, playing) = panel.audio_position();
        let running = panel.controls.is_some();
        let channels = audio_input_channels(model);
        let mono_channels = match model {
            l6fw::Model::L6 => 2,
            l6fw::Model::L6max => 4,
        };
        let entity = self.panel.clone();
        let act = move |action: fn(&mut NativePanel, &mut Context<NativePanel>)| {
            let entity = entity.clone();
            move |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| {
                entity.update(cx, action);
            }
        };
        let rows = (1..=channels).map(|channel| {
            let file = patch.get(&channel);
            let name = file
                .and_then(|path| path.file_name())
                .map_or("No file".to_owned(), |name| {
                    name.to_string_lossy().into_owned()
                });
            let (choose, clear) = (self.panel.clone(), self.panel.clone());
            let converted = progress.get(&channel).copied();
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(20.0)).child(channel.to_string()))
                .child(div().w(px(52.0)).text_color(rgb(0x8b95aa)).child(
                    if channel <= mono_channels {
                        "Mono"
                    } else {
                        "Stereo"
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(rgb(if file.is_some() { 0xe8eef8 } else { 0x6a7388 }))
                                .child(name),
                        )
                        .when_some(converted, |element, converted| {
                            element.child(
                                div()
                                    .w_full()
                                    .h(px(4.0))
                                    .rounded_sm()
                                    .bg(rgb(0x171d2c))
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_sm()
                                            .w(gpui::relative(converted.clamp(0.0, 1.0)))
                                            .bg(rgb(0x5367df)),
                                    ),
                            )
                        }),
                )
                .child(button(("choose", channel), "Choose…", !converting).when(
                    !converting,
                    |element| {
                        element.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            choose.update(cx, |panel, cx| panel.choose_audio_file(channel, cx));
                        })
                    },
                ))
                .child(
                    button(("clear", channel), "Clear", !converting && file.is_some()).when(
                        !converting && file.is_some(),
                        |element| {
                            element.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                clear.update(cx, |panel, cx| {
                                    panel.set_audio_file(channel, None, cx)
                                });
                            })
                        },
                    ),
                )
        });
        let seek = self.panel.clone();
        let can_play = running && length_ms > 0 && !converting;
        div()
            .size_full()
            .p(px(PADDING))
            .bg(rgb(0x10141f))
            .text_color(rgb(0xe8eef8))
            .text_sm()
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .pt_1()
                    .child(button("assign", "Assign Files…", !converting).when(
                        !converting,
                        |element| {
                            element.on_mouse_down(
                                MouseButton::Left,
                                act(NativePanel::assign_audio_inputs),
                            )
                        },
                    ))
                    .child(
                        button("clear-all", "Clear All", !converting && !patch.is_empty()).when(
                            !converting && !patch.is_empty(),
                            |element| {
                                element.on_mouse_down(
                                    MouseButton::Left,
                                    act(NativePanel::clear_audio_inputs),
                                )
                            },
                        ),
                    ),
            )
            .when_some(error, |element, error| {
                element.child(div().text_color(rgb(0xffb3a7)).child(error))
            })
            .child(
                div()
                    .id("position")
                    .pt_2()
                    .when(can_play, |element| {
                        element.cursor(CursorStyle::PointingHand).on_mouse_down(
                            MouseButton::Left,
                            move |event, _, cx| {
                                let fraction = (event.position.x.as_f32() - PADDING) / BAR_WIDTH;
                                seek.update(cx, |panel, cx| {
                                    let position =
                                        panel.audio_length_ms as f32 * fraction.clamp(0.0, 1.0);
                                    panel.audio_input_seek(position as i32, cx);
                                });
                            },
                        )
                    })
                    .child(bar(
                        if length_ms > 0 {
                            position_ms as f32 / length_ms as f32
                        } else {
                            0.0
                        },
                        0x4fb286,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button("play", if playing { "Pause" } else { "Play" }, can_play).when(
                            can_play,
                            |element| {
                                element.on_mouse_down(
                                    MouseButton::Left,
                                    act(if playing {
                                        |panel, cx| {
                                            panel.audio_input_transport(
                                                protocol::AUDIO_INPUT_PAUSE,
                                                cx,
                                            )
                                        }
                                    } else {
                                        |panel, cx| {
                                            panel.audio_input_transport(
                                                protocol::AUDIO_INPUT_PLAY,
                                                cx,
                                            )
                                        }
                                    }),
                                )
                            },
                        ),
                    )
                    .child(button("stop", "Stop", can_play).when(can_play, |element| {
                        element.on_mouse_down(
                            MouseButton::Left,
                            act(|panel, cx| {
                                panel.audio_input_transport(protocol::AUDIO_INPUT_STOP, cx)
                            }),
                        )
                    }))
                    .child(
                        button("rewind", "Rewind", can_play).when(can_play, |element| {
                            element.on_mouse_down(
                                MouseButton::Left,
                                act(|panel, cx| panel.audio_input_seek(0, cx)),
                            )
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .text_color(rgb(0x8b95aa))
                            .child(format!("{} / {}", time(position_ms), time(length_ms))),
                    ),
            )
    }
}
