# Firmware behavior and compatibility

[Documentation index](../README.md)

The update package contains two executable images. Investigation pointed to
the main MCU as owner of mixer state, storage, USB, and LCD rendering; the
panel MCU scans controls and drives the LED matrix. Their internal UART carries
input reports and indicator metadata. The emulator connects both chips in one
QEMU process with separate address spaces and a shared virtual clock.

## References

- [Controls and indicators](../emulator/controls.md): physical input/output wiring.
- [QEMU board](../emulator/board.md): modeled peripherals and UART/display boundaries.
- [Updates](updates.md): package checks, storage handoff, panel programming, installer limits.
- [Editor MIDI](protocols/editor-midi.md) and [factory MIDI](protocols/factory-midi.md): wire formats, commands, side effects, and verification limits.
- [Parameter values](gui/parameter-values.md): units and knob-popup formatting.
- [Notification lifetime](gui/notification-lifecycle.md): patch ownership, expiry, and nonblocking mixer handling.
- [Dismissal behavior](gui/popup-triggers.md): idle polling and regression checks.
- [Patch implementation](../../patches/knob-dialog/README.md): sources, guarded hooks, and validation.

## Supported revision

The original main image has SHA-256
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`.
The supplied stock package is L6max 1.10; main execution starts at `0x80000000`.
Addresses and patch hooks are revision-specific, not established for L6 or
other releases. Application names are assigned from behavior rather than
recovered symbols. Reviewed FreeRTOS names are used in patch bindings; the
exact kernel release and configuration remain unproven.

## Evidence boundaries

Public references describe behavior, compatibility addresses, protocols, patch
contracts, and validation. Static findings, emulator results, and particular
hardware observations establish different kinds of evidence. A successful
emulator update uses a replacement installer and does not prove every check
in the absent manufacturer bootloader.
