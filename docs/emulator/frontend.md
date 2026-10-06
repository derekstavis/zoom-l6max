# Native L6max UI

[Documentation index](../README.md)

The Rust/GPUI application composes the mixer from reusable native controls and displays the firmware framebuffer.
It starts one `l6max-dual` QEMU process containing the Cortex-M7 and panel
Cortex-M0, each with its own address space. Both CPUs use the same QEMU virtual
clock and communicate through a modeled internal UART.

## Run

For app bundles, see [firmware selection and packaging](../guides/macos-app.md).
The commands below describe development runs from a source checkout.

Build QEMU using [the board instructions](board.md), then run from
the repository root:

```sh
cargo run -p l6max-gui -- /path/to/qemu-system-arm
```

The application owns QEMU's lifetime. It uses `L6_QEMU` or the local
`/private/tmp/l6-qemu-src/build/qemu-system-arm` when no path is supplied.
Without QEMU it shows the panel only. Optional arguments are
`--firmware-dir DIR`, `--logs DIR`, `--sd-image FILE`, and
`--timing paced|adaptive` (default: `paced`).
Firmware is supplied locally and excluded from Git.

The normal persistent visual UI creates `emulator/state/sd-card.img` on first
launch and reuses it thereafter. With `--state-dir DIR`, the card is `DIR/sd-card.img`.
It is a 128 MiB FAT16 card with 16 KiB clusters, the locally supplied
`L6max.bin` copied to `/L6max.BIN`, and four generated one-second mono
48 kHz / 16-bit WAV tones in `/SOUND_PAD/PAD1` through `/SOUND_PAD/PAD4`.
These paths and audio format follow the
[ZOOM operation manual](https://zoomcorp.com/manuals/l6max-en/).
Existing cards are never repopulated or overwritten. `--sd-image FILE` selects
your own card; `--no-sd` starts without a card. Disposable `--volatile` runs and
headless tools retain explicit SD selection.

Try the samples in **Menu → Sound Pad → Sound Assign**. Audio output is still
unmodeled; file browsing and assignment exercise the firmware's SD paths.
The original package is version 1.10, so use **Menu → System → Firmware →
Firmware Update** to try reinstalling it; a newer-version startup prompt is not
expected. Select **Execute**, hold **Power** for about 3.5 seconds and release.
When the display goes blank, click **Power** again. The standard persistent visual UI
automatically enables the replacement installer on power-on. If first-boot
Date/Time appears, confirm its settings before opening the menu. A freshly
created card can also show the firmware's “New card detected — Test the card?”
notification on first insertion.

To create another populated card, including one with a locally patched package:

```sh
cargo run -p sd-image -- /path/to/card.img --demo /path/to/L6max.bin
cargo run -p l6max-gui -- --sd-image /path/to/card.img
```

Flash and the RTC backup domain persist in `emulator/state/`. Select a device
with `--state-dir DIR`, or use `--volatile` to discard storage on exit. See the
[nonvolatile storage map](storage.md) for version records,
settings banks, calibration, and restart validation.

The standard persistent visual UI enables a Rust replacement for the unavailable SD installer;
headless tools enable it with `--update-bootloader`. It runs
before QEMU starts. It acts only on a pending update record in persistent NOR,
reads `L6max.BIN` from the attached SD image, and supports the observed MAIN-only
package layout. Restart the app with the same state and SD after update
preparation. It preserves the bootloader region, calibration and settings;
the actual main firmware performs panel programming. See the
[firmware update map](../firmware/updates.md) for checks and limits.

Use `--usb` to expose the firmware's three USB MIDI ports through CoreMIDI on
macOS. The [SD and USB guide](usb.md) describes card-image
creation, host file transfers, controller wiring, validation and current limits.

The window starts at 1200×464, fitted to the device body rather than the
promotional image margins. Resizing
scales the panel, LCD, button hit zones, and encoder zones together. macOS
constrains the content aspect ratio; the minimum content size is 600×232.
Status appears in the native title bar. Closing the window quits the app and
stops/reaps QEMU. Application quit explicitly shuts the guest down.

## Display transport

Rust creates inherited Unix socket pairs for typed input/display messages and
QMP management. The `qapi` crate handles QMP.

Two completed 1 KiB display frames are shared with the UI. On macOS the
segment is created with `shm_open` and immediately unlinked; on Linux it uses
`memfd_create`. The descriptor identifies a kernel memory object. QEMU owns
guest RAM privately.

Investigation pointed to SPI/eDMA transfers as the display update boundary.
QEMU consumes those transfers into display controller RAM. Completed transfers
publish a frame slot and generation notification. The slot stays immutable
until Rust copies it and acknowledges consumption. When both slots are
occupied, the producer coalesces pending completed frames. The GPUI frame tick
drains notifications and paints the newest frame.

## Native panel components

The panel uses reusable GPUI components in `emulator/gui/src/components/`: native canvas
encoders and colored analog knobs, buttons with text or icons, equal-sized
switches, sockets, indicator LEDs, labels, and dividers. `components/layout.rs` defines
shared channel widths, control rows, and a common knob baseline. Rendering and
input hit zones consume the same regions. Reference positions use one common
body-origin translation for drawing, input, and LCD placement. The firmware display remains a
128×64 buffer with a 2:1 aspect ratio.

Small icons come from [Lucide](https://lucide.dev/) and
[Phosphor](https://github.com/phosphor-icons/core) and are embedded at compile
time using GPUI's cached SVG masks. The pinned revision and licenses are in
[assets/icons](../../emulator/gui/assets/icons/README.md). No manufacturer logo, traced panel SVG,
or product photograph is bundled. The development profile optimizes
GPUI and its icon renderer while retaining application debug assertions and
symbols.

The native composition places the outer body, blue strips, staggered combo
inputs, recorder enclosure, and LCD opening using shared layout constants.
Repeated controls use shared dimensions and baselines. A plain EMULATOR label
replaces manufacturer artwork; control symbols use Lucide and Phosphor icons.
Native macOS text requires the `font-kit` feature on `gpui_platform`.

### Indicator surface

Investigation pointed to a serial LED matrix driven by panel SPI1 and GPIO.
The model consumes SPI clock/data edges, latches the serial columns on GPIOA6,
and gates them through GPIOA4 and the eight GPIOB commons. TIM3 and TIM14 run
the firmware's refresh callbacks; DMA1 channel 1 supplies the SPI bytes.

The host receives changed 24-column row masks over the inherited panel socket.
It retains the latest eight rows without queuing UI work. A 50 ms guest-time
window combines multiplexed PWM pulses into lit/unlit outputs. No UART messages,
encoder target values, or firmware RAM variables drive the indicators.

The native renderer maps the 72 ring outputs to nine native canvas arcs per knob. Investigation pointed to the main firmware wiring table at
`0x800c460c` and the nine-output ring function at `0x8000c4f0`. The outputs
are distributed across commons; a common does not represent a whole knob.
The renderer also maps mute, Hi-Z, phantom power, channel modes, the blue
control strip, sound pads, Record, TAP, scenes, effect selection, and SIGNAL
indicators. The [control wiring map](controls.md) records logical
indices and firmware evidence. SIGNAL uses red priority when both colors are on.

Dragging a knob upward or right raises its level; downward or left lowers it.
The first movement locks the drag axis until release. Scrolling also adjusts
the knob, retaining fractional trackpad input. The cursor is an open hand over
a knob and a closed hand while dragging. Firmware computes the ring output,
including its mode-dependent pattern.

- Individual PWM brightness levels are not rendered yet.
- Play/Stop and Bounce illumination, logical LED 32, and indices 60–71
  have no verified visual assignments.
- Audio-driven SIGNAL transitions remain untested.

Control bounds come from the shared layout specification. All switches use the
same physical dimensions, channel controls share row baselines, and all thirteen
knobs are aligned. The recorder display preserves its pixel aspect ratio and
pale blue tint. Rings follow the GPIO indicator surface. Meters are drawn unlit.

GPUI is pinned to a Zed revision; macOS builds require Xcode's Metal Toolchain
component.

## Input and timing

Button down/up and encoder commands enter QEMU's device event loop. Button
minimum duration and release gaps use guest virtual time. The default minimum
is 50 ms. Physical Up maps to panel row 5, column 4; Down maps to row 4,
column 4. Encoder steps become quadrature GPIO transitions.

The four dB switches and MASTER/SUB-MIX toggle persistent GPIO state on a
click. Mouse release does not undo their position. The remaining controls
produce momentary closures. Input IDs 0–6 retain the recorder mapping; matrix
IDs are `7 + row * 5 + column`, with navigation aliases using their original
IDs. ID 47 is MASTER/SUB-MIX, 48 is TAP, and 49–52 are the sound pads.
ID 53 is Power, an active-high main GPIO2 bit 25 input. Firmware owns its
debounce and long-press recognition.

Hold Power for about 3.5 seconds and release to shut down through firmware.
The board power-hold output, GPIO3.3, determines the off transition after
cleanup. The UI stops QEMU, blanks the display and LEDs, and keeps the window
open. Click Power again to start with the same flash, RTC and SD card, applying
any prepared update. Switch and analog knob positions are retained across
the cycle. `--volatile` discards device storage at power-off. See the
[power lifecycle](power.md) for evidence and tests.

SOUND PAD, EFX RTN, MASTER, MONITOR, and SUB-OUT gestures set main ADC1
channels 5, 6, 4, 3, and 7 respectively. Samples range from 0 to 1023 and
start at 512; gesture steps change them by 32 counts. These finite-travel
knobs use the same scroll and drag handling as the channel encoders. Their
Native position marks rotate with simulated shaft position. The firmware applies
its existing filtering and scaling to the ADC readings.

Runtime command kind 6 sets an ADC sample using the physical channel as
target. Reply kind `0x80000007` reports a changed sample after the firmware
reads the completed conversion. The board-ID ADC channels are excluded from
this command.

Channel encoders emit one quadrature phase every 10 ms of guest time, giving
40 ms per detent. Shorter 2 ms and 5 ms phase trials lost firmware reports.
The LED observation window remains 50 ms so multiplexed
PWM pulses form stable lit/unlit output. A longer encoder turn can queue
multiple detents; the model preserves every transition rather than skipping
directly to the requested endpoint.

Panel runtime notifications include a 50 ms virtual-clock heartbeat
(`0x80000008`). Input tests use it for guest-time waits; timestamps on changed
LCD frames do not advance while the display is static.

Investigation pointed to firmware-controlled TIM2 interrupts and GPIO scanning
for panel input. TIM2 derives its period from PSC/ARR at 48 MHz, giving 50 µs
in the observed configuration. Firmware owns scanning and debouncing.

Timer tracing identified late host-clock dispatch as a cause of missed short
presses. The engine uses single-threaded TCG with
`-icount shift=3,align=on,sleep=on` so virtual timer deadlines interrupt CPU
execution at instruction boundaries. Each executed instruction advances the
shared virtual clock by 8 ns; QEMU sleeps when it gets ahead of wall time.
This is an instruction budget for both CPUs, not a model of either MCU's
instruction cycle costs. QMP pause freezes both chips and device timers.

Investigation also identified a pending UART receive condition during panel
startup. Enabling receive interrupts must assert the interrupt if a byte is
already pending. The model implements this transition.

Both CPUs share panel flash programming and reset state. The modeled ROM
update protocol writes executable panel flash, verifies it, and restarts the
panel application. Guest-requested chip resets remain local to that chip.

## CPU consumption

Investigation pointed to the main firmware's FreeRTOS idle task as the dominant
idle workload. Register samples repeatedly landed at `0x80085150`–`0x8008515c`:
the loop checks deleted-task cleanup and the idle-priority ready list, and
requests PendSV when another idle-priority task can run. It does not execute
`WFI`. The instruction pattern matches the cleanup and ready-list checks in
[FreeRTOS's idle task](https://github.com/FreeRTOS/FreeRTOS-Kernel/blob/main/tasks.c).
The panel usually waits at its interrupt-sleep routine (`0x080019b8`),
but TIM2 still interrupts every 50 µs.

QEMU's adaptive instruction clock adjusts virtual time against wall time; it
does not impose a host CPU budget. The paced default bounds instruction
execution and explicitly sleeps when ahead, retaining the same virtual timer
deadlines. Firmware images are loaded unchanged. See
[QEMU instruction-count timing](https://www.qemu.org/docs/master/devel/tcg-icount.html).

Compare both modes using the Rust diagnostic harness. It consumes display
publications, waits 45 seconds for startup, then measures QEMU process CPU
time over 20 seconds, samples each guest PC, and compares guest acknowledgment
timestamps with wall time. Run modes separately so they do not compete for CPU:

```sh
cargo run -p cpu-profile -- --timing paced --logs emulator/logs/cpu-paced
cargo run -p cpu-profile -- --timing adaptive --logs emulator/logs/cpu-adaptive
```

The benchmark sends two encoder steps to obtain guest timestamps. CPU readings
use `ps`; 100% represents one host core. Hardware, QEMU builds, and guest activity
affect the result.

An idle Date/Time measurement with the local macOS/QEMU build gave 115.0%
CPU in adaptive mode and 76.7% in paced mode, a 33% reduction. In paced mode,
20.036 seconds of wall time corresponded to 20.041 seconds of guest time.
The main PC was inside the identified idle loop in 119 of 125 paced samples.

- Slower fixed settings (`shift=4` and `shift=5`) caused an Error dialog during
  startup and failed visible button-response validation.
- Paced timing has been exercised through startup, Date/Time editing, control
  input, SD update/restart, and popup integration diagnostics. Audio processing
  remains unmodeled; these tests do not establish accurate hardware cycle costs.
- Idle-loop acceleration could reduce CPU further, but would require explicit
  treatment of scheduler semantics and interrupt wakeups.

## Validation

The live integration check saves firmware screenshots and checks QMP pause,
guest-time release deadlines, exact key packets, visible display changes,
encoder reports, and Date/Time editing:

```sh
cargo run -p input-smoke -- --volatile --qemu /path/to/qemu-system-arm --logs emulator/logs/input-smoke
```

Output stays local in the ignored logs directory.

The native UI check dispatches events directly through GPUI's window event
dispatcher. It uses shared component bounds and the real UI handlers, without moving,
clicking, or depending on the system pointer. Each gesture is dispatched as one
batch so physical pointer motion cannot interrupt it. The check completes
setup with two clicks, tests vertical/horizontal drags and scrolling in both
directions, and verifies GPIO rings, mute and HIGH LEDs, a persistent dB
switch, Power GPIO edges, all five ADC knobs in both directions, analog drags,
and exact firmware input counts:

```sh
cargo run -p l6max-gui -- --volatile --ui-input-smoke --logs emulator/logs/ui-input-smoke
```

Set `L6_UI_SMOKE_HOLD=1` to retain the successful test window for inspection
for up to ten minutes.

The control check exercises all 50 controls through GPIO, captures indicator
changes, checks mute outputs, and checks firmware press/release packet counts
for the remaining panel controls:

```sh
cargo run -p control-smoke -- --volatile --qemu /path/to/qemu-system-arm --logs emulator/logs/control-smoke
```

The indicator check completes first-boot setup, turns all eight encoders,
checks that only the corresponding GPIO ring changes, reverses the last turn,
and checks exact firmware encoder report counts:

```sh
cargo run -p indicator-smoke -- --volatile --qemu /path/to/qemu-system-arm --logs emulator/logs/indicator-smoke
```

## Limitations

- GPUI requires an owned image: rendering copies the 1 KiB frame and converts
  its pixels. The display path is not zero copy through the GPU.
- UART byte timing and SPI transfer timing are approximations.
- Instruction-count timing does not model hardware instruction cycle costs.
- Audio processing is unmodeled. The engine enables a diagnostic acknowledgment
  of staged audio data so setup can proceed.
- Several peripherals remain incomplete; see the [board model](board.md).
- QEMU's Rust crates are internal Meson device APIs rather than a complete
  system embedding interface. The board model remains C; the UI, lifecycle,
  protocol, diagnostics, and package tool are Rust.
