# Control and indicator wiring

The emulator selects control and indicator mappings for the loaded model. L6
has six channel rings and no display; L6max has eight rings and an OLED. These
mappings describe emulator behavior. See [current status](../development/current-status.md)
for test coverage and open limits.

## Panel matrix

L6max uses an eight-row, five-column input matrix. The GPIO columns are B7,
B8, B9, C13, and F0; GPIOB4–6 select the row. Inputs are active low.

| Row | Column 0 | Column 1 | Column 2 | Column 3 | Column 4 |
| --- | --- | --- | --- | --- | --- |
| 0 | Hi-Z 1 | Channel 5 dB | Mono 5 | AUX1 | Scene A |
| 1 | Hi-Z 2 | Channel 6 dB | Mono 6 | AUX2 | Scene B |
| 2 | 48V 1/2 | Channel 7 dB | USB 7 | EFX | Scene C |
| 3 | Mute 1 | Channel 8 dB | USB 8 | SUB-MIX | Scene D |
| 4 | Mute 2 | HIGH | Mute 5 | PAN | Down / next |
| 5 | 48V 3/4 | FREQ | Mute 6 | LEVEL | Up / previous |
| 6 | Mute 3 | MID | Mute 7 | SEL | Bounce |
| 7 | Mute 4 | LOW | Mute 8 | COMP | Confirm / Undo |

The L6 matrix uses three columns and different control positions. Both
model-specific maps are listed in the [board reference](board.md#display-and-input).

## Main GPIO controls

L6max routes Menu and Record through GPIO5 bits 2 and 0, and TAP through
GPIO2 bit 31. Its sound pads use the following active-low inputs:

| Sound pad | Input target | GPIO |
| --- | ---: | --- |
| 1 | 49 | GPIO1 bit 21 |
| 2 | 50 | GPIO1 bit 25 |
| 3 | 51 | GPIO1 bit 26 |
| 4 | 52 | GPIO1 bit 0 |

L6 uses model-specific sound-pad inputs and routes Record through its panel
matrix. Its Play input is target 4, GPIO2 bit 25; TAP is target 48, GPIO2 bit
22. The [board reference](board.md#main-mcu-button-gpio-inputs) lists raw
input targets. The emulator's [power lifecycle](power.md) describes the two
models' power behavior.

Sound-pad playback requires an assigned WAV on the SD card. `l6test` can
prepare the assignment, mode, and level in a disposable fixture card; see
[fixture configuration](../development/testing.md#fixture-and-catalog-toml).
Imported and demo cards mark WAV entries with the FAT Archive attribute, which
the L6 file browser requires.

## Analog knobs

The main MCU samples the output knobs as 10-bit ADC values. L6 has SOUND PAD,
EFX RTN, MASTER, and MONITOR; L6max also has SUB-OUT.

| Knob | L6max ADC channel | L6 ADC channel | L6max firmware event |
| --- | ---: | ---: | --- |
| SOUND PAD | 3 | 5 | `EV_VR_SOUNDPAD` (58) |
| EFX RTN | 4 | 6 | `EV_VR_EFX` (59) |
| MASTER | 5 | 4 | `EV_VR_MASTER` (60) |
| MONITOR | 6 | 3 | `EV_VR_MONITOR` (61) |
| SUB-OUT | 7 | none | `EV_VR_SUBOUT` (62) |

These assignments come from observing the firmware through the emulator. On L6max,
each channel changes the SAI slots of its knob. On L6, channels 5, 4, and 6 submit
SOUND PAD, MASTER, and EFX RTN events, and MONITOR is taken to be the remaining
channel, 3. The L6 SOUND PAD and MASTER channels each silence the output, and its
EFX RTN and MONITOR channels leave a dry signal unchanged.

MASTER/SUB-MIX press and release reach the L6max sub-out handler, which updates its
DSP routing value. A sound pad's sub-out level does not change with this switch.
L6max TAP lights logical LED 33 on effect types 3, 4, and 5, and tapping interrupts it.

Samples range from 0 to 1023 and start at 512. Each gesture step changes the
sample by 32, clamped at either end. Knob artwork shows the simulated shaft
position; firmware applies its own filtering and calibration. Channels 8 and
13 are board-ID inputs and are not controlled by knobs.

## Indicator matrix

The panel publishes sampled LED duty to the host. The renderer uses
model-specific mappings for encoder rings, buttons, effects, scenes, sound
pads, and the two six-segment MASTER meters. L6 and L6max stereo playback
scenarios check that each meter side responds independently. PWM samples are
integrated over a guest-time scan window; visual brightness and physical meter
calibration remain approximate.

L6 sound-pad playback lamps use these sampled output positions:

| Sound pad | Matrix row | Bit | Mask |
| --- | ---: | ---: | ---: |
| 1 | 3 | 15 | `0x8000` |
| 2 | 3 | 14 | `0x4000` |
| 3 | 4 | 15 | `0x8000` |
| 4 | 4 | 14 | `0x4000` |

The L6max logical indicator groups are:

| Indices | Indicator |
| --- | --- |
| 0–7 | Channel 1–8 mute |
| 8–9 | Hi-Z 1–2 |
| 10–16 | COMP, 48V, MONO and USB source selections |
| 17–26 | HIGH, FREQ, MID, LOW, AUX1, AUX2, EFX, SUB-MIX, PAN, LEVEL |
| 27–30 | Sound pads 1–4 |
| 31, 33 | Record, TAP |
| 34–39 | Effects selection |
| 40–43 | Scenes A–D |
| 44–59 | Channel SIGNAL red and green |
| 60–71 | MASTER left and right meters |
| 72–143 | Nine encoder-ring segments per channel |

L6 scene buttons save the mixer when held and recall it on a short press. See
the [L6 manual](https://zoomcorp.com/manuals/l6-en/) for the instrument's
control behavior. The renderer uses the corresponding L6-specific lamp
outputs.

## Unresolved or incomplete

- L6 Play/Stop and TAP illumination, L6max Bounce illumination, and L6max
  logical LED 32 do not have verified artwork assignments.
- Physical LED brightness and analog knob travel and taper are not calibrated.
- Audio input and analog codec behavior are incomplete. See
  [audio emulation](audio.md) and [current status](../development/current-status.md).
