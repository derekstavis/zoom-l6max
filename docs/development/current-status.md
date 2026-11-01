# Current development status

Use [`l6test`](testing.md) for repeatable emulator scenarios and
[`l6dbg`](diagnostics.md) for focused diagnostics. Results apply to the
firmware, QEMU binary, fixture, and backend used in each run; inspect the
artifacts for the configuration and result.

## Panel and audio

The L6 and L6max MASTER meters use model-specific panel outputs. The stereo
sound-pad scenarios check that each meter responds independently at multiple
levels, using a saved one-shot assignment and no USB setup. Encoder rings use
sampled PWM duty, normalized to their observed full-scale endpoints. The panel
sampler integrates pin activity over approximately 33 ms of guest time.

The L6max green SIGNAL indicators have a graded PWM path. Sound-pad and TAP
lamps can blink; scan-window samples of a binary lamp do not establish a
separate brightness control. Physical LED brightness, meter calibration,
analog audio behavior, and physical jack routing remain unverified.

## Known limits

- The full L6 panel scenario and exact encoder reversal need further
  qualification on both headless and GPUI backends.
- L6 DIN input, DIN-to-USB forwarding, and the event-dispatch scenario need
  further qualification. L6 USB startup and L6max direct USB cable-isolation
  scenarios also need live qualification.
- L6 scene-save GUI coverage, startup-key parity, and the packaged-app menu
  and bundle cases require further validation.
- Sound-pad playback and MASTER meter response are exercised for both models
  using saved assignments. The waveform cases compare SAI output with the
  source WAV. Cards must meet the firmware's
  [layout requirements](../emulator/usb.md#card-layout). The L6 Editor-assigned
  pad scenario needs further qualification. Recorder playback requires prepared media and state;
  recording to a file remains unresolved.
- L6max sound-pad key-bit assertions and DIN/Editor scenarios need live
  qualification. Physical control wiring is not established by emulator tests.

## Fixtures and execution

The [scenario catalog](../../tests/smoke/catalog.toml) specifies each case's
models, backends, and prerequisites. A `state_spec` creates disposable settings
and panel state without running setup interactions. `fixture.sound_pad` writes
pad assignments and playback settings to a disposable SD-card copy. Scenario
steps marked `setup_input` are skipped when a state spec supplies that setup.

Use a model-matched firmware package and state, pin the QEMU executable by
SHA-256, and configure the media and backend needed by each case. See the
[testing workflow](testing.md) for fixtures, permissions, suites, and reports.
The driver records the binary hash; an externally supplied binary's build
source is not attested by that hash.
