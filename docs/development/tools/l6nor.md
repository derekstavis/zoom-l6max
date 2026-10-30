# `l6nor`

`l6nor` reads and writes simulator device state: the MAIN NOR (`main-nor.bin`),
the L6 external resource NOR (`resource-nor.bin`), their settings banks, the
panel RTC (`panel-rtc.bin`) and, for L6max, a pre-programmed panel flash. The `l6nor` crate is also the library `l6test` uses
to build fixture state from a declarative [state spec](#state-specs).

```sh
cargo run --offline --locked -p l6nor -- info STATE
cargo run --offline --locked -p l6nor -- dump STATE
cargo run --offline --locked -p l6nor -- get STATE battery_type mixer.current.efx_type
cargo run --offline --locked -p l6nor -- set STATE battery_type=1 'mixer.current.channel[0].mute=1'
cargo run --offline --locked -p l6nor -- diff STATE_A STATE_B
cargo run --offline --locked -p l6nor -- seed --firmware FIRMWARE NEW_STATE
cargo run --offline --locked -p l6nor -- apply --spec tests/smoke/states/l6max-setup-complete.toml --firmware FIRMWARE STATE
cargo run --offline --locked -p l6nor -- rtc STATE --set 2026-02-02T01:01:00
```

`STATE` is a state directory or a `main-nor.bin` file. L6 settings banks live
in `resource-nor.bin`, so L6 settings commands need the state directory. `FIRMWARE` is an unpacked
firmware directory or an update package. Every write holds the same exclusive
lock QEMU takes on device files, so `l6nor` refuses to modify a running device.
Keep generated state out of Git, like any saved flash.

## What it models

| Area | Behavior | Scope |
| --- | --- | --- |
| Seeding | `seed` reproduces QEMU's first persistent boot: erased NOR, MAIN image and trailer, packaged panel image and trailer, installed-version record; for L6 also the 4 MiB resource NOR | Both models |
| Inspection | Region map, boot/system/packaged version records, `FW UPDT` marker state | Both models |
| Settings banks | Active-bank selection and the firmware's commit sequence (erase inactive slot, program payload, write the next marker last, erase the old slot); MAIN NOR `0x1f9000`/`0x1fc000` on L6max, resource NOR `0x3fb000`/`0x3fd000` on L6 | L6max 1.10, L6 1.10 |
| Settings fields | Named, range-checked fields from a revision-scoped catalog | L6max 1.10; L6 1.10; four L6 globals still `unclassified_*` |
| Panel RTC | Date/time registers and the calendar-valid marker (CR bit 18) | Both models |
| Panel pre-programming | Panel flash and option bytes as left by the main firmware's programming sequence | L6max |

Each settings catalog (`tools/l6nor/catalog/`) describes one model and
firmware revision: bank slots, payload layout, field offsets, ranges and
defaults. `l6nor` uses a catalog when the installed MAIN image matches the
catalog's reference image hash. It also uses the catalog when every code
fingerprint matches: the SHA-256 of each MAIN code range that handles settings.
Patched 1.10 images leave those ranges untouched; `info` reports which kind of
match applied.

Current emulator comparisons confirm:

- L6max: an `l6nor`-built device and one taken through first-boot setup in the
  UI had byte-identical MAIN NOR, panel flash and option bytes.
  `setup_required = 0` with a valid RTC calendar boots straight to the recorder;
  1 opens Date/Time.
- L6: the default payload matched the emulated firmware's defaulted settings
  image byte for byte, and a written bank was loaded as written. Six global
  fields were confirmed by sending each editor parameter change over USB
  SysEx and observing which byte changed. The remaining L6 globals and the L6
  mixer-record fields keep `unclassified_*` names or are not named yet.

The QEMU model of the L6 resource NOR implements reads only, so emulated L6
firmware never commits its own settings and loads defaults on every boot. A
bank written by `l6nor` is the only way to start an emulated L6 with non-default
settings.

On L6, emulator analysis found that `auto_power_off` (`0xf24`) affects emulated startup
when a bank is loaded. At 1, the firmware waits for two `ch4-mono` presses
before its boot LED sweep; at 0 it boots without input. The checked-in L6 spec
sets it to 0.

## Field paths

| Path | Addresses |
| --- | --- |
| `battery_type`, `midi_channel`, `auto_power_off` | Global settings (L6max also has `setup_required`) |
| `midi_cc_number[3]`, `sound_pad_midi_note[0]` | One element of an array field |
| `mixer.current.efx_type`, `mixer.scene-0.efx_param[4]` | A field of one mixer block |
| `mixer.current.channel[2].mute` | A per-channel record field |

L6max mixer blocks are `scene-0` through `scene-3`, `current` (the live mixer),
and `scratch`, which the firmware re-initializes on every load. L6 has
`scene-0` through `scene-2`, `current` and `scratch`; its channel-record and
block fields are not named yet. A block's index
also works (`mixer.4.…`). Values are decimal, `0x` hexadecimal or `hex:BYTES`
for byte fields. Values outside the firmware's validation range are rejected.
`dump` lists every path with its payload offset.

`set` edits the active bank. Pass `--base defaults` to start from the
firmware's default payload instead, for example when no valid bank exists.

## State specs

A state spec is a TOML description of a starting device. `l6test` applies it
to each case's disposable state copy. Use it through the fixture field
`state_spec`; see the [test workflow](../testing.md#fixture-and-catalog-toml).

```toml
version = 1
model = "l6max"

[settings]
base = "defaults"        # or "installed": edit the existing active bank

[settings.values]
setup_required = 0
battery_type = 1
"mixer.current.channel[0].mute" = 1

[rtc]
datetime = "2026-02-02T01:01:00"

[panel]
programmed = true        # L6max only
```

Missing `main-nor.bin` is seeded from the fixture firmware first. Settings are
committed with the firmware's bank protocol, so the firmware loads them on its
first boot. `setup_required = 0` together with a valid RTC calendar skips the
first-boot Date/Time and Battery Type flow; a pre-programmed panel skips panel
ROM programming.
[`l6max-setup-complete.toml`](../../../tests/smoke/states/l6max-setup-complete.toml)
reproduces the `state-l6max-setup.toml` recipe without input. The `state-spec`
case checks the result.
