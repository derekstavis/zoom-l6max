# MIDI state sync patch

[Documentation index](../README.md)

Lets a MIDI controller on the mixer-control port ask the L6max for its current
mixer state, so motorized faders and encoder rings can match it. Live changes
already reach the controller through the stock Control Change output; this adds
the missing initial and on-demand state. Version marker `0113` is a local
experiment, not an official ZOOM release.

**Status:** built and simulated only. It has not run on an L6max or in QEMU.

## Use

1. Enable MIDI mixer control in the L6 Editor (or the [Zoomie editor](../editor.md))
   and note the MIDI channel and CC map; the defaults are in
   [mixer MIDI](../firmware/protocols/mixer-midi.md#controller-numbers).
2. Have the controller send Control Change **127**, value 127, on that channel
   when it connects, or from a button.
3. The L6max replies with 84 Control Change messages: eight channels of ten
   settings each, then 4 global toggles (CH 5 MONO x2, CH 6 MONO x2, USB 1/2,
   USB 3/4). Map them to fader travel, encoder rings, LEDs or any combination.

Send nothing else special: ordinary Control Change keeps working as before.

## Build

Follow [getting started](../getting-started.md) for the toolchain, then:

```sh
rustup target add thumbv7em-none-eabihf
cargo run -p firmware-patch -- unpacked firmware-midi-sync-v113 --patch midi-sync
cargo run -p firmware-patch -- unpacked firmware-knob-midi-v113 --patch knob-dialog --patch midi-sync
```

The second command builds the [knob value patch](knob-patch.md) and this one in
one image. Install either `L6max.bin` as described in the knob guide; the
output directory must be new. Without Homebrew LLVM the builder uses
the Rust toolchain's own `rust-lld`, `llvm-nm` and `llvm-objcopy`; no setup is
needed beyond `rustup` installing the pinned toolchain. `L6_LD_LLD`,
`L6_LLVM_NM` and `L6_LLVM_OBJCOPY` override each tool.

## Check

```sh
cargo test -p firmware-patch
python3 -m pip install unicorn
python3 patches/midi-sync/tests/cpu_sim.py firmware-midi-sync-v113/main_firmware.bin
```

Hook details, resources and limits are in the
[implementation reference](../../patches/midi-sync/README.md).

## Limits

- Supported main image: 1.10 only (SHA-256 in the reference).
- The 80 continuous values and 4 of 14 button-type states are synced (CH 5/6
  MONO x2, USB 1/2, USB 3/4). MUTE and the remaining two button states are not.
- The request must use the configured MIDI channel.
- Fader surfaces that echo their own movement can feed back; enable their
  motor-feedback suppression.
