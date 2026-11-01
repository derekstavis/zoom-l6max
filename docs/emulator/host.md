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
`--midi`, `--trace-sd`, `--timing paced|adaptive|split` (default `paced`), `--gdb-socket`, and
`--control-socket PATH`. `--usb`
and `--midi` only pick the connected state at startup — both can be
connected or disconnected at any time while running; see
[Hotplug](usb.md#hotplug). Use `--help` for
the option list. `L6_QEMU` supplies the QEMU executable when not passed explicitly.

The GUI also accepts `--control-socket PATH`. This opt-in Unix socket lets
[`l6ctl`](../development/tools/l6ctl.md) send button, encoder, and
analog-knob commands to the running guest. The socket is restricted to the
current user and is removed when the engine stops. The headless host has no
command-line interaction loop; use `l6dbg` for scripted headless actions.

Development defaults resolve from the source checkout, independent of the
working directory: `unpacked/`, `emulator/logs/`, and `emulator/state/`. Pass explicit
paths when using a binary away from that checkout. The host does not automatically
create or attach a demo card; use `l6fs card demo` and `--sd-image` to select one.

The GUI keeps persistent device state separate for L6 and L6max under
`emulator/state/models/l6/` and `emulator/state/models/l6max/`. This includes
MAIN NOR, panel flash and RTC state. When opening an existing state directory,
the firmware already installed in its MAIN NOR keeps using that legacy state;
the other model is started in its own model-specific directory. SD-card images
and selections are also model-specific.

## Modules

- `engine`: process startup, inherited IPC, persistent storage, and shutdown.
- `protocol`, `input`, `qmp`: input/event protocol and QEMU management.
- `shared_memory`, `display`: anonymous shared RAM and completed LCD transfers.
- `controls`, `indicators`: hardware control and GPIO/LED mappings.
- `headless`: guest-time helpers used by diagnostic tools.
- `update_bootloader`: replacement SD installer for the unavailable bootloader.
- `usb`, `usb_midi`, `usb_storage`: modeled USB device access and host services.
- `host_midi`: the `HostMidi` trait and its CoreMIDI, ALSA and unavailable back ends.
- `sector_device`: the `SectorDevice` trait (USB mass storage, or an image file) and the
  seekable FAT volume view over it, used by `l6fs usb` and tests.

Known limitations and the replacement installer's scope are documented in
the [emulator overview](architecture.md) and [QEMU model documentation](board.md).
