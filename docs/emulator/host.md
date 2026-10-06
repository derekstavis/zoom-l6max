# Device host

[Documentation index](../README.md)

`l6max-host` contains the QEMU engine and device services, with no GPUI dependency.
Its library is shared by the visual application and firmware diagnostics. Its
binary starts the same dual-chip QEMU model without a graphical application.

From the repository root:

```sh
cargo run -p l6max-host -- --qemu /path/to/qemu-system-arm --volatile --seconds 10
```

`--seconds` is a guest-time limit. Omit it to run until Ctrl-C, SIGTERM, or guest
power-off. On exit the engine shuts down and reaps its child process. Frames are
consumed through the same shared-memory display interface used by the GUI.

Shared engine options include `--firmware-dir`, `--logs`, `--state-dir`,
`--volatile`, `--sd-image`, `--no-sd`, `--update-bootloader`, `--usb`,
`--trace-sd`, `--timing paced|adaptive`, and `--gdb-socket`. Use `--help` for
the option list. `L6_QEMU` supplies the QEMU executable when not passed explicitly.

Development defaults resolve from the source checkout, independent of the
working directory: `unpacked/`, `emulator/logs/`, and `emulator/state/`. Pass explicit
paths when using a binary away from that checkout. The host does not automatically
create or attach a demo card; use `sd-image --demo` and `--sd-image` to select one.

## Modules

- `engine`: process startup, inherited IPC, persistent storage, and shutdown.
- `protocol`, `input`, `qmp`: input/event protocol and QEMU management.
- `shared_memory`, `display`: anonymous shared RAM and completed LCD transfers.
- `controls`, `indicators`: hardware control and GPIO/LED mappings.
- `headless`: guest-time helpers used by diagnostic tools.
- `update_bootloader`: replacement SD installer for the unavailable bootloader.
- `usb`, `usb_midi`, `usb_storage`: modeled USB device access and host services.

Known limitations and the replacement installer's scope are documented in
the [emulator overview](architecture.md) and [QEMU model documentation](board.md).
