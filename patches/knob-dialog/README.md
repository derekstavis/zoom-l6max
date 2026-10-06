# Channel knob value notifications

Investigation pointed to the main MCU as the owner of mixer parameter values
and LCD rendering. The panel MCU reports encoder movement over UART and drives
indicators. This experiment modifies the main firmware; the panel image stays
original.

A channel encoder turn applies the original mixer setter, reads back the stored
value, and copies an optional presentation event without waiting for a display
lock or compositor. The existing notification callback consumes the latest
event and displays a timed notification. The title follows the selected blue
button: HIGH, FREQ, MID, LOW, AUX1, AUX2, EFX, SUB-MIX, PAN, or LEVEL. The centered body shows EQ gain in dB, frequency in Hz/kHz, normalized
control percentages for level/sends, and CENTER or L/R position for pan.
The channel number is omitted. Further turns refresh the notification and
restart its existing two-second guest timer. Blue buttons remain usable while
it is visible.

## Source layout

- `src/payload.rs`: firmware payload entry point and notification presentation.
- `src/firmware.rs`: bindings to original firmware functions and data.
- `src/notification_hooks.rs`, `src/synchronization.rs`: notification hooks and locking.
- `src/trampolines.rs`, `src/trampolines.s`: veneers that read displaced
  instructions from the locally supplied firmware at build time.
- `src/values.rs`: parameter value formatting.
- `linker/link.ld`: payload placement and prohibition on reserved mutable RAM.
- `hooks.tsv`: hook addresses, lengths, expected byte hashes, and entry symbols.

The host build and package integration live in
[`firmware-patch`](../../tools/firmware-patch/src/main.rs).
See the [patch conventions](../README.md) before adding another modification.

## Build and run

Commands below run from the repository root. The patcher requires the project's
pinned Rust toolchain with `thumbv7em-none-eabihf` installed, and Homebrew LLVM
at `/opt/homebrew`. It rejects any main image whose SHA-256 differs from
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`.

```sh
rustup target add thumbv7em-none-eabihf
cargo run --offline -p firmware-patch -- unpacked firmware-knob-dialog-recorder-fix-v112 --update-version 0112
cargo run --offline -p l6max-gui -- --qemu /path/to/qemu-system-arm --firmware-dir firmware-knob-dialog-recorder-fix-v112 --volatile
```

The output directory must be new. Input files are never changed. The output
includes the patched main image, original panel image and package components,
the compiled payload, a repacked `L6max.bin`, and a `patch.json` recording image
and package hashes and hook addresses. Use the repository's custom QEMU build. `--volatile` ensures an old
persistent NOR image cannot override the supplied firmware.

## SD update package

The patcher writes its payload at runtime address `0x80108000` into erased
space after the original main image. It grows `main_firmware.bin`, shrinks
the following padding, updates the manifest and both main-length records,
and regenerates the big-endian MAIN byte-sum checksum. The total package size,
panel image/trailer, system table, and component slot boundaries are preserved.
The encoder call and notification/resource operations receive guarded veneers.
`hooks.tsv` records displaced lengths and hashes; `patch.json` lists every
patched address. Displaced instructions are assembled from the locally
supplied image; the source tree does not embed those byte sequences.
The original startup target is unchanged. Stock operations
still run their original code and preserve synchronous completion.

The original main image is 1,081,320 bytes. The recorder polling revision is
1,089,356 bytes: 8,012 payload bytes plus 24 alignment bytes. Its observed
fixed-layout slot allows 1,716,216 bytes, leaving 626,860 erased padding bytes
before the main trailer. Main trailer/footer lengths agree, and the complete
package remains 1,737,216 bytes. This establishes package-layout fit, not an
independently verified physical bootloader size limit.

Investigation of LCD video pointed to stale title pixels after expiry. Cleanup
now clears the custom text rectangle before restoring the original widget
Y/font, and initial text is populated before stock show draws.

Caller tracing and an actual recorder-callback invocation pointed to a separate
premature dismissal: periodic recorder status polling calls busy-hide while no
busy overlay is present. Stock busy-hide returns immediately; our earlier
unconditional detach wrapper removed the custom notification first. The current
wrapper checks busy flag `8020a2b0` under the manager lock and detaches only when
it equals 1. Idle polling preserves popup allocation, pixels, and timeout.

The scene-refresh repaint workaround is removed. Original scene clearing and
stock redraw policy remain intact; a forced clear was not sufficient evidence
for the hardware dismissal cause. See the
[dismissal regression](../../docs/firmware/gui/popup-triggers.md).

- Current package: `firmware-knob-dialog-recorder-fix-v112/L6max.bin`, development
  marker `0112` (1.12). The payload is unchanged from the hardware-tested build.
- The older `firmware-knob-dialog-live-refresh/L6max.bin` still detaches on idle recorder polling. Use the recorder-fix-v112 package.
- The corrected popup behavior was confirmed on an L6max. The emulator
  regression invokes the real recorder callback through GDB; it does not
  reproduce every possible hardware scheduling interleaving.

The default patch version is `0112` (1.12), one increment above the initial
1.11 patch. It triggers the startup update prompt over stock 1.10. Use
`--update-version FOUR_DIGITS` to override it; `0110` retains the stock marker
and uses the manual **System → Firmware → Firmware Update** menu:

```sh
cargo run --offline -p firmware-patch -- unpacked firmware-knob-dialog-recorder-fix-v112 --update-version 0112
cargo run --offline -p sd-image -- emulator/state/sd-card-knob-synchronized.img --demo firmware-knob-dialog-recorder-fix-v112/L6max.bin
cargo run --offline -p l6max-gui -- --state-dir emulator/state/knob-synchronized-demo --sd-image emulator/state/sd-card-knob-synchronized.img
```

`0112` identifies this local experiment as 1.12; it is not an official ZOOM
release. The default firmware directory remains original, so a fresh state
starts unpatched and installs the patch through SD. Select **Execute**, hold
Power for about 3.5 seconds and release, wait for a blank display, then click
Power again. Complete Date/Time setup if prompted and turn a channel knob.
Existing card images and build directories are never overwritten.

To repack manually after editing compatible components:

```sh
cargo run --offline -p l6fw -- repack firmware-knob-dialog-recorder-fix-v112 /path/to/L6max.bin --recompute-checksum
```

The repacker permits the main image to grow only within its existing slot;
it requires matching image lengths, manifest offsets/sizes, and footer/version
records. Unmodified original and expanded packages unpack/repack losslessly.

## Firmware paths

Addresses refer to this main firmware revision's runtime address space.

- The channel encoder event handler calls the mixer dispatcher at
  `0x80053b28` from `0x80036c74`. The patched call preserves all five arguments
  and invokes the original dispatcher before reading the value.
- `0x800564b0` maps physical channels to mixer channels.
- `0x80006660` queues notifications. Message 15 is the timed Done notification;
  `0x800064c0` advances its existing guest timer. Callback slot 4 remains
  registered to service pending knob values even while no message is active.
- English resources 229–231 supply the notification's three text lines.
  Their pointers begin at `0x802059b8`.
- Bitmap 36 supplies the notification frame and baked MESSAGE header. A RAM
  copy replaces that header with a setting title; the original renderer draws
  the text and publishes the LCD transfer.
- `0x800070d8` copies dynamic UTF-16 text into a widget's resource buffer.
  Date/Time uses it after formatting numbers on its stack (`0x8003c260`).
- The Done resources have capacities 5, 1, and 1 UTF-16 units; they cannot hold
  our dynamic title/value. The popup allocates 1,788 bytes through the original
  FreeRTOS `pvPortMalloc` (`0x80085478`) and temporarily binds these resources to its
  allocated buffers. The firmware's text-copy routine fills them from stack
  buffers. Existing text belonging to other screens is never borrowed.
- Bitmap 36's resource pointer retains the live allocation, whose first field
  is a valid bitmap descriptor. No extra global pointer or fixed RAM address
  is reserved. Repeated turns reuse the allocation.
- Expiry restores the original resource pointers, lengths, bitmap, layout,
  font, and polarity before pop can draw a stock successor, then frees the block through FreeRTOS `vPortFree`
  (`0x8008e588`). If allocation
  returns null, the original mixer adjustment still runs and no popup is shown.
- The normal recorder window is `0x80200408`. Setup and modal screens do not
  receive knob notifications; active unrelated notifications take priority.

## Synchronization

The original maximum-count-one counting semaphore remains the exclusion
primitive. A 36-byte, initialization-time heap sidecar stores its real handle,
the current owning task, nesting depth and one copied pending value. The handle
slot at `0x8020a2a0` contains the sidecar pointer tagged with bit 0. The two
original kernel callsites in notification pop are redirected to wrappers that
decode this tag; kernel routines always receive the original semaphore handle.
No kernel object fields are repurposed and no fixed spare RAM is assumed.

Same-task nesting lets original show/tick/busy operations call original pop
without a second blocking take. The sidecar is not a FreeRTOS recursive mutex
and adds no priority inheritance. Interrupt masking is used only for short
sidecar ownership/event copies, never across drawing or waiting.

Core queue transitions, shared text/bitmap readers and stock resource setters
participate in the lock. The input-path active/current queries retain their
original nonblocking reads of flags and bounded message IDs. They do not return
pointers into popup allocations. Knob producers never acquire the manager lock;
the GUI callback revalidates the recorder window and notification priority.

Stock show, dismissal, clear and busy transitions invalidate pending optional
values and detach our popup before drawing stock content. A window change
temporarily suppresses presentation without holding the manager lock across
arbitrary window callbacks. A stock Done request is processed as stock Done,
even while our popup currently uses message 15.

If sidecar allocation fails, the slot retains the ordinary semaphore handle,
the original notification behavior remains available and knob presentation is
disabled. Popup allocation failure also leaves the applied mixer change intact.
The sidecar lives until reset; popup allocations are still freed at detach.

| Selection | Value getter | Arguments after mixer channel |
| --- | --- | --- |
| HIGH / MID / LOW | `0x8000b140` | band 0 / 1 / 2 |
| FREQ | `0x8000b120` | band 1 |
| AUX1 / AUX2 | `0x8000b080` | bus 0 / 1 |
| EFX | `0x8000b0f0` | 0 |
| SUB-MIX | `0x8000b2a0` | — |
| PAN | `0x8000b1d0` | — |
| LEVEL | `0x8000b0b0` | — |

## Validation

```sh
cargo test --offline -p firmware-patch
cargo run --offline -p knob-dialog-smoke -- --qemu /path/to/qemu-system-arm --firmware-dir firmware-knob-dialog-recorder-fix-v112 --volatile --logs emulator/logs/knob-dialog
cargo run --offline -p popup-concurrency-smoke -- --firmware-dir firmware-knob-dialog-recorder-fix-v112 --volatile --logs emulator/logs/popup-concurrency

# Build the behavioral diagnostic, then install from SD into disposable state.
cargo build --offline -p knob-dialog-smoke
cargo run --offline -p patched-update-smoke -- firmware-knob-dialog-recorder-fix-v112/L6max.bin --volatile --logs emulator/logs/patched-sd-update
```

The branch-encoding test compares known original Thumb instructions. The QEMU
smoke test uses GPIO encoder/button input, checks all ten mode titles, compares
notification values with mixer RAM, verifies updates and expiry, verifies
original resources are restored, and counts 52 UART encoder reports. It checks
the expired title band against the recorder's pre-popup pixels, catching stale
glyphs even when the notification queue reports inactive. It also checks
one firmware-heap allocation per popup, reuse during refresh, release on expiry,
and recovery of the original free-byte count. The former `0x841ff000` page stays
unchanged, and the original startup pointer is checked. It also
changes the mode while a notification is visible and exercises channel 2.
The first widget uses the firmware's seven-pixel font and black ink polarity
in the original white header band. See the [patch bindings](src/firmware.rs) and
[parameter display contract](../../docs/firmware/gui/parameter-values.md).

Screenshots come from firmware LCD transfers, without a host UI overlay.

`popup-concurrency-smoke` uses a disposable VM and a local GDB socket. It
invokes the actual recorder polling callback while busy flag 0 and checks
that popup pixels, heap binding, active state, and elapsed counter survive. It
replaces one GUI callback invocation with a stock notification request, forces
encoder input at the expiry pop boundary, exercises full-queue nesting and busy
pause/resume, forces popup allocation to return null, and injects a held semaphore
while all CPUs are stopped. It counts original mixer invocations while the GUI
waits on the actual FreeRTOS semaphore. No debug controls are added to the firmware.
This test uses GPIO input and guest breakpoints, never the host mouse.
Its debugger stops perturb timing; it verifies behavior, not hardware latency.

The recorder polling payload is 8,012 bytes, replacing 138 original bytes across
31 guarded sites while retaining the original notification routines. The
development `0112` package was validated by the five patch/formatting tests,
the forced concurrency diagnostic, and the SD-install/restart diagnostic with
all ten knob modes and 52 exact encoder reports. The forced diagnostic covers
13 reports, including six observed calls to the original mixer routine while
the GUI is blocked. Logs and generated binaries remain ignored artifacts.

`patched-update-smoke` requires the original main firmware as its starting
image and a package with a newer development marker. It validates the actual
firmware confirmation and GPIO shutdown, invokes the same replacement installer
used by the emulator, verifies its installed payload and preservation of
boot/calibration/settings before restarting the application, then runs
`knob-dialog-smoke` against that
persistent NOR on another restart. The patch is not substituted through QEMU's
initial firmware image in this test. Its SD and device fixtures are removed
on completion or failure; display captures remain in the ignored log directory.

## Limitations

The [notification ownership contract](../../docs/firmware/gui/notification-lifecycle.md)
describes the original counting semaphore. The synchronization patch widens
its protected operations
and handles same-task nesting in the sidecar. The allocator's internal scheduler
protection alone does not protect popup pointers.

- LEVEL and send percentages represent control position, not linear amplitude.
  Their dB conversion and mute thresholds remain unverified.
- Popup storage belongs to the original firmware heap. Sidecar initialization
  allocation failure and real heap exhaustion,
  all computed callers, priority inversion and physical cache/DMA timing need
  further coverage; successful QEMU tests do not prove every interleaving.
- The corrected package was installed and its popup exercised on an L6max.
  The bootloader's exact checks, arbitrary modifications, downgrades, signature
  policy and interrupted installation remain unverified.
- Additional channels, languages, SD recording workflows, and notification
  queue collisions need broader coverage beyond the tested hardware path.

See the [parameter value mapping](../../docs/firmware/gui/parameter-values.md) for
conversion evidence, rounding rules, pan notation, and remaining mappings.
