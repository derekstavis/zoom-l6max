# Testing

[Documentation index](../README.md)

Choose checks for the behavior changed. Pure Rust unit tests do not require
manufacturer firmware or a running guest. Integration diagnostics require
locally extracted firmware and the custom QEMU binary.

Commands run from the repository root except the Swift package commands below.
The first build downloads dependencies; `--offline` is optional once cached.
The [CI workflow](ci.md) runs firmware-free checks and builds a macOS app;
firmware integration diagnostics remain local.

## Unit and build checks

```sh
cargo fmt --all --check
cargo test -p l6fw
cargo test -p firmware-patch
cargo test -p l6max-host --lib
```

For a workspace-wide check in the supported development environment:

```sh
cargo check --workspace --all-targets
cargo test --workspace
```

The GUI requires the native prerequisites from
[getting started](../getting-started.md). Tool crates can be selected with
`-p` independently of GPUI.

The pin-level indicator decoder has a standalone C check:

```sh
cc -std=c11 -Wall -Wextra -Werror emulator/qemu/tests/indicators.c -o /tmp/l6-indicators
/tmp/l6-indicators
```

## Integration setup

After extracting firmware, build or rebuild the model and select it explicitly:

```sh
git submodule update --init emulator/qemu/upstream
cargo run -p qemu-build
export L6_QEMU="$PWD/emulator/qemu/build-source/build/qemu-system-arm"
```

Diagnostics use the original `unpacked/` directory by default.
`--firmware-dir DIR` selects another image set. Use `--volatile` for disposable
flash/RTC state where supported, or a fresh `--state-dir DIR` when a test needs
persistence. An explicitly attached SD image can still be written under
`--volatile`; use a separate test card. Logs and screenshots stay ignored.

## Input, display, and LEDs

```sh
cargo run -p input-smoke -- --volatile --logs emulator/logs/input-smoke
cargo run -p control-smoke -- --volatile --logs emulator/logs/control-smoke
cargo run -p indicator-smoke -- --volatile --logs emulator/logs/indicator-smoke
cargo run -p l6max-gui -- --volatile --ui-input-smoke --logs emulator/logs/ui-input-smoke
```

The headless checks exercise GPIO input, guest-time release deadlines, exact
UART reports, display changes, and indicator outputs. The native check dispatches
through GPUI handlers without moving the host mouse, covering drag/scroll input,
button bounds, switches, and ADC knobs. See
[frontend validation](../emulator/frontend.md#validation) for detailed coverage.

## Firmware patch

Build the patch using the [patch guide](../guides/knob-patch.md), then run:

```sh
cargo run -p knob-dialog-smoke -- --firmware-dir firmware-knob-dialog-recorder-fix-v112 --volatile --logs emulator/logs/knob-dialog
cargo run -p popup-concurrency-smoke -- --firmware-dir firmware-knob-dialog-recorder-fix-v112 --volatile --logs emulator/logs/popup-concurrency
cargo build -p knob-dialog-smoke
cargo run -p patched-update-smoke -- firmware-knob-dialog-recorder-fix-v112/L6max.bin --volatile --logs emulator/logs/patched-sd-update
```

`patched-update-smoke` launches the sibling `knob-dialog-smoke` executable,
so build it first. The tests cover value formatting for all ten modes,
notification expiry and resource restoration, stock notification replacement,
idle recorder polling, contention without blocking mixer changes, and SD
installation/restart with preserved storage regions.

Debugger stops perturb timing. These checks establish modeled behavior and
forced interleavings, not physical hardware latency. See the
[patch validation reference](../../patches/knob-dialog/README.md#validation).

## Power, storage, and USB

Start with the relevant reference and its diagnostic commands:

| Change | Reference and checks |
| --- | --- |
| Power input and shutdown | [Power lifecycle](../emulator/power.md#validation), `power-smoke` |
| Flash/RTC persistence | [Storage](../emulator/storage.md#validation), `persistence-smoke` |
| Update preparation and panel programming | [Updates](../firmware/updates.md#emulator-diagnostic), `firmware-update-smoke` |
| SD detect/hotplug and USB endpoints | [SD and USB](../emulator/usb.md#validation), `storage-smoke`, `usb-smoke` |
| CoreMIDI bridge | [SD and USB](../emulator/usb.md#native-midi), `midi-smoke` |
| Guest USB file access | [Host file access](../emulator/usb.md#host-file-access), `usb-files` |
| Factory commands | [Factory protocol](../firmware/protocols/factory-midi.md#emulator-diagnostic), `factory-protocol-smoke` |

Some checks intentionally write media or calibration/settings storage. Their
reference pages describe those effects and required fixtures. Each card image
must have only one running QEMU owner.

## Editor

```sh
cd editor/Packages/L6Kit
swift test
```

With a running emulator exposing CoreMIDI ports through `--usb`:

```sh
L6_LIVE=1 swift test --filter Live
```

See [editor tests and limits](../editor.md#tests). The simulated-device suite
and live firmware suite establish different kinds of evidence.
