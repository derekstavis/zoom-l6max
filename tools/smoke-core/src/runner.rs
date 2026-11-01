//! Backend-independent scenario state machine and assertions.
use crate::{
    Action, Command, Scenario, control_target, devices, model_name,
    scenario::mode_index,
    trace::{self, TraceCursor},
};
use l6fw::Model;
use l6max_host::{
    indicators::{l6_button_leds, l6_mode_leds, leds, rings},
    input::{InputHub, InputStatus},
    panel::PanelData,
    readiness::BootReadiness,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[derive(Clone)]
struct Observation {
    data: PanelData,
    main: InputStatus,
    panel: InputStatus,
    reports: u64,
    traces: BTreeMap<String, u64>,
}
#[derive(Clone)]
struct SceneSaveState {
    scene: usize,
    target: u32,
    before: [bool; 3],
    press_pattern: String,
    release_pattern: String,
    press_count: u64,
    release_count: u64,
}
impl Observation {
    fn summary(&self) -> Value {
        json!({"guest_ms": self.panel.guest_ms, "rings": rings(self.data.indicators), "rows": self.data.indicators,
            "main": {"sent":self.main.sent,"applied":self.main.applied,"released":self.main.released,"frames":self.main.frame_ready,"frame_guest_ms":self.main.frame_guest_ms},
            "panel": {"sent":self.panel.sent,"applied":self.panel.applied,"released":self.panel.released}, "encoder_reports":self.reports,"trace_counts":self.traces})
    }
}
#[derive(Debug)]
pub enum Progress {
    Waiting,
    Dispatch(Command),
    Device(devices::Operation),
    Complete,
}
pub struct Runner {
    pub scenario: Scenario,
    model: Model,
    logs: PathBuf,
    step: usize,
    entered: Option<(Instant, u32)>,
    dispatched: bool,
    baseline: Option<Observation>,
    last_command: Option<Command>,
    startup: BootReadiness,
    trace: TraceCursor,
    trace_path: PathBuf,
    history: VecDeque<Value>,
    completed: Vec<Value>,
    last: Option<Observation>,
    started: Instant,
    last_progress: Instant,
    panel_quiet: Option<([u32; 8], u32)>,
    ring_quiet: Option<([u32; 8], u32)>,
    separate_clocks: bool,
    last_report_guest: u32,
    checkpoints: BTreeMap<String, Observation>,
    scene_save: Option<SceneSaveState>,
    allowed_rejections: [u64; 2],
}
impl Runner {
    pub fn new(scenario: Scenario, model: Model, logs: PathBuf) -> Result<Self, String> {
        if !scenario.models.iter().any(|m| m == model_name(model)) {
            return Err("scenario does not support this model".into());
        }
        if scenario
            .step
            .iter()
            .any(|s| matches!(s.action, Action::Include { .. }))
        {
            return Err("load includes with Scenario::load before running".into());
        }
        let mut trace = TraceCursor::default();
        for step in &scenario.step {
            if !step.models.is_empty() && !step.models.iter().any(|m| m == model_name(model)) {
                continue;
            }
            match &step.action {
                Action::ExpectTrace { contains, absent } => {
                    for pattern in contains.iter().chain(absent) {
                        trace.watch(pattern)?;
                    }
                }
                Action::ExpectTraceCount { pattern, .. } => trace.watch(pattern)?,
                Action::ExpectButtonReport { control } => {
                    for pattern in trace::button_packets(model, control)? {
                        trace.watch(&pattern)?;
                    }
                }
                Action::ExpectStartupReport { control } => {
                    for pattern in trace::button_packets(model, control)? {
                        trace.watch(&pattern)?;
                    }
                }
                Action::SaveScene { scene } => {
                    for pattern in trace::button_packets(
                        model,
                        &format!("scene-{}", ['a', 'b', 'c'][scene - 1]),
                    )? {
                        trace.watch(&pattern)?;
                    }
                }
                _ => {}
            }
        }
        fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
        let no_setup_screens = scenario.spec_state;
        Ok(Self {
            scenario,
            model,
            trace_path: logs.join("engine.trace"),
            logs,
            step: 0,
            entered: None,
            dispatched: false,
            baseline: None,
            last_command: None,
            startup: BootReadiness::new(no_setup_screens),
            trace,
            history: Default::default(),
            completed: vec![],
            last: None,
            started: Instant::now(),
            last_progress: Instant::now(),
            panel_quiet: None,
            ring_quiet: None,
            separate_clocks: false,
            last_report_guest: 0,
            checkpoints: Default::default(),
            scene_save: None,
            allowed_rejections: [0, 0],
        })
    }
    pub fn tick(&mut self, data: PanelData, inputs: &InputHub) -> Result<Progress, String> {
        self.tick_observation(data, inputs.main.snapshot(), inputs.panel.snapshot())
    }
    pub fn tick_without_input(&mut self, data: PanelData) -> Result<Progress, String> {
        let guest_ms = self.last.as_ref().map_or(0, |last| last.panel.guest_ms);
        let status = l6max_host::input::InputStatus {
            connected: true,
            powered_off: !data.powered,
            guest_ms,
            ..Default::default()
        };
        self.tick_observation(data, status.clone(), status)
    }
    fn tick_observation(
        &mut self,
        data: PanelData,
        main: InputStatus,
        panel: InputStatus,
    ) -> Result<Progress, String> {
        let result = self.trace.sample(&self.trace_path).and_then(|reports| {
            self.advance(
                Observation {
                    data,
                    main,
                    panel,
                    reports,
                    traces: self.trace.counts.clone(),
                },
                Instant::now(),
            )
        });
        if let Err(error) = &result {
            self.finish("FAIL", Some(error))?;
        }
        result
    }
    fn advance(&mut self, obs: Observation, now: Instant) -> Result<Progress, String> {
        let mut visible = rings(obs.data.indicators);
        if self.model == Model::L6 {
            visible[6..].fill(0);
        }
        if self.ring_quiet.is_none_or(|(old, _)| old != visible) {
            self.ring_quiet = Some((visible, obs.panel.guest_ms));
        }
        if self
            .last
            .as_ref()
            .is_some_and(|last| last.reports != obs.reports)
        {
            self.last_report_guest = obs.panel.guest_ms;
        }
        let quiet_rows = settling_rows(self.model, obs.data.indicators);
        if self.panel_quiet.is_none_or(|(rows, _)| rows != quiet_rows) {
            self.panel_quiet = Some((quiet_rows, obs.panel.guest_ms));
        }
        let ready = self
            .startup
            .observe(self.model, obs.panel.guest_ms, obs.data.indicators);
        if self.last.as_ref().is_none_or(|last| {
            obs.panel.guest_ms.wrapping_sub(last.panel.guest_ms) >= 1000
                || obs.data.indicators != last.data.indicators
                || obs.panel.applied != last.panel.applied
                || obs.main.applied != last.main.applied
        }) {
            self.record(json!({"event":"observation","step":self.step+1,"state":obs.summary()}));
        }
        if now.saturating_duration_since(self.last_progress) >= Duration::from_secs(15) {
            tracing::info!(event="scenario_progress",scenario=%self.scenario.name,model=model_name(self.model),step=self.step+1,guest_ms=obs.panel.guest_ms,rings=?rings(obs.data.indicators),sweep_seen=self.startup.all_lit_seen,"scenario running");
            self.last_progress = now;
        }
        self.last = Some(obs.clone());
        let powered_off_action = self.scenario.step[self.step..]
            .iter()
            .find(|step| {
                step.models.is_empty() || step.models.iter().any(|m| m == model_name(self.model))
            })
            .is_some_and(|step| match &step.action {
                Action::ExpectPowered { powered: false } => true,
                Action::Press { control } if control == "power" => true,
                Action::Hold { control, .. } if control == "power" => true,
                Action::Press { control } if self.model == Model::L6 && control == "rec-record" => {
                    true
                }
                _ => false,
            });
        if !obs.main.connected
            || !obs.panel.connected
            || obs.main.rejected != self.allowed_rejections[0]
            || obs.panel.rejected != self.allowed_rejections[1]
            || (!obs.data.powered
                && !powered_off_action
                && !matches!(
                    self.scenario.step[self.step..]
                        .iter()
                        .find(|s| s.models.is_empty()
                            || s.models.iter().any(|m| m == model_name(self.model)))
                        .map(|s| &s.action),
                    Some(Action::Restart)
                ))
        {
            return Err(format!(
                "device input/power failed: main={:?}, panel={:?}",
                obs.main, obs.panel
            ));
        }
        while self.step < self.scenario.step.len() {
            let step = self.scenario.step[self.step].clone();
            if !step.models.is_empty() && !step.models.iter().any(|m| m == model_name(self.model)) {
                self.step += 1;
                continue;
            }
            let (entered, guest_start) = *self.entered.get_or_insert_with(|| {
                tracing::info!(event="step_started",scenario=%self.scenario.name,model=model_name(self.model),step=self.step+1,action=?step.action,guest_ms=obs.panel.guest_ms,"step started");
                (now, obs.panel.guest_ms)
            });
            let elapsed = obs.panel.guest_ms.wrapping_sub(guest_start);
            if now.duration_since(entered) > Duration::from_secs(step.wall_timeout_s)
                || elapsed > step.guest_timeout_ms
            {
                return Err(format!(
                    "step {} timed out: {:?}; observed {}; startup={:?}",
                    self.step + 1,
                    step.action,
                    obs.summary(),
                    self.startup
                ));
            }
            let done = match step.action {
                Action::Include { .. } => return Err("unresolved include".into()),
                Action::Checkpoint { name } => {
                    save_display(&obs.data, &self.logs, &name)?;
                    self.record(json!({
                        "event": "checkpoint",
                        "step": self.step + 1,
                        "name": name,
                        "trace_counts": &obs.traces,
                    }));
                    self.checkpoints.insert(name, obs.clone());
                    true
                }
                Action::ExpectDisplay {
                    checkpoint,
                    changed,
                } => {
                    if self.model == Model::L6 {
                        return Err("L6 has no display".into());
                    }
                    let base = self
                        .checkpoints
                        .get(&checkpoint)
                        .ok_or("checkpoint not executed for this model")?;
                    (base.data.display != obs.data.display) == changed
                }
                Action::ExpectTrace { contains, absent } => {
                    if absent
                        .iter()
                        .any(|p| obs.traces.get(p).copied().unwrap_or(0) > 0)
                    {
                        return Err("forbidden trace observed".into());
                    }
                    contains
                        .iter()
                        .all(|p| obs.traces.get(p).copied().unwrap_or(0) > 0)
                }
                Action::ExpectTraceCount {
                    pattern,
                    count,
                    at_least,
                    checkpoint,
                } => {
                    let current = obs.traces.get(&pattern).copied().unwrap_or(0);
                    let baseline = checkpoint
                        .as_ref()
                        .and_then(|name| self.checkpoints.get(name))
                        .and_then(|observation| observation.traces.get(&pattern))
                        .copied()
                        .unwrap_or(0);
                    let observed = current.saturating_sub(baseline);
                    if at_least {
                        observed >= count
                    } else if observed > count {
                        return Err(format!(
                            "trace {pattern:?} occurred {observed} times, expected exactly {count}"
                        ));
                    } else {
                        observed == count
                    }
                }
                Action::ExpectButtonReport { control } => {
                    if !matches!(self.last_command, Some(Command::Press { target } | Command::Switch { target, .. }) if target == control_target(self.model, &control)?)
                    {
                        return Err(
                            "button report assertion needs the matching preceding press".into()
                        );
                    }
                    let base = self.baseline.as_ref().unwrap();
                    let mut complete = true;
                    for (index, pattern) in trace::button_packets(self.model, &control)?
                        .into_iter()
                        .enumerate()
                    {
                        let delta = obs
                            .traces
                            .get(&pattern)
                            .copied()
                            .unwrap_or(0)
                            .saturating_sub(base.traces.get(&pattern).copied().unwrap_or(0));
                        let expected = match self.last_command {
                            Some(Command::Switch { enabled, .. }) => {
                                u64::from(enabled == (index == 0))
                            }
                            _ => 1,
                        };
                        if delta > expected {
                            return Err(format!(
                                "duplicate or unexpected button report for {control}"
                            ));
                        }
                        complete &= delta == expected;
                    }
                    complete && elapsed >= 1000
                }
                Action::ExpectStartupReport { control } => {
                    let press = &trace::button_packets(self.model, &control)?[0];
                    obs.traces.get(press).copied().unwrap_or(0) > 0
                }
                Action::SdCard { inserted } => {
                    return Ok(Progress::Device(devices::Operation::SdCard { inserted }));
                }
                Action::UsbDescriptors => {
                    return Ok(Progress::Device(devices::Operation::UsbDescriptors));
                }
                Action::UsbMidi { value } => {
                    return Ok(Progress::Device(devices::Operation::UsbMidi { value }));
                }
                Action::HostMidiRoundTrip => {
                    return Ok(Progress::Device(devices::Operation::HostMidiRoundTrip));
                }
                Action::DirectUsbMidiRoundTrip {
                    cable,
                    expect_ring_change,
                } => {
                    return Ok(Progress::Device(
                        devices::Operation::DirectUsbMidiRoundTrip {
                            cable,
                            expect_ring_change,
                        },
                    ));
                }
                Action::L6maxDinMixerControl {
                    expect_din_ring_change,
                } => {
                    return Ok(Progress::Device(devices::Operation::L6maxDinMixerControl {
                        expect_din_ring_change,
                    }));
                }
                Action::L6DinUsbForwarding => {
                    return Ok(Progress::Device(devices::Operation::L6DinUsbForwarding));
                }
                Action::HostMidiSync => {
                    return Ok(Progress::Device(devices::Operation::HostMidiSync));
                }
                Action::L6MidiSync => {
                    return Ok(Progress::Device(devices::Operation::L6MidiSync));
                }
                Action::L6MidiRingPinSweep { cc } => {
                    return Ok(Progress::Device(devices::Operation::L6MidiRingPinSweep {
                        cc,
                    }));
                }
                Action::L6MidiMuteIndicators => {
                    return Ok(Progress::Device(devices::Operation::L6MidiMuteIndicators));
                }
                Action::UsbSysex { requests } => {
                    return Ok(Progress::Device(devices::Operation::UsbSysex { requests }));
                }
                Action::EditorPadAssignments => {
                    return Ok(Progress::Device(devices::Operation::EditorPadAssignments));
                }
                Action::EditorPadPlayback { pad } => {
                    return Ok(Progress::Device(devices::Operation::EditorPadPlayback {
                        pad,
                    }));
                }
                Action::L6maxUiPadAssignment { pad } => {
                    return Ok(Progress::Device(devices::Operation::L6maxUiPadAssignment {
                        pad,
                    }));
                }
                Action::SoundPadPlayback {
                    pad,
                    target,
                    assignment,
                    require_pad_led_change,
                    playback_window_ms,
                    require_stereo_meter,
                    expect_source,
                    max_residual_db,
                } => {
                    return Ok(Progress::Device(devices::Operation::SoundPadPlayback {
                        pad,
                        target,
                        assignment,
                        require_pad_led_change,
                        playback_window_ms,
                        require_stereo_meter,
                        expect_source,
                        max_residual_db,
                    }));
                }
                Action::L6RecorderPlayback { target } => {
                    return Ok(Progress::Device(devices::Operation::L6RecorderPlayback {
                        target,
                    }));
                }
                Action::L6maxRecorderPlayback { target } => {
                    return Ok(Progress::Device(
                        devices::Operation::L6maxRecorderPlayback { target },
                    ));
                }
                Action::ExpectMemory { address, bytes } => {
                    return Ok(Progress::Device(devices::Operation::ExpectMemory {
                        address,
                        bytes,
                    }));
                }
                Action::HoldChord {
                    controls,
                    duration_ms,
                } => {
                    let targets = controls
                        .iter()
                        .map(|control| control_target(self.model, control))
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(Progress::Device(devices::Operation::HoldChord {
                        targets,
                        duration_ms,
                    }));
                }
                Action::UsbStorage => return Ok(Progress::Device(devices::Operation::UsbStorage)),
                Action::ExpectRecorder => {
                    return Ok(Progress::Device(devices::Operation::ExpectRecorder));
                }
                Action::Restart => return Ok(Progress::Device(devices::Operation::Restart)),
                Action::PowerPress { hold_ms, powered } => {
                    return Ok(Progress::Device(devices::Operation::PowerPress {
                        hold_ms,
                        powered,
                    }));
                }
                Action::ExpectPowered { powered } => obs.data.powered == powered,
                Action::ExpectSettings { restored } => {
                    return Ok(Progress::Device(devices::Operation::ExpectSettings {
                        restored,
                    }));
                }
                Action::AudioCallbacks => {
                    return Ok(Progress::Device(devices::Operation::AudioCallbacks));
                }
                Action::KnobDialog => {
                    return Ok(Progress::Device(devices::Operation::KnobDialog));
                }
                Action::PopupConcurrency => {
                    return Ok(Progress::Device(devices::Operation::PopupConcurrency));
                }
                Action::FirmwareUpdate => {
                    return Ok(Progress::Device(devices::Operation::FirmwareUpdate));
                }
                Action::WasmPatch => {
                    return Ok(Progress::Device(devices::Operation::WasmPatch));
                }
                Action::PanelEvent {
                    target,
                    column,
                    row,
                    event,
                } => {
                    return Ok(Progress::Device(devices::Operation::PanelEvent {
                        target,
                        column,
                        row,
                        event,
                    }));
                }
                Action::L6MainGpioEvent { target, event } => {
                    return Ok(Progress::Device(devices::Operation::L6MainGpioEvent {
                        target,
                        event,
                    }));
                }
                Action::L6maxTapHandler => {
                    return Ok(Progress::Device(devices::Operation::L6maxTapHandler));
                }
                Action::L6UsbStartup => {
                    return Ok(Progress::Device(devices::Operation::L6UsbStartup));
                }
                Action::L6DinEventDispatch { group, cc, value } => {
                    return Ok(Progress::Device(devices::Operation::L6DinEventDispatch {
                        group,
                        cc,
                        value,
                    }));
                }
                Action::L6InputSweep => {
                    return Ok(Progress::Device(devices::Operation::L6InputSweep));
                }
                Action::L6DinSweep => {
                    return Ok(Progress::Device(devices::Operation::L6DinSweep));
                }
                Action::L6maxInputTiming => {
                    return Ok(Progress::Device(devices::Operation::L6maxInputTiming));
                }
                Action::L6maxPanelKeyMap => {
                    return Ok(Progress::Device(devices::Operation::L6maxPanelKeyMap));
                }
                Action::L6maxChannelSignalPwm => {
                    return Ok(Progress::Device(devices::Operation::L6maxChannelSignalPwm));
                }
                Action::L6LevelMeterPwm => {
                    return Ok(Progress::Device(devices::Operation::L6LevelMeterPwm));
                }
                Action::Wait { until } => {
                    if until == "boot-ready" {
                        // Without the setup floor, L6max also needs its screen
                        // to have held one frame for a guest second.
                        let display_settled = !self.scenario.spec_state
                            || obs.panel.guest_ms.wrapping_sub(obs.main.frame_guest_ms) >= 1000;
                        ready
                            && (self.model == Model::L6
                                || (obs.data.display.iter().any(|b| *b != 0) && display_settled))
                    } else if until == "mixer-ready" {
                        // MAIN restores every ring at once after the sweep;
                        // L6 then lights its editing-mode lamp.
                        rings(obs.data.indicators)[..if self.model == Model::L6 { 6 } else { 8 }]
                            .iter()
                            .all(|mask| *mask != 0)
                            && (self.model != Model::L6
                                || l6_mode_leds(obs.data.indicators).contains(&true))
                    } else {
                        self.panel_quiet.is_some_and(|(_, since)| {
                            obs.panel.guest_ms.wrapping_sub(since) >= 1000
                        })
                    }
                }
                Action::SelectMode { mode } => {
                    let index = mode_index(self.model, &mode)?;
                    let lit = if self.model == Model::L6 {
                        l6_mode_leds(obs.data.indicators)[index]
                    } else {
                        leds(obs.data.indicators)[17 + index]
                    };
                    if !self.dispatched && !lit {
                        let settled = self.panel_quiet.is_some_and(|(_, since)| {
                            obs.panel.guest_ms.wrapping_sub(since) >= 1000
                        });
                        if !settled {
                            return Ok(Progress::Waiting);
                        }
                        return self.dispatch(
                            Command::Press {
                                target: control_target(self.model, &format!("mode.{mode}"))?,
                            },
                            obs,
                        );
                    }
                    lit && (!self.dispatched || self.acknowledged(&obs)?)
                }
                Action::ExpectMode { mode } => {
                    let index = mode_index(self.model, &mode)?;
                    if self.model == Model::L6 {
                        l6_mode_leds(obs.data.indicators)[index]
                    } else {
                        leds(obs.data.indicators)[17 + index]
                    }
                }
                Action::Delay { guest_ms } => elapsed >= guest_ms,
                Action::Switch { control, enabled } => {
                    if !self.dispatched {
                        return self.dispatch(
                            Command::Switch {
                                target: control_target(self.model, &control)?,
                                enabled,
                            },
                            obs,
                        );
                    }
                    self.acknowledged(&obs)?
                        && obs.data.switches[control_target(self.model, &control)? as usize]
                            == enabled
                }
                Action::Press { control } => {
                    if !self.dispatched {
                        return self.dispatch(
                            Command::Press {
                                target: control_target(self.model, &control)?,
                            },
                            obs,
                        );
                    }
                    let target = control_target(self.model, &control)?;
                    if control == "power"
                        && self
                            .baseline
                            .as_ref()
                            .is_some_and(|baseline| baseline.data.powered != obs.data.powered)
                    {
                        true
                    } else if !obs.data.powered && self.model == Model::L6 && target != 53 {
                        obs.data.switches[target as usize]
                    } else {
                        self.acknowledged(&obs)?
                    }
                }
                Action::Hold {
                    control,
                    duration_ms,
                } => {
                    let target = control_target(self.model, &control)?;
                    if !self.dispatched {
                        return self.dispatch(
                            Command::HoldDown {
                                target,
                                duration_ms: duration_ms + 1000,
                            },
                            obs,
                        );
                    }
                    if control == "power" && !obs.data.powered {
                        if elapsed < duration_ms {
                            return Err("power shut down before the required hold duration".into());
                        }
                        true
                    } else {
                        match self.last_command.as_ref() {
                            Some(Command::HoldDown { .. }) if elapsed < duration_ms => false,
                            Some(Command::HoldDown { .. }) => {
                                return self.dispatch(Command::Release { target }, obs);
                            }
                            Some(Command::Release { .. }) => self.acknowledged(&obs)?,
                            _ => return Err("hold action lost its press state".into()),
                        }
                    }
                }
                Action::SaveScene { scene } => {
                    if self.model != Model::L6 {
                        return Err("save-scene currently supports L6 only".into());
                    }
                    let control = format!("scene-{}", ['a', 'b', 'c'][scene - 1]);
                    let target = control_target(self.model, &control)?;
                    if !self.dispatched {
                        let [press_pattern, release_pattern] =
                            trace::button_packets(self.model, &control)?;
                        let state = SceneSaveState {
                            scene,
                            target,
                            before: l6max_host::indicators::l6_scene_leds(obs.data.indicators),
                            press_count: obs.traces.get(&press_pattern).copied().unwrap_or(0),
                            release_count: obs.traces.get(&release_pattern).copied().unwrap_or(0),
                            press_pattern,
                            release_pattern,
                        };
                        self.scene_save = Some(state);
                        return self.dispatch(
                            Command::HoldDown {
                                target,
                                // Keep the panel key down until the scene lamp responds.
                                // This is the same 50 ms minimum used by a normal click.
                                duration_ms: 50,
                            },
                            obs,
                        );
                    }
                    let state = self
                        .scene_save
                        .as_ref()
                        .ok_or("save-scene lost its press baseline")?
                        .clone();
                    if state.scene != scene || state.target != target {
                        return Err("save-scene state does not match the current step".into());
                    }
                    match self.last_command.as_ref() {
                        Some(Command::HoldDown { target: held, .. }) if *held == target => {
                            let press_delta = obs
                                .traces
                                .get(&state.press_pattern)
                                .copied()
                                .unwrap_or(0)
                                .saturating_sub(state.press_count);
                            if press_delta > 1 {
                                return Err(format!("duplicate L6 scene {scene} press report"));
                            }
                            let mut expected = state.before;
                            expected[scene - 1] = true;
                            if self.acknowledged(&obs)?
                                && press_delta == 1
                                && l6max_host::indicators::l6_scene_leds(obs.data.indicators)
                                    == expected
                            {
                                return self.dispatch(Command::Release { target }, obs);
                            }
                            false
                        }
                        Some(Command::Release { target: released }) if *released == target => {
                            let release_delta = obs
                                .traces
                                .get(&state.release_pattern)
                                .copied()
                                .unwrap_or(0)
                                .saturating_sub(state.release_count);
                            if release_delta > 1 {
                                return Err(format!("duplicate L6 scene {scene} release report"));
                            }
                            self.acknowledged(&obs)? && release_delta == 1
                        }
                        _ => return Err("save-scene lost its input state".into()),
                    }
                }
                Action::ExpectScene {
                    scene,
                    lit,
                    checkpoint,
                    changed,
                } => {
                    if self.model != Model::L6 {
                        return Err("scene lamp assertions currently support L6 only".into());
                    }
                    let current = l6max_host::indicators::l6_scene_leds(obs.data.indicators);
                    let previous = checkpoint
                        .as_ref()
                        .map(|name| {
                            self.checkpoints
                                .get(name)
                                .ok_or("checkpoint not executed for this model")
                                .map(|state| {
                                    l6max_host::indicators::l6_scene_leds(state.data.indicators)
                                })
                        })
                        .transpose()?;
                    current[scene - 1] == lit
                        && (!changed
                            || previous
                                .is_some_and(|previous| current[scene - 1] != previous[scene - 1]))
                }
                Action::ExpectButtonLed {
                    control,
                    lit,
                    checkpoint,
                    changed,
                } => {
                    if self.model != Model::L6 {
                        return Err(
                            "button LED assertion mapping currently supports L6 only".into()
                        );
                    }
                    let index = crate::scenario::l6_button_led_index(self.model, &control)?;
                    let current = l6_button_leds(obs.data.indicators);
                    let previous = checkpoint
                        .as_ref()
                        .map(|name| {
                            self.checkpoints
                                .get(name)
                                .ok_or("checkpoint not executed for this model")
                                .map(|state| l6_button_leds(state.data.indicators))
                        })
                        .transpose()?;
                    lit.is_none_or(|lit| current[index] == lit)
                        && (!changed
                            || previous.is_some_and(|previous| current[index] != previous[index]))
                }
                Action::ExpectEffect { effect } => {
                    if self.model != Model::L6 {
                        return Err("effect assertions currently support L6 only".into());
                    }
                    let effects = ["hall", "room", "spring", "delay", "echo"];
                    let index = effects
                        .iter()
                        .position(|value| *value == effect)
                        .ok_or("unknown effect")?;
                    l6max_host::indicators::l6_effect_leds(obs.data.indicators)
                        == std::array::from_fn(|i| i == index)
                }
                Action::Encoder {
                    channel,
                    steps,
                    gesture,
                } => {
                    if !self.dispatched {
                        return self.dispatch(
                            Command::Encoder {
                                channel,
                                steps,
                                gesture,
                            },
                            obs,
                        );
                    }
                    self.acknowledged(&obs)?
                }
                Action::Analog {
                    channel,
                    steps,
                    gesture,
                } => {
                    if !self.dispatched {
                        return self.dispatch(
                            Command::Analog {
                                channel,
                                steps,
                                gesture,
                            },
                            obs,
                        );
                    }
                    self.acknowledged(&obs)?
                }
                Action::ExpectAnalog { channel, value } => {
                    let Some(Command::Analog {
                        channel: previous, ..
                    }) = self.last_command.as_ref()
                    else {
                        return Err("analog assertion needs a preceding analog action".into());
                    };
                    if *previous != channel {
                        return Err("analog assertion channel differs from preceding action".into());
                    }
                    let adc_index = (l6max_host::controls::analog_channels(self.model)[channel - 1]
                        - 3) as usize;
                    obs.data.analog[channel - 1] == value
                        && obs.main.adc_samples[adc_index] == value
                }
                Action::ExpectEncoderReports => {
                    if !matches!(self.last_command, Some(Command::Encoder { .. })) {
                        return Err(
                            "encoder report assertion needs a preceding encoder action".into()
                        );
                    }
                    self.encoder_reports_ready(&obs)?
                        && obs.panel.guest_ms.wrapping_sub(self.last_report_guest) >= 1000
                }
                Action::ExpectRing {
                    channel,
                    mask,
                    changed,
                    only,
                    checkpoint,
                    settle_ms,
                } => {
                    let base = self
                        .baseline
                        .as_ref()
                        .ok_or("ring assertion needs a preceding action")?;
                    let before = rings(base.data.indicators);
                    let after = rings(obs.data.indicators);
                    let reports_ready = self.encoder_reports_ready(&obs)?;
                    if only
                        && after
                            .iter()
                            .take(if self.model == Model::L6 { 6 } else { 8 })
                            .zip(before)
                            .enumerate()
                            .any(|(i, (a, b))| i != channel - 1 && *a != b)
                    {
                        return Err("encoder changed an unrelated ring".into());
                    }
                    let settled = self.ring_quiet.is_some_and(|(_, since)| {
                        obs.panel.guest_ms.wrapping_sub(since) >= settle_ms
                    }) && obs.panel.guest_ms.wrapping_sub(self.last_report_guest)
                        >= settle_ms;
                    reports_ready
                        && settled
                        && checkpoint.as_ref().is_none_or(|name| {
                            // Firmware accelerates encoders by report spacing, which
                            // only repeats exactly when both MCUs share one clock.
                            if self.separate_clocks {
                                let moved = after[channel - 1].count_ones() as i64
                                    - before[channel - 1].count_ones() as i64;
                                return match self.last_command {
                                    Some(Command::Encoder { steps, .. }) => {
                                        moved.signum() == i64::from(steps.signum())
                                    }
                                    _ => moved == 0,
                                };
                            }
                            self.checkpoints.get(name).is_some_and(|o| {
                                rings(o.data.indicators)[channel - 1] == after[channel - 1]
                            })
                        })
                        && mask.is_none_or(|v| after[channel - 1] == v)
                        && (!changed || before[channel - 1] != after[channel - 1])
                }
                Action::ExpectRingLevel { channel, min, max } => {
                    let level = l6max_host::indicators::ring_levels(obs.data.indicator_levels)
                        [channel - 1]
                        .into_iter()
                        .max()
                        .unwrap_or(0);
                    (min..=max).contains(&level)
                }
                Action::ExpectMute { channel } => {
                    let base = self
                        .baseline
                        .as_ref()
                        .ok_or("mute assertion needs a preceding action")?;
                    let decode = |data: PanelData| {
                        if self.model == Model::L6 {
                            l6_button_leds(data.indicators)[channel - 1]
                        } else {
                            leds(data.indicators)[channel - 1]
                        }
                    };
                    decode(obs.data) != decode(base.data)
                        && self.panel_quiet.is_some_and(|(_, since)| {
                            obs.panel.guest_ms.wrapping_sub(since) >= 1000
                        })
                }
            };
            if !done {
                return Ok(Progress::Waiting);
            }
            self.record(
                json!({"event":"step-passed","step":self.step+1,"observation":obs.summary()}),
            );
            self.completed
                .push(json!({"step":self.step+1,"guest_ms":obs.panel.guest_ms}));
            self.step += 1;
            self.entered = None;
            self.dispatched = false;
            self.scene_save = None;
        }
        self.finish("PASS", None)?;
        Ok(Progress::Complete)
    }
    fn encoder_reports_ready(&self, obs: &Observation) -> Result<bool, String> {
        let Some(Command::Encoder { steps, .. }) = self.last_command else {
            return Ok(true);
        };
        let base = self
            .baseline
            .as_ref()
            .ok_or("encoder assertion needs an action baseline")?;
        let delta = obs.reports.saturating_sub(base.reports);
        if delta > u64::from(steps.unsigned_abs()) {
            return Err(format!("duplicate encoder reports: {delta}"));
        }
        Ok(delta == u64::from(steps.unsigned_abs()))
    }
    pub fn set_separate_clocks(&mut self, separate: bool) {
        self.separate_clocks = separate;
    }
    pub(crate) fn trace_file(&mut self, path: PathBuf) {
        self.trace_path = path;
    }
    pub(crate) fn restarted(&mut self, logs: &Path) {
        self.guest_restarted(logs);
        self.baseline = None;
        self.last_command = None;
    }
    pub fn guest_restarted(&mut self, logs: &Path) {
        self.trace.restart();
        self.trace_path = logs.join("engine.trace");
        self.startup = BootReadiness::new(self.scenario.spec_state);
        self.entered = None;
        self.panel_quiet = None;
        self.ring_quiet = None;
        self.last_report_guest = 0;
        self.last = None;
    }
    pub fn current_step(&self) -> usize {
        self.step
    }

    /// Complete a typed engine operation; its assertion evidence joins the same report.
    pub fn device_finished(&mut self, evidence: Value) {
        if let Some(expected) = evidence.get("expected_rejections") {
            if let (Some(main), Some(panel)) = (
                expected.get("main").and_then(Value::as_u64),
                expected.get("panel").and_then(Value::as_u64),
            ) {
                self.allowed_rejections = [main, panel];
            }
        }
        self.record(json!({"event":"device-result","step":self.step+1,"evidence":evidence}));
        self.completed
            .push(json!({"step":self.step+1,"device":true}));
        self.step += 1;
        self.entered = None;
        self.dispatched = false;
    }
    fn dispatch(&mut self, command: Command, obs: Observation) -> Result<Progress, String> {
        self.record(json!({"event":"dispatch","step":self.step+1,"command":format!("{command:?}"),"observation":obs.summary()}));
        self.baseline = Some(obs);
        self.last_command = Some(command.clone());
        self.dispatched = true;
        Ok(Progress::Dispatch(command))
    }
    fn acknowledged(&self, obs: &Observation) -> Result<bool, String> {
        let base = self.baseline.as_ref().unwrap();
        let (current, old, count) = match self.last_command.as_ref().unwrap() {
            Command::Encoder { .. } | Command::Switch { .. } => (&obs.panel, &base.panel, 1),
            Command::Analog { .. } => (&obs.main, &base.main, 1),
            Command::HoldDown { target, .. } | Command::Release { target } => {
                if matches!(target, 0 | 4 | 5 | 48..=53) {
                    (&obs.main, &base.main, 1)
                } else {
                    (&obs.panel, &base.panel, 1)
                }
            }
            Command::Press { target } if matches!(target, 0 | 4 | 5 | 48..=53) => {
                (&obs.main, &base.main, 2)
            }
            Command::Press { .. } => (&obs.panel, &base.panel, 2),
        };
        if current.sent > old.sent + count {
            return Err("unexpected concurrent input; leave the test window untouched".into());
        }
        // DOWN and explicit UP each return APPLIED; UP is acknowledged after
        // the minimum hold expires. RELEASED belongs to automatic TAP only.
        Ok(current.applied >= old.applied + count)
    }

    fn record(&mut self, event: Value) {
        tracing::debug!(event=event["event"].as_str().unwrap_or("observation"),scenario=%self.scenario.name,model=model_name(self.model),step=self.step+1,detail=%event,"scenario event");
        if self.history.len() == 128 {
            self.history.pop_front();
        }
        self.history.push_back(event);
    }
    pub fn finish(&self, outcome: &str, error: Option<&str>) -> Result<(), String> {
        if let Some(error) = error {
            tracing::error!(event="scenario_finished",scenario=%self.scenario.name,model=model_name(self.model),step=self.step+1,outcome,%error,"scenario failed");
        } else {
            tracing::info!(event="scenario_finished",scenario=%self.scenario.name,model=model_name(self.model),outcome,"scenario completed");
        }
        let report = json!({"schema":1,"scenario":self.scenario.name,"model":model_name(self.model),"outcome":outcome,
            "failed_step": if error.is_some(){Some(self.step+1)}else{None}, "error":error,"wall_seconds":self.started.elapsed().as_secs_f64(),
            "trace_file":self.trace_path,"completed_steps":self.completed,"last_observation":self.last.as_ref().map(Observation::summary),"events":self.history});
        fs::write(
            self.logs.join("scenario.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        if let Some(last) = &self.last {
            save_display(&last.data, &self.logs, "display")?;
        }
        Ok(())
    }
}

fn save_display(data: &PanelData, logs: &Path, name: &str) -> Result<(), String> {
    fs::write(logs.join(format!("{name}.txt")), data.display_ascii()).map_err(|e| e.to_string())?;
    image::GrayImage::from_fn(128, 64, |x, y| {
        let index = ((63 - y as usize) / 8) * 128 + x as usize;
        image::Luma(
            [if data.display[index] >> ((63 - y as usize) % 8) & 1 != 0 {
                255
            } else {
                0
            }],
        )
    })
    .save(logs.join(format!("{name}.png")))
    .map_err(|e| e.to_string())
}

/// L6max non-ring lamp index of TAP in `indicators::LED_PINS`.
const L6MAX_TAP_LED: usize = 33;
/// Indicator rows used to decide that panel output has settled. The L6max
/// TAP lamp flashes at the effect tempo (about every 500 guest ms at the
/// default 120 BPM) for as long as the device runs, so it never settles.
/// It is excluded; every other lamp and ring still has to hold still.
fn settling_rows(model: Model, mut rows: [u32; 8]) -> [u32; 8] {
    if model == Model::L6max {
        let (row, column) = l6max_host::indicators::LED_PINS[L6MAX_TAP_LED];
        rows[row] &= !(1 << (column ^ 7));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn sample(guest_ms: u32) -> Observation {
        Observation {
            data: PanelData {
                powered: true,
                ..Default::default()
            },
            main: InputStatus {
                connected: true,
                ..Default::default()
            },
            panel: InputStatus {
                connected: true,
                guest_ms,
                ..Default::default()
            },
            reports: 0,
            traces: BTreeMap::new(),
        }
    }
    fn test_runner(steps: &str) -> Runner {
        let scenario = Scenario::parse(&format!(
            "version=1\nname='unit'\nmodels=['l6']\nfixture='prepared'\n{steps}"
        ))
        .unwrap();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Runner::new(
            scenario,
            Model::L6,
            std::env::temp_dir().join(format!(
                "smoke-unit-{}-{stamp}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )),
        )
        .unwrap()
    }

    #[test]
    fn direct_usb_midi_cable_is_configurable_and_range_checked() {
        let default = Scenario::parse(
            "version=1\nname='midi'\nmodels=['l6max']\nfixture='prepared'\nbackends=['headless']\n[[step]]\naction='direct-usb-midi-round-trip'\n",
        )
        .unwrap();
        assert!(matches!(
            default.step[0].action,
            Action::DirectUsbMidiRoundTrip {
                cable: 1,
                expect_ring_change: true
            }
        ));

        let ignored_cable = Scenario::parse(
            "version=1\nname='midi'\nmodels=['l6max']\nfixture='prepared'\nbackends=['headless']\n[[step]]\naction='direct-usb-midi-round-trip'\ncable=5\nexpect_ring_change=false\n",
        )
        .unwrap();
        assert!(matches!(
            ignored_cable.step[0].action,
            Action::DirectUsbMidiRoundTrip {
                cable: 5,
                expect_ring_change: false
            }
        ));
        assert!(Scenario::parse(
            "version=1\nname='midi'\nmodels=['l6max']\nfixture='prepared'\nbackends=['headless']\n[[step]]\naction='direct-usb-midi-round-trip'\ncable=16\n",
        )
        .is_err());
    }
    #[test]
    fn output_alone_cannot_pass_before_the_command_acknowledgement() {
        let mut runner = test_runner(
            "[[step]]\naction='encoder'\nchannel=1\nsteps=12\n[[step]]\naction='expect-ring'\nchannel=1\nmask=31\nchanged=true\nonly=true\n",
        );
        let now = Instant::now();
        assert!(matches!(
            runner.advance(sample(0), now).unwrap(),
            Progress::Dispatch(_)
        ));
        let mut feedback = sample(100);
        for &(row, column) in l6max_host::indicators::RING_PINS[0].iter().take(5) {
            feedback.data.indicators[row] |= 1 << (column ^ 7);
        }
        feedback.reports = 12;
        feedback.panel.sent = 1;
        assert!(matches!(
            runner.advance(feedback.clone(), now).unwrap(),
            Progress::Waiting
        ));
        feedback.panel.applied = 1;
        assert!(matches!(
            runner.advance(feedback.clone(), now).unwrap(),
            Progress::Waiting
        ));
        feedback.panel.guest_ms = 1100;
        assert!(matches!(
            runner.advance(feedback, now).unwrap(),
            Progress::Complete
        ));
        assert!(runner.logs.join("scenario.json").is_file());
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn gui_power_scenario_can_cross_shutdown_and_verify_startup_key_reports() {
        let mut runner = test_runner(
            "[[step]]\naction='press'\ncontrol='power'\n[[step]]\naction='expect-powered'\npowered=false\n[[step]]\naction='press'\ncontrol='rec-record'\n[[step]]\naction='press'\ncontrol='power'\n[[step]]\naction='expect-startup-report'\ncontrol='rec-record'\n",
        );
        let mut data = PanelData {
            powered: true,
            ..Default::default()
        };
        assert!(matches!(
            runner.tick_without_input(data).unwrap(),
            Progress::Dispatch(Command::Press { target: 53 })
        ));

        data.powered = false;
        assert!(matches!(
            runner.tick_without_input(data).unwrap(),
            Progress::Dispatch(Command::Press { target: 24 })
        ));
        data.switches[24] = true;
        assert!(matches!(
            runner.tick_without_input(data).unwrap(),
            Progress::Dispatch(Command::Press { target: 53 })
        ));

        runner.guest_restarted(&runner.logs.clone());
        let [press, release] = trace::button_packets(Model::L6, "rec-record").unwrap();
        fs::write(
            runner.logs.join("engine.trace"),
            format!("{press}{release}"),
        )
        .unwrap();
        data.powered = true;
        assert!(matches!(
            runner.tick_without_input(data).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn gui_power_hold_can_finish_when_shutdown_prevents_release_acknowledgement() {
        let mut runner = test_runner(
            "[[step]]\naction='hold'\ncontrol='power'\nduration_ms=4000\n[[step]]\naction='expect-powered'\npowered=false\n",
        );
        let now = Instant::now();
        assert!(matches!(
            runner.advance(sample(0), now).unwrap(),
            Progress::Dispatch(Command::HoldDown {
                target: 53,
                duration_ms: 5000
            })
        ));
        let mut release = sample(4000);
        release.main.sent = 1;
        release.main.applied = 1;
        assert!(matches!(
            runner.advance(release.clone(), now).unwrap(),
            Progress::Dispatch(Command::Release { target: 53 })
        ));
        release.data.powered = false;
        release.main.powered_off = true;
        release.panel.powered_off = true;
        assert!(matches!(
            runner.advance(release, now).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn guest_time_and_wall_deadlines_are_independent() {
        let mut runner = test_runner("[[step]]\naction='delay'\nguest_ms=1000\nwall_timeout_s=2\n");
        let now = Instant::now();
        assert!(matches!(
            runner.advance(sample(500), now).unwrap(),
            Progress::Waiting
        ));
        assert!(matches!(
            runner
                .advance(sample(1499), now + Duration::from_millis(1900))
                .unwrap(),
            Progress::Waiting
        ));
        assert!(matches!(
            runner
                .advance(sample(1500), now + Duration::from_millis(1901))
                .unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
        let mut stalled =
            test_runner("[[step]]\naction='delay'\nguest_ms=1000\nwall_timeout_s=2\n");
        stalled.advance(sample(0), now).unwrap();
        assert!(
            stalled
                .advance(sample(0), now + Duration::from_secs(3))
                .unwrap_err()
                .contains("timed out")
        );
        fs::remove_dir_all(&stalled.logs).unwrap();
    }
    #[test]
    fn duplicate_reports_and_unrelated_ring_changes_are_failures() {
        for unrelated in [false, true] {
            let mut runner = test_runner(
                "[[step]]\naction='encoder'\nchannel=1\nsteps=12\n[[step]]\naction='expect-ring'\nchannel=1\nmask=31\nonly=true\n",
            );
            let now = Instant::now();
            runner.advance(sample(0), now).unwrap();
            let mut feedback = sample(100);
            feedback.panel.sent = 1;
            feedback.panel.applied = 1;
            feedback.reports = if unrelated { 12 } else { 13 };
            if unrelated {
                let (row, column) = l6max_host::indicators::RING_PINS[1][0];
                feedback.data.indicators[row] |= 1 << (column ^ 7);
            }
            assert!(runner.advance(feedback, now).is_err());
            fs::remove_dir_all(&runner.logs).unwrap();
        }
    }
    #[test]
    fn explicit_up_is_acked_with_applied_not_the_automatic_tap_release_counter() {
        let mut runner = test_runner("[[step]]\naction='press'\ncontrol='channel.1.mute'\n");
        let now = Instant::now();
        assert!(matches!(
            runner.advance(sample(0), now).unwrap(),
            Progress::Dispatch(_)
        ));
        let mut ack = sample(100);
        ack.panel.sent = 2;
        ack.panel.applied = 1;
        assert!(matches!(
            runner.advance(ack.clone(), now).unwrap(),
            Progress::Waiting
        ));
        ack.panel.applied = 2;
        assert_eq!(ack.panel.released, 0);
        assert!(matches!(
            runner.advance(ack, now).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn scene_save_holds_until_the_lamp_changes_then_checks_both_panel_edges() {
        let mut runner = test_runner("[[step]]\naction='save-scene'\nscene=1\n");
        let now = Instant::now();
        assert!(matches!(
            runner.advance(sample(0), now).unwrap(),
            Progress::Dispatch(Command::HoldDown {
                target: 15,
                duration_ms: 50
            })
        ));

        let [press, release] = trace::button_packets(Model::L6, "scene-a").unwrap();
        let mut held = sample(100);
        held.panel.sent = 1;
        held.panel.applied = 1;
        held.traces.insert(press, 1);
        held.data.indicators[2] |= 1 << 11;
        assert!(matches!(
            runner.advance(held.clone(), now).unwrap(),
            Progress::Dispatch(Command::Release { target: 15 })
        ));

        held.panel.sent = 2;
        held.panel.applied = 2;
        held.traces.insert(release, 1);
        assert!(matches!(
            runner.advance(held, now).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn l6_button_lamps_in_unused_generic_ring_slots_are_not_other_encoders() {
        let mut runner = test_runner(
            "[[step]]\naction='encoder'\nchannel=1\nsteps=12\n[[step]]\naction='expect-ring'\nchannel=1\nmask=31\nonly=true\n",
        );
        let now = Instant::now();
        runner.advance(sample(0), now).unwrap();
        let mut obs = sample(100);
        obs.panel.sent = 1;
        obs.panel.applied = 1;
        obs.reports = 12;
        for &(row, column) in l6max_host::indicators::RING_PINS[0].iter().take(5) {
            obs.data.indicators[row] |= 1 << (column ^ 7);
        }
        let (row, column) = l6max_host::indicators::RING_PINS[6][7];
        obs.data.indicators[row] |= 1 << (column ^ 7);
        assert!(matches!(
            runner.advance(obs.clone(), now).unwrap(),
            Progress::Waiting
        ));
        obs.panel.guest_ms = 1100;
        assert!(matches!(
            runner.advance(obs, now).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn restored_mixer_values_must_cover_all_visible_channels_and_light_the_mode_lamp() {
        let mut runner = test_runner("[[step]]\naction='wait'\nuntil='mixer-ready'\n");
        let now = Instant::now();
        let mut obs = sample(0);
        let (row, column) = l6max_host::indicators::RING_PINS[0][0];
        obs.data.indicators[row] |= 1 << (column ^ 7);
        runner.advance(obs.clone(), now).unwrap();
        obs.panel.guest_ms = 1000;
        assert!(matches!(
            runner.advance(obs.clone(), now).unwrap(),
            Progress::Waiting
        ));
        for pins in l6max_host::indicators::RING_PINS.iter().take(6) {
            let (row, column) = pins[0];
            obs.data.indicators[row] |= 1 << (column ^ 7);
        }
        obs.panel.guest_ms = 2000;
        assert!(matches!(
            runner.advance(obs.clone(), now).unwrap(),
            Progress::Waiting
        ));
        obs.data.indicators[4] |= 1 << 8;
        obs.panel.guest_ms = 2050;
        assert!(matches!(
            runner.advance(obs, now).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn parser_rejects_typos_and_unavailable_model_controls() {
        let prefix = "version=1\nname='test'\nmodels=['l6']\nfixture='prepared'\n";
        assert!(
            Scenario::parse(&format!(
                "{prefix}[[step]]\naction='press'\ncontrol='recorder.down'\n"
            ))
            .is_err()
        );
        assert!(
            Scenario::parse(&format!(
                "{prefix}[[step]]\naction='encoder'\nchannel=8\nsteps=12\n"
            ))
            .is_err()
        );
        assert!(
            Scenario::parse(&format!(
                "{prefix}[[step]]\naction='wait'\nuntil='boot-ready'\ntypo=true\n"
            ))
            .is_err()
        );
        assert!(
            Scenario::parse(&format!("{prefix}[[step]]\naction='save-scene'\nscene=1\n")).is_ok()
        );
        assert!(
            Scenario::parse(&format!("{prefix}[[step]]\naction='save-scene'\nscene=4\n")).is_err()
        );
        let l6max = prefix.replace("['l6']", "['l6max']");
        assert!(
            Scenario::parse(&format!("{l6max}[[step]]\naction='save-scene'\nscene=1\n")).is_err()
        );
    }

    #[test]
    fn usb_sysex_scenario_requires_complete_messages_and_accepts_reply_prefixes() {
        let valid = Scenario::parse(
            "version=1\nname='sysex'\nmodels=['l6max']\nbackends=['headless']\nfixture='prepared'\n[[step]]\naction='usb-descriptors'\n[[step]]\naction='usb-sysex'\nrequests=[{bytes=[240,126,127,6,1,247],expect_prefix=[240,126,127,6,2],reject_prefixes=[[240,82]]}]\n",
        )
        .unwrap();
        assert!(matches!(valid.step[1].action, Action::UsbSysex { .. }));
        let invalid = Scenario::parse(
            "version=1\nname='sysex'\nmodels=['l6max']\nbackends=['headless']\nfixture='prepared'\n[[step]]\naction='usb-sysex'\nrequests=[{bytes=[240,1,2]}]\n",
        );
        assert!(invalid.unwrap_err().contains("complete F0..F7"));
        let invalid_reject = Scenario::parse(
            "version=1\nname='sysex'\nmodels=['l6max']\nbackends=['headless']\nfixture='prepared'\n[[step]]\naction='usb-sysex'\nrequests=[{bytes=[240,1,247],reject_prefixes=[[]]}]\n",
        );
        assert!(
            invalid_reject
                .unwrap_err()
                .contains("reject_prefixes must not contain empty prefixes")
        );
        let invalid_timeout = Scenario::parse(
            "version=1\nname='sysex'\nmodels=['l6max']\nbackends=['headless']\nfixture='prepared'\n[[step]]\naction='usb-sysex'\nguest_timeout_ms=100\nrequests=[{bytes=[240,1,247],reply_timeout_ms=101}]\n",
        );
        assert!(
            invalid_timeout
                .unwrap_err()
                .contains("reply_timeout_ms must be positive and fit")
        );
    }
    #[test]
    fn editor_pad_assignment_requires_prior_usb_midi_enumeration() {
        let prefix =
            "version=1\nname='pads'\nmodels=['l6max']\nfixture='prepared'\nbackends=['headless']\n";
        let without_usb = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='editor-pad-assignments'\n"
        ));
        assert!(
            without_usb
                .unwrap_err()
                .contains("requires a preceding usb-descriptors step")
        );
        let with_usb = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='usb-descriptors'\n[[step]]\naction='editor-pad-assignments'\n"
        ));
        assert!(with_usb.is_ok());
        let playback = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='usb-descriptors'\n[[step]]\naction='editor-pad-assignments'\n[[step]]\naction='editor-pad-playback'\npad=3\n"
        ));
        assert!(playback.is_err(), "Editor SysEx playback is L6-only");
        let l6_prefix = prefix.replace("l6max", "l6");
        let playback = Scenario::parse(&format!(
            "{l6_prefix}[[step]]\naction='usb-descriptors'\n[[step]]\naction='editor-pad-assignments'\n[[step]]\naction='editor-pad-playback'\npad=3\n"
        ));
        assert!(playback.is_ok());
        let invalid_pad = Scenario::parse(&format!(
            "{l6_prefix}[[step]]\naction='usb-descriptors'\n[[step]]\naction='editor-pad-assignments'\n[[step]]\naction='editor-pad-playback'\npad=4\n"
        ));
        assert!(invalid_pad.unwrap_err().contains("pad must be 0..3"));
        let ui_playback = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='usb-descriptors'\n[[step]]\naction='l6max-ui-pad-assignment'\npad=2\n"
        ));
        assert!(ui_playback.is_ok());
        let wrong_model = prefix.replace("l6max", "l6");
        let wrong_model = Scenario::parse(&format!(
            "{wrong_model}[[step]]\naction='usb-descriptors'\n[[step]]\naction='l6max-ui-pad-assignment'\npad=2\n"
        ));
        assert!(wrong_model.unwrap_err().contains("scoped to L6max"));
    }
    #[test]
    fn sound_pad_playback_supports_editor_or_saved_assignment() {
        let prefix = "version=1\nname='pad-playback'\nmodels=['l6']\nfixture='prepared'\nbackends=['headless']\n";
        let action = "[[step]]\naction='usb-descriptors'\n[[step]]\naction='editor-pad-assignments'\n[[step]]\naction='sound-pad-playback'\npad=0\ntarget=49\n";
        assert!(Scenario::parse(&format!("{prefix}{action}")).is_ok());
        let l6max = prefix.replace("models=['l6']", "models=['l6max']");
        assert!(Scenario::parse(&format!("{l6max}{action}")).is_ok());
        let without_assignment = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='sound-pad-playback'\npad=0\ntarget=49\n"
        ));
        assert!(
            without_assignment
                .unwrap_err()
                .contains("preceding editor-pad-assignments")
        );
        let saved_state = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='sound-pad-playback'\npad=0\ntarget=49\nassignment='saved-state'\nplayback_window_ms=30000\nrequire_pad_led_change=false\n"
        ));
        assert!(saved_state.is_ok());
        let invalid_target = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='usb-descriptors'\n[[step]]\naction='editor-pad-assignments'\n[[step]]\naction='sound-pad-playback'\npad=0\ntarget=53\n"
        ));
        assert!(
            invalid_target
                .unwrap_err()
                .contains("target must match pad")
        );
        let mismatched_target = Scenario::parse(&format!(
            "{prefix}[[step]]\naction='usb-descriptors'\n[[step]]\naction='editor-pad-assignments'\n[[step]]\naction='sound-pad-playback'\npad=1\ntarget=49\n"
        ));
        assert!(
            mismatched_target
                .unwrap_err()
                .contains("target must match pad")
        );
    }

    #[test]
    fn l6_recorder_playback_is_scoped_to_the_verified_play_gpio() {
        let prefix = "version=1\nname='recorder-playback'\nmodels=['l6']\nfixture='prepared'\nbackends=['headless']\n";
        let scenario = format!("{prefix}[[step]]\naction='l6-recorder-playback'\ntarget=4\n");
        assert!(Scenario::parse(&scenario).is_ok());
        assert!(
            Scenario::parse(&scenario.replace("target=4", "target=5"))
                .unwrap_err()
                .contains("MAIN target 4")
        );
        assert!(
            Scenario::parse(&scenario.replace("models=['l6']", "models=['l6max']"))
                .unwrap_err()
                .contains("scoped to L6")
        );
    }
    #[test]
    fn l6max_recorder_playback_is_scoped_to_its_verified_firmware_and_gpio() {
        let prefix = "version=1\nname='recorder-playback'\nmodels=['l6max']\nfixture='prepared'\nbackends=['headless']\nmain_sha256='980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461'\n";
        let scenario = format!("{prefix}[[step]]\naction='l6max-recorder-playback'\ntarget=4\n");
        assert!(Scenario::parse(&scenario).is_ok());
        assert!(Scenario::parse(&scenario.replace("target=4", "target=5")).is_err());
        assert!(Scenario::parse(&scenario.replace("models=['l6max']", "models=['l6']")).is_err());
        assert!(
            Scenario::parse(&scenario.replace(
                "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461",
                "0000000000000000000000000000000000000000000000000000000000000000"
            ))
            .is_err()
        );
    }
    #[test]
    fn unsupported_schema_and_zero_timeouts_fail_before_boot() {
        let text = "version=1\nname='test'\nmodels=['l6max']\nfixture='prepared'\n[[step]]\naction='wait'\nuntil='boot-ready'\n";
        assert!(Scenario::parse(text).is_ok());
        assert!(Scenario::parse(&text.replace("version=1", "version=2")).is_err());
        assert!(Scenario::parse(&format!("{text}wall_timeout_s=0\n")).is_err());
    }
    #[test]
    fn l6_main_gpio_event_scenario_rejects_wrong_model_and_mapping() {
        let base = "version=1\nname='gpio'\nmodels=['l6']\nfixture='prepared'\nbackends=['headless']\n[[step]]\naction='l6-main-gpio-event'\ntarget=48\nevent=25\n";
        assert!(Scenario::parse(base).is_ok());
        assert!(Scenario::parse(&base.replace("models=['l6']", "models=['l6max']")).is_err());
        assert!(Scenario::parse(&base.replace("event=25", "event=26")).is_err());
        assert!(Scenario::parse(&base.replace("target=48", "target=53")).is_err());
    }
    #[test]
    fn l6_usb_startup_requires_first_step_l6_scope_and_exact_firmware_hash() {
        let base = "version=1\nname='usb'\nmodels=['l6']\nfixture='fresh'\nbackends=['headless']\nmain_sha256='827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799'\n[[step]]\naction='l6-usb-startup'\n";
        assert!(Scenario::parse(base).is_ok());
        assert!(
            Scenario::parse(&base.replace(
                "main_sha256='827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799'\n",
                ""
            ))
            .is_err()
        );
        assert!(Scenario::parse(&base.replace("models=['l6']", "models=['l6max']")).is_err());
        let late = base.replace(
            "[[step]]\naction='l6-usb-startup'",
            "[[step]]\naction='delay'\nguest_ms=1\n[[step]]\naction='l6-usb-startup'",
        );
        assert!(Scenario::parse(&late).is_err());
    }
    #[test]
    fn l6_din_sweep_requires_its_l6_110_main_hash() {
        let base = "version=1\nname='din-sweep'\nmodels=['l6']\nfixture='prepared'\nbackends=['headless']\nmain_sha256='827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799'\n[[step]]\naction='l6-din-sweep'\n";
        assert!(Scenario::parse(base).is_ok());
        let without_hash = base.replace(
            "main_sha256='827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799'\n",
            "",
        );
        assert!(Scenario::parse(&without_hash).is_err());
        let wrong_hash = base.replace(
            "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799",
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(Scenario::parse(&wrong_hash).is_err());
        assert!(Scenario::parse(&base.replace("models=['l6']", "models=['l6max']")).is_err());
    }
    #[test]
    fn every_checked_in_scenario_loads_after_composition() {
        let root = l6max_host::paths::repository_root().join("tests/smoke/scenarios");
        for path in fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        {
            let scenario =
                Scenario::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(
                !scenario
                    .step
                    .iter()
                    .any(|s| matches!(s.action, Action::Include { .. }))
            );
            for model in &scenario.models {
                let mut runner = test_runner("[[step]]\naction='delay'\nguest_ms=1\n");
                let temp = runner.logs.clone();
                runner = Runner::new(
                    scenario.clone(),
                    if model == "l6" {
                        Model::L6
                    } else {
                        Model::L6max
                    },
                    temp,
                )
                .unwrap_or_else(|e| panic!("{} / {model}: {e}", path.display()));
                fs::remove_dir_all(runner.logs).unwrap();
            }
        }
    }
    #[test]
    fn includes_are_confined_cycle_checked_and_preserve_model_scope() {
        let runner = test_runner("[[step]]\naction='delay'\nguest_ms=1\n");
        let root = &runner.logs;
        fs::write(
            root.join("fragment.toml"),
            "[[step]]\naction='delay'\nguest_ms=2\n",
        )
        .unwrap();
        let prefix = "version=1\nname='include'\nmodels=['l6','l6max']\nfixture='prepared'\n[[step]]\nmodels=['l6max']\naction='include'\npath='fragment.toml'\n";
        fs::write(root.join("test.toml"), prefix).unwrap();
        let loaded = Scenario::load(&root.join("test.toml")).unwrap();
        assert_eq!(loaded.step[0].models, ["l6max"]);
        fs::write(
            root.join("fragment.toml"),
            "[[step]]\naction='include'\npath='fragment.toml'\n",
        )
        .unwrap();
        assert!(
            Scenario::load(&root.join("test.toml"))
                .unwrap_err()
                .contains("cycle")
        );
        fs::write(
            root.join("test.toml"),
            prefix.replace("fragment.toml", "../outside.toml"),
        )
        .unwrap();
        fs::write(
            root.parent().unwrap().join("outside.toml"),
            "[[step]]\naction='delay'\nguest_ms=1\n",
        )
        .unwrap();
        assert!(
            Scenario::load(&root.join("test.toml"))
                .unwrap_err()
                .contains("escapes")
        );
        fs::remove_file(root.parent().unwrap().join("outside.toml")).unwrap();
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn l6max_tap_tempo_lamp_does_not_block_panel_settling() {
        let mut rows = [0x28, 0x1000, 0x40002, 0x8000, 0x110, 4, 1, 0];
        let blinking = {
            let mut r = rows;
            r[1] ^= 0x20000;
            r
        };
        assert_eq!(
            settling_rows(Model::L6max, rows),
            settling_rows(Model::L6max, blinking)
        );
        assert_ne!(
            settling_rows(Model::L6, rows),
            settling_rows(Model::L6, blinking)
        );
        rows[1] ^= 0x10000;
        assert_ne!(
            settling_rows(Model::L6max, rows),
            settling_rows(Model::L6max, blinking)
        );
    }
    #[test]
    fn setup_input_propagates_through_includes_and_is_dropped_for_spec_state() {
        let runner = test_runner("[[step]]\naction='delay'\nguest_ms=1\n");
        let root = &runner.logs;
        fs::write(
            root.join("fragment.toml"),
            "[[step]]\naction='delay'\nguest_ms=2\n[[step]]\naction='delay'\nguest_ms=3\n",
        )
        .unwrap();
        fs::write(
            root.join("test.toml"),
            "version=1\nname='setup'\nmodels=['l6max']\nfixture='prepared'\n[[step]]\nsetup_input=true\naction='include'\npath='fragment.toml'\n[[step]]\naction='delay'\nguest_ms=4\n",
        )
        .unwrap();
        let loaded = Scenario::load(&root.join("test.toml")).unwrap();
        assert_eq!(
            loaded
                .step
                .iter()
                .map(|s| s.setup_input)
                .collect::<Vec<_>>(),
            [true, true, false]
        );
        let kept = loaded.without_setup_input();
        assert_eq!(kept.step.len(), 1);
        assert!(matches!(kept.step[0].action, Action::Delay { guest_ms: 4 }));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn unsupported_operations_and_switch_semantics_fail_before_boot() {
        let prefix = "version=1\nname='invalid'\nmodels=['l6max']\nfixture='prepared'\n";
        for step in [
            "action='usb-descriptors'",
            "action='press'\ncontrol='ch5-pad-switch'",
            "action='switch'\ncontrol='channel.1.mute'\nenabled=true",
            "action='expect-ring'\nchannel=1\nmask=1024",
            "action='expect-ring'\nchannel=1\nchanged=true\nsettle_ms=0",
            "action='checkpoint'\nname='../escape'",
            "action='expect-display'\ncheckpoint='missing'\nchanged=true",
        ] {
            assert!(
                Scenario::parse(&format!("{prefix}[[step]]\n{step}\n")).is_err(),
                "{step}"
            );
        }
    }

    #[test]
    fn typed_input_sweep_allows_only_its_verified_rejections() {
        let now = Instant::now();
        let mut runner = test_runner("backends=['headless']\n[[step]]\naction='l6-input-sweep'\n");
        runner.device_finished(json!({"expected_rejections":{"main":2,"panel":2}}));
        let mut obs = sample(0);
        obs.main.rejected = 2;
        obs.panel.rejected = 2;
        assert!(matches!(
            runner.advance(obs, now).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();

        let mut runner = test_runner("backends=['headless']\n[[step]]\naction='l6-input-sweep'\n");
        runner.device_finished(json!({"expected_rejections":{"main":2,"panel":2}}));
        let mut obs = sample(0);
        obs.main.rejected = 3;
        obs.panel.rejected = 2;
        assert!(runner.advance(obs, now).is_err());
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn wire_edges_wait_for_release_and_reject_duplicate_edges() {
        let text = "version=1\nname='edges'\nmodels=['l6max']\nfixture='prepared'\n[[step]]\naction='press'\ncontrol='scene-a'\n[[step]]\naction='expect-button-report'\ncontrol='scene-a'\n";
        for duplicate in [false, true] {
            let mut runner = test_runner("[[step]]\naction='delay'\nguest_ms=1\n");
            runner.scenario = Scenario::parse(text).unwrap();
            runner.model = Model::L6max;
            let now = Instant::now();
            runner.advance(sample(0), now).unwrap();
            let mut obs = sample(100);
            obs.panel.sent = 2;
            obs.panel.applied = 2;
            let [on, off] = trace::button_packets(Model::L6max, "scene-a").unwrap();
            obs.traces.insert(on, if duplicate { 2 } else { 1 });
            if duplicate {
                assert!(runner.advance(obs, now).unwrap_err().contains("duplicate"));
            } else {
                assert!(matches!(
                    runner.advance(obs.clone(), now).unwrap(),
                    Progress::Waiting
                ));
                obs.traces.insert(off, 1);
                obs.panel.guest_ms = 1200;
                assert!(matches!(
                    runner.advance(obs, now).unwrap(),
                    Progress::Complete
                ));
            }
            fs::remove_dir_all(&runner.logs).unwrap();
        }
    }
    #[test]
    fn late_ring_feedback_resets_the_declared_quiet_window() {
        let mut runner = test_runner(
            "[[step]]\naction='encoder'\nchannel=1\nsteps=12\n[[step]]\naction='expect-ring'\nchannel=1\nchanged=true\nonly=true\nsettle_ms=3000\n",
        );
        let now = Instant::now();
        runner.advance(sample(0), now).unwrap();
        let mut obs = sample(100);
        obs.panel.sent = 1;
        obs.panel.applied = 1;
        obs.reports = 12;
        let (row, col) = l6max_host::indicators::RING_PINS[0][0];
        obs.data.indicators[row] |= 1 << (col ^ 7);
        runner.advance(obs.clone(), now).unwrap();
        obs.panel.guest_ms = 1100;
        assert!(matches!(
            runner.advance(obs.clone(), now).unwrap(),
            Progress::Waiting
        ));
        let (row, col) = l6max_host::indicators::RING_PINS[0][1];
        obs.data.indicators[row] |= 1 << (col ^ 7);
        obs.panel.guest_ms = 2000;
        runner.advance(obs.clone(), now).unwrap();
        obs.panel.guest_ms = 4999;
        assert!(matches!(
            runner.advance(obs.clone(), now).unwrap(),
            Progress::Waiting
        ));
        obs.panel.guest_ms = 5000;
        assert!(matches!(
            runner.advance(obs, now).unwrap(),
            Progress::Complete
        ));
        fs::remove_dir_all(&runner.logs).unwrap();
    }
    #[test]
    fn unexpected_power_loss_fails_but_explicit_restart_accepts_shutdown() {
        let now = Instant::now();
        for restart in [false, true] {
            let mut runner = test_runner(if restart {
                "backends=['headless']\n[[step]]\naction='restart'\n"
            } else {
                "[[step]]\naction='delay'\nguest_ms=1000\n"
            });
            if restart {
                let mut skipped = runner.scenario.step[0].clone();
                skipped.models = vec!["l6max".into()];
                skipped.action = Action::Delay { guest_ms: 1 };
                runner.scenario.step.insert(0, skipped);
            }
            let mut obs = sample(0);
            obs.data.powered = false;
            let result = runner.advance(obs, now);
            if restart {
                assert!(matches!(
                    result.unwrap(),
                    Progress::Device(devices::Operation::Restart)
                ));
            } else {
                assert!(result.is_err());
            }
            fs::remove_dir_all(&runner.logs).unwrap();
        }
    }
}
