# MIDI state sync

Controllers on the mixer-control MIDI port (for example motorized-fader
surfaces) can ask the L6max to send its whole channel mixer state. The stock
firmware already sends a Control Change for each live change; it never sends
values that existed before the controller connected. This patch adds that
missing request. The panel image stays original.

A Control Change **127** with a value of 64 or more, on the configured MIDI
channel, makes the firmware send every channel setting through its own Control
Change sender. The user's CC map, MIDI channel and output settings therefore
apply unchanged. Anything else is passed to the stock handler.

## Source layout

- `src/payload.rs`: hook entry, request detection and state broadcast.
- `src/parameters.rs`: parameter identifier layout, getter table and request
  rule. No firmware calls, so the builder crate unit-tests it on the host.
- `src/firmware.rs`: bindings to original functions.
- `src/trampolines.rs`, `src/trampolines.s`: veneer that reads the displaced
  instructions from the locally supplied firmware at build time.
- `linker/link.ld`: payload placement (`PAYLOAD_BASE` comes from the builder) and
  prohibition on reserved mutable RAM.
- `hooks.tsv`: hook address, length and expected byte hash.
- `tests/cpu_sim.py`: runs the patched hook in a CPU simulator.

The builder is [`firmware-patch`](../../tools/firmware-patch/src/main.rs). See the
[patch conventions](../README.md).

## Build

Supported input is the 1.10 main image with SHA-256
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`.
From the repository root, with an unpacked package:

```sh
rustup target add thumbv7em-none-eabihf
cargo run -p firmware-patch -- unpacked firmware-midi-sync-v113 --patch midi-sync
# Both patches in one image:
cargo run -p firmware-patch -- unpacked firmware-knob-midi-v113 --patch knob-dialog --patch midi-sync
```

The default version marker is `0113` for anything but a knob-dialog-only build
(which keeps `0112`); `--update-version FOUR_DIGITS` overrides it. The output
directory must be new. Without Homebrew LLVM the builder uses the Rust toolchain's own `rust-lld`,
`llvm-nm` and `llvm-objcopy` (`llvm-tools` component). `L6_LD_LLD`,
`L6_LLVM_NM` and `L6_LLVM_OBJCOPY` override each tool.

## Hook

| Address | Length | Replaces | Behaviour |
| --- | --- | --- | --- |
| `0x8003f7c8` | 4 | Prologue of the inbound MIDI message handler | Broadcast on request; otherwise run the displaced prologue and continue at `0x8003f7cc` |

The handler receives a message whose status byte is at offset 4 and data bytes
at offsets 5 and 6. The patch compares the status with the configured channel
from the settings lookup at `0x8003feb0`, so it matches what the stock handler
would accept.

Broadcast order is physical channel 1-8, and within each channel the ten
blue-button settings. For each, the patch asks the channel mapper
`0x800564b0` for the mixer channel, reads the stored value with the getter
below, drops anything above 127, and calls `0x8003f9b0(identifier, value)`.

| Setting | Getter (as in the [knob patch](../knob-dialog/src/firmware.rs)) | Identifier base |
| --- | --- | --- |
| HIGH / MID / LOW | `0x8000b140`, band 0 / 1 / 2 | `0x00` / `0x10` / `0x18` |
| FREQ | `0x8000b120`, band 1 | `0x08` |
| SUB-MIX | `0x8000b2a0` | `0x20` |
| AUX1 / AUX2 | `0x8000b080`, bus 0 / 1 | `0x28` / `0x30` |
| EFX | `0x8000b0f0` | `0x38` |
| PAN | `0x8000b1d0` | `0x40` |
| LEVEL | `0x8000b0b0` | `0x48` |

Identifier = base + channel index (0-7). See the
[mixer MIDI reference](../../docs/firmware/protocols/mixer-midi.md) for the CC
numbers this yields.

## Resources and combining

- No RAM is reserved, no heap is used and no timer or task is created. The
  hook runs in the caller's context and uses a small stack frame.
- The payload is 350 bytes (rust-lld; sizes vary slightly with the linker).
- Combining with `knob-dialog` is supported by the builder: payloads are placed
  back to back from `0x80108000`, in catalog order (knob-dialog first), each
  16-byte aligned. It refuses overlapping hook regions. The two patches share
  no hook, no data and no firmware state; both read mixer values through the
  same getters without modifying them.
- A combined image was verified to apply, to pass the CPU simulation, and to
  pass the QEMU smoke checks (knob modes, 80 sync replies).
- `din-mixer-control` also calls into `0x8003f7c8` (from the DIN MIDI IN path
  instead of the USB Mixer Control port). Combining is safe and tested: this
  patch's own hook only intercepts its CC-127 broadcast request and otherwise
  falls through to the stock handler unchanged, so the DIN path reaches the
  same stock behavior regardless of whether this patch is present.

## Validation

```sh
cargo test -p firmware-patch
python3 -m pip install unicorn
python3 patches/midi-sync/tests/cpu_sim.py firmware-midi-sync-v113/main_firmware.bin
cargo run -p midi-smoke -- --firmware-dir firmware-midi-sync-v113 --volatile
L6_MIDI_SYNC=1 cargo run -p midi-smoke -- --firmware-dir firmware-midi-sync-v113 --volatile
```

`cpu_sim.py` executes the real patched hook, veneer and displaced prologue. It
stubs only the getters, the channel mapper, the stock sender and the stock
handler body. It checks that a request sends 80 distinct identifiers with the
right getter for each setting, that other messages reach the stock handler
untouched, that the configured MIDI channel is honoured and that out-of-range
values are skipped. Run against the unpatched image it fails, as it should.

`midi-smoke` with `L6_MIDI_SYNC=1` boots the patched firmware in the emulator
and sends a live CC 127 request over the real USB Mixer Control port,
confirming 80 Control Change replies on the configured channel and CC map.
Also confirmed combined with `knob-dialog` and `din-mixer-control` in one
image: `midi-smoke` (sync unaffected), `din-editor-mixer-smoke` (its own fix
unaffected), `input-smoke`, and `patched-update-smoke` (full SD update
install) all pass against the three-patch build.

## Limitations

- Run in QEMU, both alone and combined with `knob-dialog` and
  `din-mixer-control`; not run on an L6max. Emulator timing is not
  cycle-accurate to real silicon.
- Only the 80 continuous settings are synced. Button-type values (identifiers
  `0x50`-`0x5D`) are not, because their state sources are not mapped.
- The request must arrive on the configured MIDI channel, and the stock
  receive gating applies. Whether `mixerControlViaMidi` gates the parser before
  this handler is not established.
- A full dump is 80 three-byte messages (240 bytes) in the 2048-byte transmit
  ring, which the stock queue writes without a full check. A host that does not
  read the port can lose older unread bytes, as with stock traffic.
- A fader surface that echoes its own movement back can form a feedback loop
  with the stock receive path; use the surface's motor-feedback suppression.
- Values are control positions 0-127, equal to what the stock firmware sends on
  a knob turn. EQ positions map to dB and Hz through nonlinear tables.
- Payload bytes depend on the linker (knob-dialog: 8,012 bytes with Homebrew
  LLVM, 8,004 with `rust-lld`), so builds are only byte-comparable within one
  toolchain.
- Update-package acceptance by the physical bootloader remains unverified.
