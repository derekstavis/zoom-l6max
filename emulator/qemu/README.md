# QEMU model

Custom board and peripheral sources for the L6 and L6max simulator machines.
The `l6max-main` machine supports both models and detects the board profile from the
MAIN firmware reset vector. The `l6max-panel` machine has no MAIN image, so select its
profile with
`L6_MODEL=l6` or `L6_MODEL=l6max`. Build against QEMU 11.1.2.

Initialize the pinned upstream source with
`git submodule update --init emulator/qemu/upstream`, then run
`cargo run -p qemu-build`. It stages the model in ignored `build-source/`,
leaving the upstream submodule clean.

QEMU overlays under patches/ are applied to the staged source during builds.
The CoreAudio underrun fix, sub-millisecond poll timeouts, and a hardfloat
shortcut for single-precision multiplies by zero are maintained there, keeping
the pinned upstream submodule pristine.

- [Board build, model coverage, and diagnostics](../../docs/emulator/board.md)
- [Control and LED wiring](../../docs/emulator/controls.md)
- [Persistent storage](../../docs/emulator/storage.md)
- [SD and USB](../../docs/emulator/usb.md)
