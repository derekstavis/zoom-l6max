//! The window that selects which firmware patches the guest runs.
use crate::NativePanel;
use firmware_patch_core::PATCHES;
use gpui::{Context, CursorStyle, Entity, MouseButton, Render, Window, div, prelude::*, px, rgb};
use std::path::PathBuf;

pub const WIDTH: f32 = 480.0;

/// The patches offered here, in catalog order, as the web patcher offers them.
/// `midi-control` stays available to `--patch`, `config.toml` and tests.
const OFFERED: [&str; 3] = ["knob-dialog", "midi-sync", "din-mixer-control"];

fn description(patch: &str) -> &'static str {
    match patch {
        "knob-dialog" => "Show channel encoder values on the display",
        "midi-sync" => "Send the mixer state as Control Change on request",
        "din-mixer-control" => "Let the MIDI IN/OUT jacks control the mixer",
        _ => "",
    }
}

pub struct PatchesWindow {
    panel: Entity<NativePanel>,
    /// The firmware that `available` and `selected` describe.
    firmware: PathBuf,
    available: Result<Vec<&'static str>, String>,
    selected: Vec<String>,
}

impl PatchesWindow {
    pub fn new(panel: Entity<NativePanel>, _: &mut Context<Self>) -> Self {
        Self {
            panel,
            firmware: PathBuf::new(),
            available: Ok(Vec::new()),
            selected: Vec::new(),
        }
    }

    fn toggle(&mut self, patch: &str) {
        if let Some(index) = self.selected.iter().position(|name| name == patch) {
            self.selected.remove(index);
        } else {
            self.selected.push(patch.to_owned());
        }
    }

    /// Close and restart with the selection. The main window shows the progress,
    /// and this window comes back if the selection cannot apply.
    pub fn apply(&mut self, selected: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        // Catalog order, so that the same selection always reads the same.
        let selected: Vec<String> = PATCHES
            .iter()
            .filter(|patch| selected.iter().any(|name| name == *patch))
            .map(|patch| (*patch).to_owned())
            .collect();
        self.panel
            .update(cx, |panel, cx| panel.apply_patches(selected, cx));
        window.remove_window();
    }
}

impl Render for PatchesWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = self.panel.read(cx);
        let model = panel.model;
        let can_start = panel.options.qemu.is_some();
        let active = panel.options.selected_patches(model);
        // Follow a change of firmware or model while the window is open.
        if self.firmware != panel.options.firmware_dir {
            self.firmware = panel.options.firmware_dir.clone();
            self.available = l6max_host::firmware_patches::available(&panel.options, model);
            self.selected = active.clone();
        }
        let available = self.available.as_deref().unwrap_or(&[]).to_vec();
        let changed = PATCHES.iter().any(|patch| {
            self.selected.iter().any(|name| name == patch)
                != active.iter().any(|name| name == patch)
        });
        let enabled = changed && can_start;
        let patch_error = panel.patch_error.clone();
        let rows = OFFERED.iter().map(|&patch| {
            let usable = available.contains(&patch);
            let checked = self.selected.iter().any(|name| name == patch);
            // A saved patch can always be turned off, so that the guest can start again.
            let togglable = usable || checked;
            div()
                .id(patch)
                .flex()
                .items_center()
                .gap_3()
                .when(togglable, |row| {
                    row.cursor(CursorStyle::PointingHand).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |window, _, _, cx| {
                            window.toggle(patch);
                            cx.notify();
                        }),
                    )
                })
                .child(
                    div()
                        .size(px(16.0))
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(0x34405c))
                        .bg(rgb(if checked { 0x5b8def } else { 0x171d2c })),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_color(rgb(if togglable { 0xe8eef8 } else { 0x6a7388 }))
                                .child(patch),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x8b95aa))
                                .child(description(patch)),
                        ),
                )
        });
        let message = patch_error.or_else(|| self.available.as_ref().err().cloned());
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0x10131c))
            .text_color(rgb(0xe8eef8))
            .children(rows)
            .children(
                message.map(|message| div().text_sm().text_color(rgb(0xf0a0a0)).child(message)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8b95aa))
                    .child("Applying restarts the emulator. Settings are kept."),
            )
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("apply")
                        .rounded_md()
                        .px_3()
                        .py_1()
                        .border_1()
                        .border_color(rgb(0x34405c))
                        .bg(rgb(if enabled { 0x2a3550 } else { 0x171d2c }))
                        .text_color(rgb(if enabled { 0xe8eef8 } else { 0x6a7388 }))
                        .child("Apply and Restart")
                        .when(enabled, |button| {
                            button.cursor(CursorStyle::PointingHand).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.apply(this.selected.clone(), window, cx)
                                }),
                            )
                        }),
                ),
            )
    }
}
