# Patch loader

Patch loader is a persistent firmware component for L6 and L6max 1.10 that
provides a safer way to load selected patches from an SD card. Install it
through the normal firmware update process once. It checks each patch file's
format, firmware compatibility, hook addresses, and CRC before registering the
patch, then loads its code into allocated RAM. Place one file per patch in the
card's `PATCHES/` directory. A loaded patch stays in RAM until reboot, including
across card removal. Firmware-installed patches continue to work alongside SD
modules.

The checks protect against unsupported or accidentally damaged patch files;
they do not make third-party patch code trustworthy or defend against a
deliberately crafted file.

The current per-file dynamic loader supports function-entry and function-call
hooks declared at cataloged sites. `midi-sync` uses an entry hook on both
models. `knob-dialog` uses a call-post hook on L6max and loads from
`PATCHES/KNOB-DIALOG.BIN`. The call instruction at `0x80036c74` remains intact;
the resident dispatcher at `mixer_apply_channel_parameter` selects the wrapper
by the original caller return address. The wrapper calls the next registered
handler or stock continuation, then invokes the dialog handler only for that
call site. The dialog registers its DisplayCycle2 callback in slot 5 and
allocates its small state on first use. `din-mixer-control` uses an
instruction-offset hook and remains static-only. The loader itself is installed
through the ordinary firmware update.

The loader accepts one hook registration per file. Multiple files can hook the
same function; they are linked in numeric filename-prefix order. The dynamic
knob-dialog emulator case loads two ordered modules for the same callee and
exercises the recorder guard, dialog rendering, timeout, cleanup, and task
context. The MIDI scenarios cover dynamic entry hooks.

**The listed emulator cases pass; physical hardware has not been tested.** QEMU
does not establish physical SDRAM permissions, cache behavior, stack headroom,
or task-interleaving safety.

## Build experimental artifacts

From the repository root, using locally unpacked stock firmware:

```sh
# One-time loader; optionally add --patch NAME to install a feature statically.
cargo run --offline --locked -p l6patch -- STOCK_DIR LOADER_DIR --experimental-patch-loader

# Build a separate SD set from the same stock firmware. Select only patches
# not already installed statically in LOADER_DIR.
cargo run --offline --locked -p l6patch -- STOCK_DIR BUNDLE_DIR \
  --experimental-patch-bundle --patch midi-sync
```

The builder emits one compact file at `BUNDLE_DIR/PATCHES/MIDI-SYNC.BIN`. At mount,
the loader enumerates `A:\PATCHES\*.BIN` in numeric filename-prefix order. For
example, `2-EARLY.BIN` runs before `10-LATE.BIN`; unnumbered names follow numbered
names, with a stable case-insensitive filename tie-break. The loader repeats a
bounded directory scan to choose the next filename instead of retaining a large
filename array on the task stack. It streams each file independently and skips
corrupt or unsupported candidates.

Hooks are stored in a fixed-capacity firmware table: up to eight function chains,
with up to eight hooks per function. The loader has dynamic dispatch stubs for
the cataloged MIDI input and mixer parameter functions. Each module carries a
`next` function-pointer slot. The loader connects hooks in filename order and
points the tail at the displaced stock handler. A hook keeps the target
function's calling convention; it calls `next` to continue, or returns without
calling it to override the remaining chain. The builder emits one hook per
module file. DIN still requires a static firmware edit. Both commands also generate
an ordinary update package in their output directory; only the loader update is
installed persistently.

The file uses format version 8 with a 24-byte header: magic, format version,
loader ABI, code length, hook count, relocation count, initializer offset, and
CRC-32. Its body is code followed by 12-byte hook records (function address,
entry offset, and optional `next` slot offset) and fixed-size relocation records.
The loader reads the header, allocates exactly the code length, streams code
into that allocation while processing bounded metadata, validates CRC and hook
address, performs load-time fixups, and publishes the handler only after
validation. It makes no whole-file staging allocation. The CRC covers the
header except its CRC field plus all declared code and records; trailing bytes
are rejected. It detects accidental damage, not malicious code.

The payload is linked at base zero. The builder retains ARM relocation records
for module-internal absolute references; the loader applies the allocation
address as the load delta. Relative calls and PC-relative references need no
fixup when the complete module moves. Firmware function and data bindings stay
at supported firmware addresses. This is load-time relocation rather than
zero-fixup PIC, avoiding a per-module PIC base register and writable GOT. The
registry holds fixed arrays in reserved firmware RAM, so registering a hook
does not allocate heap memory. Each patch still gets one exact-size allocation
for its code. The loader image has dispatch stubs for the cataloged `midi_in`
site on both models and `mixer_apply_channel_parameter` on L6max. Other
functions must be added to the catalog and loader dispatch before dynamic
modules can use them.

## Emulator scenarios

These focused scenarios build disposable loader firmware and a card from the
configured stock package. They require the cross-compilation tools documented
for `l6patch` as well as the normal emulator prerequisites:

```sh
cargo run --offline --locked -p l6test -- run --case sd-midi-sync --backend headless
cargo run --offline --locked -p l6test -- run --case sd-l6max-midi-sync --backend headless
cargo run --offline --locked -p l6test -- run --case sd-din-mixer-control --model l6max --backend headless
cargo run --offline --locked -p l6test -- run --case sd-knob-dialog --model l6max --backend headless
cargo run --offline --locked -p l6test -- run --case sd-knob-dialog-dynamic --model l6max --backend headless
cargo run --offline --locked -p l6test -- run --case sd-all-patches --model l6max --backend headless
cargo run --offline --locked -p l6test -- run --case sd-mixed-static --model l6max --backend headless
cargo run --offline --locked -p l6test -- run --case sd-midi-absent --backend headless
cargo run --offline --locked -p l6test -- run --case sd-midi-corrupt --backend headless
cargo run --offline --locked -p l6test -- run --case sd-midi-volatile --backend headless
cargo run --offline --locked -p l6test -- run --case sd-warm-reset --backend headless
```

They are registered as focused cases. The L6 `sd-midi-sync` case exercises the
allocated handler with a GDB-injected request and verifies its mapped CC sends.
The L6max MIDI case uses the host endpoint and verifies all 94 sync replies.
The serial L6max matrix passed all combinations of no static patch, DIN,
knob-dialog, or both, each with and without the dynamic MIDI module. It also
covers loader-only boot, static-plus-dynamic behavior across restart, and the
MIDI round-trip alongside the knob-dialog interaction. Run it with
`cargo run --offline --locked -p l6test -- run --suite sd-matrix --model l6max --backend headless --jobs 1 --config config.toml`.
Absent and corrupt bundle cases pass on both models. The volatile case passes
on both models: ejecting leaves a loaded set active, restarting without the
card clears it, and reinserting the card loads it again. QEMU's restart path
starts a fresh guest process. The `sd-warm-reset` case additionally ejects the
card and resets each guest in the same QEMU process, verifying both firmware
reset hooks clear published handlers while external RAM stays present.
`sd-knob-dialog-dynamic` additionally exercises a call-post hook loaded from
two ordered files at the same callee. Physical-device behavior remains
unverified.
Host-side checks are
`cargo test --offline --locked -p firmware-patch-compile -p l6patch -p patch-loader-core -p l6test -p smoke-core`.
