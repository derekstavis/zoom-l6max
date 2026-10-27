# Control and indicator wiring

The emulator maps controls and indicators by detected firmware model. L6 has six
channel rings and no display; L6max has eight rings and an OLED. The tables below
describe emulator inputs and outputs. Physical wiring is unverified unless
explicitly stated. See [current status](../development/current-status.md) for
test limits.

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

On L6max, `refresh_channel_control_switches` (`0x80052f08`) reads logical key
codes `0x19`–`0x1c`. Their lookup entries at `0x800c48c4` map to bits 1–4 of
key-state word 4 at `0x80883564`. QEMU targets 15, 20, 25, and 30 set those
bits, but those targets are AUX2, EFX, SUB-MIX, and PAN. This is a
channel-control state path, not a Sound Pad trigger mapping. L6max Sound Pad
controls 49–52 use the key-reader table at `0x800c42b8`: GPIO1 bits
21, 25, 26, and 0. In four isolated QEMU runs, each control selected its
expected reader, produced fresh SAI1 samples with its corresponding WAV
assigned, and changed the matching logical pad LED (27–30). The firmware pad
records stayed zero throughout, so direct record-state correlation remains
open. These simulator routes do not establish physical button wiring.

The earlier L6-derived L6max candidate map (GPIO2.31, GPIO2.30, GPIO1.21, and
GPIO1.20) has been superseded. The shared mapping helper now keeps L6's
EventPort routes separate from L6max's key-reader routes. In particular,
L6max target 50 uses GPIO1.25; GPIO2.30 remains the board-ID strap and is not
driven by the Pad 2 input. The probe injects through MAIN GPIO rather than the
panel matrix.

A saved-state L6max UI probe confirmed that the Sound Assign picker opens with
the first WAV selected. Moving Down selects “None Assigned”; confirming that
row clears the pad. After removing that extra Down, the Editor's assignment
query confirmed Pad 1 as `TONE220.WAV`. A separate playback attempt with Editor
playback disabled drove candidate 49 low on the earlier GPIO2.31 map and
observed the firmware test bit 31, but Pad 1's runtime record and SAI1 samples
stayed zero. That run did not exercise the current GPIO1.21 key-reader route.

A later menu-navigation run saved an LCD frame after each of eight key presses
with `L6_SCREEN_SETTLE_MS=1500`; the captures showed the main menu, Sound Assign,
Pad 3 selection, and WAV picker in sequence. The first verification attempt
discarded the assignment reply already collected by the Editor probe and its
redundant query returned no replies. The probe now returns its collected
assignment replies; a rerun verified Pad 3 as `TONE440.WAV`. The verification
skips Editor playback, so it confirms the screen-driven assignment and timing,
not audio output.

The earlier L6-derived target-51/GPIO1.21 probe changed a non-pad LED and did
not identify Pad 3; it used the wrong L6max route. The corrected key-reader
mapping was tested in four isolated runs. Each target selected its expected
GPIO1 bit, produced nonzero SAI1 output with the corresponding WAV assigned,
and changed the matching logical pad LED 27–30. The firmware's runtime pad
records remained `00 00`; the test saves row samples and reports those records
so future probes can pursue direct state and sample-content correlation. The
independent Editor-play request did not enter playing state and is not needed
to establish these physical-key audio paths.

MASTER/SUB-MIX is a separate persistent GPIOA3 input. Its panel routine at
`0x08000aa0` uses four-sample debounce history. It sends bank 0, row 0:
`90 00 00` when the pin goes low and `91 00 00` when it goes high.
This polarity differs from the matrix packets.

## Main GPIO controls

| Control | Active-low input |
| --- | --- |
| Menu | GPIO5 bit 2 |
| Play/Stop | Target 4 → active-low GPIO1 bit 9 during startup; runtime action and physical wiring unverified |
| Record | GPIO5 bit 0 |
| TAP | GPIO2 bit 22 |
| L6max Sound Pad 1 (target 49) | GPIO1 bit 21 |
| L6max Sound Pad 2 (target 50) | GPIO1 bit 25 |
| L6max Sound Pad 3 (target 51) | GPIO1 bit 26 |
| L6max Sound Pad 4 (target 52) | GPIO1 bit 0 |

A saved-state L6max QEMU trace with `L6MAX_TRACE_RECORDER_EVENT_PATH=1`
verified the Record input through the firmware action path. Target 5 drives
GPIO5 bit 0 low; the guest reads GPIO5 `DR=0x00000004`, and the reader callback
`l6max_recorder_record_gpio_reader` (`0x80021d70`) reaches the debounced scanner
`l6max_main_button_scan` (`0x800452f0`). It queues event `0x2b` through
`l6max_ui_event_queue_send` (`0x80028088`). The main handler
`l6max_handle_main_button_event` (`0x80036a40`) identifies that event as
`EV_BTN_REC`; callback slot 1 selects `l6max_recorder_button_callback`
(`0x8004e818`), which dispatches operation 3 through
`l6max_recorder_view_action_dispatch` (`0x8004d768`) and reaches
`l6max_recorder_start_action` (`0x80047238`). That action queues
`l6max_recorder_start_callback` (`0x80047268`), and the live trace reached the
callback; it logs `RecPlayCtl_RecStart`, updates recorder playback state and
sets the recorder mode flag. This confirms the modeled GPIO, event route and
asynchronous callback work together. The captured recorder screen still showed
`00:00:00`, so continued recording and file growth remain unverified.

This L6max trace agrees with the i.MX RT GPIO read rule: for an input pin,
reading `DR` returns the sampled pin level from `PSR`; QEMU's upstream
`imx_gpio` implements that direction-dependent read. The physical L6 chip
marking `MIMXRT1041DFP6B` places that model in NXP's [i.MX RT1040
family](https://www.nxp.com/products/i.MX-RT1040), whose listed reference
manual is [IMXRT1040RM](https://www.nxp.com/products/i.MX-RT1040). That PDF
requires an NXP account, so the exact RT1040 wording remains to be checked
against an L6 hardware trace. The searchable [RT1060 reference
manual](https://www.pjrc.com/teensy/IMXRT1060RM_rev2.pdf) states the same
direction-dependent DR rule; this is supporting family evidence, not a
substitute for checking IMXRT1040RM. QEMU's implementation is in
`emulator/qemu/upstream/hw/gpio/imx_gpio.c`.

With `TONE220.WAV`, `TONE330.WAV`, `TONE440.WAV`, and `TONE660.WAV` assigned
to Pads 1–4 respectively, each target selected the table-indicated bit,
produced nonzero SAI1 samples, and changed its corresponding logical LED 27,
28, 29, or 30. The runtime pad-record words remained zero. GPIO2.30 is not
used for L6max Pad 2, preserving its board-ID strap value.

On L6max, Power is an **active-high GPIO2 bit 25** input. Its reader at
`0x80021d58` inverts the pin before the main button scanner consumes it. The
main key table at `0x800c42b8` associates it with event 0 (`EV_BTN_POWER`).
System shutdown ends by clearing **GPIO3 bit 3**, the board power-hold output.
The system event handler at `0x80037328` calls output 1 with value 0; its
callback at `0x80021ea8` writes GPIO3 DR_CLEAR with mask `8`. QEMU models this
falling edge as guest shutdown after firmware cleanup.

The L6 exposes Power at simulator input ID 53, but the corresponding L6 1.10
firmware reader has not been identified. Native testing did not observe the
L6max GPIO3.3 shutdown edge, so the L6 QEMU profile currently cuts virtual power
after a 500 ms press. That interval is an emulator approximation. See [power
lifecycle](power.md).

The main GPIO reader at `0x80043538` and button callbacks at
`0x80021d48`–`0x80021db0` establish these inputs. GPIO outputs also control
analog routing and phantom-power circuitry; they are not treated as scene LEDs.
The L6 board profile accepts TAP and sound-pad keys through the normal
`EventPort` GPIO scan. Play target 4 maps to active-low GPIO2.25 and submits
event `0x20`; active-low GPIO1.9 submits Power Switch event `0x22`. Playback
and physical key wiring remain unverified. Its Record button instead reaches
MAIN event 31 through the panel matrix;
the L6 profile does not claim the L6max Menu or main-GPIO Record inputs.
The native UI smoke dispatches the verified TAP and four sound-pad targets
through their L6 hit regions. With no audio or samples playing, holding these
keys for 400 ms produced no panel-matrix indicator-row changes. The
official [L6 operation manual](https://zoomcorp.com/manuals/l6-en/) states
that a SOUND PAD button blinks during playback. An Editor probe assigns and
plays a WAV on each pad; repeated playback traces show the matching panel
matrix lamp blinking at roughly 250 ms intervals. The physical pad order maps
to row/bit pairs `(3,15)`, `(3,14)`, `(4,15)`, and `(4,14)` (zero-based row
and bit indices), now exposed by `l6_sound_pad_leds`. For each pad, the
firmware's corresponding two-byte record at `0x806293f4 + 2 * pad` changes
from `10 00` to `10 02` during playback. The MAIN GPIO scan continues through
the same output phases for all pads and is not the pad-specific lamp path.

| Playback lamp | Panel matrix row | Bit | Mask |
| --- | ---: | ---: | ---: |
| SOUND PAD 1 | 3 | 15 | `0x8000` |
| SOUND PAD 2 | 3 | 14 | `0x4000` |
| SOUND PAD 3 | 4 | 15 | `0x8000` |
| SOUND PAD 4 | 4 | 14 | `0x4000` |

Rows and bits are zero-based. The host decoder is
`l6max_host::indicators::l6_sound_pad_leds`; the L6 UI uses its four outputs
to light the corresponding numbered buttons. These are firmware panel-matrix
outputs, not the unrelated MAIN GPIO scan transitions.

Firmware analysis locates `sound_pad_led_task_entry` at `0x80037288`; it
updates four per-pad LED state records through `0x80007c90`/`0x8004af10`.
Set
`L6_MAIN_BUTTON_LED_TRACE=1` when running `l6dbg input` against L6 firmware to
repeat the before/held/released row capture. The same diagnostic opens an L6
Editor SysEx session, asks firmware for the sound-pad file lists, then requests
play and reads each pad state. L6 only enumerates WAV directory entries whose
FAT Archive attribute (`0x20`) is set; generated demo and imported-folder
images now set this bit. A QEMU probe with an Archive-marked `PAD1/001.WAV`
reported one file, accepted assignment, and entered playing state. Its first
logical LED record at `0x806293f4` became `10 02`. The time-series row trace
above correlates this logical state with its physical panel lamp. Set
`L6_REQUIRE_PAD_FILES=1` with `L6_MAIN_BUTTON_LED_TRACE=1` to make the input
smoke fail unless all four generated WAVs enumerate and assign successfully,
and each probed pad reports playing. Set `L6_PAD_LED_PROBE_ONLY=0`, `1`, `2`,
or `3` to play one zero-based pad per fresh volatile boot while still checking
all four files and assignments. Without the selector, the probe plays all four
pads in sequence. To isolate a physical key from playback, set
`L6_PAD_LED_SKIP_PLAYBACK=1` and `L6_PAD_GPIO_PROBE_ONLY=0` through `3`; the
latter injects only the selected pad key (targets 49–52) after a fresh
volatile boot. Use `L6_PAD_GPIO_PROBE_ONLY=all` to test targets 49–52 in one
run. Four such runs left all panel indicator rows and logical pad LED records
unchanged on key press and release when the demo card had no playable files.
Active-playback row changes repeat and identify each pad-specific blink lamp;
the idle key-press test does not exercise playback.
Add `L6_MAIN_GPIO_TRACE=1` to require a QEMU trace from an actual guest read of
GPIOx_DR or GPIOx_PSR while a modeled active-low pin is low. The trace also
reports the guest PC, link register, and tested bit; a port read alone does
not prove firmware tested that candidate pin. The normal `EventPort` task at
`0x8006d470` uses helper `0x80032ae4` to select GPIO2.31, GPIO2.30, GPIO1.21,
GPIO1.20, and GPIO2.22. The 1 ms input-refresh timer alone does not count as a
firmware read.
Add `L6_MAIN_GPIO_OUTPUT_TRACE=1` to capture timestamped changes to MAIN GPIO
output latches while investigating the multiplexed path.

Earlier L6 probes correlated targets 49–52 with GPIO2.31, GPIO2.30, GPIO1.21,
and GPIO1.20 and reported corresponding playback records and SAI1 changes.
Later GDB tracing showed that target 49 reaches the correct Pad 1 handler while
the assignment-pending flag is still set; the firmware's start helper returns
in that state. The assignment callback was not reached and SAI1 stayed silent
in a fresh saved-state test. Treat the earlier L6 end-to-end mapping as
provisional until assignment completion is observed and the four pads are
retested. GPIO2.22 is event `0x19`, changed no pad record, and produced no
changing SAI1 samples; QEMU maps target 48 (TAP) to that input. Physical L6 key
wiring still needs hardware confirmation. The slower scan branch reads
GPIO2.25 for Play event `0x20` and active-low GPIO1.9 for Power Switch event
`0x22`.

For L6, the GPIO-to-handler route is verified, but end-to-end pad playback
remains open until Editor assignments complete. With
`L6_REQUIRE_PAD_FILES=1`, `l6dbg input` waits for the pending flag to clear
before pressing a pad; `L6_PAD_ASSIGNMENT_TIMEOUT_MS` defaults to 30000 and
accepts 1–120000. A timed-out assignment skips the GPIO test. The L6max probe
must wait three seconds after Editor assignments before testing GPIO; pressing
too early can miss playback. Isolated runs with the four matching
WAVs assigned verified target 49/GPIO1.21, 50/GPIO1.25, 51/GPIO1.26, and
52/GPIO1.0 as audio-positive simulator routes with matching pad LEDs. The
runtime pad records stayed zero, and physical wiring remains unverified.
GPIO2.30 remains reserved for the board-ID strap. Targets 15, 20, 25, and 30
are channel-mode controls and must not be used as Sound Pad inputs.

The `l6dbg input` UI setup and menu probes wait for the key-release
acknowledgement, then wait 1.5 seconds before saving the corresponding LCD
frame. The release acknowledgement matters because the initial TAP
acknowledgement only means QEMU accepted the press; it does not mean the key
has been released or that the firmware has redrawn the screen. Set
`L6_SCREEN_SETTLE_MS` to adjust the post-release capture delay when
investigating firmware redraw timing. The probes wait 42 seconds for firmware
startup by default; `L6_BOOT_SETTLE_MS` adjusts that wait, which is useful
with persistent states that have already completed onboarding.

For assigned-sample playback, attach the generated card with `--sd-image` and
set `L6_REQUIRE_PAD_FILES=1 L6_PAD_LED_PROBE_ONLY=0` through `3` plus
`L6_PAD_GPIO_PROBE_ONLY` to the same pad index. Four fresh volatile runs
enumerated and assigned all four WAVs, set each selected logical pad record to
`10 02`, and produced the corresponding blink in the matrix row/bit table
above. `L6_PAD_GPIO_PROBE_ONLY=none` omits physical pad-key presses, allowing
playback output to be isolated from key scanning. A duration-matched run with
`L6_PAD_LED_SKIP_PLAYBACK=1` serves as the no-playback control. Without
`--sd-image`, the Editor correctly reports zero playable files and the
playback probe cannot test active pad lamps.

A separate SDHCI/SDMA trace confirmed the firmware's CMD17 reads at the
`SOUND_PAD` and `PAD1`–`PAD4` directory-sector offsets, with the transfers
targeting its SD buffer. Testing `001.WAV` with attribute `0x00` returned no
files; changing only its Archive bit to `0x20` made it enumerable and playable.
The physical LED transport and visible lamp behavior still need mapping.

Analysis refines the logical mapping: `l6_set_sound_pad_state` writes two bytes
per pad at `0x806293f4 + 2 * pad`, with zero-based pad indices 0–3. Its first
byte is the requested intensity (`0x10` for the nonzero states observed); the
second is a mode code translated from the setter's state argument. The task at
`0x80037288` updates those records using each pad's sample handle and the
current pad index, emitting state arguments 0, 1, 3, and 4. This identifies the
four logical pad LED channels and firmware state inputs. Runtime playback
traces map their visible blink outputs to the panel matrix; the physical lamp
color and brightness are not modeled by the host indicator protocol.

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

This logical-index table is specific to L6max. L6 mute, button, effect,
channel-mode, scene, and Record lamp assignments are model-specific and listed above.
L6 SIGNAL, power, meter, and remaining status indicators still need
faceplate-level maps.

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

L6 scene buttons save their current mixer settings when held until the lamp
comes on, then recall that scene on a short press. The [L6 operation manual](https://zoomcorp.com/manuals/l6-en/)
describes unlit buttons as empty scenes. A fresh volatile QEMU run confirmed
the three save lamps by holding each button for three seconds:

| Scene | Panel target | MAIN event | Indicator output |
| --- | ---: | ---: | --- |
| A | 15 | 27 | row 2, bit 11 (`0x0800`) |
| B | 18 | 28 | row 2, bit 10 (`0x0400`) |
| C | 21 | 29 | row 3, bit 11 (`0x0800`) |

The scene buttons are clickable in the L6 panel; the renderer lights each from
its measured output. `L6_PANEL_SCENE_TRACE=1` runs a clean volatile boot and
holds only these three inputs while reporting the raw indicator rows.

## Unresolved or incomplete

- Logical LED 32 and indices 60–71 have no physical artwork assignment.
- Play/Stop and TAP illumination have not been assigned to verified outputs;
  their idle-state traces showed no indicator-row changes. Sound-pad playback
  lamps are mapped and verified above. Bounce is not an L6 control.
- SIGNAL assignments come from firmware routines; audio-driven transitions
  remain untested because audio processing is unmodeled. The L6 host ADC inputs
  control only the five global knobs on channels 3–7; channels 8/13 are fixed
  board-ID ladder values and channel 0 is synthetic. None supplies audio-level
  data, so knob motion cannot serve as a SIGNAL-lamp stimulus. For L6, the
  L6max address `0x8002fe98` is not an LED routine: in the L6 image it falls
  inside a function that builds a `0x31` report from packed state fields. Keep
  the L6 SIGNAL path unresolved until its own output routine and audio input
  path are identified.
- PWM brightness and physical LED colors are not calibrated.
- Analog knob travel and taper are approximate. Audio input and analog codec
  behavior remain unmodeled; see the [audio investigation](audio.md).
