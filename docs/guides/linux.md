# Linux

[Documentation index](../README.md)

The emulator host, QEMU board, firmware tools and patch builder run on Linux in
addition to macOS. macOS remains the reference environment for the native UI
and the app bundle. This page lists what was exercised on Linux and what was
not.

Commands run from the repository root. Package names are for Debian and
Ubuntu; the build was exercised on Ubuntu 26.04 x86_64.

## Prerequisites

| Purpose | Debian/Ubuntu packages |
| --- | --- |
| QEMU build | `build-essential pkg-config ninja-build python3 libglib2.0-dev libpixman-1-dev libfdt-dev` |
| ALSA MIDI bridge (needed to compile `l6max-host`) | `libasound2-dev` |
| Native UI build (GPUI Wayland backend: fonts, keyboard) | `libfontconfig-dev libxkbcommon-dev` |
| Native UI at run time (GPUI loads these dynamically) | `libwayland-client0 libxkbcommon0 libfontconfig1 libvulkan1`, plus a Vulkan driver such as `mesa-vulkan-drivers` |
| MIDI inspection | `alsa-utils` (`aconnect`, `amidi`) |

```sh
sudo apt install build-essential pkg-config ninja-build python3 \
  libglib2.0-dev libpixman-1-dev libfdt-dev libasound2-dev \
  libfontconfig-dev libxkbcommon-dev alsa-utils
```

Also needed, as on macOS: Rust through rustup (`rust-toolchain.toml` selects
the toolchain), the pinned QEMU submodule, and a locally obtained stock 1.10
package. Add the Thumb target to build patches:
`rustup target add thumbv7em-none-eabihf`.

`python3-venv` was not required: QEMU's configure installs its vendored Meson
wheel into a non-isolated environment.

## Build

`cargo run -p l6max-gui` (or any binary) builds QEMU and unpacks `L6max.bin` from
the repository root on first use; the steps below are for doing it explicitly.
`qemu-build` fetches the pinned QEMU submodule (shallow) when it is missing;
`git submodule update --init emulator/qemu/upstream` does the same by hand.

```sh
cargo run -p l6fw -- unpack /path/to/L6max.bin unpacked
cargo run -p qemu-build
cargo run -p l6max-host -- --qemu emulator/qemu/build-source/build/qemu-system-arm \
  --volatile --seconds 10
```

The first Cargo build needs the network: the workspace includes the GUI crate,
whose pinned GPUI Git dependency must be fetched (`cargo fetch --locked`)
before any `cargo` command, including ones for tools that do not use GPUI,
resolves. After the fetch, `--offline` works. The default QEMU lookup of
`l6max-host` also checks `emulator/qemu/build-source/build/qemu-system-arm`,
so `--qemu` may be omitted after `qemu-build`.

`cargo run -p l6max-gui -- emulator/qemu/build-source/build/qemu-system-arm`
starts the native UI. It compiles on Linux, but see the limits below.

## Patch builder

`firmware-patch` needs no LLVM installation or environment variables on Linux.
It links with `rust-lld` and extracts symbols and the flat binary with
`llvm-nm` and `llvm-objcopy`, all from the active Rust toolchain (the
`llvm-tools` component, which `rust-toolchain.toml` requests). Resolution order
for each tool is the `L6_LD_LLD`, `L6_LLVM_NM` or `L6_LLVM_OBJCOPY` variable, an
existing Homebrew LLVM path, then the toolchain's copy. The linker affects the
payload bytes: knob-dialog is 8,012 bytes with Homebrew LLVM and 8,004 with
`rust-lld`; both pass the same checks, but outputs are only byte-comparable
within one toolchain.

## MIDI ports

With `--usb`, the host creates one ALSA sequencer client, `L6max Emulator`,
with three duplex ports, named like the macOS CoreMIDI endpoints:

- `L6max MIDI I/O Port (Emulator)`
- `L6max Mixer Control Port (Emulator)`
- `for L6 Editor Port (Emulator)`

Ports exist while the guest is in normal (MIDI) USB mode and are removed in USB
File Transfer mode or on shutdown. Inspect and connect them with:

```sh
aconnect -l
amidi -l          # raw MIDI devices only; sequencer ports appear in aconnect
aconnect 'L6max Emulator:1' 'Your Synth:0'
```

Opening `/dev/snd/seq` requires membership of the `audio` group on many
distributions (`sudo usermod -aG audio "$USER"`, then log in again).

## Checks

```sh
cargo run -p firmware-patch -- unpacked firmware-knob-midi-v113 --patch knob-dialog --patch midi-sync
cargo run -p knob-dialog-smoke -- --qemu "$QEMU" --firmware-dir firmware-knob-midi-v113 --volatile
# USB-level MIDI check, independent of the host MIDI system:
L6_MIDI_DIRECT=1 L6_MIDI_SYNC=1 cargo run -p midi-smoke -- --qemu "$QEMU" \
  --firmware-dir firmware-knob-midi-v113 --volatile
# Through ALSA (needs /dev/snd/seq access):
L6_MIDI_SYNC=1 cargo run -p midi-smoke -- --qemu "$QEMU" --firmware-dir firmware-knob-midi-v113 --volatile
```

`QEMU` is the path of the built `qemu-system-arm`. Without `L6_MIDI_SYNC`,
`midi-smoke` verifies the knob CC and the host CC to LED path as on macOS. With
it, the check sends CC 127 and requires 80 Control Change replies, which only a
midi-sync image produces.

## AppImage

CI builds `L6max-Emulator-x86_64.AppImage` (see [CI and releases](../development/ci.md)).
Locally, after `qemu-build`:

```sh
cargo run -p linux-package -- emulator/qemu/build-source/build/qemu-system-arm \
  --debug --appimagetool /path/to/appimagetool --runtime /path/to/runtime-x86_64
```

Without `--appimagetool` it only assembles `emulator/dist/L6max-Emulator.AppDir`.
The AppImage bundles the GUI, the custom QEMU and the few libraries a typical
system may lack (GLib, PCRE2, libfdt, libatomic on Ubuntu). It leaves the C
library, graphics (Vulkan, GL, DRM), display-server and ALSA libraries to the
host, so it needs a Wayland desktop session (the X11 backend is not enabled), a Vulkan driver, and a C library
at least as new as the build image's. Firmware is never bundled; select your
package at first launch. User data lives in `$XDG_DATA_HOME/l6max-emulator`
(default `~/.local/share/l6max-emulator`). Every bundled library is traced to a
Debian package and listed in `usr/share/l6max-emulator/native-dependencies.json`;
packaging fails for a library of unknown origin.

## Native UI

`cargo run -p l6max-gui` needs a Wayland session (`WAYLAND_DISPLAY`, or
`$XDG_RUNTIME_DIR/wayland-0`) and exits with a message when there is none; the
X11 backend is not enabled. GPUI also requires the compositor to advertise a
`wl_seat`, so Weston's headless backend (no input devices) crashes it, while sway
with `WLR_BACKENDS=headless` works for automated checks. The build needs
`libfontconfig-dev` and `libxkbcommon-dev`, as GPUI does everywhere.

## Verified and not verified

Verified on Ubuntu 26.04 x86_64:

- QEMU 11.1.2 builds from the pinned submodule with the custom board.
- `l6fw`, `firmware-patch`, `l6max-host` and the GUI crate build; the host test
  suite, including the MIDI bridge and FAT-volume tests, passes.
- The macOS back end and `midi-smoke` type-check for `aarch64-apple-darwin`
  from Linux (`cargo check`; not linked or run).
- The headless host boots the stock firmware; `knob-dialog-smoke` passes on a
  patched image.
- `midi-smoke` through live ALSA ports passes on a knob+midi-sync image: knob CC,
  host CC to the LED ring, 80 replies to CC 127, and the ports disappear on
  shutdown. The same check passes in direct USB mode, and on the stock image
  the sync check receives 0 replies.
- `linux-package` builds the AppDir and an AppImage with pinned tools. Its checks
  show the bundled QEMU runs with only bundled and host libraries and lists
  `l6max-dual`, in the AppDir and in the extracted AppImage.

Not verified:

- The native UI opens under a headless sway (wlroots) compositor with software
  Vulkan, and shows the firmware screen on a real Wayland desktop. Not
  exercised: mouse and keyboard input, scaling, the aspect-ratio constraint
  (macOS-only code) and firmware import dialogs, and none of it from the
  AppImage.
- The CI Linux job and release publication have not run on a hosted runner;
  the first run may need package or version adjustments.
- Running the AppImage on other distributions, and X11-only sessions, which the Wayland-only build does not support.
- macOS regressions: macOS code paths were only type-checked, not run.
