# L6max QEMU board

[Documentation index](../README.md)

The custom `l6max-dual` machine runs the main Cortex-M7 and panel Cortex-M0
with separate address spaces and a shared virtual clock. QEMU's Cortex-M0
stands in for the panel's Cortex-M0+. Internal UART links the chips, and the
main firmware can program, verify, and restart the panel's executable flash.

## Build and run

Initialize the QEMU 11.1.2 submodule with
`git submodule update --init emulator/qemu/upstream`. It is pinned to
`4fc49f46dc95d4a27de2509e7fceb2931e91faeb`. The builder stages and patches it
in the ignored `emulator/qemu/build-source/` tree, retaining build outputs.
An external QEMU 11.1.2 tree can also be passed explicitly and is patched in place.
The original source archive used during development has SHA-256
`731b5681e4bb18be313231579b8efd0296c5b015fa36dc533874b639ba838016`.
QEMU's upstream build requires Python, Ninja, a C compiler, and libfdt. The
repository's UI and host tools are Rust. macOS UI builds also require Xcode's
Metal Toolchain component.

Supply `L6max.bin` locally and extract it using the
[package tool](../../README.md). Firmware packages and extracted components
are excluded from Git.

From the repository root:

```sh
cargo run -p qemu-build
cargo run -p l6max-gui -- emulator/qemu/build-source/build/qemu-system-arm
```

The native app starts and owns the QEMU process. Closing the window shuts down
both the UI and guest. `--firmware-dir DIR` selects the extracted images;
`--logs DIR` selects runtime output. Attach a raw SD image with
`--sd-image /path/to/card.img`.
The [SD and USB implementation](usb.md) covers card detection and
hotplug, native macOS MIDI with `--usb`, and host SD file access through USB.
The [nonvolatile storage map](storage.md) describes persistent
main NOR, panel flash, option bytes, RTC state, and installed version records.

## Display and input

QEMU consumes SPI/eDMA transfers into display controller RAM. Completed
transfers publish immutable frames in two anonymous shared-memory slots.
Inherited socket pairs carry publication and consumption notifications,
button/encoder commands, and guest-time acknowledgments. QMP management uses
a separate inherited socket. See the [native UI architecture](frontend.md)
for ownership and transport details.

The engine uses single-threaded TCG with
`-icount shift=3,align=on,sleep=on` to enforce virtual timer deadlines and pace
execution against wall time. `--timing adaptive` selects the previous automatic
instruction clock. See [CPU consumption](frontend.md#cpu-consumption)
for the idle-loop findings and comparison harness. Panel TIM2 derives its period from the firmware's
registers at 48 MHz; the current configuration gives 50 µs. Button holds and
release gaps default to 50 ms of guest time. QMP pause freezes both chips and
device timers. Firmware owns GPIO scanning, debouncing, and key reports.

| Physical button | Modeled input | Firmware event |
| --- | --- | --- |
| Menu | Main GPIO5 bit 2 | `FUNC1` (46) |
| Up (`\|<<`) | Panel row 5, column 4 | `FUNC2` (47) |
| Down (`>>\|`) | Panel row 4, column 4 | `FUNC3` (48) |
| Confirm (Undo) | Panel row 7, column 4 | `FUNC4` (49) |
| Play/Pause | Main GPIO1 bit 20 | `PLAY` (44) |
| Record | Main GPIO5 bit 0 | `REC` (43) |
| Bounce | Panel row 6, column 4 | `BOUNCE` (45) |

The eight main buttons are active low. Panel columns use GPIOB7/8/9, GPIOC13,
and GPIOF0; GPIOB4–6 select the scan row. Panel key presses and releases are
reported as `91 <bank> <row>` and `90 <bank> <row>`.
The [control wiring map](controls.md) covers all 50 buttons and switches,
the persistent switches, the separate MASTER/SUB-MIX input polarity, and
the indicator assignments.
Power uses active-high main GPIO2 bit 25. The five global knobs are main ADC1
inputs on channels 3–7; the model supplies variable 10-bit conversion results
and retains firmware scanning, filtering, and event generation.

The eight channel knobs drive quadrature GPIO inputs. The panel reports them
as `A1 <channel> <direction> <count>`:

| Knob | Channel | Phase A | Phase B |
| --- | ---: | --- | --- |
| 1 | 0 | GPIOA0 | GPIOA1 |
| 2 | 1 | GPIOA2 | GPIOF1 |
| 3 | 2 | GPIOA11 | GPIOA12 |
| 4 | 3 | GPIOA15 | GPIOD0 |
| 5 | 4 | GPIOD1 | GPIOD2 |
| 6 | 5 | GPIOD3 | GPIOB3 |
| 7 | 6 | GPIOB15 | GPIOA8 |
| 8 | 7 | GPIOC6 | GPIOC7 |

## Validation and diagnostics

Run the live integration check:

```sh
cargo run -p input-smoke -- --qemu /path/to/qemu-system-arm --logs emulator/logs/input-smoke
```

It checks published display changes, exact press/release counts, guest-time
release deadlines while paused, encoder reporting, and Date/Time editing.
Logs and screenshots are generated locally and excluded from Git.

The standalone `qemu-run` tool provides separate-chip diagnostics, register
snapshots, RAM probes, and framebuffer captures. It uses a separate-process
launch arrangement; use the native app for integrated dual-chip behavior.

```sh
cargo run -p qemu-run -- --qemu /path/to/qemu-system-arm --seconds 48 \
  --button confirm:38000:39000 --button down:40500:42000
```

`--button NAME:START_MS:END_MS` schedules a button interval in guest time.
`--panel-key ROW:COL:START_MS:END_MS` and
`--main-key NAME:START_MS:END_MS` select raw inputs. `--probe ADDRESS:SIZE`
and `--panel-probe ADDRESS:SIZE` capture RAM. `--trace-sd` adds SD controller
logging. Native output is `engine.trace` and `engine.stderr`; standalone
output uses `main.*` and `panel.*` files.

## Model findings

Investigation pointed to the observed flash layout, selected FlexSPI reads, ADC
startup responses, display DMA descriptors, GPIO inputs, UART traffic, panel
RTC state, and a partial STM32 ROM update protocol as the required device
interfaces. The model implements these paths. Panel flash writes affect
the running chip's executable memory; reset and boot selection hold the CPU
during programming and restart it afterward. Each chip's guest-requested
reset is local, including NVIC and SysTick state.

Date/Time and Battery Type setup reach the recorder's **No SD Card** screen
with the optional audio queue acknowledgment. The native app enables that
acknowledgment; standalone diagnostics require `--service-audio-queue`.
It acknowledges staged audio without processing it.

## Limitations

- UART byte timing remains an approximation.
- Main display SPI transfers complete instantly in virtual time. Panel SPI1
  DMA transfer duration uses its configured baud divider.
- Instruction-count timing does not model hardware instruction cycle costs.
- Physical encoder direction has not been verified on hardware.
- The exact display controller remains unidentified.
- Broader SD and audio behavior remains incomplete.
- Audio processing is unmodeled; queue acknowledgment is a diagnostic
  approximation.
- Several peripherals use sparse register storage instead of full device
  models.

## Panel indicator wiring

Investigation of the panel refresh routines pointed to an eight-common,
24-column LED matrix. The external matrix model consumes peripheral pin
outputs and exposes all 192 outputs independently of UART decoding.

| Signal | Panel pin / peripheral | Model behavior |
| --- | --- | --- |
| Serial clock | GPIOA5 / SPI1 SCK | Shift on rising edges |
| Serial data | GPIOA7 / SPI1 MOSI | MSB first, three bytes per transfer |
| Latch | GPIOA6 | Rising edge copies shift register to output register |
| Blanking | GPIOA4 | High disables outputs |
| Commons 0–7 | GPIOB0, B1, B2, B10, B11, B12, B13, B14 | Active low |
| Transfer source | DMA1 channel 1 | CMAR to SPI1 DR, CNDTR bytes |
| Transfer completion | TIM14, IRQ 19 | Firmware checks SPI and invokes its completion callback |
| LED dwell | TIM3, IRQ 16 | Firmware advances the common and PWM phase |

Panel routines at `0x080014e4`, `0x08000ca0`, `0x08000cb4`, and
`0x08000cc6` establish the GPIO control signals. `0x080018d4` starts each
three-byte transfer. `0x08002850` latches the data and selects its common;
`0x08002834` blanks the outputs before the next transfer. Firmware indicator
column `n` corresponds to bit `n ^ 7` in the serial output word: the
firmware reverses byte order while assigning bits within each byte MSB first.

Indicator events use runtime message kind `0x80000005`: sequence is a nonzero
publication counter, target is common 0–7, value is the 24-bit lit-column mask,
and the final word is guest virtual milliseconds. Only changed rows are sent.
The 50 ms observation window represents lit/unlit state, not calibrated PWM
brightness. The UI retains unmapped columns for future hardware mappings.

The panel also publishes kind `0x80000008` every 50 guest milliseconds,
with a nonzero sequence, zero target/value, and guest time in the final word.
This clock notification advances even when neither LEDs nor LCD pixels change;
input tests use it instead of a display publication timestamp.

Main GPIO output changes use kind `0x80000006`: target 0–4 identifies GPIO1–5,
and value contains all 32 output-latch bits. DR, DR_SET, DR_CLEAR, and DR_TOGGLE
writes update the latch. These outputs are retained for analog-control
investigation and do not drive the artwork's scene indicators.

The main ring function at `0x8000c4f0` writes nine consecutive logical LEDs
per channel, starting at logical index 72. The wiring table at `0x800c457c`
maps logical indices to common/column pairs. Ring pairs occupy
`0x800c460c` through `0x800c469b`; the native renderer preserves this wiring
in `emulator/host/src/indicators.rs`.

Pin-level decoder checks (from the repository root):

```sh
cc -std=c11 -Wall -Wextra -Werror emulator/qemu/tests/indicators.c -o /tmp/l6-indicators
/tmp/l6-indicators
```
