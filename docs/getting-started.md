# Getting started

[Documentation index](README.md)

This guide runs the stock firmware in the native emulator. Building a patch
is a separate workflow in the [knob patch guide](guides/knob-patch.md).

## Prerequisites

- A source checkout of this repository.
- Rust installed through rustup. `rust-toolchain.toml` selects
  `nightly-2026-08-05`; Cargo installs that toolchain through rustup as needed.
- A locally obtained stock **L6 1.10** (`L6.BIN`) or **L6max 1.10** (`L6max.bin`) update package.
  Firmware is not included in this repository.
- The pinned **QEMU 11.1.2** submodule, initialized with
  `git submodule update --init emulator/qemu/upstream`. The builder rejects
  other versions; an external source tree can still be supplied explicitly.
- QEMU's native build dependencies: a C compiler, Python, Ninja, GLib,
  pkg-config, and libfdt. Python is used by upstream QEMU's build system;
  the emulator host and tools are Rust.
- For the native macOS UI: Xcode and its **Metal Toolchain** component.
- On Linux: the packages listed in the [Linux guide](guides/linux.md), including
  `libasound2-dev` for the ALSA MIDI bridge.

macOS is the established development and validation environment. Linux builds
the QEMU board, host, tools and patches and runs them headless; see the
[Linux guide](guides/linux.md) for the verified scope. The separate SwiftUI
editor is a macOS feature. The QEMU builder adds `/opt/homebrew`
include/library paths on macOS only. The patch builder uses Homebrew LLVM when
present and otherwise the Rust toolchain's own `rust-lld`, `llvm-nm` and
`llvm-objcopy` (the `llvm-tools` component pinned in `rust-toolchain.toml`).

The first Cargo build needs network access for crates and the pinned GPUI
Git dependency. Use `--offline` only after dependencies are cached.

## 1. Provide the firmware

Place `L6.BIN` or `L6max.bin` in the repository root (or `firmware/`). The first
run unpacks it into `emulator/state/firmware/<package SHA-256>/`, once. To use
another location pass `--firmware-package FILE` or set `L6_FIRMWARE`. An already
unpacked set works with `--firmware-dir DIR`, and a directory named `unpacked/`
in the root is used as is. To unpack by hand:

```sh
cargo run -p l6fw -- unpack /path/to/L6.BIN unpacked
```

The tool creates a new directory with main and panel images, package records,
and a manifest. It rejects unsupported layouts and never edits the input.
See the [package guide](guides/firmware-packages.md) for supported hashes and
repacking.

## 2. The QEMU board

Nothing to do: when no QEMU is found, the first run of the UI, `l6max-host` or any
smoke tool builds it, and rebuilds when `emulator/qemu/hw/` changes. Concurrent
runs wait for one build. `--qemu PATH` or `L6_QEMU` selects your own build and
is never rebuilt. To build explicitly:

```sh
cargo run -p qemu-build
```

This fetches the pinned submodule if needed, copies the pinned source into the ignored `emulator/qemu/build-source/`
directory, installs the model, adds its Meson entry, and builds
`emulator/qemu/build-source/build/qemu-system-arm`. The submodule stays clean.
A standard QEMU binary does not contain this board. Re-run after changing the model.
`cargo run -p qemu-build -- /path/to/qemu-11.1.2` still builds an external tree
in place. Add `--minimal` to disable unrelated optional host dependencies,
as CI does. QEMU's configure step downloads its pinned build dependencies.

## 3. Launch the native UI

```sh
cargo run -p l6max-gui
```

The application owns QEMU and stops it when the window closes. The screen
contains firmware-rendered pixels, while native components reproduce the
panel controls and show modeled GPIO indicators.

On a fresh L6max device, the firmware asks for Date/Time and Battery Type
settings. Use **Up**, **Down**, and **Confirm** to select, edit, and confirm
fields, then select **OK**. The Date/Time screen initially selects OK. **Menu**
opens the firmware menu once setup is complete.

The L6 firmware also boots. The UI includes verified hit targets for its six
mutes, MONO and USB source keys, channel modes, COMP, EFX SEL, phantom power,
scenes, Record, TAP, and four sound pads. Play target 4 drives active-low
GPIO2.25 and submits runtime event `0x20`; the separately startup-sampled,
active-low GPIO1.9 input is the Power Switch. Playback and physical Play lamp
correlation remain unverified. The L6 has no screen, so
first-boot setup and menu navigation remain incomplete, and encoder changes
have no LCD readback. Some unassigned panel matrix coordinates and downstream
control effects remain under investigation.

- Scroll or drag a channel encoder to change its selected setting.
- On L6max, click the blue control-strip buttons to select LEVEL, PAN, EQ, or
  send mode, scroll or drag the five global knobs, and click switches.
- Hold **Power** for about 3.5 seconds and release to request firmware shutdown;
  click it again after the display goes blank to restart.

## Device state and media

The visual emulator creates a model-matched demo card on first persistent
launch: `emulator/state/sd-card.img` for L6max and
`emulator/state/sd-card-l6.img` for L6. It contains four generated sample WAVs,
an additional snare sample under Pad 1, and the locally supplied update
package. Existing cards are reused without overwriting their contents. The
macOS GUI routes SAI1 playback to CoreAudio by default. Headless runs can set
`L6_AUDIODEV='driver=coreaudio,id=l6-audio'`. Both models route firmware output to
the host backend using their SAI1 TDM stereo slots; see
[audio emulation](emulator/audio.md).

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

- **"no firmware found":** put `L6.BIN` or `L6max.bin` in the repository root, or pass
  `--firmware-package FILE` / set `L6_FIRMWARE`. Use `--firmware-dir DIR` for an
  extracted or patched image set. Inspect `emulator/logs/engine.stderr` if the
  panel appears without a running firmware screen.
- **"could not build QEMU":** the first run builds QEMU itself; install the
  packages in the [Linux guide](guides/linux.md) or the macOS prerequisites above,
  or pass `--qemu PATH`.
- **Metal build fails:** install Xcode's Metal Toolchain and ensure the intended
  Xcode installation is selected as the active developer directory.
- **QEMU build fails:** confirm the source version and native dependencies.
  On macOS, check the Homebrew paths assumed by the builder; on Linux, install
  the packages in the [Linux guide](guides/linux.md).
- **`cargo` cannot resolve the workspace offline:** the GPUI Git dependency has
  not been fetched yet. Run `cargo fetch --locked` once with network access.
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
