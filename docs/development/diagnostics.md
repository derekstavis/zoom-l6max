# Diagnostics

Use diagnostics to investigate a specific runtime behavior and collect
observations. Use the [l6test driver](testing.md) when you need repeatable
assertions, regression coverage, or a qualification result. A diagnostic command
can exit successfully after collecting its observations; that alone does not
mean the behavior passed a test.

Read [current status](current-status.md) first, then select a diagnostic that
matches the question. Keep firmware, state, SD cards, and generated logs local.
Use a fresh `--state-dir` or `--volatile` for experiments that may change guest
state, and choose a new output directory for every run.

## TOML observation recipes

The `l6dbg` recipe runner starts a headless emulator and performs ordered
observations from TOML. Begin with the checked-in example:

```sh
cargo run --offline --locked -p l6dbg -- tools/l6dbg/example.toml \
  --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-firmware --logs /tmp/l6-diagnostic
```

The recipe chooses its model and records waits, button presses, encoder turns,
snapshots, trace excerpts, or hash-guarded MAIN memory captures. Results go to
`diagnostic.json`; raw engine logs and display artifacts are written to the
selected log directory. Memory reads require the recipe's expected MAIN image
hash. See the complete [`l6dbg` reference](tools/l6dbg.md), including every
TOML recipe block and action.

Use the recipe runner for targeted observations that can be expressed as a
short, reusable sequence. Do not use its successful exit as a substitute for
test-driver assertions.

## Focused input and MIDI probes

The same binary retains lower-level `input` and `midi` subcommands:

```sh
cargo run --offline --locked -p l6dbg -- input \
  --volatile --qemu /path/to/qemu-system-arm

cargo run --offline --locked -p l6dbg -- midi \
  --volatile --qemu /path/to/qemu-system-arm
```

`input` provides opt-in recorder, sound-pad, GPIO, audio, and debugger probes.
`midi` provides focused DIN/editor forwarding and dispatch probes and currently
requires macOS with CoreMIDI. These probes preserve their own output formats and
are not qualification tests; use the corresponding driver cases for standard
regression assertions.

## Other focused tools

The [tool index](tools.md) links to focused references for `qemu-run`,
`l6test probe gui`, `cpu-profile`, and `l6fs usb`. These tools are useful when a
focused probe is more convenient than a recipe. Keep regression assertions in
`l6test`.

For `l6test` cases that cover panel, MIDI, USB, update, storage, and patch
behavior, use the [test catalog and workflow](testing.md).

QEMU suites and probes use POSIX shared memory and inherited sockets. Ensure
the execution environment permits both before starting a guest.
