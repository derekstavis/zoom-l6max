# Getting started

[Documentation index](README.md)

This guide runs the stock firmware in the native emulator. Building a patch
is a separate workflow in the [knob patch guide](guides/knob-patch.md).

## Prerequisites

- A source checkout of this repository.
- Rust installed through rustup. `rust-toolchain.toml` selects
  `nightly-2026-08-05`; Cargo installs that toolchain through rustup as needed.
- A locally obtained stock **L6max 1.10** update package, `L6max.bin`.
  Firmware is not included in this repository.
- The pinned **QEMU 11.1.2** submodule, initialized with
  `git submodule update --init emulator/qemu/upstream`. The builder rejects
  other versions; an external source tree can still be supplied explicitly.
- QEMU's native build dependencies: a C compiler, Python, Ninja, GLib,
  pkg-config, and libfdt. Python is used by upstream QEMU's build system;
  the emulator host and tools are Rust.
- For the native macOS UI: Xcode and its **Metal Toolchain** component.

macOS is the established development and validation environment. The host
has Linux shared-memory support, but full Linux frontend/build validation is
not established. CoreMIDI bridging and the separate SwiftUI editor are macOS
features. The QEMU builder currently adds `/opt/homebrew` include/library paths
on macOS; the patch builder also requires LLVM there.

The first Cargo build needs network access for crates and the pinned GPUI
Git dependency. Use `--offline` only after dependencies are cached.

## 1. Extract the firmware

Run from the repository root:

```sh
cargo run -p l6fw -- unpack /path/to/L6max.bin unpacked
```

The tool creates a new directory with main and panel images, package records,
and a manifest. It rejects unsupported layouts and never edits the input.
See the [package guide](guides/firmware-packages.md) for supported hashes and
repacking.

## 2. Build the QEMU board

```sh
git submodule update --init emulator/qemu/upstream
cargo run -p qemu-build
```

This copies the pinned source into the ignored `emulator/qemu/build-source/`
directory, installs the model, adds its Meson entry, and builds
`emulator/qemu/build-source/build/qemu-system-arm`. The submodule stays clean.
A standard QEMU binary does not contain this board. Re-run after changing the model.
`cargo run -p qemu-build -- /path/to/qemu-11.1.2` still builds an external tree
in place. Add `--minimal` to disable unrelated optional host dependencies,
as CI does. QEMU's configure step downloads its pinned build dependencies.

## 3. Launch the native UI

```sh
cargo run -p l6max-gui -- emulator/qemu/build-source/build/qemu-system-arm
```

The application owns QEMU and stops it when the window closes. The screen
contains firmware-rendered pixels, while native components reproduce the
panel controls and show modeled GPIO indicators.

On a fresh device, the firmware asks for Date/Time and Battery Type settings.
Use **Up**, **Down**, and **Confirm** to select, edit, and confirm fields, then
select **OK**. The Date/Time screen initially selects OK. **Menu** opens the
firmware menu once setup is complete.

- Scroll or drag a channel encoder to change its selected setting.
- Click the blue control-strip buttons to select LEVEL, PAN, EQ, or send mode.
- Scroll or drag the five global knobs to change their modeled ADC inputs.
- Click switches to toggle their retained position.
- Hold **Power** for about 3.5 seconds and release to request firmware shutdown;
  click it again after the display goes blank to restart.

## Device state and media

The visual emulator creates `emulator/state/sd-card.img` on first persistent
launch. It contains four generated sample WAVs and the locally supplied update
package. Existing cards are reused without overwriting their contents.
Audio output is unmodeled; browsing and assignment still exercise firmware.

Flash and RTC settings also persist in `emulator/state/`. For an independent
device, select another state directory:

```sh
cargo run -p l6max-gui -- /path/to/qemu-system-arm --state-dir emulator/state/demo-device
```

For a fresh disposable run without an SD card:

```sh
cargo run -p l6max-gui -- /path/to/qemu-system-arm --volatile --no-sd
```

`--volatile` discards device flash/RTC state; it does not make an explicitly
attached SD image disposable. Use a separate card image for tests that write
media. The [storage reference](emulator/storage.md) maps state files and
firmware regions.

## Headless mode

```sh
cargo run -p l6max-host -- --qemu /path/to/qemu-system-arm --volatile --seconds 10
```

`--seconds` uses guest time. Omit it to run until a signal or guest power-off.
The headless host does not automatically attach a demo card; pass
`--sd-image FILE` explicitly. See [host options](emulator/host.md).

## Troubleshooting

- **Panel appears without a running firmware screen:** pass the custom QEMU
  binary explicitly and check that `unpacked/` exists. Use `--firmware-dir DIR`
  for another extracted or patched image set. Inspect `emulator/logs/engine.stderr`.
- **Metal build fails:** install Xcode's Metal Toolchain and ensure the intended
  Xcode installation is selected as the active developer directory.
- **QEMU build fails:** confirm the source version and native dependencies.
  On macOS, check the Homebrew paths assumed by the builder.
- **State from another experiment affects startup:** use a new `--state-dir`
  or a `--volatile --no-sd` run.
- **Controls feel slower than the host pointer:** input transitions and debounce
  use guest time. See [input and timing](emulator/frontend.md#input-and-timing).
- **No audio or mounted USB disk:** these are current model limits. See
  [SD and USB coverage](emulator/usb.md#current-limits).

## Next steps

Try the [knob value patch](guides/knob-patch.md), explore
[firmware internals](firmware/README.md), or read
[contributing](../CONTRIBUTING.md) to work on the model or UI.
