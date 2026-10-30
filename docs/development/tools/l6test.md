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
