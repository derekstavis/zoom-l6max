# L6 and L6max compatibility

[Documentation index](../README.md)

The emulator and package tools support the L6 and L6max 1.10 packages. Package
validation identifies the model from its system header and MAIN checksum; the
emulator selects its profile from the MAIN reset vector.

| Model | Package size | Board mask | MAIN reset vector | Panel image offset |
| --- | ---: | ---: | ---: | ---: |
| L6 | `0x395200` | `0x01` | `0x80001415` | `0x285200` |
| L6max | `0x1a8200` | `0x40` | `0x80000415` | `0x1a3200` |

Both packages contain MAIN and panel images. L6 also includes a resource
region that `l6fw` preserves on unpack and repack. The shared QEMU machine
uses model-specific flash mappings, UART routes, inputs, and panel layouts.
See the [board reference](../emulator/board.md) for the profile differences.

L6 has six channel rings and no screen; L6max has eight rings and an OLED.
The native interface selects matching controls and keeps separate default SD
cards for the two models. Both models support assigned sound-pad playback and
stereo MASTER meter output in the emulator. Recording, physical jack routing,
and hardware fidelity remain incomplete; see [current status](../development/current-status.md).

The native patch builder selects `midi-sync` by default for L6 and `knob-dialog`
for L6max. Explicit patch selections are checked against the detected model
and supported firmware revision. L6 `knob-dialog` is unavailable because L6
has no LCD notification display. The browser patcher supports the L6 MIDI-sync
profile and the L6max profiles. See the [patch index](../../patches/README.md)
and [MIDI sync guide](midi-sync-patch.md) for support and usage.

The emulator's replacement installer selects the matching package name and
flash layout. Its behavior does not establish acceptance by the physical
bootloader. The [firmware package guide](firmware-packages.md) covers local
package handling; the [firmware overview](../firmware/README.md) links to
model-specific protocol references.
