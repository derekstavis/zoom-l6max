# Firmware behavior and compatibility

[Documentation index](../README.md)

The emulator runs MAIN and panel firmware in separate address spaces. MAIN handles mixer state, storage, USB, and L6max display
rendering; the panel handles control scanning and indicators. Profiles and
patches are specific to the model and supported firmware revision. See
[model compatibility](../guides/model-compatibility.md) and
[firmware packages](../guides/firmware-packages.md).

## Supported revision

The patch profiles target their declared L6 or L6max 1.10 MAIN image and
validate the image and hook guards before building. See the
[model compatibility guide](../guides/model-compatibility.md) for model scope.

## References

- [Controls and indicators](../emulator/controls.md): model-specific inputs and lamps.
- [QEMU board](../emulator/board.md): modeled peripherals and display boundaries.
- [Updates](updates.md): package handling, installer behavior, and limits.
- [Editor MIDI](protocols/editor-midi.md): message formats and session behavior.
- [Factory protocol scenario](protocols/factory-midi.md): emulator test setup and scope.
- [Mixer MIDI](protocols/mixer-midi.md): Control Change numbers and state requests.
- [Parameter values](gui/parameter-values.md): units and knob-popup formatting.
- [Notification lifetime](gui/notification-lifecycle.md): popup ownership and expiry.
- [Dismissal behavior](gui/popup-triggers.md): popup behavior during recorder refresh.
- [Patch implementation](../../patches/knob-dialog/README.md): sources, guards, and validation.

Emulator observations and physical-device observations have different scopes.
In particular, the emulator uses a replacement installer; successful emulator
installation does not establish every rule of the physical bootloader.
