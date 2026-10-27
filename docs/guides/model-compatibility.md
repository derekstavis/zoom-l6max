# L6 and L6max compatibility

[Documentation index](../README.md)

The repository now accepts both observed MAIN 0110 / panel 0100 update
packages. Package identity comes from the validated system header and MAIN
checksum. QEMU's board code identifies the model from the extracted MAIN reset
vector rather than the package filename:

| Model | Package size | Board mask | MAIN reset vector | Panel image offset |
| --- | ---: | ---: | ---: | ---: |
| L6 | `0x395200` | `0x01` | `0x80001415` | `0x285200` |
| L6max | `0x1a8200` | `0x40` | `0x80000415` | `0x1a3200` |

Both packages contain executable MAIN and panel images. The L6 package also
contains a trailing resource region, which `l6fw` preserves during
unpack/repack. The simulator uses one dual-MCU QEMU machine for either image;
the MAIN vector table selects the flash mapping. QEMU 11.1.2 builds with the
custom board, and both stock packages have started under `l6max-host` for a
five-second guest run. The historical
`l6max-dual`, `l6max-host`, and `l6max-gui` names are retained for compatibility.

The demo SD-card builder detects the package model and writes it under its
matching filename (`L6.BIN` or `L6max.BIN`). The persistent visual UI detects
the booted model from the MAIN reset vector, creates separate default card
paths (`sd-card.img` for L6max and `sd-card-l6.img` for L6), and selects a
matching firmware package for the demo card. The replacement update installer
now detects the booted model, reads its matching package filename, and maps
the L6 main, panel, and resource regions separately from L6max's contiguous
MAIN payload. This is emulator behavior; physical bootloader acceptance
remains unverified.

The `audio-path` driver case validates clocked SAI/eDMA activity for either
model, including TX/RX FIFO DMA requests, FIFO traffic, and audio-channel TCD accesses.
This check passed on L6 firmware 1.10 and an L6max 1.11 package.

L6 boots and receives panel reports over LPUART7. The UI binds the verified
mute, MONO/USB source, mode, effects, phantom-power, scene, Record, TAP, and
sound-pad controls to their model-specific inputs. Runtime probing identifies
Play target 4 as active-low GPIO2.25, which submits class-0 Play event `0x20`;
active-low GPIO1.9 is the separate Power Switch event. The saved-state Play
probe reaches the handler and changes PlayStop/transport state and a panel row.
A saved-state mode-0 run with `MASTER.WAV` now verifies decoded samples in the
channel-14 SAI1 TX buffer and a changed FIFO sample signature. Physical
Play/Stop lamp mapping and jack output remain unverified. Native UI smoke tests exercise the L6 hit regions and verify
the measured button, effect, mode, and scene lamps. First-boot setup and menu
navigation remain incomplete because the L6 has no display; labels for some
panel matrix coordinates and their downstream effects also remain unresolved.
See the
[L6 panel input map](../emulator/board.md#l6-panel-inputs).

The native patch builder detects the model from the MAIN reset vector. With no
`--patch`, it selects `knob-dialog` for L6max and `midi-sync` for L6; an
explicit patch selection is still checked against the detected model. The
builder supports `midi-sync` on the observed L6 1.10 image with a separate L6
hook and payload profile and rejects other L6 patch selections rather than
using L6max addresses. The L6 patch was compiled and repacked, passed a CPU
simulation of the real hook/trampoline, and booted in QEMU. A GDB-injected live
QEMU smoke now exercises its state getters, CC mapping and mapped-CC queue
sender. Stock L6 USB enumeration and basic MIDI IN/OUT pass in QEMU, as does
the stock DIN-to-mixer path. The direct USB smoke confirms all 54 replies from
patched L6 firmware. A fresh persistent-state DIN smoke verifies all 384
incoming bytes, 66 mapped event submissions and Main-reader calls, native
mixer setters, and a channel-1 indicator change. Physical hardware remains
unverified. The browser patcher embeds the L6 MIDI-sync profile and preserves
the existing L6max profiles. L6 `knob-dialog` remains unavailable because L6
has no LCD notification display. The L6max `din-mixer-control` patch is not an
L6 substitute.

The L6 MIDI-sync profile uses the L6 MIDI message handler at `0x8002f490`.
Equivalent L6 hooks for the other patches are not established. See the [patch
guide](midi-sync-patch.md) and [L6 patch implementation notes](../../patches/midi-sync/l6/README.md).

Protocol and hardware observations remain scoped per-model in the
[firmware overview](../firmware/README.md) and the individual protocol pages.
