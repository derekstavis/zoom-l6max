# Standalone tools

[Documentation index](../README.md)

Each CLI is its own Cargo package with a binary of the same name. Run from the
repository root using `cargo run -p <name> -- <arguments>`. These crates do not
depend on GPUI. The workspace lockfile and toolchain apply to all tools.

## Firmware and media

```sh
cargo run -p l6fw -- unpack L6max.bin unpacked
cargo run -p l6fw -- repack unpacked L6max-repacked.bin --recompute-checksum
cargo run -p firmware-patch -- unpacked firmware-knob-dialog-new --update-version 0112
cargo run -p sd-image -- emulator/state/demo.img --demo L6max.bin
cargo run -p sd-image -- emulator/state/folder-card.img --directory /path/to/sd-folder
```

- `l6fw`: package unpack/repack CLI and reusable fixed-layout package library.
- `firmware-patch`: guarded knob notification payload builder and repacker.
- `sd-image`: empty/demo cards and directory snapshots, with reusable media builders.
- `macos-package`: firmware-free macOS app/ZIP assembly with relocated native dependencies.
- `qemu-build`: stage and build the pinned QEMU submodule with the custom board;
  an explicit external QEMU 11.1.2 source path still builds in place.
- `qemu-run`: low-level scripted diagnostic runner using QEMU monitors.
- `usb-files`: list, download, upload, and round-trip files through modeled USB storage.
- `gui-probe`: run scripted guest buttons and capture display state through the host.
- `cpu-profile`: measure idle QEMU CPU use and guest clock progression.

## Emulator diagnostics

Each diagnostic below has its own crate and binary, and uses `l6max-host`:

```sh
cargo run -p input-smoke -- --volatile --qemu /path/to/qemu-system-arm
cargo run -p knob-dialog-smoke -- --volatile --firmware-dir firmware-knob-dialog-recorder-fix-v112
```

- `input-smoke`, `control-smoke`, `indicator-smoke`: input and GPIO/LED behavior.
- `power-smoke`, `persistence-smoke`: device shutdown and persistent state.
- `storage-smoke`, `usb-smoke`, `midi-smoke`: SD and USB services.
- `factory-protocol-smoke`: firmware's factory MIDI protocol.
- `firmware-update-smoke`, `patched-update-smoke`: update preparation, replacement
  installation, and restart checks.
- `knob-dialog-smoke`, `popup-concurrency-smoke`, `popup-trigger-probe`: patched
  notification behavior and recorder polling diagnostics.

`diagnostics/` is a library of shared GDB helpers used by diagnostic binaries.
Visual input checks remain in the GUI crate because they exercise GPUI event
dispatch. See [the GUI documentation](../emulator/frontend.md) and
[patch validation commands](../../patches/knob-dialog/README.md#validation).

Run diagnostics with explicit disposable state where required. Tools that modify
packages or create media require new output paths; proprietary firmware and
generated artifacts remain excluded from Git.
