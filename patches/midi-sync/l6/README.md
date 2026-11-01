# L6 MIDI state sync

The L6 1.10 profile for [`midi-sync`](../README.md) uses a separately
model-specific hook, value getters, channel mapping, and mapped-CC sender. It does
not reuse the L6max addresses or assume the L6max's 94-setting layout.

Send Control Change 127 with value 64 or higher on the configured MIDI
channel through the L6 Mixer Control port. The hook broadcasts six physical
channels across the L6's nine six-entry CC map groups: HIGH, FREQ, MID, LOW,
AUX1, AUX2, EFX, PAN, and LEVEL. The stock mapped sender applies the user's
configured CC numbers and MIDI channel. The hook intercepts only this request;
other messages fall through to the original handler.

## Firmware profile

The supported L6 1.10 MAIN image has SHA-256
`827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799` and loads
at `0x80001000`. The hook replaces four bytes at `0x8002f490`, the L6
`MidiControlMsg` handler. Its independent byte guard is in `hooks.tsv`; the
displaced prologue is read from the user's input image during payload build.

The hook reads the message at the stock handler's offsets `+4`, `+5`, and `+6`.
For a sync request it loops over six strips and nine groups, maps each strip
through `0x80040ae8`, reads the stored value through the L6 getters, and calls
the L6 mapped-CC sender at `0x80008330`. Values above 127 and invalid mapped
channels are skipped. `tests/cpu_sim_l6.py` executes the compiled hook and
trampoline while stubbing those firmware helpers.

The sender and getter roles are supported by a live QEMU hook test on the
L6 1.10 image. The `l6-midi-sync` driver case pauses
the L6 guest at its Idle task, injects the request at the patched handler, and
checks that every enabled CC-map entry reaches the stock mapped-CC queue sender
with the configured channel and a valid value. The test lets the final sender
call return successfully. It bypasses USB/DIN ingress, so host delivery and
physical-device behavior remain unverified. A separate DIN probe confirms
the unattended L6 boot initializes LPUART2 after QEMU acknowledges the audio
profile ramp handshake; bytes reach the raw MIDI ring and the stock handler.
USB enumeration remains incomplete in the current emulator.

## Build and checks

From the repository root, unpack the supplied L6 package and build into a new
directory. The native and browser builders detect L6 from the MAIN reset
vector and select this profile automatically.

```sh
cargo run -p l6fw -- unpack /path/to/L6.BIN unpacked-l6
cargo run -p l6patch -- unpacked-l6 firmware-midi-sync-l6 --patch midi-sync
python3 patches/midi-sync/tests/cpu_sim_l6.py firmware-midi-sync-l6/main_firmware.bin
cargo run -p qemu-run -- --qemu /path/to/qemu-system-arm \
  --firmware-dir firmware-midi-sync-l6 --logs /tmp/l6-midi-sync --seconds 5
cargo run -p l6test -- run --config smoke.local.toml --case l6-midi-sync --model l6
cargo run -p l6test -- run --config smoke.local.toml \
  --case patched-update --model l6
```

Configure the `prepared` L6 fixture in `smoke.local.toml` with a stock package
and `patches = ["midi-sync"]`. The CPU check validates the request match, six-channel/nine-group traversal,
value filtering, and fall-through behavior. The smoke validates the live hook,
getters, CC mapping, and queue sender inside QEMU; it does not establish USB or
DIN transport. The `patched-update` driver case installs the generated L6 package into
the emulator's split flash and repeats the live hook check from the installed
image. See [current status](../../../docs/development/current-status.md) for
remaining validation.
