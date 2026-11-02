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
- L6 DIN-to-USB forwarding and USB startup scenarios need further qualification.
- L6 scene-save GUI coverage, startup-key parity, and the packaged-app menu
  and bundle cases require further validation.
- Sound-pad playback and MASTER meter response are exercised for both models
  using saved assignments. The waveform cases compare SAI output with the
  source WAV. Cards must meet the firmware's
  [layout requirements](../emulator/usb.md#card-layout). The Editor-assigned
  pad scenarios need further qualification. Recorder playback requires prepared media and state;
  recording to a file remains unresolved.
- L6max sound-pad key-bit assertions and DIN/Editor scenarios need live
  qualification. Physical control wiring is not established by emulator tests.

## Test setup and execution

The [scenario catalog](../../tests/smoke/catalog.toml) specifies each case's
models, backends, and prerequisites. Each scenario declares its state, card, and patches, and the local
configuration supplies the firmware packages. A state spec creates disposable settings
and panel state without running setup interactions, and a card spec builds the case's
SD card with its pad assignments. Scenario steps marked `setup_input` are skipped when
a state spec supplies that setup. See the
[testing workflow](testing.md) for configuration, permissions, suites, and reports.
The driver records the binary hash; an externally supplied binary's build
source is not attested by that hash.
