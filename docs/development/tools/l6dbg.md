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

Use `--volatile` or a disposable `--state-dir`. The guest can modify persistent
state. `--logs` must name a new directory so previous evidence is preserved.
Additional host engine options, such as `--sd-image`, `--timing`, and
`--trace-sd`, can follow the recipe path. Use `--help` for command usage.

## TOML recipe schema

Top-level fields:

| Field | Required | Meaning |
| --- | --- | --- |
| `version` | yes | Must be `1`. |
| `model` | yes | `"l6"` or `"l6max"`; must match selected firmware. |
| `main_sha256` | memory actions | Exact 64-digit hexadecimal MAIN image digest required to authorize memory reads. |
| `wall_timeout_s` | no | Whole-recipe wall limit, default `600`, valid range `1..3600`. |
| `step` | yes, repeated | Ordered `[[step]]` action blocks; between 1 and 1000. |

Each `[[step]]` has one required `action`. The action value is kebab-case and
the action’s fields are strict; unknown fields fail parsing.

| `action` | Fields | Result |
| --- | --- | --- |
| `wait` | `guest_ms` | Wait for the requested guest time (`1..600000` ms). |
| `press` | `target`, optional `hold_ms` (default `50`) | Press GPIO target `0..53`; hold is `1..60000` ms. |
| `encoder` | `channel`, `steps` | One-based channel (L6 `1..6`, L6max `1..8`); signed steps `-1000..1000`, excluding zero. |
| `snapshot` | `name` | Save guest time, indicator rows/rings, and display text/image observations. |
| `trace` | `name`, `contains`, optional `limit` (default `100`) | Record up to `limit` matching trace lines; `contains` is a nonempty substring. |
| `memory` | `name`, `address`, `length` | Read MAIN memory; requires `main_sha256`, length `1..4096`, and a non-overflowing address range. |

Observation names (`snapshot`, `trace`, `memory`) must be unique and contain
only letters, digits, `_`, and `-`. Recipe output includes `diagnostic.json`,
per-observation artifacts, and raw QEMU logs in the new log directory.

## Focused probes

The same binary retains low-level probes that do not use recipe reporting:

```sh
cargo run --offline --locked -p l6dbg -- input --volatile --qemu /path/to/qemu-system-arm
cargo run --offline --locked -p l6dbg -- midi --volatile --qemu /path/to/qemu-system-arm
cargo run --offline --locked -p l6dbg -- popup-trigger --volatile --qemu /path/to/qemu-system-arm --firmware-dir /path/to/patched-l6max
cargo run --offline --locked -p l6dbg -- uart-startup --volatile --qemu /path/to/qemu-system-arm --firmware-dir /path/to/l6
```

`input` selects recorder, sound-pad, GPIO, and audio probes through its
`L6_*` environment variables. `midi` selects DIN/editor dispatch and forwarding
probes and requires macOS with CoreMIDI. These existing probes keep their own
output behavior; use `l6test` for repeatable assertions.

`popup-trigger` collects a focused L6max popup/callback observation and writes
`popup-trigger.json`. `uart-startup` traces L6 startup, panel/DIN input and
selected audio paths; options include `--startup-only`, `--exercise-inputs`,
`--force-panel-parser` (requires `--exercise-inputs`), `--connect-midi`,
`--trace-audio-path`, `--stop-after-handshake`, `--stop-after-mixer-lock`,
`--stop-after-mixer-callback-dispatch`, `--step-audio-profile-write`, and
`--release-audio-profile-lock`. Both accept host engine options. The UART probe
requires L6 firmware and writes `uart-startup.json` with the selected log path.
