//! Physical controls derived from main key table 0x800c4a0c and GPIO readers.
#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub id: u32,
    pub name: &'static str,
    pub switch: bool,
}
pub const INPUT_COUNT: usize = 54;
/// Physical ADC channels in UI order: SOUND PAD, EFX RTN, MASTER, MONITOR, SUB-OUT.
/// L6 has no SUB-OUT knob. See docs/emulator/controls.md.
pub const fn analog_channels(model: l6fw::Model) -> [u32; 5] {
    match model {
        l6fw::Model::L6 => [5, 6, 4, 3, 7],
        l6fw::Model::L6max => [3, 4, 5, 6, 7],
    }
}
pub const ANALOG_COUNTS_PER_STEP: i64 = 32;
pub const CONTROLS: &[Control] = &[
    Control {
        id: 0,
        name: "recorder-function-1",
        switch: false,
    },
    Control {
        id: 1,
        name: "recorder-function-2",
        switch: false,
    },
    Control {
        id: 2,
        name: "recorder-function-3",
        switch: false,
    },
    Control {
        id: 3,
        name: "recorder-function-4",
        switch: false,
    },
    Control {
        id: 4,
        name: "rec-play-stop",
        switch: false,
    },
    Control {
        id: 5,
        name: "rec-record",
        switch: false,
    },
    Control {
        id: 6,
        name: "rec-bounce",
        switch: false,
    },
    Control {
        id: 7,
        name: "input1-hi-z",
        switch: false,
    },
    Control {
        id: 12,
        name: "input2-hi-z",
        switch: false,
    },
    Control {
        id: 17,
        name: "input1-48v",
        switch: false,
    },
    Control {
        id: 22,
        name: "ch1-mute",
        switch: false,
    },
    Control {
        id: 27,
        name: "ch2-mute",
        switch: false,
    },
    Control {
        id: 32,
        name: "input3-48v",
        switch: false,
    },
    Control {
        id: 37,
        name: "ch3-mute",
        switch: false,
    },
    Control {
        id: 42,
        name: "ch4-mute",
        switch: false,
    },
    Control {
        id: 8,
        name: "ch5-pad-switch",
        switch: true,
    },
    Control {
        id: 13,
        name: "ch6-pad-switch",
        switch: true,
    },
    Control {
        id: 18,
        name: "ch7-pad-switch",
        switch: true,
    },
    Control {
        id: 23,
        name: "ch8-pad-switch",
        switch: true,
    },
    Control {
        id: 28,
        name: "mode-eq-high",
        switch: false,
    },
    Control {
        id: 33,
        name: "mode-freq",
        switch: false,
    },
    Control {
        id: 38,
        name: "mode-mid",
        switch: false,
    },
    Control {
        id: 43,
        name: "mode-low",
        switch: false,
    },
    Control {
        id: 9,
        name: "ch5-mode",
        switch: false,
    },
    Control {
        id: 14,
        name: "ch6-mode",
        switch: false,
    },
    Control {
        id: 19,
        name: "ch7-mode",
        switch: false,
    },
    Control {
        id: 24,
        name: "ch8-mode",
        switch: false,
    },
    Control {
        id: 29,
        name: "ch5-mute",
        switch: false,
    },
    Control {
        id: 34,
        name: "ch6-mute",
        switch: false,
    },
    Control {
        id: 39,
        name: "ch7-mute",
        switch: false,
    },
    Control {
        id: 44,
        name: "ch8-mute",
        switch: false,
    },
    Control {
        id: 10,
        name: "mode-aux1",
        switch: false,
    },
    Control {
        id: 15,
        name: "mode-aux2",
        switch: false,
    },
    Control {
        id: 20,
        name: "mode-efx",
        switch: false,
    },
    Control {
        id: 25,
        name: "mode-sub-mix",
        switch: false,
    },
    Control {
        id: 30,
        name: "mode-pan",
        switch: false,
    },
    Control {
        id: 35,
        name: "mode-level",
        switch: false,
    },
    Control {
        id: 40,
        name: "efx-select",
        switch: false,
    },
    Control {
        id: 45,
        name: "compressor",
        switch: false,
    },
    Control {
        id: 11,
        name: "scene-a",
        switch: false,
    },
    Control {
        id: 16,
        name: "scene-b",
        switch: false,
    },
    Control {
        id: 21,
        name: "scene-c",
        switch: false,
    },
    Control {
        id: 26,
        name: "scene-d",
        switch: false,
    },
    Control {
        id: 47,
        name: "master-submix-switch",
        switch: true,
    },
    Control {
        id: 48,
        name: "tap-button",
        switch: false,
    },
    Control {
        id: 49,
        name: "sound-pad-1",
        switch: false,
    },
    Control {
        id: 50,
        name: "sound-pad-2",
        switch: false,
    },
    Control {
        id: 51,
        name: "sound-pad-3",
        switch: false,
    },
    Control {
        id: 52,
        name: "sound-pad-4",
        switch: false,
    },
    Control {
        id: 53,
        name: "power",
        switch: false,
    },
];

/// Stable semantic control names shared by command-line control and scenarios.
pub fn control_target(model: l6fw::Model, control: &str) -> Result<u32, String> {
    if let Some(channel) = control
        .strip_prefix("channel.")
        .and_then(|s| s.strip_suffix(".mute"))
        .and_then(|s| s.parse::<usize>().ok())
    {
        let ids: &[u32] = if model == l6fw::Model::L6 {
            &[10, 13, 16, 19, 22, 25]
        } else {
            &[22, 27, 37, 42, 29, 34, 39, 44]
        };
        return channel
            .checked_sub(1)
            .and_then(|i| ids.get(i))
            .copied()
            .ok_or_else(|| format!("invalid control {control}"));
    }
    if let Some(mode) = control.strip_prefix("mode.") {
        let index = mode_index(model, mode)?;
        return Ok(if model == l6fw::Model::L6 {
            [8, 11, 14, 17, 20, 23, 26, 29, 28][index]
        } else {
            [28, 33, 38, 43, 10, 15, 20, 25, 30, 35][index]
        });
    }
    if model == l6fw::Model::L6max {
        if let Some(control) = CONTROLS.iter().find(|entry| entry.name == control) {
            return Ok(control.id);
        }
    }
    match (model, control) {
        (l6fw::Model::L6, "scene-a") => Ok(15),
        (l6fw::Model::L6, "scene-b") => Ok(18),
        (l6fw::Model::L6, "scene-c") => Ok(21),
        (l6fw::Model::L6, "phantom-48v") => Ok(7),
        (l6fw::Model::L6, "ch3-mono") => Ok(2),
        (l6fw::Model::L6, "play") => Ok(4),
        (l6fw::Model::L6, "rec-record" | "record") => Ok(24),
        (l6fw::Model::L6, "power") => Ok(53),
        (l6fw::Model::L6, "tap") => Ok(48),
        (l6fw::Model::L6, "sound-pad-1") => Ok(49),
        (l6fw::Model::L6, "sound-pad-2") => Ok(50),
        (l6fw::Model::L6, "sound-pad-3") => Ok(51),
        (l6fw::Model::L6, "sound-pad-4") => Ok(52),
        (l6fw::Model::L6, "ch5-usb-1-2") => Ok(6),
        (l6fw::Model::L6, "ch6-usb-3-4") => Ok(47),
        (l6fw::Model::L6, "efx-type") => Ok(9),
        (l6fw::Model::L6, "comp-master") => Ok(12),
        (l6fw::Model::L6, "ch4-mono") => Ok(3),
        (l6fw::Model::L6, "mode.level") => Ok(28),
        (l6fw::Model::L6max, "mode.level") => Ok(35),
        (l6fw::Model::L6max, "recorder.up") => Ok(1),
        (l6fw::Model::L6max, "recorder.menu") => Ok(0),
        (l6fw::Model::L6max, "recorder.down") => Ok(2),
        (l6fw::Model::L6max, "recorder.confirm") => Ok(3),
        _ => Err(format!(
            "unsupported {} control {control}",
            match model {
                l6fw::Model::L6 => "L6",
                l6fw::Model::L6max => "L6max",
            }
        )),
    }
}

pub fn mode_index(model: l6fw::Model, mode: &str) -> Result<usize, String> {
    let names: &[&str] = if model == l6fw::Model::L6 {
        &[
            "eq-high", "freq", "mid", "low", "aux1", "aux2", "efx", "pan", "level",
        ]
    } else {
        &[
            "eq-high", "freq", "mid", "low", "aux1", "aux2", "efx", "sub-mix", "pan", "level",
        ]
    };
    names
        .iter()
        .position(|name| *name == mode)
        .ok_or_else(|| format!("unsupported mode {mode}"))
}
