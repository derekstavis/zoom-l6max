//! Strict scenario schema, composition and semantic model mappings.
use crate::trace;
use l6fw::Model;
pub use l6max_host::controls::{control_target, mode_index};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
pub fn model_name(model: Model) -> &'static str {
    match model {
        Model::L6 => "l6",
        Model::L6max => "l6max",
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub version: u32,
    pub name: String,
    pub models: Vec<String>,
    pub fixture: String,
    pub main_sha256: Option<String>,
    #[serde(default)]
    pub main_sha256_any: Vec<String>,
    #[serde(default = "default_backends")]
    pub backends: Vec<String>,
    #[serde(default)]
    pub trace_sd: bool,
    pub step: Vec<Step>,
    /// The case state was built from a state spec: setup input is dropped
    /// and boot readiness does not wait out first-boot setup screens.
    #[serde(skip)]
    pub spec_state: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub models: Vec<String>,
    /// UI input that only brings the device to the scenario's starting
    /// state. Skipped when the case state was built from a state spec.
    #[serde(default)]
    pub setup_input: bool,
    #[serde(default = "guest_timeout")]
    pub guest_timeout_ms: u32,
    #[serde(default = "wall_timeout")]
    pub wall_timeout_s: u64,
    #[serde(flatten)]
    pub action: Action,
}
fn default_backends() -> Vec<String> {
    vec!["headless".into(), "gui".into()]
}
fn default_midi_cable() -> u8 {
    1
}
fn default_true() -> bool {
    true
}
fn ring_settle() -> u32 {
    1000
}
fn default_playback_window_ms() -> u32 {
    6000
}

fn default_max_residual_db() -> f64 {
    -60.
}
fn guest_timeout() -> u32 {
    10_000
}
fn wall_timeout() -> u64 {
    180
}
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Gesture {
    #[default]
    Scroll,
    DragVertical,
    DragHorizontal,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PadAssignmentSource {
    #[default]
    Editor,
    SavedState,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbSysexRequest {
    pub bytes: Vec<u8>,
    pub expect_prefix: Option<Vec<u8>>,
    #[serde(default)]
    pub reject_prefixes: Vec<Vec<u8>>,
    #[serde(default)]
    pub expect_reply: bool,
    pub min_reply_bytes: Option<usize>,
    pub reply_timeout_ms: Option<u32>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    Include {
        path: PathBuf,
    },
    Checkpoint {
        name: String,
    },
    ExpectDisplay {
        checkpoint: String,
        changed: bool,
    },
    ExpectTrace {
        contains: Vec<String>,
        #[serde(default)]
        absent: Vec<String>,
    },
    ExpectTraceCount {
        pattern: String,
        count: u64,
        #[serde(default)]
        at_least: bool,
        checkpoint: Option<String>,
    },
    ExpectButtonReport {
        control: String,
    },
    ExpectStartupReport {
        control: String,
    },
    SdCard {
        inserted: bool,
    },
    UsbDescriptors,
    UsbStorage,
    UsbMidi {
        value: u8,
    },
    HostMidiRoundTrip,
    DirectUsbMidiRoundTrip {
        #[serde(default = "default_midi_cable")]
        cable: u8,
        #[serde(default = "default_true")]
        expect_ring_change: bool,
    },
    L6maxDinMixerControl {
        expect_din_ring_change: bool,
    },
    L6DinUsbForwarding,
    HostMidiSync,
    L6MidiSync,
    L6MidiRingPinSweep {
        cc: u8,
    },
    L6MidiMuteIndicators,
    UsbSysex {
        requests: Vec<UsbSysexRequest>,
    },
    EditorPadAssignments,
    EditorPadPlayback {
        pad: u8,
    },
    L6maxUiPadAssignment {
        pad: u8,
    },
    SoundPadPlayback {
        pad: u8,
        target: u32,
        #[serde(default)]
        assignment: PadAssignmentSource,
        #[serde(default = "default_true")]
        require_pad_led_change: bool,
        #[serde(default = "default_playback_window_ms")]
        playback_window_ms: u32,
        #[serde(default)]
        require_stereo_meter: bool,
        #[serde(default)]
        expect_source: Option<String>,
        #[serde(default = "default_max_residual_db")]
        max_residual_db: f64,
    },
    L6RecorderPlayback {
        target: u32,
    },
    L6maxRecorderPlayback {
        target: u32,
    },
    ExpectMemory {
        address: u32,
        bytes: Vec<u8>,
    },
    HoldChord {
        controls: Vec<String>,
        duration_ms: u32,
    },
    AudioCallbacks,
    KnobDialog,
    PopupConcurrency,
    FirmwareUpdate,
    WasmPatch,
    Restart,
    ExpectRecorder,
    PowerPress {
        hold_ms: u32,
        powered: bool,
    },
    ExpectPowered {
        powered: bool,
    },
    ExpectSettings {
        restored: bool,
    },
    Wait {
        until: String,
    },
    Delay {
        guest_ms: u32,
    },
    Press {
        control: String,
    },
    Hold {
        control: String,
        duration_ms: u32,
    },
    SaveScene {
        scene: usize,
    },
    ExpectScene {
        scene: usize,
        lit: bool,
        checkpoint: Option<String>,
        #[serde(default)]
        changed: bool,
    },
    ExpectButtonLed {
        control: String,
        lit: Option<bool>,
        checkpoint: Option<String>,
        #[serde(default)]
        changed: bool,
    },
    ExpectEffect {
        effect: String,
    },
    Switch {
        control: String,
        enabled: bool,
    },
    Encoder {
        channel: usize,
        steps: i32,
        #[serde(default)]
        gesture: Gesture,
    },
    Analog {
        channel: usize,
        steps: i32,
        #[serde(default)]
        gesture: Gesture,
    },
    ExpectAnalog {
        channel: usize,
        value: u16,
    },
    PanelEvent {
        target: u32,
        column: u8,
        row: u8,
        event: u8,
    },
    L6MainGpioEvent {
        target: u32,
        event: u8,
    },
    L6maxTapHandler,
    L6UsbStartup,
    L6DinEventDispatch {
        group: u8,
        cc: u8,
        value: u8,
    },
    L6InputSweep,
    L6DinSweep,
    L6maxInputTiming,
    L6maxPanelKeyMap,
    L6maxChannelSignalPwm,
    L6LevelMeterPwm,
    ExpectEncoderReports,
    ExpectRing {
        channel: usize,
        mask: Option<u32>,
        #[serde(default)]
        changed: bool,
        #[serde(default)]
        only: bool,
        checkpoint: Option<String>,
        #[serde(default = "ring_settle")]
        settle_ms: u32,
    },
    /// The brightest segment of a ring must read this raw PWM level.
    ExpectRingLevel {
        channel: usize,
        min: u8,
        max: u8,
    },
    ExpectMute {
        channel: usize,
    },
    SelectMode {
        mode: String,
    },
    ExpectMode {
        mode: String,
    },
}

impl Scenario {
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), String> {
        let value = self;
        if value.version != 1 || value.name.is_empty() || value.step.is_empty() {
            return Err("scenario needs version=1, a name and nonempty steps".into());
        }
        if value.main_sha256.as_ref().is_some_and(|hash| {
            hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err("main_sha256 must be a 64-digit hexadecimal SHA-256".into());
        }
        if value
            .main_sha256_any
            .iter()
            .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(
                "main_sha256_any entries must be 64-digit hexadecimal SHA-256 values".into(),
            );
        }
        validate_models(&value.models)?;
        if value.backends.is_empty() || value.backends.iter().any(|b| b != "headless" && b != "gui")
        {
            return Err("backends must contain headless and/or gui".into());
        }
        let mut checkpoints = std::collections::BTreeSet::new();
        let mut usb_descriptors_seen = false;
        let mut pad_assignments_seen = false;
        for step in &value.step {
            if matches!(step.action, Action::EditorPadAssignments) && !usb_descriptors_seen {
                return Err(
                    "editor-pad-assignments requires a preceding usb-descriptors step".into(),
                );
            }
            if matches!(step.action, Action::UsbDescriptors) {
                usb_descriptors_seen = true;
            }
            match &step.action {
                Action::EditorPadAssignments => pad_assignments_seen = true,
                Action::L6maxUiPadAssignment { pad } => {
                    if !usb_descriptors_seen {
                        return Err(
                            "l6max-ui-pad-assignment requires a preceding usb-descriptors step"
                                .into(),
                        );
                    }
                    if *pad > 3 {
                        return Err("l6max-ui-pad-assignment pad must be 0..3".into());
                    }
                    if value.models.as_slice() != ["l6max"]
                        || step.models.iter().any(|model| model != "l6max")
                    {
                        return Err(
                            "l6max-ui-pad-assignment scenarios must be scoped to L6max".into()
                        );
                    }
                }
                Action::EditorPadPlayback { pad } => {
                    if !pad_assignments_seen {
                        return Err(
                            "editor-pad-playback requires a preceding editor-pad-assignments step"
                                .into(),
                        );
                    }
                    if *pad > 3 {
                        return Err("editor-pad-playback pad must be 0..3".into());
                    }
                    if value.models.as_slice() != ["l6"]
                        || step.models.iter().any(|model| model != "l6")
                    {
                        return Err("editor-pad-playback scenarios must be scoped to L6".into());
                    }
                }
                Action::SoundPadPlayback {
                    pad,
                    target,
                    assignment,
                    playback_window_ms,
                    ..
                } => {
                    if *assignment == PadAssignmentSource::Editor && !pad_assignments_seen {
                        return Err(
                            "sound-pad-playback requires a preceding editor-pad-assignments step"
                                .into(),
                        );
                    }
                    if *pad > 3 || *target != 49 + u32::from(*pad) {
                        return Err(
                            "sound-pad-playback target must match pad (pad 0..3 maps to target 49..52)".into(),
                        );
                    }
                    if !(1000..=120_000).contains(playback_window_ms) {
                        return Err(
                            "sound-pad-playback playback_window_ms must be 1000..120000".into()
                        );
                    }
                }
                Action::L6RecorderPlayback { target } => {
                    if *target != 4 {
                        return Err("l6-recorder-playback requires MAIN target 4".into());
                    }
                    if value.models.as_slice() != ["l6"]
                        || step.models.iter().any(|model| model != "l6")
                    {
                        return Err("l6-recorder-playback scenarios must be scoped to L6".into());
                    }
                }
                _ => {}
            }
            if !step.models.is_empty() {
                validate_models(&step.models)?;
            }
            if step.wall_timeout_s == 0 || step.guest_timeout_ms == 0 {
                return Err("timeouts must be positive".into());
            }
            if step.models.iter().any(|m| !value.models.contains(m)) {
                return Err("step model outside scenario scope".into());
            }
            if let Action::ExpectRing {
                mask, settle_ms, ..
            } = &step.action
            {
                if mask.is_some_and(|v| v > 0x1ff)
                    || *settle_ms == 0
                    || *settle_ms > step.guest_timeout_ms
                {
                    return Err("invalid ring mask or settling interval".into());
                }
            }
            if let Action::ExpectMode { mode } | Action::SelectMode { mode } = &step.action {
                for model in &value.models {
                    if step.models.is_empty() || step.models.contains(model) {
                        mode_index(
                            if model == "l6" {
                                Model::L6
                            } else {
                                Model::L6max
                            },
                            mode,
                        )?;
                    }
                }
            }
            if matches!(
                step.action,
                Action::ExpectRecorder | Action::ExpectSettings { .. }
            ) && value
                .models
                .iter()
                .any(|m| m == "l6" && (step.models.is_empty() || step.models.contains(m)))
            {
                return Err("startup/settings layout currently supports L6max only".into());
            }
            if let Action::PowerPress { hold_ms, .. } = &step.action {
                if *hold_ms == 0 || *hold_ms > step.guest_timeout_ms {
                    return Err("power hold must fit the guest deadline".into());
                }
            }
            if let Action::Analog { channel, steps, .. } = &step.action {
                if !(1..=5).contains(channel) || *steps == 0 {
                    return Err("analog action requires channel 1..5 and nonzero steps".into());
                }
            }
            if let Action::ExpectAnalog { channel, value } = &step.action {
                if !(1..=5).contains(channel) || *value > 1023 {
                    return Err("analog assertion requires channel 1..5 and value 0..1023".into());
                }
            }
            if let Action::PanelEvent {
                target,
                column,
                row,
                event,
            } = &step.action
            {
                if !value.models.iter().all(|m| m == "l6")
                    || *target >= 54
                    || *column > 3
                    || *row > 7
                    || *event > 31
                {
                    return Err("panel-event requires L6 scope and valid target/map values".into());
                }
            }
            if let Action::L6DinEventDispatch { group, .. } = &step.action {
                if value.models.iter().any(|m| m != "l6")
                    || step.models.iter().any(|m| m != "l6")
                    || *group > 15
                {
                    return Err("l6-din-event-dispatch requires L6 and a group from 0 to 15".into());
                }
            }
            if matches!(step.action, Action::L6maxChannelSignalPwm)
                && (value.models.iter().any(|model| model != "l6max")
                    || step.models.iter().any(|model| model != "l6max")
                    || value.main_sha256.as_deref()
                        != Some("980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461"))
            {
                return Err(
                    "l6max-channel-signal-pwm requires L6max MAIN 1.10 and its exact firmware hash"
                        .into(),
                );
            }
            if matches!(step.action, Action::L6LevelMeterPwm)
                && (value.models.iter().any(|model| model != "l6")
                    || step.models.iter().any(|model| model != "l6")
                    || value.main_sha256.as_deref()
                        != Some("827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799"))
            {
                return Err(
                    "l6-level-meter-pwm requires L6 MAIN 1.10 and its exact firmware hash".into(),
                );
            }
            if let Action::L6maxRecorderPlayback { target } = step.action {
                if value.models.iter().any(|model| model != "l6max")
                    || step.models.iter().any(|model| model != "l6max")
                    || target != 4
                    || value.main_sha256.as_deref()
                        != Some("980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461")
                {
                    return Err(
                        "l6max-recorder-playback requires L6max MAIN 1.10, MAIN target 4, and its exact firmware hash".into(),
                    );
                }
            }
            if matches!(step.action, Action::L6DinUsbForwarding)
                && (value.models.iter().any(|m| m != "l6") || step.models.iter().any(|m| m != "l6"))
            {
                return Err("l6-din-usb-forwarding requires an L6-only scenario".into());
            }
            if matches!(step.action, Action::L6InputSweep)
                && (value.models.iter().any(|m| m != "l6") || step.models.iter().any(|m| m != "l6"))
            {
                return Err("l6-input-sweep requires L6 scope".into());
            }
            if matches!(step.action, Action::L6DinSweep)
                && (value.models.iter().any(|m| m != "l6")
                    || step.models.iter().any(|m| m != "l6")
                    || value.main_sha256.as_deref()
                        != Some("827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799"))
            {
                return Err(
                    "l6-din-sweep requires L6 1.10 and its exact MAIN firmware hash".into(),
                );
            }
            if matches!(step.action, Action::L6maxInputTiming)
                && (value.models.iter().any(|m| m != "l6max")
                    || step.models.iter().any(|m| m != "l6max"))
            {
                return Err("l6max-input-timing requires L6max scope".into());
            }
            if matches!(step.action, Action::L6maxPanelKeyMap)
                && (value.models.iter().any(|m| m != "l6max")
                    || step.models.iter().any(|m| m != "l6max"))
            {
                return Err("l6max-panel-key-map requires L6max scope".into());
            }
            if let Action::L6MainGpioEvent { target, event } = step.action
                && (value.models.iter().any(|m| m != "l6")
                    || step.models.iter().any(|m| m != "l6")
                    || !matches!(
                        (target, event),
                        (48, 0x19) | (49, 0x15) | (50, 0x16) | (51, 0x17) | (52, 0x18)
                    ))
            {
                return Err(
                    "l6-main-gpio-event requires L6 target/event pairs 48/0x19 or 49–52/0x15–0x18"
                        .into(),
                );
            }
            if matches!(step.action, Action::L6maxTapHandler)
                && (value.models.iter().any(|m| m != "l6max")
                    || step.models.iter().any(|m| m != "l6max"))
            {
                return Err("l6max-tap-handler requires L6max scope".into());
            }
            if matches!(step.action, Action::L6UsbStartup)
                && (value.models.iter().any(|m| m != "l6")
                    || step.models.iter().any(|m| m != "l6")
                    || !matches!(
                        value.step.first().map(|first| &first.action),
                        Some(Action::L6UsbStartup)
                    ))
            {
                return Err("l6-usb-startup must be the first action and requires L6 scope".into());
            }
            if matches!(step.action, Action::L6UsbStartup) && value.main_sha256.is_none() {
                return Err("l6-usb-startup requires an exact MAIN firmware hash guard".into());
            }
            if matches!(step.action, Action::KnobDialog)
                && (value.models.iter().any(|m| m != "l6max")
                    || step.models.iter().any(|m| m != "l6max"))
            {
                return Err("knob-dialog requires L6max scope".into());
            }
            if matches!(step.action, Action::PopupConcurrency)
                && (value.models.iter().any(|m| m != "l6max")
                    || step.models.iter().any(|m| m != "l6max"))
            {
                return Err("popup-concurrency requires L6max scope".into());
            }
            if matches!(step.action, Action::FirmwareUpdate)
                && (value.models.is_empty()
                    || value.step.len() != 1
                    || step
                        .models
                        .iter()
                        .any(|m| !["l6", "l6max"].contains(&m.as_str())))
            {
                return Err("firmware-update is a standalone L6/L6max scenario operation".into());
            }
            if matches!(step.action, Action::WasmPatch)
                && (value.models.is_empty()
                    || value.step.len() != 1
                    || step
                        .models
                        .iter()
                        .any(|m| !["l6", "l6max"].contains(&m.as_str())))
            {
                return Err("wasm-patch is a standalone L6/L6max scenario operation".into());
            }
            if let Action::UsbSysex { requests } = &step.action {
                if requests.is_empty() {
                    return Err("usb-sysex needs at least one request".into());
                }
                for request in requests {
                    if request.bytes.len() < 2
                        || request.bytes.first() != Some(&0xf0)
                        || request.bytes.last() != Some(&0xf7)
                    {
                        return Err("usb-sysex bytes must be a complete F0..F7 message".into());
                    }
                    if request.expect_prefix.as_ref().is_some_and(Vec::is_empty) {
                        return Err("usb-sysex expect_prefix must not be empty".into());
                    }
                    if request.reject_prefixes.iter().any(Vec::is_empty) {
                        return Err(
                            "usb-sysex reject_prefixes must not contain empty prefixes".into()
                        );
                    }
                    if request
                        .reply_timeout_ms
                        .is_some_and(|timeout| timeout == 0 || timeout > step.guest_timeout_ms)
                    {
                        return Err("usb-sysex reply_timeout_ms must be positive and fit the step guest timeout".into());
                    }
                }
            }
            if let Action::ExpectMemory { address, bytes } = &step.action {
                if bytes.is_empty()
                    || bytes.len() > 4096
                    || address.checked_add(bytes.len() as u32).is_none()
                {
                    return Err("expect-memory needs 1..4096 bytes in a valid address range".into());
                }
                if value.main_sha256.is_none() && value.main_sha256_any.is_empty() {
                    return Err(
                        "expect-memory requires a scenario main_sha256 firmware guard".into(),
                    );
                }
            }
            if let Action::HoldChord {
                controls,
                duration_ms,
            } = &step.action
            {
                if controls.is_empty()
                    || controls.len() > 16
                    || *duration_ms == 0
                    || *duration_ms > step.guest_timeout_ms
                {
                    return Err(
                        "hold-chord requires 1..16 controls and a duration within its deadline"
                            .into(),
                    );
                }
                for model in &value.models {
                    if step.models.is_empty() || step.models.contains(model) {
                        let model = if model == "l6" {
                            Model::L6
                        } else {
                            Model::L6max
                        };
                        let mut targets = std::collections::BTreeSet::new();
                        for control in controls {
                            let target = control_target(model, control)?;
                            if !targets.insert(target) {
                                return Err("hold-chord controls must be unique".into());
                            }
                        }
                    }
                }
            }
            match &step.action {
                Action::Checkpoint { name } => {
                    if name.is_empty()
                        || name.len() > 64
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                        || !checkpoints.insert(name.clone())
                    {
                        return Err("checkpoint needs a unique nonempty name".into());
                    }
                }
                Action::ExpectDisplay { checkpoint, .. }
                | Action::ExpectScene {
                    checkpoint: Some(checkpoint),
                    ..
                }
                | Action::ExpectButtonLed {
                    checkpoint: Some(checkpoint),
                    ..
                }
                | Action::ExpectTraceCount {
                    checkpoint: Some(checkpoint),
                    ..
                }
                | Action::ExpectRing {
                    checkpoint: Some(checkpoint),
                    ..
                } => {
                    if !checkpoints.contains(checkpoint) {
                        return Err(format!("unknown checkpoint {checkpoint}"));
                    }
                }
                Action::SdCard { .. }
                | Action::UsbDescriptors
                | Action::UsbStorage
                | Action::UsbMidi { .. }
                | Action::HostMidiRoundTrip
                | Action::DirectUsbMidiRoundTrip { .. }
                | Action::L6maxDinMixerControl { .. }
                | Action::L6DinUsbForwarding
                | Action::L6maxRecorderPlayback { .. }
                | Action::HostMidiSync
                | Action::L6MidiSync
                | Action::L6MidiRingPinSweep { .. }
                | Action::L6MidiMuteIndicators
                | Action::UsbSysex { .. }
                | Action::EditorPadAssignments
                | Action::EditorPadPlayback { .. }
                | Action::L6maxUiPadAssignment { .. }
                | Action::SoundPadPlayback { .. }
                | Action::ExpectMemory { .. }
                | Action::HoldChord { .. }
                | Action::AudioCallbacks
                | Action::KnobDialog
                | Action::PopupConcurrency
                | Action::FirmwareUpdate
                | Action::Restart
                | Action::ExpectRecorder
                | Action::PowerPress { .. }
                | Action::ExpectSettings { .. }
                | Action::PanelEvent { .. }
                | Action::L6MainGpioEvent { .. }
                | Action::L6maxTapHandler
                | Action::L6UsbStartup
                | Action::L6DinEventDispatch { .. }
                | Action::L6InputSweep
                | Action::L6DinSweep
                | Action::L6maxInputTiming
                | Action::L6maxPanelKeyMap
                | Action::L6maxChannelSignalPwm
                | Action::L6LevelMeterPwm
                    if value.backends.iter().any(|b| b != "headless") =>
                {
                    return Err("device operations require backends=[\"headless\"]".into());
                }
                Action::ExpectTrace { contains, absent }
                    if contains.is_empty() && absent.is_empty() =>
                {
                    return Err("trace assertion must contain a condition".into());
                }
                Action::ExpectTraceCount { pattern, .. } if pattern.is_empty() => {
                    return Err("trace count assertion requires a pattern".into());
                }
                Action::ExpectEffect { effect }
                    if !["hall", "room", "spring", "delay", "echo"].contains(&effect.as_str()) =>
                {
                    return Err("effect must be hall, room, spring, delay or echo".into());
                }
                Action::Hold { duration_ms, .. } if *duration_ms == 0 || *duration_ms > 59_000 => {
                    return Err("hold duration must be between 1 and 59000 ms".into());
                }
                Action::SaveScene { scene } if !(1..=3).contains(scene) => {
                    return Err("scene index must be between 1 and 3".into());
                }
                Action::SaveScene { .. }
                    if (value.models.iter().any(|m| m == "l6max")
                        && (step.models.is_empty()
                            || step.models.iter().any(|m| m == "l6max"))) =>
                {
                    return Err("save-scene currently supports L6 only".into());
                }
                Action::ExpectScene { scene, .. } if !(1..=3).contains(scene) => {
                    return Err("scene index must be between 1 and 3".into());
                }
                Action::ExpectScene { .. }
                    if (value.models.iter().any(|m| m == "l6max")
                        && (step.models.is_empty()
                            || step.models.iter().any(|m| m == "l6max"))) =>
                {
                    return Err("scene lamp assertions currently support L6 only".into());
                }
                Action::ExpectButtonLed { control, .. } => {
                    for model in &value.models {
                        if step.models.is_empty() || step.models.contains(model) {
                            l6_button_led_index(
                                if model == "l6" {
                                    Model::L6
                                } else {
                                    Model::L6max
                                },
                                control,
                            )?;
                        }
                    }
                }
                Action::ExpectEffect { .. }
                    if value.models.iter().any(|m| m == "l6max")
                        && (step.models.is_empty() || step.models.iter().any(|m| m == "l6max")) =>
                {
                    return Err("effect assertions currently support L6 only".into());
                }
                Action::ExpectMode { mode } if mode_index(Model::L6max, mode).is_err() => {
                    return Err("unsupported mode".into());
                }
                Action::Wait { until }
                    if until != "boot-ready"
                        && until != "panel-settled"
                        && until != "mixer-ready" =>
                {
                    return Err(format!("unknown wait condition: {until}"));
                }
                Action::Press { control }
                | Action::Hold { control, .. }
                | Action::Switch { control, .. }
                | Action::ExpectButtonReport { control }
                | Action::ExpectStartupReport { control } => {
                    for model in &value.models {
                        if step.models.is_empty() || step.models.contains(model) {
                            let target = control_target(
                                if model == "l6" {
                                    Model::L6
                                } else {
                                    Model::L6max
                                },
                                control,
                            )?;
                            if matches!(step.action, Action::Switch { .. })
                                && !is_switch(
                                    if model == "l6" {
                                        Model::L6
                                    } else {
                                        Model::L6max
                                    },
                                    target,
                                )
                            {
                                return Err("switch action requires a latching switch".into());
                            }
                            if matches!(step.action, Action::Press { .. } | Action::Hold { .. })
                                && is_switch(
                                    if model == "l6" {
                                        Model::L6
                                    } else {
                                        Model::L6max
                                    },
                                    target,
                                )
                            {
                                return Err("use switch action for a latching switch".into());
                            }
                            if matches!(
                                step.action,
                                Action::ExpectButtonReport { .. }
                                    | Action::ExpectStartupReport { .. }
                            ) {
                                trace::button_packets(
                                    if model == "l6" {
                                        Model::L6
                                    } else {
                                        Model::L6max
                                    },
                                    control,
                                )?;
                            }
                        }
                    }
                }
                Action::Encoder { channel, steps, .. }
                    if *steps == 0 || steps.unsigned_abs() > 120 || *channel == 0 =>
                {
                    return Err(
                        "encoder channel must be positive, steps must be nonzero and at most 120"
                            .into(),
                    );
                }
                Action::ExpectRingLevel { channel, min, max } if *channel == 0 || min > max => {
                    return Err("ring level assertion needs a channel and min <= max".into());
                }
                Action::ExpectRing {
                    channel,
                    mask,
                    changed,
                    checkpoint,
                    ..
                } if *channel == 0
                    || mask.is_some_and(|v| v > 0x1ff)
                    || (mask.is_none() && !changed && checkpoint.is_none()) =>
                {
                    return Err(
                        "ring assertion needs a valid channel and mask or changed=true".into(),
                    );
                }
                _ => {}
            }
            if let Action::Encoder { channel, .. }
            | Action::ExpectRingLevel { channel, .. }
            | Action::ExpectRing { channel, .. }
            | Action::ExpectMute { channel } = &step.action
            {
                for model in &value.models {
                    if (step.models.is_empty() || step.models.contains(model))
                        && !(1..=if model == "l6" { 6 } else { 8 }).contains(channel)
                    {
                        return Err(format!("channel {channel} is not available on {model}"));
                    }
                }
            }
        }
        for step in &value.step {
            if let Action::DirectUsbMidiRoundTrip { cable, .. } = &step.action
                && *cable > 15
            {
                return Err("direct-usb-midi-round-trip cable must be 0..15".into());
            }
            if matches!(step.action, Action::L6maxDinMixerControl { .. })
                && (value.models.iter().any(|model| model != "l6max")
                    || step.models.iter().any(|model| model != "l6max"))
            {
                return Err("l6max-din-mixer-control requires L6max scope".into());
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
        let root = path.parent().unwrap();
        let mut value: Self =
            toml::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        value.step = expand_steps(value.step, root, root, &mut vec![path.clone()])?;
        value.validate()?;
        Ok(value)
    }
    /// Load a scenario for a case process. When the driver built the case
    /// state from a state spec it sets [`STATE_SPEC_ENV`], and setup-input
    /// steps are dropped because the state already holds their result.
    pub fn load_for_case(path: &Path) -> Result<Self, String> {
        let scenario = Self::load(path)?;
        Ok(if std::env::var(STATE_SPEC_ENV).as_deref() == Ok("1") {
            scenario.without_setup_input()
        } else {
            scenario
        })
    }
    pub fn without_setup_input(mut self) -> Self {
        self.step.retain(|step| !step.setup_input);
        self.spec_state = true;
        self
    }
    /// Native host MIDI endpoints have fixed per-model names, so two guests
    /// publishing them at once would connect to each other's ports.
    pub fn uses_native_midi(&self) -> bool {
        self.step.iter().any(|step| {
            matches!(
                step.action,
                Action::HostMidiRoundTrip
                    | Action::HostMidiSync
                    | Action::L6DinSweep
                    | Action::L6DinUsbForwarding
                    | Action::L6maxDinMixerControl { .. }
            )
        })
    }
}
/// Environment variable the driver sets to `1` for cases whose persistent
/// state was built from a state spec.
pub const STATE_SPEC_ENV: &str = "SMOKE_STATE_SPEC";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fragment {
    step: Vec<Step>,
}
fn expand_steps(
    steps: Vec<Step>,
    base: &Path,
    root: &Path,
    stack: &mut Vec<PathBuf>,
) -> Result<Vec<Step>, String> {
    let mut expanded = vec![];
    for step in steps {
        if let Action::Include { path } = &step.action {
            if stack.len() >= 8 {
                return Err("scenario include depth exceeds eight".into());
            }
            let path = fs::canonicalize(base.join(path)).map_err(|e| e.to_string())?;
            if !path.starts_with(root) || stack.contains(&path) {
                return Err("include escapes scenario directory or creates a cycle".into());
            }
            let fragment: Fragment =
                toml::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            stack.push(path.clone());
            let mut nested = expand_steps(fragment.step, path.parent().unwrap(), root, stack)?;
            stack.pop();
            for child in &mut nested {
                child.setup_input |= step.setup_input;
                if !step.models.is_empty() {
                    if child.models.is_empty() {
                        child.models = step.models.clone();
                    } else {
                        child.models.retain(|m| step.models.contains(m));
                        if child.models.is_empty() {
                            return Err("include and child model scopes do not intersect".into());
                        }
                    }
                }
            }
            expanded.extend(nested);
        } else {
            expanded.push(step);
        }
    }
    Ok(expanded)
}
fn validate_models(models: &[String]) -> Result<(), String> {
    if models.is_empty() || models.iter().any(|m| m != "l6" && m != "l6max") {
        return Err("models must contain l6 and/or l6max".into());
    }
    Ok(())
}

pub fn is_switch(model: Model, target: u32) -> bool {
    model == Model::L6max && matches!(target, 8 | 13 | 18 | 23 | 47)
}

pub fn l6_button_led_index(model: Model, control: &str) -> Result<usize, String> {
    if model != Model::L6 {
        return Err("button LED assertion mapping currently supports L6 only".into());
    }
    let target = control_target(model, control)?;
    match target {
        10 => Ok(0),
        13 => Ok(1),
        16 => Ok(2),
        19 => Ok(3),
        22 => Ok(4),
        25 => Ok(5),
        2 => Ok(6),
        3 => Ok(7),
        6 => Ok(8),
        47 => Ok(9),
        12 => Ok(10),
        7 => Ok(11),
        _ => Err(format!("{control} has no L6 button LED")),
    }
}
