# Smoke scenario format, version 1

A scenario is a strict TOML sequence, executed by one `smoke-core::Runner` on
both adapters. Unknown actions, misspelled fields, unsupported controls,
model/channel mismatches, unsupported versions and zero timeouts are errors
before boot. Channels are one-based. Electrical IDs and firmware addresses
belong in Rust model mappings, not scenario files.

```toml
version = 1
name = "mute"
models = ["l6", "l6max"]
fixture = "prepared"

[[step]]
action = "wait"
until = "boot-ready"
guest_timeout_ms = 120000
wall_timeout_s = 600

[[step]]
action = "press"
control = "channel.1.mute"

[[step]]
action = "expect-mute"
channel = 1
guest_timeout_ms = 3000
```

The example abbreviates prepared setup (SD navigation, mixer restoration and
LEVEL selection); see the checked-in
`tests/smoke/scenarios/mute.toml` for the prepared fixture sequence. Fixture
names are contracts, not automatic setup commands.

## Scenario header and common step fields

The scenario document has one header table followed by one or more `[[step]]`
tables. Unknown header fields fail parsing.

| Header field | Required | Meaning |
| --- | --- | --- |
| `version` | yes | Schema version; currently `1`. |
| `name` | yes | Nonempty scenario name used in reports. |
| `models` | yes | One or both of `"l6"` and `"l6max"`. |
| `fixture` | yes | Named local fixture the scenario expects. |
| `main_sha256` | no | Exact 64-character hexadecimal MAIN firmware hash for revision-specific operations. |
| `main_sha256_any` | no | Additional explicitly allowed MAIN hashes for the same guarded operations. |
| `backends` | no | `"headless"` and/or `"gui"`; defaults to both. Typed device/GDB operations require headless. |
| `trace_sd` | no | Enable SD trace instrumentation for the scenario. |

Every `[[step]]` has a required `action` and may also set:

| Common step field | Default | Meaning |
| --- | --- | --- |
| `models` | all scenario models | Restrict this step to a subset of the scenario models. |
| `guest_timeout_ms` | `10000` | Guest-clock deadline for the step. Must be positive. |
| `wall_timeout_s` | `180` | Host wall-clock deadline for the step. Must be positive. |
| `setup_input` | `false` | The step is UI input that only brings the device to the scenario's starting state, such as confirming startup prompts. The driver drops it when the case state was built from a fixture `state_spec`. On an `include`, it applies to every included step. |

Each action table adds its action-specific fields below. For composed scenarios,
`include` takes a path to a TOML fragment containing only `[[step]]` tables.
`state_recipe` in a fixture points to a normal scenario document; it does not
introduce a second recipe syntax.

Mark only input whose result a state spec reproduces with `setup_input`. Input
that is the behavior under test, or that sets runtime-only state (for example
card-reader mode), stays unmarked.

## Step operations

| Action | Fields | Meaning |
| --- | --- | --- |
| `wait` | `until = "boot-ready"`, `"mixer-ready"` or `"panel-settled"` | Boot waits for the full LED sweep, then an L6 mode lamp or L6max pixels; prepared mixer readiness requires nonzero values on every visible ring and, on L6, a lit mode lamp; settling waits for one guest second of unchanged output |
| `delay` | `guest_ms` | Wait on guest time, not host time |
| `include` | `path` | Expand a strict `[[step]]` fragment relative to its containing file; paths stay inside the scenario directory, cycles/depth are checked |
| `select-mode` | `mode` | Pass if the mode lamp is lit; otherwise wait for one guest second of stable panel output and press the mode button |
| `switch` | `control`, `enabled` | Set a latching switch using its real GUI toggle or one headless electrical edge |
| `checkpoint` | `name` | Save display/rings and PNG/ASCII evidence under a unique simple name |
| `expect-display` | `checkpoint`, `changed` | Compare retained pixels with the checkpoint; scoped to L6max |
| `expect-trace` | `contains`, optional `absent` | Require every listed pattern and reject forbidden patterns in the active trace |
| `expect-trace-count` | `pattern`, `count`, optional `at_least`, `checkpoint` | Assert exact or minimum active-trace match count, optionally relative to a checkpoint |
| `expect-button-report` | `control` | Exact L6max press/release UART edges relative to the preceding press/switch, including no spurious switch release |
| `expect-startup-report` | `control` | Check the expected startup report for a mapped control |
| `press` | `control` | Real down/up pair, 50 guest-ms minimum hold; await both applied acknowledgements; UP is acknowledged after release |
| `encoder` | `channel`, `steps`, optional `gesture` | Positive steps raise the knob; use production polarity mapping; await application |
| `expect-ring` | `channel`, `mask`, `checkpoint` and/or `changed = true`, optional `only = true`, `settle_ms` | Verify output and exact encoder firmware report count relative to the preceding action |
| `expect-encoder-reports` | — | Exact firmware report count for the preceding encoder command, without assuming its editing mode changes a ring |
| `expect-mode` | `mode` | Verify the model-specific mode lamp before taking ring baselines |
| `expect-mute` | `channel` | Verify the model-specific mute lamp toggled relative to the preceding action |
| `hold` | `control`, `duration_ms` | Keep a semantic control held for a bounded duration |
| `hold-chord` | `controls`, `duration_ms` | Hold multiple semantic controls together |
| `save-scene` | `scene` | Save the current assignment to a scene |
| `expect-scene` | `scene`, `lit`, optional `checkpoint`, `changed` | Check a scene indicator, optionally against a checkpoint |
| `expect-button-led` | `control`, optional `lit`, `checkpoint`, `changed` | Check a mapped button lamp's level or change |
| `expect-effect` | `effect` | Check the current effect indication |
| `analog` | `channel`, `steps`, optional `gesture` | Move a one-based analog control by signed steps |
| `expect-analog` | `channel`, `value` | Check a control's analog value |
| `expect-powered` | `powered` | Check the current power state |
| `expect-memory` | `address`, `bytes` | Compare MAIN memory bytes under the scenario's firmware hash guard |
| `panel-event` | `target`, `column`, `row`, `event` | Inject a panel-controller event for focused event-path scenarios |

Boot steps explicitly allow a longer ceiling for slow L6 TCG. Assertion
baselines are captured immediately before an action, never after its output
has already changed.

`gesture` is `scroll` (default), `drag-vertical` or `drag-horizontal`. Headless
execution applies the equivalent logical knob movement. GPUI dispatches the
specified gesture into its actual hit region and handlers. Thus event geometry,
scroll accumulation and redraw bugs remain GUI-adapter coverage. Fractional
motion is additionally covered by production gesture unit tests.

Supported semantic controls include each physical channel mute, modes
`eq-high`, `freq`, `mid`, `low`, `aux1`, `aux2`, `efx`, `pan`, `level` and L6max
`sub-mix`. L6max also accepts its canonical `controls::CONTROLS` names and
`recorder.menu/up/down/confirm`. Wiring stays in `control_target`; unsupported
model/control combinations fail early. The L6 USB startup scenario uses `ch4-mono`.

## Device and lifecycle steps

Scenarios default to `backends = ["headless", "gui"]`. Typed device steps require
`backends = ["headless"]` and are rejected before boot on unsupported backends.
`trace_sd = true` enables SD trace instrumentation (also enabled by `sd-card`).

| Action | Fields | Contract |
| --- | --- | --- |
| `sd-card` | `inserted` | QMP removal or insertion of the case's copied card |
| `usb-descriptors` | — | Validate device/configuration descriptors and SET_CONFIGURATION |
| `usb-midi` | `value` | Require the preceding encoder input's model-mapped channel-1 CC on USB MIDI IN, then send that CC with `value` |
| `host-midi-round-trip` | — | Verify host MIDI input/output delivery through the configured host MIDI backend |
| `direct-usb-midi-round-trip` | optional `cable` (0–15), `expect_ring_change` (defaults to true) | Check the encoder report and channel-1 mixer CC on a selected USB MIDI cable; set `expect_ring_change = false` to verify other cables are ignored |
| `l6-din-usb-forwarding` | — | Verify L6 DIN-to-USB forwarding behavior |
| `host-midi-sync` | — | Exercise MIDI clock synchronization through the host MIDI path |
| `l6-midi-sync` | — | Exercise the L6 MIDI synchronization path |
| `l6-midi-ring-pin-sweep` | `cc` | Sweep L6 MIDI CC values and compare ring-pin output |
| `l6-midi-mute-indicators` | — | Check L6 MIDI mute-indicator responses |
| `usb-sysex` | `requests` with `bytes`, optional `expect_prefix`, `expect_reply`, `min_reply_bytes` | Send cable-2 SysEx requests, reassemble replies, and check presence, length, or prefix as declared |
| `editor-pad-assignments` | — | Initialize the Editor session, enumerate WAV files, assign the first file on all four pads and verify assignment state through SysEx readback; requires a preceding `usb-descriptors` step |
| `editor-pad-playback` | `pad` | L6-only assigned-pad playback check; requires preceding `editor-pad-assignments` |
| `l6max-ui-pad-assignment` | `pad` | L6max UI assignment check; requires preceding `usb-descriptors` and an L6max-scoped scenario |
| `sound-pad-playback` | `pad`, `target`, optional `assignment` (`editor` by default or `saved-state`), optional `require_pad_led_change` (default `true`), optional `require_stereo_meter` (default `false`), optional `playback_window_ms` (default `6000`, range `1000..120000`), optional `expect_source` (WAV path on the card; requires `L6_AUDIO_RAW_CAPTURE`) and `max_residual_db` (default `-60`) | Press one L6 or L6max MAIN pad input and require nonzero SAI1 TX samples; require its model-specific pad LED response unless `require_pad_led_change = false`; `editor` requires a preceding Editor assignment step, while `saved-state` uses the assignment already present in the fixture; capture indicator PWM levels throughout playback; when `require_stereo_meter = true`, require two distinct levels of at least two bars on each side independently using a stereo isolation WAV; enable `L6_AUDIO_SAMPLE_TRACE=1` in the case environment |
| `wasm-patch` | — | Exercise a generated WebAssembly patch package through the firmware update path |
| `l6-recorder-playback` | `target` | L6 only; GDB verifies event `0x20`, handler acceptance, and the mode-0 callback on MAIN target 4; after five guest seconds it requires a changed SAI1 TX signature and sample data in the channel-14 TCD source. Requires a mode-0 state, valid `MASTER.WAV`, GDB fixture support, and `L6_AUDIO_SAMPLE_TRACE=1` |
| `l6max-recorder-playback` | `target` | L6max only, MAIN target 4; GDB verifies Play event `0x2c`, event dispatch, callback slot 2, and the recorder action, then requires nonzero changed SAI1 TX windows and sample data in channel-14's TCD source. Requires a prepared playable recording, GDB fixture support, and `L6_AUDIO_SAMPLE_TRACE=1` |
| `l6max-din-mixer-control` | `expect_din_ring_change` | After editor SysEx setup, verify USB mixer control, stock/patched DIN response, and DIN mute-echo suppression |
| `usb-storage` | — | Enumerate storage, compare sector zero to the copied card, round-trip its last sector, then create/read/compare/delete a 1,537-byte FAT file through the USB disk |
| `audio-callbacks` | — | First step only; arm revision-guarded breakpoints before boot and require every model callback |
| `knob-dialog` | — | Patched L6max only; verify all ten popup modes, firmware-formatted values, heap lifetime, busy-aware expiry, display cleanup, widget restoration and exact encoder reports; frequency text allows one display unit of firmware rounding |
| `popup-concurrency` | — | Patched L6max only; exercise recorder polling, stock notification priority, expiry-boundary input, queue overflow, busy pause/resume, allocation failure and held-lock contention through GDB |
| `firmware-update` | — | L6 and L6max disposable-state update lifecycle, corrupt-checksum rejection, NOR segment install, L6max panel programming, protected-region preservation and clean restart; a fixture `update_package` additionally checks model-specific post-install behavior |
| `power-press` | `hold_ms`, `powered` | Check expected power result, model cutoff timing and stopped clock |
| `restart` | — | Retire the session before reopening its isolated persistent state; reset readiness, trace counters and action baselines |
| `expect-recorder` | — | L6max startup mode restored after restart |
| `expect-settings` | `restored` | L6max edited RTC/NOR/version records, or reload and RTC startup timing after restart |
| `l6max-panel-key-map` | — | Verify that L6max sound-pad targets 15, 20, 25 and 30 set their MAIN key-state bits and clear them after release |
| `l6max-channel-signal-pwm` | — | Seed the twelve firmware channel-level accumulators and verify five distinct increasing sampled PWM levels on all eight green SIGNAL lamps; requires L6max MAIN 1.10 and GDB. This qualifies the firmware-to-panel brightness path, not audio-input routing. |
| `l6-level-meter-pwm` | — | Seed ten L6 1.10 level accumulators at the measured brightness thresholds and sample the raw panel matrix; require at least one output cell with five distinct nonzero duty means. Uses GDB and no SD, USB, MIDI, or host audio. |
| `l6-main-gpio-event` | `target`, `event` | Hash-guarded L6 GDB check that Tap target 48 submits event `0x19` and Sound Pad targets 49–52 submit `0x15`–`0x18` |
| `l6max-tap-handler` | none | Hash-guarded L6max GDB check that Tap target 48 reaches the firmware's TAP press handler. |
| `l6-din-event-dispatch` | `group`, `cc`, `value` | Hash-guarded L6 check of DIN CC-to-event dispatch |
| `l6-usb-startup` | — | First step only; exact-hash L6 probe replays the startup USB-enable sequence, verifies three firmware entry points and controller writes |
| `l6-input-sweep` | — | Sweep supported L6 controls and record their input responses |
| `l6-din-sweep` | — | Sweep supported L6 DIN values and record dispatch behavior |
| `l6max-input-timing` | — | Measure L6max input timing and applied acknowledgements |

Peripheral calls have bounded I/O and the step's guest/wall deadline. Callback,
startup and settings operations require the known MAIN revision; existing
installed MAIN bytes must match the selected image. Firmware addresses stay in
Rust. Restart preserves the settings comparison checkpoint but clears stale
per-action observations and boot readiness.

`main_sha256` allows one exact MAIN image for address-sensitive work.
`main_sha256_any` adds explicitly qualified alternatives while retaining the
same runtime checks.

A fragment contains only `[[step]]` entries. Parent include model scope is
intersected with child scopes. Use fragments for shared setup or control groups;
there are no arbitrary shell commands, expressions or loop constructs.

## Extending the driver

Add a scenario and catalog entry for sequences using existing operations. Add
complex USB, MIDI, storage or debugger assertions as typed Rust operations with
focused tests. Avoid arbitrary shell commands, embedded expressions, loops or
firmware-memory addressing in TOML.

A catalog case has either a scenario path or a supervised GUI executable. A
local `[legacy."case-id"]` entry may declare an external `executable` and its
required `executable_sha256`, plus `arguments`, `environment`, and a named
fixture. This lets the driver attest and supervise a packaged GUI binary while
keeping native menu and picker interactions in the GUI adapter. A local entry
can select another named fixture (for example, a generated patched-image
directory) without changing the stock fixture used by other cases. The default
fixture is `prepared`; persistence uses `fresh`. Configuration-required cases
stay BLOCKED until their matching configuration section is present. Read the
case prerequisites before enabling them.

Legacy arguments support `{firmware_dir}`, `{logs}`, `{qemu}` and `{package}`
substitutions. Remaining braces are an error. Paths in these arguments must be
absolute. The driver strips ambient `L6_*` and `L6MAX_*` variables
from every child, then applies declared case configuration and explicit logging/
case-context settings. This prevents old probe flags silently changing a later run.

Packaged bundle/menu tests run under the driver with a hash-pinned app
executable, isolated `L6_APP_DATA` beneath the case artifacts, and an explicit
hash check for the QEMU shipped inside the bundle. The bundle case imports both
model packages and checks their separate persisted state; the menu case
exercises native picker dispatch while the guest is running. Both cases need a
local config with the packaged assets before they can run.

Ring expectations wait for `settle_ms` (default 1000 guest milliseconds) with
unchanged physical ring values and no new encoder reports. Full-ring scenarios
use 3000 ms for delayed L6 feedback; the guest/wall step limits still apply. The `only` assertion checks six physical rings on
L6 and eight on L6max; extra generic decoder slots alias button lamps on L6.
Mapping scenarios use `changed = true` because presets and firmware processing
can change the numerical value associated with detents. Use `mask` when an exact
value is established by the fixture/firmware contract. Reverse movement in the
quick scenario must restore channel 2's known initial mask `0x001`.
