# Build the knob value patch

[Documentation index](../README.md)

This patch shows a timed value notification when a channel encoder turns.
The title follows the selected blue button. EQ uses table-backed dB and
frequency values; level/sends use control percentages; pan uses CENTER or
L/R position. The original mixer change runs immediately, before presentation.

Current builds report development version **1.12** (`0112`). The executable
payload is unchanged from the corrected payload exercised on an L6max.
Version 1.12 is a local patch marker, not an official manufacturer release.

## Build

Follow [getting started](../getting-started.md) first. The builder additionally
requires the Thumb Rust target and Homebrew LLVM at `/opt/homebrew`:

```sh
rustup target add thumbv7em-none-eabihf
cargo run -p firmware-patch -- unpacked firmware-knob-dialog-recorder-fix-v112
```

The builder validates the original main SHA-256 and displaced hook regions,
compiles the payload, and repacks with a regenerated checksum. The output
directory must be new. Its `L6max.bin` is the update package; its extracted
components can also run directly in the emulator. `patch.json` records hashes,
placement, hooks, and the version marker. `--update-version FOUR_DIGITS`
explicitly overrides the default marker.

## Run the patched images directly

```sh
cargo run -p l6max-gui -- /path/to/qemu-system-arm --firmware-dir firmware-knob-dialog-recorder-fix-v112 --volatile --no-sd
```

Complete first-boot setup, select a blue-strip setting, and turn a channel
encoder. This starts patched images directly and does not exercise installation.

## Exercise SD installation

Use a new SD image and an isolated persistent device:

```sh
cargo run -p sd-image -- emulator/state/knob-update-card.img --demo firmware-knob-dialog-recorder-fix-v112/L6max.bin
cargo run -p l6max-gui -- /path/to/qemu-system-arm --firmware-dir unpacked --state-dir emulator/state/knob-update-device --sd-image emulator/state/knob-update-card.img
```

Stock 1.10 offers the newer package at startup. Select **Execute**, hold
**Power** for about 3.5 seconds, release, then click Power after the screen
blanks. The persistent visual UI enables the replacement installer on power-on.
The main application then performs any required panel update itself.

This exercises the firmware's validation and update preparation plus a Rust
replacement for the absent main bootloader. It does not execute the original
bootloader. See the [update model and limits](../firmware/updates.md#replacement-installer-model).

## Validate and investigate

See [patch testing](../development/testing.md#firmware-patch) for unit,
concurrency, SD-install, and restart checks. The
[implementation reference](../../patches/knob-dialog/README.md) retains hook,
heap, synchronization, and placement details.

Useful background:

- [Patch bindings](../../patches/knob-dialog/src/firmware.rs): original routines reused.
- [Parameter values](../firmware/gui/parameter-values.md): units and formatting.
- [Notification lifetime](../firmware/gui/notification-lifecycle.md): resource
  ownership, expiry, and nonblocking mixer handling.
- [Dismissal triggers](../firmware/gui/popup-triggers.md): why idle recorder
  polling must preserve stock busy-hide behavior.

## Limits

- The patch is guarded for the investigated main revision only.
- The panel image stays original; audio processing is not modeled.
- Hardware-tested popup behavior does not prove every allocation failure,
  scheduling interleaving, or bootloader policy.
- Multiple patches are not automatically composable; see
  [patch conventions](../../patches/README.md).
