# Read mixer values over MIDI

A MIDI controller — a motorized-fader surface, for example — only finds out
about settings it's there to see change live. The stock firmware never
tells a newly connected controller what the mixer's values already were, so
the controller's faders and knobs start blank or out of sync until someone
touches every control by hand. This patch adds a request a controller can
send to get the mixer's entire state in one go.

**To request it:** send Control Change **127** with a value of 64 or higher,
on the MIDI channel the mixer is already configured to use. The selected
firmware profile replies with its mapped Control Change messages, so a
controller can use the same live-change protocol to synchronize its state.
L6max sends 94 values across its eight strips and global controls. L6 sends 54
values across its six strips and nine mapped groups. The L6 uses a separate
hook and payload; see the [L6 implementation notes](l6/README.md). The builder
detects the model from the MAIN image and selects the matching profile.

## How it works

The stock firmware already sends a Control Change for each live change; it
never sends values that existed before the controller connected. This patch
adds that missing request. The panel image stays original.

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

The builder is [`l6patch`](../../docs/development/tools/l6patch.md). See the
[patch conventions](../README.md).

## Build

The supported 1.10 MAIN images are L6max
(`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`) and L6
(`827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799`). From
the repository root, unpack each local package and build the matching profile:

```sh
rustup target add thumbv7em-none-eabihf
cargo run -p l6fw -- unpack /path/to/L6max.bin unpacked-l6max
cargo run -p l6patch -- unpacked-l6max firmware-midi-sync-v113 --patch midi-sync
# The separate L6 hook and payload are selected from its MAIN reset vector.
cargo run -p l6fw -- unpack /path/to/L6.BIN unpacked-l6
cargo run -p l6patch -- unpacked-l6 firmware-midi-sync-l6 --patch midi-sync
# Both patches in one image:
cargo run -p l6patch -- unpacked-l6max firmware-knob-midi-v113 --patch knob-dialog --patch midi-sync
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

Broadcast order is physical channel 1-8, and within each channel the eleven
blue-button settings (the original ten, plus MUTE — found after the initial
release, see [Limitations](#limitations)). For each, the patch asks the
channel mapper `0x800564b0` for the mixer channel, reads the stored value
with the getter below, drops anything above 127, and calls
`0x8003f9b0(identifier, value)`.

| Setting | Getter (as in the [knob patch](../knob-dialog/src/firmware.rs)) | Identifier base |
| --- | --- | --- |
| HIGH / MID / LOW | `0x8000b140`, band 0 / 1 / 2 | `0x00` / `0x10` / `0x18` |
| FREQ | `0x8000b120`, band 1 | `0x08` |
| SUB-MIX | `0x8000b2a0` | `0x20` |
| AUX1 / AUX2 | `0x8000b080`, bus 0 / 1 | `0x28` / `0x30` |
| EFX | `0x8000b0f0` | `0x38` |
| PAN | `0x8000b1d0` | `0x40` |
| LEVEL | `0x8000b0b0` | `0x48` |
| MUTE | `0x8000b0c0` | `0x50` |

Identifier = base + channel index (0-7). MUTE's getter sits in the address
gap between `LEVEL` (`0x8000b0b0`) and `EFX` (`0x8000b0f0`); its setter is
`0x8000bb90` (not called by this patch, which only reads state).

After the 88 per-channel values, six single globals follow, each sent once
(not per channel). Four read a fixed slot of a per-slot flag table
`0x8000b190` the knob patch never needed; the other two (EFX TYPE,
COMPRESSOR) read through their own, differently-shaped getters.

| Setting | Getter | Argument | Identifier |
| --- | --- | --- | --- |
| CH 5 MONO x2 | `0x8000b190` | slot `4` | `0x58` |
| CH 6 MONO x2 | `0x8000b190` | slot `6` | `0x59` |
| USB 1/2 | `0x8000b190` | slot `8` | `0x5A` |
| USB 3/4 | `0x8000b190` | slot `10` | `0x5B` |
| EFX TYPE | `0x8000b270` | bus `0` | `0x5C` |
| COMPRESSOR | `0x8000b1c0` | none | `0x5D` |

This is all 94 of the stock firmware's parameter identifiers
(`controlChangeSlotCount` in the editor protocol) — full coverage. See the
[mixer MIDI reference](../../docs/firmware/protocols/mixer-midi.md) for the CC
numbers this yields.

## Resources and combining

- No RAM is reserved (the linker script asserts `SIZEOF(.state) == 0` and the
  build fails if that's violated), no heap is used and no timer or task is
  created. The hook runs in the caller's context and uses a small stack
  frame: `broadcast_state`'s locals are a handful of scalars in a bounded,
  non-recursive loop (94 iterations total, fixed at compile time), not a
  buffer that grows with the broadcast size.
- The payload is 1,179 bytes (rust-lld; grew from 350 bytes when MUTE and the
  6 globals were added; sizes vary slightly with the linker). The combined
  three-patch image (knob-dialog + midi-sync + din-mixer-control) uses 9,263
  bytes total, well inside the builder's enforced payload region
  (`0x80108000`-`0x801a2ff8`, checked at build time; it refuses to place a
  payload over non-erased reserved bytes).
- Combining with `knob-dialog` is supported by the builder: payloads are placed
  back to back from `0x80108000`, in catalog order (knob-dialog first), each
  16-byte aligned. It refuses overlapping hook regions. The two patches share
  no hook, no data and no firmware state; both read mixer values through the
  same getters without modifying them.
- A combined image was verified to apply, to pass the CPU simulation, and to
  pass the QEMU smoke checks (knob modes, 94 sync replies).
- `din-mixer-control` also calls into `0x8003f7c8` (from the DIN MIDI IN path
  instead of the USB Mixer Control port). Combining is safe and tested: this
  patch's own hook only intercepts its CC-127 broadcast request and otherwise
  falls through to the stock handler unchanged, so the DIN path reaches the
  same stock behavior regardless of whether this patch is present.

## Validation

```sh
cargo test -p l6patch
python3 -m pip install unicorn
python3 patches/midi-sync/tests/cpu_sim.py firmware-midi-sync-v113/main_firmware.bin
cargo run -p l6test -- run --config smoke.local.toml --case host-midi-sync --model l6max
cargo run -p l6test -- run --config smoke.local.toml --case l6max-direct-midi-sync --model l6max
python3 patches/midi-sync/tests/cpu_sim_l6.py firmware-midi-sync-l6/main_firmware.bin
cargo run -p l6test -- run --config smoke.local.toml --case l6-midi-sync --model l6
cargo run -p l6test -- run --config smoke.local.toml --case l6-direct-midi-sync --model l6
```

Configure the model's `prepared` fixture with its stock package and
`patches = ["midi-sync"]`, plus suitable state if needed. See the
[testing workflow](../../docs/development/testing.md). `cpu_sim.py` executes the L6max patched hook, veneer and displaced prologue. It
stubs only the getters, the channel mapper, the stock sender and the stock
handler body. It checks that a request sends 94 distinct identifiers — the 88
per-channel values (ten original settings plus MUTE) plus the 6 globals below
— with the right getter and argument for each, that other messages reach the
stock handler untouched, that the configured MIDI channel is honoured and
that out-of-range values are skipped. Run against the unpatched image it
fails, as it should.

Use the shared driver for live validation. `host-midi-sync` verifies 94
L6max replies on the configured native host endpoint; `l6max-direct-midi-sync`
checks the direct USB route. L6 uses `l6-midi-sync` for the paused-guest hook
path and `l6-direct-midi-sync` for direct USB, with 54 mapped sends/replies.
The DIN mixer scenarios are `l6max-din-mixer-control` and
`l6max-din-mixer-control-patched`; their new driver implementations still need
live qualification. A prior DIN mixer run exercised the combined
build, but that result has not yet been reproduced by the driver. The
`patched-update` case separately validates the SD-card update-install flow.
The two MIDI-sync profiles use different firmware hooks and have separate CPU simulations and
runtime checks; see the [L6 notes](l6/README.md) for its scope.

## Limitations

- Run in QEMU, both alone and in the documented compatible combinations; not
  run on physical L6 or L6max hardware. Emulator timing is not cycle-accurate
  to real silicon.
- The L6max profile syncs all 94 of its stock parameter identifiers — the 10
  original continuous settings, MUTE (found after the initial release), and
  all 6 button-type globals (CH 5/6 MONO x2, USB 1/2, USB 3/4, EFX TYPE,
  COMPRESSOR). The L6 profile syncs its 54 enabled CC-map values; its distinct
  mapping and evidence are listed in the [L6 implementation notes](l6/README.md).
- The request must arrive on the configured MIDI channel, and the stock
  receive gating applies. Whether `mixerControlViaMidi` gates the parser before
  this handler is not established.
- A full dump is 94 three-byte messages (282 bytes) in the 2048-byte transmit
  ring, which the stock queue writes without a full check. A host that does not
  read the port can lose older unread bytes, as with stock traffic.
- The broadcast runs 94 getter-and-send iterations, which occupies its
  caller's context for that long per request. A sync request arriving over the
  USB Mixer Control port runs inside the `MidiControlMsg` FreeRTOS task, the
  only caller observed for the hooked function (`0x8003f7c8`). That task's priority (18) sits strictly below every
  audio-capture and recording-related task found (audio ADC sampling: 27;
  audio timing compensation: 26; recorder/sampler file I/O: 28). FreeRTOS's
  preemptive scheduler guarantees a strictly-higher-priority task always
  preempts a lower one, so this broadcast — run from the normal USB trigger,
  regardless of how long it takes — **cannot delay audio capture or
  recording**, categorically, not just probably. One narrower case remains:
  a sync request arriving over the DIN jack (forwarded by the
  [din-mixer-control patch](../din-mixer-control/README.md)) runs inside a
  different task that ties in priority with audio ADC sampling specifically
  (both 27) rather than sitting below it, which bounds (but doesn't fully
  rule out) a brief round-robin scheduling delay to audio sampling on that
  path only — recording is still unaffected either way, since the
  recorder/sampler file I/O task (28) outranks both.
- All three new getters (MUTE `0x8000b0c0`, EFX TYPE `0x8000b270`,
  COMPRESSOR `0x8000b1c0`) are single-instruction, unguarded memory
  reads with no locking, no RAM writes and no calls of their own — the same
  shape as the ten getters already in production use, so they add no new
  category of risk, only new addresses of an already-trusted shape.
- A fader surface that echoes its own movement back can form a feedback loop
  with the stock receive path; use the surface's motor-feedback suppression.
- Values are control positions 0-127, equal to what the stock firmware sends on
  a knob turn. EQ positions map to dB and Hz through nonlinear tables.
- Payload bytes depend on the linker (knob-dialog: 8,012 bytes with Homebrew
  LLVM, 8,004 with `rust-lld`), so builds are only byte-comparable within one
  toolchain.
- Update-package acceptance by the physical bootloader remains unverified.
