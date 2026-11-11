# Show knob values on display

Turn any channel knob — an EQ band, level, pan, or an aux send — and the
L6max's screen shows the value you just set, for about two seconds: dB for EQ,
Hz or kHz for frequency, percent for level and sends, or a left/right
position for pan. The stock firmware changes the value but never displays it,
so there's no way to tell exactly where a knob landed without comparing by
ear. This patch adds that readout; nothing about how the knobs themselves work
changes.

The rest of this page documents how the patch is built and verified.

## How it works

The main MCU owns mixer parameter values and LCD rendering. The panel MCU
reports encoder movement over UART and drives indicators. The patch modifies
the main firmware and keeps the panel image unchanged.

A channel encoder turn applies the original mixer setter and publishes the
channel/mode tag through a relaxed atomic byte mailbox. The display callback reads
the current mixer value when it renders, so queued events cannot carry a stale
value snapshot. The first valid turn allocates scene state and registers
DisplayCycle2 slot 5. That callback consumes the event and updates the dialog.
The state and callback remain for the rest of the boot; expiry and navigation
restore the borrowed GUI descriptors but retain the small allocation. Stock
callback slots 1, 3, and 4 remain available to text scrolling, busy animation,
and notifications. The title
follows the selected blue button: High, Freq, Mid, Low, Aux1, Aux2, Efx,
Sub-Mix, Pan, or Level. The centered body shows EQ gain in dB, frequency in
Hz/kHz, normalized control percentages for level/sends, and CENTER or L/R
position for pan.
The channel number is omitted. Further turns refresh the dialog and restart its
two-second display timeout. Blue buttons remain usable while it is visible.

## Source layout

- `src/payload.rs`: payload entry point, slot-5 display callback, dialog
  presentation, and atomic latest-event mailbox. The patch does not take the
  notification semaphore.
- `src/firmware.rs`: bindings to original firmware functions and data.
- `src/values.rs`: parameter value formatting.
- `linker/link.ld`: payload placement (base `PAYLOAD_BASE` comes from the builder: `0x80108000` when built first) and prohibition on reserved mutable RAM.
- The `knob_changed` post-call handler is declared with
  `#[function_call_post_hook(mixer_apply_channel_parameter, at = 0x8003_6c74)]`.
  The builder validates the call target and generates the callee-entry dispatcher.

The host build and package integration live in
[`l6patch`](../../docs/development/tools/l6patch.md).
See the [patch conventions](../README.md) before adding another modification.

## Build and run

Commands below run from the repository root. The patcher requires the project's
pinned Rust toolchain with `thumbv7em-none-eabihf` installed. Homebrew LLVM at
`/opt/homebrew` is used when present; otherwise the toolchain's own `rust-lld`,
`llvm-nm` and `llvm-objcopy` (`llvm-tools` component) are used. It rejects any main image whose SHA-256 differs from
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`.

```sh
rustup target add thumbv7em-none-eabihf
cargo run --offline -p l6patch -- unpacked firmware-knob-dialog-recorder-fix-v112 --update-version 0112
cargo run --offline -p l6max-gui -- --qemu /path/to/qemu-system-arm --firmware-dir firmware-knob-dialog-recorder-fix-v112 --volatile
```

The output directory must be new. Input files are never changed. The output
includes the patched main image, original panel image and package components,
the compiled payload, a repacked `L6max.bin`, and a `patch.json` recording image
and package hashes and generated hooks. Use the repository's custom QEMU build. `--volatile` ensures an old
persistent NOR image cannot override the supplied firmware.

## SD update package

The patcher writes its payload at runtime address `0x80108000` into erased
space after the original main image. It grows `main_firmware.bin`, shrinks
the following padding, updates the manifest and both main-length records,
and regenerates the big-endian MAIN byte-sum checksum. The total package size,
panel image/trailer, system table, and component slot boundaries are preserved.
The call at `0x80036c74` remains a normal BL. The builder redirects the
callee entry to a dispatcher, which matches the saved caller return address
and runs the post-call handler after the stock function returns. The
DisplayCycle2 callback notices a window change and restores borrowed
descriptors on its next tick, including while a stock notification is visible.
Notification manager and RTOS call sites remain untouched.
The original startup target is unchanged. Stock operations
still run their original code and preserve synchronous completion.

Expiry and window changes restore the original widgets and remove custom text.
Stock notifications can temporarily cover the dialog; its timeout pauses while
the notification or busy overlay is active. The current persistent-state,
semaphore-free design has passed the focused emulator scenario. An earlier
notification-backed version was exercised on an L6max; this revision has not
been hardware-tested.

The default patch version is `0112` (1.12), a local development marker that
triggers the startup update prompt over stock 1.10. Use
`--update-version FOUR_DIGITS` to override it; `0110` retains the stock marker
and uses the manual **System → Firmware → Firmware Update** menu:

```sh
cargo run --offline -p l6patch -- unpacked firmware-knob-dialog-recorder-fix-v112 --update-version 0112
cargo run --offline -p l6fs -- card demo emulator/state/sd-card-knob-synchronized.img --firmware-package firmware-knob-dialog-recorder-fix-v112/L6max.bin
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
- Stock notifications use layer 5 for frames, layer 6 for text, and callback
  slot 4 for expiry. The custom scene uses layer 4 and slot 5. Notifications
  can appear above the dialog; the scene callback pauses its timeout while a
  stock notification or busy overlay is active. No notification function is
  hooked.
- `0x800070d8` (`gui_set_widget_text`) copies arbitrary UTF-16 text into the
  widget's existing resource buffer using the current language. Its localized
  implementation at `0x80006fa8` bounds the copy by that resource's capacity;
  it does not allocate or grow the buffer. The supported firmware's language
  getter returns 0. `0x80006ad8` renders through the stock font/layout path.
  Widgets 7 and 33 temporarily share resource 179, a 255-character buffer
  whose stock contents are verified before use. Its pointer and length tables
  remain unchanged. First use scans the other initialized widget and background
  descriptors, declining if another widget references resource 179 or another
  descriptor already uses scene layer 4.
- The scene temporarily changes widget descriptors 7 and 33 and background
  descriptors 10, 37, and 48 to render on layer 4. At first use, the patch
  checks both widget descriptors against the exact recorder defaults in the
  supported firmware image before publishing state. The display callback
  repeats the check immediately before the first draw because it runs in a
  different task. Both widgets must be on stock
  layer 1; the patch restores the known descriptor values when the scene
  expires or the window changes. The patch also checks the three backgrounds
  against their stock layers (0, 0 and 8). If any layer differs, it declines
  to start the scene without touching the heap.
- No heap allocation is needed for state. A 4-byte record (signature, timeout,
  and atomic pending event) lives directly in bitmap-table slot 0
  (`0x802073c8`), the null sentinel in this firmware. `gui_draw_background`
  returns before looking up bitmap ID 0, so the stock renderer never interprets
  the record as a bitmap. The stock notification descriptor remains in entry
  36, untouched. The record stays in slot 0 while the callback is registered.
- The normal recorder window is `0x80200408`. The mixer-dispatch call site at
  `0x80036c74` redirects knob updates to the patch. The DisplayCycle2 callback
  restores borrowed descriptors on the first tick after a window transition.
  Setup and modal screens do not receive knob dialogs.

## Synchronization

The knob dispatcher call redirect remains the only firmware hook site.
Mixer changes run first; afterward the hook publishes the channel and mode
with one `AtomicU8` store. The DisplayCycle2 callback takes the event with an
atomic swap, then reads the current mixer value. Input and drawing do not share
a mutable Rust state reference and neither waits on a semaphore. First use
publishes fully initialized state before registering the callback, so
DisplayCycle2 cannot observe a partial state. The callback stays registered
after the scene expires so later knob events need no re-registration or state
allocation.

The emulator probe reads `current_task_control_block` at both entry points and
confirms the mixer hook and DisplayCycle2 callback run under different FreeRTOS
tasks. Keep the atomic mailbox for this cross-task handoff.

The scene does not take the stock notification semaphore. It uses widgets 7
and 33 and backgrounds 10, 37, and 48 on layer 4; notifications use their own
text widgets and layers 5–6. The emulator scenario verifies bitmap-table slot 0
starts null, holds the inline state record, and is not needed by stock rendering;
entry 36 remains the original notification descriptor. It also holds the
notification semaphore unavailable during first use and refresh. The patch
uses no heap and retains one idle callback for the rest of the boot.

Expiry and window changes clear layer 4, restore the text and background
descriptors, and reset the pending event on navigation. They do not free the
state, clear bitmap-table slot 0, or unregister slot 5. The next recorder knob
event can therefore reuse the same inline state safely.

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
cargo test --offline -p l6patch
cargo run --offline -p l6test -- run --config config.toml --case knob-dialog --model l6max

cargo run --offline -p l6test -- run --config config.toml \
  --case patched-update --model l6max
```

The branch-encoding test checks synthetic Thumb instructions. The QEMU
smoke test uses GPIO encoder/button input, checks all ten mode titles, compares
notification values with mixer RAM, verifies updates and expiry, verifies
original resources are restored, and counts 52 UART encoder reports. It checks
the expired title band against the recorder's pre-popup pixels, catching stale
glyphs even when the notification queue reports inactive. It also checks
one firmware-heap allocation, reuse during refresh, and recovery of the
original free-byte count in the legacy notification implementation. The former `0x841ff000` page stays
unchanged, and the original startup pointer is checked. It also
changes the mode while a notification is visible and exercises channel 2.
The scene-prototype smoke case overlaps a three-line stock notification with
the dialog, verifies bitmap-table slot 0 starts null and holds the inline state
record, and confirms entry 36 remains untouched. It holds the notification
semaphore unavailable during first use and refresh. Both updates still render,
the atomic mailbox is consumed, and cleanup restores the visible GUI resources
while retaining the state record. A separate first-use probe changes a
widget descriptor and verifies that the patch declines without allocating or
claiming callback slot 5.
It cycles through all ten knob selections, checks that each title row changes,
matches every displayed value against mixer state, rejects a corrupted state
signature, verifies that widget and background layer drift is rejected, and
confirms that the scene performs no heap allocations.
Both lines use the firmware's stock text renderer and font resources. See the
[patch bindings](src/firmware.rs) and
[parameter display contract](../../docs/firmware/gui/parameter-values.md).

Screenshots come from firmware LCD transfers, without a host UI overlay.

The current scene prototype has one knob-dispatch call redirect and no
function-entry hooks. The focused emulator check verifies the text, expiry
restoration, semaphore independence, retained slot-5 callback, descriptor
validity during stock notifications, and cleanup on window navigation. The
earlier notification-based design was also checked by an SD-install/restart
diagnostic with all ten knob modes and 52 exact encoder reports, and by a forced
concurrency diagnostic for its semaphore sidecar. That design and its
`popup-concurrency` case are retired; see the
[notification lifecycle](../../docs/firmware/gui/notification-lifecycle.md).
Logs and generated binaries remain ignored artifacts.

The `patched-update` driver case requires the original main firmware as its
starting image and a package with a newer development marker. It validates the
firmware confirmation and GPIO shutdown, invokes the replacement installer,
verifies its installed payload and preservation of boot/calibration/settings,
then runs the typed `knob-dialog` operation against that persistent NOR on
another restart. The patch is not substituted through QEMU's
initial firmware image in this test. Its SD and device fixtures are removed
on completion or failure; display captures remain in the ignored log directory.

The scene prototype exercises the firmware's generic bounded text setter
with resource 179, a 255-character buffer not assigned to any initialized
widget. Widgets 7 and 33 temporarily share that resource; the patch writes and
draws each line in turn, without redirecting the resource pointer table. It
checks the buffer is still the stock `*` filler before use, then restores that
filler and both widget descriptors when the dialog closes. The focused
`knob-dialog-scene-prototype` smoke case checks that the text pointer table
stays unchanged, resource aliases and layer collisions are rejected without
allocation, both lines appear in the LCD capture, and the resource and widget
state are restored. Before first use it opens the main menu, turns an encoder,
and verifies that state and callback slot 5 remain unclaimed. It saves
`knob-dialog-non-recorder-before.png` as evidence. The case also captures
`knob-dialog-active.png` and `knob-dialog-timeout-cleared.png` to show the
dialog and the recorder screen after expiry. It changes a borrowed descriptor after
the mixer-side preflight passes but before the first DisplayCycle2 draw; the
callback recheck refuses that draw, and a retry after restoring the descriptor
succeeds. Bitmap-table slot 0 holds the retained state record; entry 36 remains
the stock notification descriptor. This remains an experiment: the overlay
currently uses white text on black.

## Limitations

The [notification lifecycle](../../docs/firmware/gui/notification-lifecycle.md)
describes how the scene coexists with the stock notification manager. This
scene does not acquire, replace, or modify its counting semaphore.

- LEVEL and send percentages represent control position, not linear amplitude.
  Their dB conversion and mute thresholds remain unverified.
- Bitmap-table slot 0 stores the state record. The firmware drawing path skips
  bitmap ID 0 before lookup. A static Ghidra reference scan found only the
  guarded background-renderer table lookup in the supported MAIN image, and
  the focused emulator case verifies the zero-ID path plus stock notifications
  using entry 36 untouched. An external module that directly indexes slot 0
  would conflict with this use.
- All computed callers, simultaneous window transitions and physical cache/DMA
  timing need further coverage; emulator tests do not prove every interleaving.
- This persistent-state revision is emulator-tested only. The earlier
  notification-backed package was exercised on an L6max. The bootloader's
  exact checks, arbitrary modifications, downgrades, signature policy and
  interrupted installation remain unverified.
- Additional channels, languages, SD recording workflows, and notification
  queue collisions need broader coverage beyond the tested hardware path.

See the [parameter value mapping](../../docs/firmware/gui/parameter-values.md) for
conversion evidence, rounding rules, pan notation, and remaining mappings.
