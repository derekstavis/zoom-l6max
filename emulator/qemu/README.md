# QEMU model

Custom board and peripheral sources for the `l6max-dual` machine. Build against QEMU 11.1.2.

Initialize the pinned upstream source with
`git submodule update --init emulator/qemu/upstream`, then run
`cargo run -p qemu-build`. It stages the model in ignored `build-source/`,
leaving the upstream submodule clean.

- [Board build, model coverage, and diagnostics](../../docs/emulator/board.md)
- [Control and LED wiring](../../docs/emulator/controls.md)
- [Persistent storage](../../docs/emulator/storage.md)
- [SD and USB](../../docs/emulator/usb.md)
