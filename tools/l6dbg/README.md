# Diagnostic recipes

The maintained user and agent reference is
[`docs/development/tools/l6dbg.md`](../../docs/development/tools/l6dbg.md). It
documents all recipe fields and focused subcommands (`input`, `midi`,
`popup-trigger`, and `uart-startup`). This directory contains the checked-in
example recipe and its implementation.

`l6dbg` runs a headless emulator and records observations from a TOML
recipe. It is intended for investigating behavior; its successful exit means
the requested observations were collected, not that a smoke test passed.

```sh
cargo run --offline --locked -p l6dbg -- tools/l6dbg/example.toml \
  --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-firmware --logs /tmp/l6-diagnostic
```

Use `--volatile` or explicitly select a disposable `--state-dir`. A state
directory can be modified by the guest. `--logs` must name a new artifact
directory, so an earlier run is never overwritten. The recipe's `model` must match the
firmware. `--firmware-package` can select a local package when automatic setup
is enabled. Existing engine options also select timing, SD media and tracing.

Actions are executed in order:

- `wait`: wait for `guest_ms`, bounded by the recipe's wall timeout.
- `press`: press a model-specific input `target`, optionally with `hold_ms`;
  see the [control map](../../docs/emulator/controls.md) for target IDs.
- `encoder`: turn a one-based `channel` by signed `steps`.
- `snapshot`: record indicator rows, physical rings and a 64×32 ASCII display
  file. `display_available` distinguishes a retained frame from a blank
  fallback when no frame has arrived; L6 has no screen.
- `trace`: capture the last matching trace lines; `limit` bounds their count
  and only the last 2 MiB of the active trace is searched.
- `memory`: capture up to 4096 bytes from MAIN. This requires `main_sha256` in
  the recipe and fails if the selected or installed firmware does not match.

Observation names must be unique within a recipe. The report is
`diagnostic.json` in the selected log directory, with per-step
guest times and structured observations. If an action fails, the report retains
completed observations and the error. Raw QEMU logs remain in that directory.
Use disposable media and avoid committing generated artifacts.

The same command provides the existing deeper investigations as subcommands:

```sh
cargo run -p l6dbg -- input --volatile --qemu /path/to/qemu-system-arm
cargo run -p l6dbg -- midi --volatile --qemu /path/to/qemu-system-arm
```

`input` retains the recorder, sound-pad, GPIO and audio probes selected by
their existing `L6_*` environment variables. `midi` retains the DIN/editor
dispatch and forwarding probes; it currently requires macOS and CoreMIDI.
These subcommands retain their existing output and engine-option behavior;
they do not yet use the recipe report or its artifact-isolation checks.
Use `l6test` for regression assertions and qualification.
