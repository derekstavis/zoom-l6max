# MIDI state sync patch

[Documentation index](../README.md)

Lets a MIDI controller ask an L6 or L6max for model-specific mixer state, so
motorized faders and encoder rings can match it. Live changes already reach the
controller through the stock Control Change output; this adds the initial and
on-demand state request. Version marker `0113` is a local experiment, not an
official ZOOM release.

**Status:** the L6max payload is CPU-simulated and confirmed live in QEMU,
alone and combined with `knob-dialog` + `din-mixer-control`. An L6-specific
payload builds, repacks as `L6.BIN`, passes CPU simulation and boots in
QEMU. A GDB-injected QEMU smoke confirms that its hook sends all enabled
mapped CCs through the stock L6 queue sender. Stock L6 USB enumeration and
basic MIDI IN/OUT, plus stock DIN-to-mixer behavior, also pass in QEMU.
The `l6-direct-midi-sync` driver case also verifies all 54 mapped replies from
the patched L6 image. Physical hardware remains unverified.

The native builder selects the model from the unpacked MAIN reset vector. For
L6 it currently supports only `midi-sync`; L6max keeps the existing patch
profiles. The L6 uses a separate hook at `0x8002f490`, its own state getters,
and the mapped-CC sender at `0x80008330`. Its 54 replies cover six strips and
nine mode groups. The L6 request hook and trampoline are tested with the real
compiled firmware code, while the CPU simulator stubs the getters and sender.
The browser patcher detects the uploaded model and supports this L6 profile;
L6 `knob-dialog` and `din-mixer-control` profiles remain unavailable.

## Use

1. Enable MIDI mixer control in the L6 Editor (or the [Zoomie editor](../editor.md))
   and note the MIDI channel and CC map; the defaults are in
   [mixer MIDI](../firmware/protocols/mixer-midi.md#controller-numbers).
2. Have the controller send Control Change **127**, value 127, on that channel
   when it connects, or from a button.
3. The L6max replies with 94 Control Change messages: eight channels of eleven
   settings each (the ten originals plus MUTE), then 6 global toggles (CH 5
   MONO x2, CH 6 MONO x2, USB 1/2, USB 3/4, EFX TYPE, COMPRESSOR) — every
   parameter the stock firmware has. Map them to fader travel, encoder rings,
   LEDs or any combination.

On L6, the native profile replies with up to 54 mapped Control Changes: six
strips across nine groups (HIGH, FREQ, MID, LOW, AUX1, AUX2, EFX, PAN, LEVEL).
It uses the L6's own getters and CC-map sender. The CPU simulation checks the
hook and trampoline; the live QEMU smoke injects the request at the handler and
checks every enabled CC-map entry at the queue sender. It bypasses USB/DIN
ingress and host delivery.

Send nothing else special: ordinary Control Change keeps working as before.

## Build

Follow [getting started](../getting-started.md) for the toolchain, then:

```sh
rustup target add thumbv7em-none-eabihf
cargo run -p l6patch -- unpacked firmware-midi-sync-v113 --patch midi-sync
cargo run -p l6patch -- unpacked firmware-knob-midi-v113 --patch knob-dialog --patch midi-sync
cargo run -p l6fw -- unpack /path/to/L6.BIN unpacked-l6
cargo run -p l6patch -- unpacked-l6 firmware-midi-sync-l6-v113 --patch midi-sync
python3 patches/midi-sync/tests/cpu_sim_l6.py firmware-midi-sync-l6-v113/main_firmware.bin
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case l6-midi-sync --model l6
```

The second command builds the [knob value patch](knob-patch.md) and this one in
one L6max image. The L6 command emits `L6.BIN`; its CPU test checks 54 ordered
channel/group sends and stock-handler fallback. The L6 output is for emulator
validation; physical bootloader acceptance remains unverified. The output
directory must be new. Without Homebrew LLVM the builder uses the Rust
toolchain's own `rust-lld`, `llvm-nm` and `llvm-objcopy`; no setup is needed
beyond `rustup` installing the pinned toolchain. `L6_LD_LLD`, `L6_LLVM_NM` and
`L6_LLVM_OBJCOPY` override each tool.

## Check

```sh
cargo test -p l6patch
python3 -m pip install unicorn
python3 patches/midi-sync/tests/cpu_sim.py firmware-midi-sync-v113/main_firmware.bin
python3 patches/midi-sync/tests/cpu_sim_l6.py firmware-midi-sync-l6-v113/main_firmware.bin
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case l6-midi-sync --model l6
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case l6-direct-midi-sync --model l6
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case l6max-direct-midi-sync --model l6max
```

The L6 live check detects the model from `main_firmware.bin`. Without direct
USB mode it injects at the patched handler and checks 54 mapped sends at the
stock queue sender; with direct USB mode it sends CC 127 through the running
firmware and requires all 54 host replies. L6max direct USB similarly requires
all 94 replies. The driver selects the scenario from the named case and
validates the fixture's firmware model.

Hook details, resources and limits are in the
[implementation reference](../../patches/midi-sync/README.md).

## Limits

- Supported main image: 1.10 only (SHA-256 in the reference).
- All 94 parameter identifiers are synced: the 10 continuous settings, MUTE,
  and all 6 button-type globals. Full coverage.
- The request must use the configured MIDI channel.
- Fader surfaces that echo their own movement can feed back; enable their
  motor-feedback suppression.
