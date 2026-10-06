# Control and indicator wiring

[Documentation index](../README.md)

Investigation pointed to the main key table at `0x800c4a0c` and its dispatcher
at `0x8004c578` for the panel controls. Event names in the main dispatch table
at `0x80036b76` identify the physical functions. The emulator changes GPIO
inputs; firmware scans, debounces, and dispatches them.

## Panel matrix

Physical columns 0–4 are GPIOB7, GPIOB8, GPIOB9, GPIOC13, and GPIOF0.
GPIOB4–6 select the row. The GPIO selection words for rows 0–7 are
`00 40 20 60 10 50 30 70`.

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

Matrix inputs are active low. Panel UART packets use `91 column row` for
closure and `90 column row` for release; UART column is physical column + 1.
The main key table inverts the logical state of the four dB switches.

MASTER/SUB-MIX is a separate persistent GPIOA3 input. Its panel routine at
`0x08000aa0` uses four-sample debounce history. It sends bank 0, row 0:
`90 00 00` when the pin goes low and `91 00 00` when it goes high.
This polarity differs from the matrix packets.

## Main GPIO controls

| Control | Active-low input |
| --- | --- |
| Menu | GPIO5 bit 2 |
| Play/Stop | GPIO1 bit 20 |
| Record | GPIO5 bit 0 |
| TAP | GPIO2 bit 31 |
| Sound pad 1 | GPIO1 bit 21 |
| Sound pad 2 | GPIO1 bit 25 |
| Sound pad 3 | GPIO1 bit 26 |
| Sound pad 4 | GPIO1 bit 0 |

Power is a separate **active-high GPIO2 bit 25** input. Its reader at
`0x80021d58` inverts the pin before the main button scanner consumes it. The
main key table at `0x800c42b8` associates it with event 0 (`EV_BTN_POWER`).
The UI forwards momentary down/up edges, including a sustained mouse hold;
firmware owns debounce and long-press recognition. A native test established
one GPIO closure and release for a click, separated by 49 ms of guest time.

System shutdown ends by clearing **GPIO3 bit 3**, the board power-hold output.
The system event handler at `0x80037328` calls output 1 with value 0;
its callback at `0x80021ea8` writes GPIO3 DR_CLEAR with mask `8`.
QEMU models this falling edge as guest shutdown after firmware cleanup.
The native UI blanks the display/LEDs and reaps QEMU, keeping the window open.
A Power click then launches both chips with the same storage and SD card.
See [power lifecycle](power.md).

The main GPIO reader at `0x80043538` and button callbacks at
`0x80021d48`–`0x80021db0` establish these inputs. GPIO outputs also control
analog routing and phantom-power circuitry; they are not treated as scene LEDs.

## Analog knobs

These five controls are potentiometers sampled by the main MCU's ADC1.
The ADC scan table at `0x80094820` lists channels `0, 3, 4, 5, 6, 7, 8, 13`.
The VR dispatch table at `0x800c18cc` links each knob to its scan index and
named event; the ADC reader is `0x800213c0` and the VR processing routine
is `0x80044ef8`.

| Knob | ADC channel | Scan index | Firmware event |
| --- | ---: | ---: | --- |
| SOUND PAD | 5 | 3 | `EV_VR_SOUNDPAD` (58) |
| EFX RTN | 6 | 4 | `EV_VR_EFX` (61) |
| MASTER | 4 | 2 | `EV_VR_MASTER` (60) |
| MONITOR | 3 | 1 | `EV_VR_MONITOR` (59) |
| SUB-OUT | 7 | 5 | `EV_VR_SUBOUT` (62) |

The model supplies 10-bit samples (0–1023), initially 512. Each gesture step
changes the physical sample by 32 counts, clamped at either end. Firmware
retains its own median filtering, calibration, hysteresis, and event dispatch.
The existing ADC conversion-complete interrupt invokes the firmware's reader.
Changed sample notifications are emitted only when firmware reads ADC1 R0.
The five SVG position marks rotate over an estimated 270-degree travel.
Their positions represent the simulated physical shafts, rather than the
firmware's processed volume values. Native gesture checks verified the distinct
ADC samples in both directions, including vertical and horizontal dragging.

Channels 8 and 13 retain the board-ID ladder values and cannot be changed by
the knob command. Channel 0 retains its existing synthetic non-knob sample.

## Indicator matrix

The main firmware's wiring table at `0x800c457c` maps logical LEDs to panel
common/column pairs. The renderer reads the modeled serial LED GPIO surface,
not UART control messages or firmware RAM. See the [board model](board.md)
for SPI, latch, blanking, and PWM observation details.

| Logical index | Physical indicator | Main routine evidence |
| --- | --- | --- |
| 0–7 | Channel 1–8 mute | `0x8000c628` |
| 8–9 | Hi-Z 1–2 | `0x8000c5c8` |
| 10 | COMP | `0x8000c490` |
| 11–12 | 48V 1/2 and 3/4 | `0x8000c650` |
| 13–14 | Mono 5–6 | `0x8000c5f8` |
| 15–16 | USB 7–8 | `0x8000c788` |
| 17–26 | HIGH, FREQ, MID, LOW, AUX1, AUX2, EFX, SUB-MIX, PAN, LEVEL | `0x8000c450` |
| 27–30 | Sound pads 1–4 | Sampler task, including `0x80049f94` |
| 31 | Record | `0x8000c680` |
| 33 | TAP | `0x8000c758`, `0x8000c7d8` |
| 34–39 | Hall, Room, Spring, Delay, Echo, AI Noise Reduction | `0x8000c4b0` |
| 40–43 | Scenes A–D | `0x8000c6f8`, scene refresh `0x80050f38` |
| 44–51 | Channel 1–8 SIGNAL red | `0x8002fe98` |
| 52–59 | Channel 1–8 SIGNAL green | `0x8002fec8` |
| 72–143 | Nine ring segments per channel | `0x8000c4f0` |

Live GPIO checks established distinct mute outputs for all eight channels,
the ten mode selections, the Hall-to-Room change through SEL, and COMP.
The native UI test also checks mute, HIGH selection, and a persistent dB
switch through GPUI's event dispatcher without using the system pointer.

## Unresolved or incomplete

- Logical LED 32 and indices 60–71 have no physical artwork assignment.
- Play/Stop and Bounce illumination have not been assigned to verified outputs.
- SIGNAL assignments come from firmware routines; audio-driven transitions
  remain untested because audio processing is unmodeled.
- PWM brightness and physical LED colors are not calibrated.
- Analog knob travel and taper are approximate; audio output is unmodeled.
