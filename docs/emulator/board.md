# L6/L6max QEMU board

[Documentation index](../README.md)

The custom `l6max-dual` machine runs either model's main Cortex-M7 and panel Cortex-M0
with separate address spaces and a shared virtual clock. QEMU's Cortex-M0
stands in for the panel's Cortex-M0+. Internal UART links the chips, and the
main firmware can program, verify, and restart the panel's executable flash.

The L6 MAIN processor marking reads `MIMXRT 1041DFP6B 1N00X` (with the
visible space after `MIMXRT`); NXP's ordering part number is
`MIMXRT1041DFP6B`, identifying an i.MX RT1041 in the RT1040 family. NXP's
[part listing](https://www.nxp.com/part/MIMXRT1041DFP6B) and current
[RT1040 consumer data sheet, Rev. 5](https://www.nxp.com/docs/en/data-sheet/IMXRT1040CEC.pdf)
lists this part as a 600 MHz commercial-grade Cortex-M7 device without the
LCD/PXP feature, in a 169-pin, 9 × 9 mm BGA with 0.65 mm pitch. The L6max
MAIN processor marking has not been verified, so RT1052-specific silicon
claims must not be applied to both products. The shared QEMU board models
firmware-observed peripherals and wiring; it does not assert identical silicon.

For the confirmed L6 part, NXP's [MIMXRT1041 CMSIS feature header](https://raw.githubusercontent.com/nxp-mcuxpresso/mcux-devices-rt/main/RT1040/MIMXRT1041/MIMXRT1041_features.h)
lists 8 LPUARTs, 3 LPSPIs, 4 LPI2Cs, 2 USDHCs, one eDMA, and one DMAMUX.
That confirms the relevant peripheral families exist in the L6 silicon. It
does not establish that every instance is routed on the product board or that
L6max uses the same part. The RT1040 reference manual is the silicon-level
register source; firmware traces remain the source for the L6's actual
addresses, IRQs, and active instances. One naming discrepancy is unresolved:
the firmware path at `0x403a0000` had been called LPSPI4 by comparison with
RT1050 materials. NXP's [RT1041 CMSIS common header](https://raw.githubusercontent.com/nxp-mcuxpresso/mcux-devices-rt/main/RT1040/MIMXRT1041/MIMXRT1041_COMMON.h)
defines `LPSPI3_BASE` as `0x403a0000`; RT1041 has three LPSPI instances. Thus
the L6 path is LPSPI3, while LPSPI4 is the RT1050-family name used in earlier
analysis. The L6 firmware's register accesses and QEMU's synthetic NOR reply
are verified in emulation; that does not prove the external flash wiring,
electrical protocol, or returned bytes on physical hardware.

## QEMU device-model reuse

The confirmed RT1041 identity helps distinguish reusable QEMU devices from
blocks that only share a peripheral name. In the pinned QEMU 11.1.2 source,
the firmware's L6 DMAMUX source IDs 19/20 and 83/84 are confirmed by NXP's
[part-specific RT1041 SDK header](https://github.com/nxp-mcuxpresso/mcux-sdk/blob/main/devices/MIMXRT1041/MIMXRT1041.h#L1121-L1128)
as SAI1 and SAI3 RX/TX. The RT1050 manual remains only a family cross-check
for eDMA TCD details. QEMU's
`hw/i2c/imx_i2c.c` models the older i.MX I²C controller, not RT1041 LPI2C;
`hw/ssi/imx_spi.c` is ECSPI, not LPSPI. The i.MX8MP SAI and eDMA address
ranges are registered as unimplemented devices, and there is no RT eDMA model
in the tree. Those are not substitutes for the RT1041 audio path.
An external QEMU fork has tested SAI FIFO/audio and 32-bit-TCD eDMA models,
but its controller register maps differ from RT1041; see the
[audio model](audio.md#modeling-direction-and-remaining-work).

| RT1041 block in scope | Existing QEMU component | Reuse boundary |
| --- | --- | --- |
| LPI2C | Generic I²C bus and slave APIs; older `imx.i2c` controller | Keep the RT LPI2C register/FIFO controller; attach supported external devices through generic I²C. |
| LPSPI | Generic SSI bus; `imx_spi` ECSPI controller | Generic bus can host devices, but the RT LPSPI MMIO/DMA controller still needs firmware-matched behavior. |
| SAI + eDMA + DMAMUX | Generic audio backends and DMA helpers | The shared RT SAI model handles four channel FIFOs, RX FIFO-combine distribution and aggregated request/status state, advances at 48 kHz, supplies silent RX words, and routes SAI1 TX frames to an optional QEMU audio backend. Both models pass the audio-ring/DSP callback smoke. Earlier L6 Sound Pad probes reported audio, but a fresh L6 trace found the assignment-pending flag still set at the Pad 1 trigger; repeat the L6 mapping/audio checks after assignment completion. L6max targets 49–52 are audio-positive with matching WAVs assigned and corresponding pad LEDs changing, though runtime pad records remain zero. Recorder Play reaches its action but stream-file reads and TX remain open. Capture and physical jack routing remain open. See the [audio investigation](audio.md#modeling-direction-and-remaining-work). |
| USDHC / SD | `imx-usdhc`, SDHCI, and QEMU SD bus | Existing controller and card infrastructure is already used by the board. |
| USB OTG | ChipIdea host controller and USB device bus | Existing controller/device infrastructure is already used for USB MIDI and storage. |
| GPIO | i.MX GPIO controller and generic GPIO lines | Reuse GPIO primitives where register compatibility holds; board-specific pins and wiring remain profile data. |
| LPUART | `imx.serial` (IMX31 UART) and STM32 LPUART models | Neither is an RT1041 LPUART register model. Keep the RT LPUART register behavior and firmware-specific UART routing in the board layer; use QEMU chardevs for byte-stream transport. The panel's STM32 USART already uses QEMU's STM32L4x5 USART model. |
| Cortex-M7 / NVIC | QEMU `armv7m` device and ARM CPU models | The board already reuses these CPU/NVIC building blocks for both MAIN images; profile-specific IRQ counts and priority bits are configured at realization. |

This inventory is based on source inspection of the pinned QEMU tree, not just
device names or the RT1050 family. The board still uses custom adapters where
QEMU lacks a compatible register interface. See the [audio investigation](audio.md)
for the narrower SAI/eDMA findings and remaining audio work.

The main peripheral address range is currently backed by a sparse firmware-
focused MMIO model, with more accurate QEMU devices overlaid for GPIO and
USDHC/SD. This is deliberate where RT1041-compatible blocks are absent: a
same-named i.MX or STM32 controller is not register-compatible merely because
it exposes UART, SPI, or I²C. Replacing one of these handlers should follow
register/IRQ traces from both firmware families and keep the shared board
profiles working. The standalone L6 and L6max panel firmware use the same
STM32 USART device, while their GPIO wiring and control matrices remain
profile-specific.

## L6 family model selection

There is one auto-detected dual-MCU machine type, not separate L6 and L6max
machine types. The MAIN reset vector selects the model profile at boot. The
profiles currently share the same emulated CPU topology, isolated main/panel
address spaces, virtual clock, UART bridge, SD controller, and USB controller.
Their known differences are kept at the board-profile and control-surface
layers:

The C board path was also checked directly with the same `l6max-main` machine
and `L6_MODEL` unset: the stock L6 MAIN image logged `L6`, and the stock L6max
image logged `L6max`, each selected from its reset vector before guest
execution. This confirms profile choice does not depend on the Rust launcher.

| Hardware profile | L6 | L6max |
| --- | --- | --- |
| MAIN image base | `0x80001000` | `0x80000000` |
| Panel switch matrix | 8 rows × 3 columns | 8 rows × 5 columns |
| Panel encoders | 6 | 8 |
| Main-MCU panel UART | LPUART7 (IRQ 26) | LPUART1 (IRQ 20) |
| Panel ROM BOOT GPIO | GPIO3.7 | GPIO1.29 |
| Panel reset GPIO | GPIO2.29 | GPIO5.1 |
| External MIDI UART | LPUART2 | LPUART3 |
| Display | No screen | 128 × 64 OLED |
| External resource NOR | Present | Not modeled |

The panel ROM/update exchange uses the same profile-selected UART as normal
controls. L6 startup sets BOOT through GPIO3.7, pulses reset through GPIO2.29,
and uses LPUART7; L6max uses GPIO1.29, GPIO5.1, and LPUART1. The L6 values
are observed in the panel service path; the L6max values are associated with
its panel updater. The QEMU profile applies the
matching GPIO set/clear and UART paths automatically.

The L6-specific resource-NOR path and high-RAM overlay, matrix wiring, encoder
count, and MIDI UART selection are model-dependent. QEMU peripheral helpers
that both profiles use remain shared; the display, indicator, and USB models
are separate helper modules already. The available evidence does not justify two machine
types: both MAIN images use Cortex-M7 firmware and the observed
topology is covered by the existing shared machine. The L6 RT1041 identity and
still-unverified L6max silicon make the board profiles a compatibility layer,
not an assertion of identical SoCs. If hardware analysis establishes a
materially different SoC or reset/peripheral topology, keep common device
models shared and split only board initialization and wiring.

## Model fidelity and open approximations

The board is a firmware-focused model, not a complete RT1041 or L6max hardware
implementation. Keep observed compatibility behavior distinct from known
hardware semantics, and keep both distinct from unimplemented behavior. The
[manual audit](manual-audit.md) tracks register-level coverage; subsystem pages
below hold the detailed traces and validation evidence.

| Item | Scope and evidence | Change or removal criterion |
| --- | --- | --- |
| SAI1 RX read-stream compatibility exception | L6max only. The profile retains `FCOMB=2` but distributes received words across enabled FIFOs. Removing it restored roughly 750 channel-15 IRQ assertions/s and froze panel/encoder indicators; enabled behavior measured 187–188/s and is exercised by the eight-ring indicator smoke. This is empirical firmware compatibility, not established silicon behavior. | Revisit when firmware/eDMA analysis or physical SAI capture explains the indicator freeze and establishes correct RX placement. Until then, preserve the exception and its L6max-only scope. See [audio investigation](audio.md#observed-firmware-configuration). |
| Power cutoff fallback | L6 only. QEMU cuts virtual power after a 500 ms press because the stock L6 shutdown GPIO edge/path is unconfirmed. The interval is synthetic. | Replace it when the L6 firmware power-switch input, shutdown callback and expected board power signal are established. See [power behavior](power.md). |
| Codec and analog input | L6max codec addresses `0x4c`/`0x4d` accept observed writes through register-bank slaves; physical codec identity is not confirmed. ADC samples are host controlled, with conversion timing and analog sources approximated. SAI RX input is silent. | Expand or replace these models when component identity, register transactions, timing and physical signal routing are established. See [audio investigation](audio.md) and [manual audit](manual-audit.md). |
| Partial peripheral behavior | RT eDMA/SAI/LPI2C/LPSPI/LPUART/ADC and FlexSPI use firmware-focused implementations because the pinned QEMU tree has no compatible RT1041 drop-in blocks. Observed paths are modeled, while unobserved commands, timing, flags and features remain incomplete; UART and LPI2C timing are approximate. | Implement additional semantics when firmware traces exercise them or manual-based checks require them. Do not treat a passing firmware smoke as full peripheral conformance; use the [manual audit](manual-audit.md) to track coverage. |
| Clock and oscillator assumptions | The SAI frame cadence uses the firmware's nominal 24.576 MHz source value; bootloader-inherited clock roots are unverified. Several PLL/oscillator transitions settle immediately on virtual time. | Replace assumptions with established bootloader clock state and documented transition timing. See [clock findings](manual-audit.md#findings). |

These rows describe current implementation limits, not permission to bypass
hardware semantics silently. Any future exception should name its model scope,
observation, regression coverage and evidence that would allow it to be removed.

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

Supply `L6.BIN` or `L6max.bin` locally and extract it using the
[package tool](../guides/firmware-packages.md). Package detection is based on
the validated system header, and QEMU selects the MAIN flash mapping and both
integrated MCU profiles directly from the booted MAIN reset vector.
`l6max-host` also detects the model for its UI and input profile. The
split-process `qemu-run` detects the same MAIN image and passes the result to
its standalone panel guest, which has no MAIN image of its own. Fresh volatile
boots of the stock L6 and L6max 1.10 packages with the same QEMU binary selected
the corresponding profile from each MAIN reset vector; use the `l6-input-sweep`
and `l6max-input-smoke` driver cases for input validation.
The L6 run checked 24 matrix keys, five direct switches, six encoders, and the
no-display path. The L6max run published display changes and accepted Menu,
Play, Record, encoder, and date-editor input. The standalone two-process `qemu-run`
diagnostic now captures live registers for both L6 and L6max. The L6 main CPU
also reads its external `resource_region.bin` NOR image through LPSPI3 at
`0x403a0000`. The current model reaches its Idle task, but its LCD framebuffer
mapping and published display frames remain unknown. The L6max-only framebuffer
snapshot is therefore omitted for L6. On L6 the panel USART uses the external
serial socket to reach the separate main-chip process; the integrated machine
keeps its in-process UART bridge. Firmware packages and extracted components
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

For L6max, QEMU consumes OLED SPI/eDMA transfers into display controller RAM.
Completed transfers publish immutable frames in two anonymous shared-memory
slots. The L6 profile has no OLED, closes the host display descriptor, and
keeps LPSPI3 at `0x403a0000` assigned to its external resource NOR.
Inherited socket pairs carry publication and consumption notifications,
button/encoder commands, and guest-time acknowledgments. QMP management uses
a separate inherited socket. See the [native UI architecture](frontend.md)
for ownership and transport details.

The engine uses single-threaded TCG with
`-icount shift=2,align=on,sleep=on` to enforce virtual timer deadlines and pace
execution against wall time. `--timing adaptive` selects the previous automatic
instruction clock. See [CPU consumption](frontend.md#cpu-consumption)
for the idle-loop findings and comparison harness. Panel TIM2 derives its period from the firmware's
registers at 48 MHz; the current configuration gives 50 µs. Button holds and
release gaps default to 50 ms of guest time. QMP pause freezes both chips and
device timers. Firmware owns GPIO scanning, debouncing, and key reports. The
host selects the matching physical input wiring from the detected model.

### L6max panel inputs

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

### L6 panel inputs

The L6 panel scans 24 matrix switches (8 rows by 3 columns on GPIOB7–9), five
direct GPIO switches, and six rotary encoders. The direct-switch GPIO-block and
pin pairs, in firmware scan order, are `(2,13)`, `(1,15)`, `(0,8)`, `(2,6)`, and
`(2,7)`. The panel emits matrix events as column, row, and state; state 0 is
release (`90`) and state 1 is press (`91`).

The host input socket uses targets 7–30 for matrix positions in row-major order
(`7 + row * 3 + column`) and targets 1, 2, 3, 6, and 47 for the five direct
switches in scan order. Encoder targets 0–5 drive the six L6 rotary encoders;
target 6 is rejected. These are simulator input IDs, not printed control
labels. Analysis of the supplied L6 1.10 MAIN image identified its
panel-report-to-event table: report columns 1–3 map matrix positions to logical
event indices `7,0–5,20`, `12–19`, and `26,6,27–29,31` (in row order); column 0
maps direct rows 1–4 to indices 8–11. MAIN submits a press as event state 0 and
a release as state 1. Column-0 row 0 and matrix-column-3 rows 6–7 have no
event. The `l6-panel-event-map` driver scenario passed against L6 1.10 for all
26 event-producing inputs, verifying each complete report coordinate and the
event index submitted to MAIN. Additional live mappings identify the mute,
MONO/USB source, mode, effects, phantom-power, scene, and Record controls. The
GUI enables these panel targets and the mapped main-GPIO TAP and sound-pad
targets. L6 Play target 4 maps to active-low GPIO2.25 and submits runtime event
`0x20`; saved-state recorder WAV playback is verified through SAI1 TX, while
physical key wiring remains unverified. Active-low GPIO1.9 is
the separate Power Switch input (event `0x22`). Some remaining matrix
coordinates and their user-visible effects still need faceplate correlation. The L6 firmware configures SDK UART
instance 6 (LPUART7) at
115200 baud, base `0x4019c000`, IRQ 26; the L6max panel link remains on LPUART1.
The QEMU profiles route the panel link and ROM-entry GPIOs accordingly. A normal
L6 `l6dbg uart-startup --exercise-inputs` run completes ROM sync, reaches the
panel parser (`0x08000544`), event callback (`0x80039ef8`), and dispatcher
(`0x800393d0`), then records panel UART reports for injected inputs. The L6
probe injects one encoder detent per channel at two-second intervals and
captures report-buffer and dispatcher state. A stock L6 1.10 run produced one
`A1` report and one MAIN dispatcher hit for each channel, retaining channel
IDs 0–5, but none of these single detents changed raw indicator rows. This
confirms delivery to MAIN dispatch. A separate L6 dispatch trace records
class-5 state events for parameter IDs 35–40 with delta `-1`, but none reaches
the candidate mixer setters. The DIN class-2 path does reach those setters and
updates mixer RAM. The class-5 records enter and drain from MAIN's queue; the
trace sees 12 calls total to routines `0x800344e0` and `0x80034530` across the
six isolated turns. MAIN subtracts `0x2c` from the event parameter before range
checks, so IDs 35–40 take the out-of-range branches. Per-turn snapshots show no
change in the sampled mixer state. The remaining gap is to determine whether
these routines affect other state or drive L6 ring outputs; ring output cannot
yet be attributed to encoder wiring. The L6 input smoke passes for all 29
switches and six encoders.
Native UI smoke also exercises the identified faceplate controls and checks
the measured button, effect, mode, and scene lamps. Some event labels and
downstream hardware effects remain unconfirmed.

### MAIN-MCU button GPIO inputs

The MAIN-MCU GPIO inputs are model-specific. A matched QEMU probe held Menu,
Play, and Record for five seconds each; L6max sampled all three. L6's normal
`EventPort` scan selects GPIO2.31, GPIO2.30, GPIO1.21, GPIO1.20, and GPIO2.22.
Controlled L6 playback probes map the first four to Sound Pads 1–4 (targets
49–52), and GPIO2.22 to TAP (target 48). L6max's key-reader table
maps targets 49–52 to GPIO1 bits 21, 25, 26, and 0. Four isolated QEMU runs
selected those bits, produced SAI1 audio with the corresponding WAVs assigned,
and changed matching pad LEDs 27–30; runtime pad records remained zero.
GPIO2.30 stays at its startup board-ID level and is not used for L6max Pad 2.
Physical button-to-pin wiring remains unverified. L6 target 4 maps to
active-low GPIO2.25 and is sampled by the normal GPIO scan as Play event
`0x20`. Active-low GPIO1.9 is Power Switch event `0x22`. L6-specific targets
are available through the host input API and standalone `qemu-run` scheduler. L6 does not claim the L6max Menu or
main-GPIO Record targets; its Record key maps through panel target 24 instead.
Idle presses of the five mapped L6 GPIO controls did not change indicator rows.
Some scanned panel coordinates remain unlabelled.

## Validation and diagnostics

Run the live integration check:

```sh
cargo run --offline --locked -p l6test -- run --case l6-input-sweep --model l6 --config smoke.local.toml
cargo run --offline --locked -p l6test -- run --case l6max-input-smoke --model l6max --config smoke.local.toml
```

On L6max it checks published display changes, exact press/release counts,
guest-time release deadlines while paused, encoder reporting, and Date/Time
editing. On L6 it checks all 24 matrix positions, five direct switches, six
encoders, and rejects inputs outside the model's hardware range. Logs and
screenshots are generated locally and excluded from Git.

The standalone `qemu-run` tool provides separate-chip diagnostics, register
snapshots, RAM probes, and framebuffer captures. It uses a separate-process
launch arrangement; use the native app for integrated dual-chip behavior.

```sh
cargo run -p qemu-run -- --qemu /path/to/qemu-system-arm --seconds 48 \
  --button confirm:38000:39000 --button down:40500:42000
```

`--button NAME:START_MS:END_MS` schedules a button interval in guest time.
`--panel-key ROW:COL:START_MS:END_MS` and
`--main-key NAME:START_MS:END_MS` select raw inputs. Main-key names are model
specific: L6 accepts `play`, `tap`, and `sound-pad-1` through `sound-pad-4`.
Hold L6 `play` after startup to exercise its GPIO2.25 runtime event path;
playback remains unverified. L6max accepts `menu`, `play`, and
`record`. The short
`--button` form supports the same main-key names as well as the panel
navigation controls.
`--probe ADDRESS:SIZE` and `--panel-probe ADDRESS:SIZE` capture RAM.
`--trace-sd` writes QEMU's SDHCI trace to `sdhci.trace` in the logs directory.
Native CPU/system output remains in `engine.trace` and `engine.stderr`; the
standalone runner writes its `main.*` and `panel.*` files there as well.

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
It acknowledges staged audio without processing it. A separate 1 ms service
can acknowledge audio startup waits through opt-in diagnostic overrides. Both
model defaults now leave those acknowledgments disabled because clocked
SAI/eDMA reaches the firmware callbacks. The audio clock feeds silence on RX;
configured QEMU audio backends receive SAI1 TX frames. See the
[audio model](audio.md#observed-firmware-configuration).
The periodic queue acknowledgment uses profile-specific SRAM indices: L6
producer/consumer offsets `0xbb0c`/`0xbb10`, L6max `0x7d08`/`0x7d0c`.
Using the L6max offsets on L6 left its staged queue unconsumed and blocked
Main's initialization. The persistent-state DIN smoke now verifies ordinary
L6 ingress end to end: the UART delivers all 384 bytes, Main consumes all 66
mapped events, and the native mixer setters update state. The separate
event-dispatch smoke continues to isolate queue consumption from UART ingress.
An opt-in audio trace captures SAI register windows and firmware eDMA TCDs for
both models. The board executes request-paced transfers for two full-duplex
SAI paths on L6 and one on L6max; see [audio peripheral investigation](audio.md).

## Limitations

- UART byte timing remains an approximation.
- Main display SPI transfers complete instantly in virtual time. Panel SPI1
  DMA transfer duration uses its configured baud divider.
- Instruction-count timing does not model hardware instruction cycle costs.
- Physical encoder direction has not been verified on hardware.
- The exact display controller remains unidentified.
- Broader SD and audio behavior remains incomplete.
- Audio capture and analog codec behavior are unmodeled. The queue service
  remains a recorder-screen stand-in. Host output is verified for L6 and L6max
  pad playback. Physical jack routing remains unverified. See the
  [audio investigation](audio.md).
- Several peripherals use sparse register storage instead of full device
  models.

## Reusable QEMU peripheral blocks

The vendored QEMU 11.1.2 source already provides useful models, but most are
not register-compatible drop-ins for every MAIN MCU peripheral. The L6 is
confirmed as RT1041; L6max silicon is unverified. MAIN and panel use the same
sparse MMIO handler in `l6max.c`; selected devices are overlaid with
individual QEMU models. This inventory separates existing reuse from possible
candidates:

For L6, the peripheral-family comparison below is grounded in the RT1041
CMSIS device files and firmware-observed accesses. QEMU block reuse is based on
register behavior and smoke coverage, not on the shared `i.MX` product name.

| Block | QEMU support | L6/L6max fit |
| --- | --- | --- |
| SD/MMC | `imx-usdhc` | Already used for the MAIN SD slot, with a small board adapter for the SoC system-control behavior. |
| Panel UART | STM32L4x5 USART/LPUART | Already used for the panel-to-MAIN serial link. The MAIN RT LPUART instances have a different register interface. |
| GPIO | i.MX GPIO and STM32L4x5 GPIO | MAIN now reuses QEMU's i.MX GPIO register and pin-input model, with a board adapter for RT GPIO atomic aliases and panel BOOT/RESET wiring. STM32L4x5 GPIO remains a partial panel match; its exported pin IRQs report sampled input changes, not output-pin callbacks. |
| CCM_ANALOG | No complete RT1041 CCM/PLL model | The board models the observed PLL_AUDIO lock poll and USB1 PLL control/SET/CLR/TOG state. PLL lock settles immediately on virtual time. SAI frame cadence is derived from the active master divider and frame format using the firmware's nominal 24.576 MHz source assumption; inherited bootloader clock-root values and non-audio peripheral rates remain unverified/unmodeled. See the [manual-led audit](manual-audit.md). |
| MAIN UART | i.MX serial and STM32L4x5 USART/LPUART | Neither is a demonstrated match for MAIN RT LPUART. The i.MX serial model is the older UART register block; the STM32 model is currently used for the panel-side link. |
| MAIN SPI | i.MX ECSPI and several unrelated controllers; generic SSI bus and M25P80 serial-NOR device | No drop-in RT LPSPI controller exists in the pinned QEMU tree. L6 firmware framing is known: `0x0b` fast read uses a 24-bit address and dummy byte, and `0x05` reads status. The reusable SSI bus and `m25p80` flash model (`hw/block/m25p80.c`) are candidates for replacing the hand-written command parser once the physical flash type/capacity and required write behavior are confirmed. L6's LPSPI3 path at `0x403a0000` and L6max's OLED SPI still need firmware-matched controller behavior; L6max's silicon-level instance name remains unverified. |
| I²C | i.MX I²C controller and generic I²C bus/slave framework | The controller uses a different register set from RT LPI2C. The L6max MMIO adapter forwards observed START, data, and STOP commands to QEMU's I²C bus and two lightweight register-bank slaves at `0x4c`/`0x4d`; writes match a TLV320ADC6140-compatible configuration, but do not verify physical silicon or analog behavior. L6 made no LPI2C1 accesses in the tested stock boot. |
| ADC | Controllers for STM32F2, Aspeed, NPCM, and Xilinx devices | No compatible RT ADC model is present. The current board shim supplies conversion completion and host-controlled samples. |
| USB | Generic USB device framework, ChipIdea EHCI host model, and i.MX USB PHY | The ChipIdea controller is an EHCI host model; its generic registers are mostly stubs. It does not replace the custom RT USB device-controller register and endpoint path used for USB MIDI. |
| Audio DMA / SAI | No drop-in RT eDMA or SAI controller in pinned QEMU | The board has shared RT SAI register/FIFO state and request-paced eDMA for the observed descriptor layout, with separate L6/L6max request routes. SAI1 TX can use a QEMU audio backend. Earlier L6 Sound Pad and recorder probes reported output; a fresh GPIO trace showed the Pad 1 assignment still pending, so the four-pad mapping/audio result needs retesting after completion. L6max targets 49–52 each produced audio with matching LEDs when their WAVs were assigned. L6max SNARE8 data is verified in DMA slots 4–7 and host output uses slots 4–5. Runtime L6max pad records remain zero; recorder capture/playback and physical jack routing remain open. See the [audio peripheral investigation](audio.md#modeling-direction-and-remaining-work). |

QEMU's i.MX GPIO model now backs all five MAIN GPIO banks on both firmware
profiles. Its `DR`, `GDIR`, `PSR`, interrupt configuration, and status
registers use the offsets observed in the L6/L6max firmware. A thin board adapter
implements the reserved `+0x80` window and the atomic aliases at local offsets
`+4/+8/+0xc`, which also drive panel BOOT/RESET behavior. Host switches and
buttons enter through QEMU GPIO input lines; board telemetry, GPIO2 display
D/C, SD detect, and power-hold behavior remain board-level wiring. L6 and
L6max input smokes both pass with this adapter, and model-specific power
smokes verify cutoff and restart behavior.

The STM32L4x5 GPIO model is also close to the panel register layout. Its
`MODER`, `PUPDR`, `IDR`, `ODR`, and `BSRR` registers match the accesses handled
by the current panel path. Its pin IRQ lines represent changes in sampled
input state, though, and do not expose driven output levels. The current LED
matrix decoder observes panel output writes, so adopting this model would
require preserving that observation in a board adapter or adding an output
signal to the device. Either route is still smaller than replacing its GPIO
register behavior with a new implementation.

This inventory is specific to the pinned QEMU submodule revision
`4fc49f46dc95d4a27de2509e7fceb2931e91faeb`; revisit it if that revision
changes. Implementations are under `emulator/qemu/upstream/hw/` and device
declarations are under `emulator/qemu/upstream/include/hw/`.

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
