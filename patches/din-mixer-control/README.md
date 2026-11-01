# Mixer control over MIDI jack

The L6max can already be controlled from a computer over MIDI via its USB
port — that's the "Mixer Control via MIDI" setting (`Menu → Mixer → Mixer
Control via MIDI`, or the matching toggle in ZOOM L6 Editor). The same
setting is supposed to also work over the mixer's physical MIDI IN jack, so a
standalone controller or sequencer can drive the mixer with no computer
involved. The stock firmware never actually implements that: the setting
exists and toggles fine, it just doesn't do anything when turned on for that
jack. This patch makes turning the setting on actually work for MIDI IN, the
same as it already does over USB.

(This patch is named "din" after the setting's usual MIDI terminology, but
the L6max's physical jack is a 3.5mm TRS connector, not a 5-pin DIN.)

## Model support

The DIN receive patch targets L6max. L6 has a different native route: its DIN
CC parser submits mapped records to a generic event queue, and the stock Main
class-2 handler applies queued values through the native channel setters. A
volatile QEMU DIN sweep verifies that path end to end: LPUART2 receives and
drains 384 bytes, Main consumes all 66 mapped events, native setters run, and
mixer RAM plus the channel-1 indicator change. L6 DIN mixer control is
provided by stock firmware and does not need this patch.

The native and browser builders select the L6max profile only for L6max
firmware. L6's native event route is documented in
[`development/testing.md`](../../docs/development/testing.md).

## How it works

Zoom's firmware toggles the setting correctly and gates an unrelated OUT-side
debug mirror, but the DIN UART receive parser has no code path that applies a
received message to the mixer. This patch adds it.

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

The builder is [`l6patch`](../../docs/development/tools/l6patch.md). See
the [patch conventions](../README.md).

## Build

Supported input is the 1.10 main image with SHA-256
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`.
From the repository root, with an unpacked package:

```sh
rustup target add thumbv7em-none-eabihf
cargo run -p l6patch -- unpacked firmware-din-mixer-control-v113 --patch din-mixer-control
# All three patches in one image:
cargo run -p l6patch -- unpacked firmware-full-v113 \
  --patch knob-dialog --patch midi-sync --patch din-mixer-control
```

## Hook

| Address | Length | Replaces | Behavior |
| --- | --- | --- | --- |
| `0x8003e8ee` | 4 | After the DIN receive parser finishes a complete message and releases its lock, before the factory-jig check | Call the stock USB Mixer Control port handler (`0x8003f7c8`) with the same message when `mixerControlViaMidi` is on and the message is 3 bytes; always replay the displaced instruction and resume the parser |

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
cargo run -p l6patch -- unpacked firmware-din-mixer-control-v113 --patch din-mixer-control
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case l6max-din-mixer-control-patched --model l6max
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case l6max-din-mixer-control --model l6max
```

The driver cases run the editor's identity request, global-setting dump, and
`mixerControlViaMidi` parameter change before testing the USB reference route,
DIN input, and mute-echo suppression. Configure a `din-mixer-patched` fixture
for the patched case; the stock case uses the regular prepared L6max fixture.
Both cases still need live qualification. The separate `patched-update` case
validates package installation and its configured post-install behavior.

Confirmed on L6max, both alone and combined with `knob-dialog` and `midi-sync`:

- The update package installs correctly: GPIO shutdown saves the install
  marker, the installed main image matches the built payload byte for byte,
  and boot/calibration/settings regions are preserved.
- A fader Control Change sent into the DIN MIDI IN jack, with Mixer Control
  via MIDI enabled through the real editor session, moves the mixer
  (confirmed on channel-1 LEVEL), the same as the USB Mixer Control port.
- `midi-sync`'s own CC 127 state broadcast and `knob-dialog`'s
  notification behavior both still work unchanged in the combined build.
- General boot and input behavior is covered by the `l6max-input-smoke` driver case.

## Limitations

- Only 3-byte Channel Voice messages are forwarded, matching what
  `0x8003f7c8` itself expects; System messages and realtime bytes are not.
- The forwarded message is subject to the same configured MIDI channel and CC
  map filtering the USB Mixer Control port already applies; nothing here
  changes that behavior or the firmware's other DIN IN behavior (it still also
  bridges unconditionally to `L6max MIDI I/O Port` on USB cable 0, regardless
  of this patch).
- The patch does not apply to L6, whose stock firmware already routes mapped
  DIN Control Changes through its native event queue. Update-package
  acceptance by the physical bootloader remains unverified.
- The hook splices into the DIN UART receive path, not a clean function entry;
  the veneer preserves every register, but the added call's stack depth in
  that context has not been formally analyzed, only exercised cleanly in the
  QEMU tests above. Emulator timing is not cycle-accurate to real silicon, so
  the added instructions' effect on interrupt/ISR latency under worst-case
  real-hardware conditions is unverified.
- `0x8003f7c8` can be reached from both the USB Mixer Control port and the
  DIN jack. Combining with `midi-sync` is tested (see above), but near-
  simultaneous calls from USB and DIN into the same handler have not been
  stress-tested; if that handler is not reentrant-safe against two callers at
  once, a transient glitch is possible (the same code path already runs from
  USB alone in stock firmware, so this is added exposure, not a new failure
  mode).
