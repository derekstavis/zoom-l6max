//! Render each Patches window state without an emulator, so a layout panic
//! shows up without a display. Rendering needs the platform's main thread,
//! so this runs the application binary.
#![cfg(feature = "headless-screenshots")]

use std::process::Command;

#[test]
fn patches_window_renders_its_states() {
    let directory = std::env::temp_dir().join(format!("l6-patches-render-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for (name, args) in [
        (
            "sd",
            &[
                "l6max",
                "--sd-card",
                "knob-dialog,midi-sync",
                "--firmware",
                "din-mixer-control",
            ][..],
        ),
        (
            "no-card",
            &["l6max", "--no-card", "--active-sd-card", "midi-sync"][..],
        ),
        ("l6", &["l6", "--active-firmware", "midi-sync"][..]),
        (
            "error",
            &[
                "l6max",
                "--sd-card",
                "midi-sync",
                "--error",
                "SD card patches need an SD card",
            ][..],
        ),
    ] {
        let output = directory.join(format!("{name}.png"));
        let status = Command::new(env!("CARGO_BIN_EXE_l6max-gui"))
            .arg("--render-patches")
            .args(args)
            .arg("--output")
            .arg(&output)
            .status()
            .unwrap();
        assert!(status.success(), "{name}: {status}");
        assert!(std::fs::metadata(&output).unwrap().len() > 0, "{name}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}
