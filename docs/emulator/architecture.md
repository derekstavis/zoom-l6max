# L6max emulator

[Documentation index](../README.md)

The emulator UI and host tools are Rust. The native GPUI application starts
both firmware images in one QEMU process, receives completed display frames
through anonymous shared memory, and sends input over inherited sockets.

## Build and run

Commands run from the repository root. The root Cargo workspace and toolchain
are shared by the emulator and [standalone tools](../development/tools.md).

Build a custom QEMU binary from the pinned QEMU 11.1.2 submodule:

```sh
# Run from the repository root.
git submodule update --init emulator/qemu/upstream
cargo run -p qemu-build
```

QEMU's upstream configure/build system still requires its normal native build
dependencies, including Python, Ninja, a C compiler, and libfdt. No Python
project or Python launcher is used by this repository.

Start the visual emulator:

```sh
cargo run -p l6max-gui -- emulator/qemu/build-source/build/qemu-system-arm
```

The visual UI creates a persistent SD card with four sample WAVs and the local
firmware package on first launch. It reuses that card, and completes prepared
firmware updates on Power-on using the replacement installer. Hold the native
Power button to shut down and click it again to start. See the
[native UI run instructions](frontend.md#run) for the menu sequence,
custom cards, and `--no-sd`.

Run the same device engine without GPUI:

```sh
cargo run -p l6max-host -- --qemu /path/to/qemu-system-arm --volatile --seconds 10
```

Without `--seconds`, the host runs until Ctrl-C, SIGTERM, or guest power-off.
The duration uses the guest clock. Headless media selection is explicit:
pass `--sd-image FILE` and `--update-bootloader` when needed. See the
[host documentation](host.md) for lifecycle and defaults.

For a separate low-level diagnostic run with scripted buttons, traces, RAM probes,
and a framebuffer snapshot:

```sh
cargo run -p qemu-run -- --qemu /path/to/qemu-11.1.2/build/qemu-system-arm --seconds 2
```

See [native UI notes](frontend.md) and
[QEMU bring-up](board.md) for input mappings and model limitations.
Device flash and RTC state persist in `emulator/state/`. Use `--state-dir DIR`
for an isolated device or `--volatile` for a fresh disposable run. The
[storage map](storage.md) describes the files and NOR regions.

## Crate boundaries

```text
emulator/
  host/   # l6max-host library + headless binary
  gui/    # l6max-gui binary, GPUI components and assets
  qemu/   # Custom QEMU board and peripheral models
tools/
  l6fw/   # Package library + unpack/repack binary
  ...     # One crate per CLI; shared diagnostic helpers in diagnostics/
```

The GUI depends on the host library and owns an engine instance. The headless
binary owns the same engine without creating a window. QEMU remains a child
process in both cases; the host and GUI are separate binaries, not separate
communicating processes.

The host owns QEMU lifetime, sockets, anonymous shared memory, input, LCD frame
publication, persistence, and USB services. GPUI, UI components, assets, pointer
gestures, and visual demo defaults live in the GUI crate. Pure package validation
belongs to `l6fw`; demo card creation belongs to `sd-image`. Both can be built
without GPUI. Emulator diagnostics use the host library directly.

```sh
cargo check --offline --workspace --all-targets
cargo test --offline --workspace
cargo build --offline -p l6max-host
cargo build --offline -p l6max-gui
```
