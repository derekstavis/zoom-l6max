# `l6dbg`

`l6dbg` performs targeted observations in a disposable headless guest. A
successful diagnostic run means its requested observations were collected; it
does not mean the firmware passed a qualification suite.

## Recipe command

```sh
cargo run --offline --locked -p l6dbg -- tools/l6dbg/example.toml \
  --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-firmware --logs /tmp/l6-diagnostic
```

Checked-in recipes in `tools/l6dbg/recipes/` record black-box observations
that documentation cites, such as
[`usb-modes-l6max.toml`](../../../tools/l6dbg/recipes/usb-modes-l6max.toml)
for the [L6max USB modes](../../emulator/usb.md#l6max-usb-modes) and
[`heap-load-l6max.toml`](../../../tools/l6dbg/recipes/heap-load-l6max.toml)
for [heap headroom](../../../patches/README.md#heap-headroom). A
first-boot recipe confirms setup with two `recorder.confirm` presses before
USB starts.

Use `--volatile` or a disposable `--state-dir`. The guest can modify persistent
state. `--logs` must name a new directory so previous evidence is preserved.
Additional host engine options, such as `--sd-image` and
`--trace-sd`, can follow the recipe path. Use `--help` for command usage.

## TOML recipe schema

Top-level fields:

| Field | Required | Meaning |
| --- | --- | --- |
| `version` | yes | Must be `1`. |
| `model` | yes | `"l6"` or `"l6max"`; must match selected firmware. |
| `main_sha256` | memory actions | Exact 64-digit hexadecimal MAIN image digest required to authorize memory reads and writes. |
| `wall_timeout_s` | no | Whole-recipe wall limit, default `600`, valid range `1..3600`. |
| `step` | yes, repeated | Ordered `[[step]]` action blocks; between 1 and 1000. |

Each `[[step]]` has one required `action`. The action value is kebab-case and
the action’s fields are strict; unknown fields fail parsing.

| `action` | Fields | Result |
| --- | --- | --- |
| `wait` | `guest_ms` | Wait for the requested guest time (`1..600000` ms). |
| `audio-input-transport` | `command`, optional `position_ms` | Play, pause, stop or seek prepared audio input; waits for the emulator's acknowledgement. `command` is `play`, `pause`, `stop`, or `seek`. A nonnegative `position_ms` is allowed only for seek. |
| `press` | `target` or `control`, optional `hold_ms` (default `50`) | Press GPIO target `0..53`, or the semantic control name scenarios use, such as `recorder.confirm`; hold is `1..60000` ms. |
| `button` | `target` or `control`, `pressed` | Hold (`true`) or release (`false`) the input across subsequent steps. A release requires an earlier hold in the same recipe; a hold may remain active until the guest exits. |
| `encoder` | `channel`, `steps` | One-based channel (L6 `1..6`, L6max `1..8`); signed steps `-1000..1000`, excluding zero. |
| `snapshot` | `name` | Save guest time, indicator rows/rings, a compact display preview and the full display frame when available. |
| `trace` | `name`, `contains`, optional `limit` (default `100`) | Record up to `limit` matching trace lines; `contains` is a nonempty substring. |
| `memory` | `name`, `address`, `length` | Read MAIN memory; requires `main_sha256`, length `1..4096`, and a non-overflowing address range. |
| `memory-write` | `name`, `address`, `bytes` | Write `1..256` bytes to MAIN RAM (`0x80000000..0x82000000`), verify the readback, and report both values. Requires `main_sha256` and `--gdb-socket`; use only with a disposable guest. |
| `heap` | `name` | L6max only. Report the FreeRTOS heap size, free bytes, lowest free bytes since boot, and allocation and free counts. Needs no `main_sha256`: it checks the 1.10 allocator code in guest memory instead, so it also runs on patched firmware selected with `--patch`. |
| `usb-descriptors` | `name`, optional `guest_timeout_ms` (default `60000`) | Wait for the firmware's USB controller, enumerate and configure the device, then read every referenced string plus the device qualifier, other-speed configuration and BOS descriptors. Saves `NAME-device.bin` and `NAME-config.bin` and reports every descriptor decoded, with the endpoint numbers used in each direction. Optional descriptors the device stalls are reported as errors. |
| `usb-control` | `name`, `setup` | Send one 8-byte control request, IN or OUT without a data stage, and report the data or the error, such as a stall. |
| `usb-midi` | `name`, optional `cable` (default `0`), `send` bytes, `listen_ms` (default `1000`) | Send MIDI bytes as USB MIDI packets on the configured MIDI OUT endpoint, then collect device-to-host messages for up to `listen_ms` guest milliseconds. A device detach, such as a firmware USB mode change, ends the window and reports `detached`. Needs an earlier `usb-descriptors` step. |

The transport controls prepared channel files in `L6_AUDIO_INPUT_DIR`, or in
the engine's default audio-input directory. Prepare them using the emulator's
audio-input workflow; the transport does not create or assign files. See
[audio input](../../emulator/audio.md#input) for the file and routing model.

Observation names (`snapshot`, `trace`, `memory`, `memory-write`, `heap` and
the USB actions) must be unique and contain
only letters, digits, `_`, and `-`. Recipe output includes `diagnostic.json`,
per-observation artifacts, and raw QEMU logs in the new log directory.
For an available display, `display_raw` names a 1,024-byte `.oled` file with
the same native 128×64 packed frame used by `PanelData`; it is null when no
display frame has been published. The existing `.txt` file remains a 64×32
preview, and a retained static frame is valid. Pixel `(x, y)` is bit
`(63 - y) % 8` of byte `((63 - y) / 8) * 128 + x`, for zero-based screen
coordinates. The raw file is a diagnostic observation, not a timing or
physical-display qualification.

For example, with Python and Pillow installed, convert a raw frame to a PNG:

```sh
python3 - /path/to/first.oled /tmp/first.png <<'PY'
from pathlib import Path
from PIL import Image
import sys

frame = Path(sys.argv[1]).read_bytes()
if len(frame) != 1024:
    raise ValueError("expected a 1,024-byte OLED frame")
image = Image.new("L", (128, 64))
for y in range(64):
    for x in range(128):
        bit = (frame[((63 - y) // 8) * 128 + x] >> ((63 - y) % 8)) & 1
        image.putpixel((x, y), 255 * bit)
image.save(sys.argv[2])
PY
```

`memory-write` is intended for controlled firmware-input probes such as a PWM
level sweep when rotary timing would add unrelated variation. Keep writes
hash-scoped, confined to MAIN RAM, and limited to a disposable state directory.

## Focused probes

The same binary retains low-level probes that do not use recipe reporting:

```sh
cargo run --offline --locked -p l6dbg -- input --volatile --qemu /path/to/qemu-system-arm
cargo run --offline --locked -p l6dbg -- midi --volatile --qemu /path/to/qemu-system-arm
cargo run --offline --locked -p l6dbg -- popup-trigger --volatile --qemu /path/to/qemu-system-arm --firmware-dir /path/to/patched-l6max
```

`input` selects recorder, sound-pad, GPIO, and audio probes through its
`L6_*` environment variables. `midi` selects DIN/editor dispatch and forwarding
probes and requires macOS with CoreMIDI. These existing probes keep their own
output behavior; use `l6test` for repeatable assertions.

### Input probe options

The `input` probe accepts these environment variables:

| Variable | Purpose |
| --- | --- |
| `L6_MAIN_BUTTON_LED_TRACE=1` | Capture MAIN button and panel LED observations. |
| `L6_MAIN_GPIO_TRACE=1` | Include MAIN GPIO read observations. |
| `L6_MAIN_GPIO_OUTPUT_TRACE=1` | Capture MAIN GPIO output changes. |
| `L6_REQUIRE_PAD_FILES=1` | Require the configured pad WAVs to enumerate and be assigned. |
| `L6_PAD_LED_PROBE_ONLY=0..3` | Limit Editor playback to one zero-based pad. |
| `L6_PAD_LED_SKIP_PLAYBACK=1` | Skip Editor-driven playback. |
| `L6_PAD_GPIO_PROBE_ONLY=0..3`, `all`, or `none` | Select which sound-pad GPIO targets are exercised. |
| `L6_PAD_ASSIGNMENT_TIMEOUT_MS` | Assignment timeout in milliseconds, from 1 to 120000; default 30000. |
| `L6_SCREEN_SETTLE_MS`, `L6_BOOT_SETTLE_MS` | Adjust screen-capture and startup wait times. |

For a repeatable playback assertion without Editor or USB setup, use the
sound-pad scenario and a card spec's `[[sound_pad]]` in [l6test](l6test.md).

`popup-trigger` collects a focused L6max popup/callback observation and writes
`popup-trigger.json`. It accepts host engine options.
