//! Physical controls derived from main key table 0x800c4a0c and GPIO readers.
#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub id: u32,
    pub name: &'static str,
    pub switch: bool,
}
pub const INPUT_COUNT: usize = 54;
/// Physical ADC channels in UI order, following VR table 0x800c18cc.
pub const ANALOG_CHANNELS: [u32; 5] = [5, 6, 4, 3, 7];
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
