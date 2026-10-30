# `l6test`

`l6test` is the suite driver for repeatable emulator scenarios. It validates
fixture configuration, prepares isolated guest state, supervises the requested
headless or GUI cases, and writes a structured report with case artifacts.
Use [diagnostic tools](l6dbg.md) for observation without pass/fail assertions.

## Commands

```sh
cargo run --offline --locked -p l6test -- list
cargo run --offline --locked -p l6test -- check --config smoke.local.toml
cargo run --offline --locked -p l6test -- run --config smoke.local.toml --suite quick
cargo run --offline --locked -p l6test -- run --config smoke.local.toml --case boot --model l6
cargo run --offline --locked -p l6test -- run --config smoke.local.toml --suite quick --backend both
```

- `list` prints catalog cases, model scope, suites, and cases needing explicit
  local configuration.
- `check` validates `smoke.local.toml`, its pinned QEMU digest, fixture paths,
  firmware model, and patch selections without starting a guest.
- `run` selects one `--case ID` or a `--suite NAME`, optionally filtered by
  `--model l6|l6max|all` and `--backend headless|gui|both`.
- `--output NEW_DIRECTORY` chooses an artifact directory. It must not already
  exist. By default the driver creates a unique directory under `/tmp`.
- `--no-build` uses existing emulator binaries instead of building them.
- `--jobs N` runs up to N cases at once. The default is a quarter of the host's
  cores, between 1 and 4. Each case has its own process group, state, card,
  sockets and artifact directory. GUI cases, scenarios that publish native
  host MIDI ports and external executables run alone. Shareable cases run
  first. `--jobs 1` restores strictly sequential execution.

Copy the [example fixture file](../../../tests/smoke/fixtures.example.toml) to
`smoke.local.toml`, pin the QEMU binary’s SHA-256, and provide firmware and
optional state/card paths. Firmware, device state, and generated reports stay
local. See the
[testing workflow](../testing.md).

## GUI screen probe

The exploratory `probe gui` command captures the panel while pressing GPIO
targets. It is for visual/behavioral investigation and does not qualify a test.

```sh
cargo run --offline --locked -p l6test -- probe gui \
  --keys 4,5,6 --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-firmware --logs /tmp/gui-capture
```

Use `--help` after `probe gui` for the short usage. Engine options are the same
as the GUI/headless host; `--volatile` avoids persistent device state.

## Capture the L6max screen as a GIF

`capture screen` boots L6max into the recorder screen and records its LCD as a
looping GIF. Add timed taps and encoder turns to demonstrate a screen change:

```sh
cargo run --offline --locked -p l6test -- capture screen \
  --out /tmp/knob-dialog.gif --duration-ms 9000 --fps 5 \
  --tap 35:0:50 \
  --encoder 1:800:-4 --encoder 1:1400:-4 --encoder 1:2000:-4 \
  --encoder 1:2600:-4 --encoder 1:3200:-3 \
  --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/patched-l6max
```

`--out FILE.gif` is required and existing files are never overwritten.
`--duration-ms` defaults to 12000 and accepts 1000..60000; `--fps` defaults to
5 and accepts 1..15. Repeat `--tap TARGET:AT_MS:HOLD_MS` and
`--encoder CHANNEL:AT_MS:STEPS`; event times are relative to the start of
capture. Capture requires L6max firmware and isolated state (`--volatile` or a
disposable `--state-dir`). The GIF contains the emulator LCD framebuffer,
scaled with nearest-neighbor pixels, and does not require ffmpeg.

## Capture the live emulator GUI without a display server

`capture gui` boots an isolated L6max guest, samples its live panel indicators
and LCD, then renders the full emulator panel with GPUI's offscreen renderer.
It works without a window server and retains the sampled panel data next to the
PNG for inspection. Use a state directory built from a `state_spec` when a
specific mixer value or indicator state is needed:

```sh
cargo run --offline --locked -p l6test -- capture gui \
  --out /tmp/l6max-panel.png --state-dir /tmp/prepared-state \
  --wait-guest-ms 45000 --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-l6max
```

Use `--volatile` instead of `--state-dir` for a fresh guest. The command
refuses to overwrite the PNG or its `.panel.json` data file. The JSON includes
the sampled indicator levels and masks, analog positions and LCD framebuffer.

## TOML files

There are three related TOML inputs:

1. `smoke.local.toml` supplies a pinned QEMU and named local fixtures, optional
   supervised legacy executables, and expected failures. See
   [fixture and catalog configuration](../testing.md#fixture-and-catalog-toml).
2. `tests/smoke/catalog.toml` registers each case, model/suite membership,
   scenario or GUI package, prerequisites, and timeout.
3. Each `tests/smoke/scenarios/*.toml` file describes an ordered scenario.
   Fixture `state_recipe` points to this same scenario format and uses its
   device-preparation operations. The complete list of header, per-step, and
   action blocks is in the [scenario format reference](../scenario-format.md).
   Fixture `state_spec` instead declares the starting device directly; see
   [`l6nor` state specs](l6nor.md#state-specs).

Fixture and catalog configuration, scenario headers, and action-specific
fields are strict. Paths in local configuration are resolved relative to the
TOML file. Scenario includes are confined to their scenario directory.

### L6max stereo level-meter sweep

The `l6max-volume-meter-sweep` case uses the Pad 4 assignment already saved
on the SD card, presses pad 4, and records indicator PWM levels through a short
sweep and decay. It does not enumerate USB or assign the pad during the test.
Configure a dedicated `volume-meter` fixture with
`tests/smoke/states/l6max-volume-meter-ready.toml` and an SD image containing
a short stereo WAV with successive left-only and right-only level stages. A
`[[fixture.sound_pad]]` block prepares the Pad 4 assignment and one-shot mode on
the disposable copy. For example, add this to `smoke.local.toml`:

```toml
[[fixture]]
name = "volume-meter"
model = "l6max"
firmware_dir = "unpacked"
state_spec = "tests/smoke/states/l6max-volume-meter-ready.toml"
sd_image = "/path/to/meter-card.img"

[[fixture.sound_pad]]
pad = 4
file = "SOUND_PAD/PAD4/TONE660.WAV"
mode = "one-shot"
level = 59
```

This fixture omits `audio_device`, so QEMU uses no host audio backend. The state
spec sets the first-run settings and panel clock needed for a ready guest. The
scenario allows the audio engine to start after mixer readiness, verifies the
active Pad 4 assignment, and turns the analog output controls to maximum. It
then presses the pad and captures the indicator PWM levels during playback and
decay. Use a low-amplitude sweep to avoid saturating the meter with the controls
at maximum.
The default `sound-pad-playback` window is six seconds;
`playback_window_ms` can extend it to 120 seconds for long fixtures.

```sh
cargo run --offline --locked -p l6test -- run \
  --config smoke.local.toml --case l6max-volume-meter-sweep \
  --backend headless
```

The `volume-meter` fixture's `sd_image` must contain the test WAV at the
filename declared in `fixture.sound_pad`. Inspect `sound-pad-playback.json` in the
case logs.
For an offscreen screenshot, use the `capture gui` command described above.
To render a recorded indicator sample, place its `indicator_levels` in a full
panel snapshot JSON containing `indicators`, `switches`, `analog`, `powered`,
and the 1024-byte `display` as shown in a `capture gui` artifact, then pass it
to `l6max-gui --render-panel l6max --data panel.json --output meter.png`.
The scenario requires at least two distinct left meter levels of two or more
bars with the right side off, and the same for the right with the left off.
The JSON records decoded meter levels alongside the raw panel output. This checks channel isolation and does
not establish physical meter calibration.

### L6 stereo level-meter sweep

The `l6-volume-meter-playback` case uses the same stereo isolation WAV at
`SOUND_PAD/PAD1/TONE220.WAV`. Set `pad = 1` and that path in its
`[[fixture.sound_pad]]` block. Its `l6-volume-meter` fixture needs L6 firmware,
the L6 settings state spec, and that card. The scenario checks the active
assignment and one-shot mode, sets the four analog output controls to maximum,
plays the pad, and requires independent left and right meter responses without
USB setup. The report contains the per-sample
meter levels and panel output.

### L6 thresholded meter PWM

The `l6-level-meter-pwm` case drives the ten L6 1.10 meter accumulators at the
measured threshold values and samples all 192 raw panel matrix cells. It uses
a dedicated fresh state, no SD image, no USB or MIDI setup, and no host audio.
Add a `l6-meter-pwm` fixture to `smoke.local.toml` with L6 1.10 firmware,
`tests/smoke/states/l6-settings-defaults.toml`, and `gdb = true`. Run it with:

```sh
cargo run --offline --locked -p l6test -- run \
  --config smoke.local.toml --case l6-level-meter-pwm \
  --backend headless
```

The case requires at least one panel cell to show five distinct nonzero mean
duty readings. Inspect `l6-level-meter-pwm.json` for all cell means at each
input level. This tests the firmware-to-panel path with controlled accumulator
input; it does not qualify physical meter wiring or audio-input routing.
