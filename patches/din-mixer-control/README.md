# DIN mixer control

Zoom's manual documents a "Mixer Control via MIDI" setting (`Menu → Mixer →
Mixer Control via MIDI`, or the equivalent toggle in ZOOM L6 Editor): turning
it on is supposed to let a MIDI device connected to the physical MIDI IN/OUT
(DIN/TRS) jacks control the mixer, the same way the USB Mixer Control port
already does. The stock firmware never implements that for the IN direction:
the setting exists, toggles correctly, and gates an unrelated OUT-side debug
mirror, but the DIN UART receive parser has no code path that applies a
received message to the mixer. This patch adds it.

## What it changes

The DIN UART receive parser (`MidiUartRcvMsg`) already has exactly one
consumer of a just-completed message at the point this patch hooks: a
factory-jig Note On check, gated on a session state that is never true in
normal use. Right after that check, with the setting on, this patch calls the
USB Mixer Control port's own inbound handler with the same message, applying
it through the user's CC map and configured MIDI channel exactly as if it had
arrived on that port. The jig check's own behavior is untouched.

## Source layout

- `src/payload.rs`: the mixer-forward logic, called from the veneer below.
- `src/firmware.rs`: bindings to the original functions and message layout.
- `src/trampolines.rs`, `src/trampolines.s`: veneer that preserves every
  register around the call into Rust, then replays the one displaced
  instruction and resumes `MidiUartRcvMsg` exactly where it left off. Unlike
  `midi-sync`'s entry hook, this one splices into the middle of a function,
  so the veneer (not the Rust side) carries the register-safety burden.
- `linker/link.ld`: payload placement (`PAYLOAD_BASE` comes from the builder)
  and prohibition on reserved mutable RAM.
- `hooks.tsv`: hook address, length and expected byte hash.

The builder is [`firmware-patch`](../../tools/firmware-patch/src/main.rs). See
the [patch conventions](../README.md).

## Build

Supported input is the 1.10 main image with SHA-256
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`.
From the repository root, with an unpacked package:

```sh
rustup target add thumbv7em-none-eabihf
cargo run -p firmware-patch -- unpacked firmware-din-mixer-control-v113 --patch din-mixer-control
# All three patches in one image:
cargo run -p firmware-patch -- unpacked firmware-full-v113 \
  --patch knob-dialog --patch midi-sync --patch din-mixer-control
```

## Hook

| Address | Length | Replaces | Behavior |
| --- | --- | --- | --- |
| `0x8003e8ee` | 4 | One instruction (`movw r0,#0x90b4`) right after the DIN receive parser finishes a complete message and releases its lock, before the factory-jig check | Call the stock USB Mixer Control port handler (`0x8003f7c8`) with the same message when `mixerControlViaMidi` is on and the message is 3 bytes; always replay the displaced instruction and resume the parser |

The message buffer `MidiUartRcvMsg` builds (`&_DAT_80898938`, a scratch area
with the message's status byte at `+4` and its data bytes at `+5`/`+6`) uses
the same layout `0x8003f7c8` expects, confirmed by the existing jig check
already using that exact pointer. No new scratch buffer is needed.

Combining with `midi-sync` is safe and tested: `0x8003f7c8` is that patch's
own hook target, and it always falls through to the same stock handler for
anything but its own CC 127 broadcast request, so a forwarded DIN message is
treated identically whether or not `midi-sync` is present.

## Validation

```sh
cargo run -p firmware-patch -- unpacked firmware-din-mixer-control-v113 --patch din-mixer-control
cargo run -p din-editor-mixer-smoke -- --firmware-dir firmware-din-mixer-control-v113 --volatile
L6_DIN_MIXER_CONTROL_FIXED=1 cargo run -p din-editor-mixer-smoke -- \
  --firmware-dir firmware-din-mixer-control-v113 --volatile
cargo run -p patched-update-smoke -- firmware-din-mixer-control-v113/L6max.bin --volatile
```

`din-editor-mixer-smoke` runs the editor's own MIDI protocol (identity, global
setting dump, then the `mixerControlViaMidi` parameter change) to turn the
setting on exactly as the real application would, then sends a fader Control
Change into the DIN MIDI IN jack and checks the firmware-driven channel-1 GPIO
ring, with `L6_DIN_MIXER_CONTROL_FIXED=1` flipping its expectation from the
stock gap to this patch's fix. `patched-update-smoke` validates the package
through the full SD-card update-install flow (its own post-install check is
specific to `knob-dialog`, so it only fully passes when that patch is also
combined in).

Confirmed, both alone and combined with `knob-dialog` and `midi-sync`:

- The update package installs correctly: GPIO shutdown saves the install
  marker, the installed main image matches the built payload byte for byte,
  and boot/calibration/settings regions are preserved.
- A fader Control Change sent into the DIN MIDI IN jack, with Mixer Control
  via MIDI enabled through the real editor session, now moves the mixer
  (confirmed on channel-1 LEVEL), the same as the USB Mixer Control port.
- `midi-sync`'s own CC 127 state broadcast (80 replies) and `knob-dialog`'s
  notification behavior both still work unchanged in the combined build.
- General boot and input behavior (`input-smoke`) is unaffected.

## Limitations

- Only 3-byte Channel Voice messages are forwarded, matching what
  `0x8003f7c8` itself expects; System messages and realtime bytes are not.
- The forwarded message is subject to the same configured MIDI channel and CC
  map filtering the USB Mixer Control port already applies; nothing here
  changes that behavior or the firmware's other DIN IN behavior (it still also
  bridges unconditionally to `L6max MIDI I/O Port` on USB cable 0, regardless
  of this patch).
- Not run on an L6max. Update-package acceptance by the physical bootloader
  remains unverified.
- The hook splices into the DIN UART receive path, not a clean function entry;
  the veneer preserves every register, but the added call's stack depth in
  that context has not been formally analyzed, only exercised cleanly in the
  QEMU tests above. Emulator timing is not cycle-accurate to real silicon, so
  the added instructions' effect on interrupt/ISR latency under worst-case
  real-hardware conditions is unverified.
- `0x8003f7c8` can now be reached from both the USB Mixer Control port and the
  DIN jack. Combining with `midi-sync` is tested (see above), but near-
  simultaneous calls from USB and DIN into the same handler have not been
  stress-tested; if that handler is not reentrant-safe against two callers at
  once, a transient glitch is possible (the same code path already runs from
  USB alone in stock firmware, so this is added exposure, not a new failure
  mode).
