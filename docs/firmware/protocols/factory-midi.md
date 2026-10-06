# Factory inspection MIDI protocol

[Documentation index](../../README.md)

Investigation pointed to a factory inspection window and a separate MIDI SysEx
session in the original main image. This map applies to SHA-256
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`
(SYSTEM 1.10). Addresses below are main MCU runtime addresses.

Routine names are investigator assigned.
Packet meanings come from the receive state machine, event dispatch, and
response builders. Hardware acceptance has not been tested.

## Inspection startup

`startup_select_mode` at `0x800100f0` checks the following after higher-priority
startup conditions (including calendar setup):

- SOUND PAD 1, 2, and 3 are pressed.
- SOUND PAD 4 is released.
- The SD filesystem contains a file matching `A:\D445_JIG_???.txt`.

A successful file search selects startup mode **7**, stored at `0x80462d9c`.
The mode-to-window table pushes the inspection window at `0x802002b8`.
Its entry callback `0x8002fd48` initializes the inspection dispatcher and
resets mixer/DSP routing for testing. Its draw callback displays `Test`.
This entry has runtime side effects; it is not merely a debug overlay.

Main key table `0x800c42b8` has 36-byte entries. Indices 15, 16, 17, and 18
use readers `0x80021d90`, `0x80021d98`, `0x80021da0`, and `0x80021da8`:
GPIO1 bits 21, 25, 26, and 0. `0x80045490` returns 1 for an active-low closure.
These correspond to emulator controls 49 through 52.

The boot check requires a matching filename. Calibration later parses the
file contents separately. An empty or arbitrary file is not valid calibration
data. The diagnostic creates a disposable fixture and never starts calibration.

## Transport and framing

USB MIDI cable **2** is the `for L6 Editor Port`. The receive task at
`0x8003e5b0` collects SysEx from this port. The packet buffer stores a 32-bit
length followed by MIDI bytes, so buffer offsets `+4`, `+8`, and `+9` mean
MIDI byte indices 0, 4, and 5.

The factory packets use this envelope (hexadecimal):

```text
F0 52 00 00 <command> <arguments...> F7
```

The observed response builders use `00 00`; the inspected parser does not
validate these two header bytes. All arguments below are MIDI data bytes.
The stream parser recognizes `F0`, manufacturer `52` or universal `7E`, and
`F7`, with a bounded receive buffer. It does not establish a general debugger.

## Handshake and session state

The byte at `0x805dca98` is the SysEx session state. It is separate from startup
mode and the factory-connected flag at `0x80462da8`.

| State | Accepted request | Transition or effect |
| --- | --- | --- |
| 0 | `F0 7E 7F 06 01 F7` | State 1; queue identity reply |
| 1 | Envelope + `67 0B` | State 3; queue factory-connect event `0x57` |
| 1 | Envelope + `2B` | State 2; normal editor session |
| 1 | Envelope + `52` | Editor-related event `0x56`; state unchanged |
| 2 | Envelope + `67 01` | Editor information event `0x54`; not the factory handshake |
| 3 | Envelope + `10`, `53`, or `67` | Decode factory command |

In the inspection window, event `0x57` becomes window event 16 and inspection
operation 4. It sets the connected flag and replies:

```text
F0 52 00 00 67 0C F7
```

Identity is available without inspection mode. Selecting parser state 3 alone
does not initialize the inspection window or guarantee a connection reply.

`10 03` also arms/restarts RTOS timer 22 with a 1000 ms interval. Its timeout
callback at `0x800401f8` clears the factory-connected flag, resets the session
to state 0, and logs `JIG APP KEEPALIVE TIMEOUT!!!`. After arming it, send this
request more frequently than once per second. It doubles as a metadata query.
The handshake itself does not arm this timer in the inspected path.

## Factory commands

Prefix requests and replies with `F0 52 00 00` and suffix with `F7`.
`p` is the parameter byte; `n` is a selector; `v` is a returned byte.

| Request body | Meaning / effect | Reply body | Side effects |
| --- | --- | --- | --- |
| `10 n` | Read four metadata bytes, selectors below | `11 n d0 d1 d2 d3` | `n=3` arms/restarts keepalive |
| `53` | Clear connected flag and reset session | None in inspected handler | Disconnects; does not itself restore mixer routing |
| `67 0D` | Inspection operation 6 | `67 0E` | Clears session/connection |
| `67 0F 00 n` | Read input selector | `67 0F 01 n 01 v` | Read query |
| `67 0F 02 p on` | Toggle test control index `p`; zero/nonzero state `on` | `67 0F 03` | Changes analog mute/routing/attenuation |
| `67 0F 04 p` | Check calibration marker | `67 0F 05 present` | Read query; `p` unused |
| `67 0F 08 p` | Start ADC calibration | Asynchronous calibration result | Can overwrite calibration in NOR |
| `67 0F 0D p` | Select audio test routing 0 through 3 | `67 0F 0E` | Changes DSP routing |
| `67 0F 11 p` | Start MIDI loopback test | Three test bytes on cable 1, timer-driven follow-up | Sends MIDI; starts timer 21 |
| `67 0F 13 p` | Leave inspection and request normal startup | `67 0E` | Restores normal routing; changes UI |

For the `02` control operation, the decoder packs `p * 2` and adds 1 when `on`
is nonzero. The dispatcher then splits this into control index and on/off state.
Index 0 controls AMUTE; 1–8 call the channel routing setter; 9–16 call a second
channel routing setter; 17 controls the input attenuator. Exact electrical
interpretation of every routing state remains unverified.

Only selectors `00`, `02`, `04`, `08`, `0D`, `11`, and `13` reach the inspection
command dispatcher. Other `67 0F` selectors are ignored. The dispatcher also
contains operation 7 (parameter echo, reply `67 0F 12 p`); the inspected MIDI
decoder does not route a selector to that operation.

### Metadata selectors for `10 n`

| `n` | Source | Representation |
| --- | --- | --- |
| 0 | Boot version cache at `0x8020aaa0`, loaded from NOR `0x04fffc` | Four original bytes |
| 1 | System version cache at `0x8020aac0`, loaded from NOR `0x1f7ffc` | Four original bytes; `0110` for stock SYSTEM 1.10 |
| 2 | Packaged panel-version cache at `0x80899a80`, loaded from NOR `0x1f6ffc` | Four original bytes; `0100` for stock panel 1.00; packaged version, not a live panel-flash query |
| 3 | Cached value read by `0x8000ae00` | First four characters of `%04X`; checksum interpretation remains to be confirmed |
| Other | Zero-filled local result buffer | Four zero bytes |

Response bytes are masked with `0x7f`. This is a four-byte metadata response,
not an arbitrary address read or a general memory dump.

### Input selectors for `67 0F 00 n`

| `n` | Reader / source |
| --- | --- |
| 0 | Cached startup ADC classification, scan index 6 (`0x80022e58`) |
| 1 | Cached startup ADC classification, scan index 7 (`0x80022e68`) |
| 2 | Cached startup digital status (`0x80022e78`) |
| 3–8 | Logical GPIO input indices 5–10, respectively (`0x80045490`) |
| 9 | Logical input index 29 (`0x80045490`) |
| 10 | ADC scan index 3: SOUND PAD |
| 11 | ADC scan index 2: MASTER |
| 12 | ADC scan index 1: MONITOR |
| 13 | ADC scan index 4: EFX RTN |
| 14 | ADC scan index 5: SUB-OUT |
| 15 or above | Fallback `0x7f`; no further input-index bound in this handler |

The response value is narrowed to a byte and masked with `0x7f`; do not assume
it is a full ADC sample or calibrated knob percentage. The channel encoders
are not included in this query table.

## Calibration storage and results

`calibration_has_record` at `0x80026428` compares 32 bytes at `0x805c5920`
against the `Dual AD Calibration` marker. It checks record presence, not a
cryptographic signature or complete calibration validity.

The start routine searches the jig file and caps its input at 1028 bytes. It
recognizes these eleven field names (check mask `0x7ff`):

- `OUTPUT_LEVEL_REF_FREQ`, `OUTPUT_LEVEL_LOW_FREQ`
- `ABSOLUTE_MIN`, `ABSOLUTE_MAX`, `RELATIVE_MIN`, `RELATIVE_MAX`
- `LO_FREQ_ABSOLUTE_MIN`, `LO_FREQ_ABSOLUTE_MAX`
- `LO_FREQ_RELATIVE_MIN`, `LO_FREQ_RELATIVE_MAX`
- `VERSION`

The failure/default path
still proceeds into calibration, so a missing or invalid file is not a
safe way to disable the write operation.

`factory_calibration_step` at `0x80086a08` calls the NOR calibration writer
`0x800264b8` on successful completion. Subsequent event IDs `0x5a`–`0x5d` are
routed to inspection operations 7–10. These are asynchronous calibration
processing/result events, not additional directly accepted SysEx commands.

The result builder `0x80040510` emits an **89-byte** SysEx packet:

```text
F0 52 00 00 67 0F 09 status <80 data bytes> F7
```

The data contains four groups of four IEEE-754 binary32 values. Each value
occupies five MIDI bytes, containing successive bit groups 0–6, 7–13, 14–20,
21–27, and 28–31. Reconstruct the 32-bit word as
`d0 | d1<<7 | d2<<14 | d3<<21 | d4<<28`, then reinterpret its bits as `f32`.
These are calibration measurements/coefficients, not executable code or a
memory address. Field-level physical units remain unverified.

The MIDI loopback timeout routes inspection operation 7 (the parameter echo
mentioned above), explaining why that operation exists without a direct
factory SysEx selector.

## Emulator diagnostic

`factory-protocol-smoke` uses the unmodified firmware, emulated USB packets,
and GPIO button closures. It requires `--volatile`, rejects a supplied SD
image, and creates its own disposable FAT32 card and persistent state fixture.
The first boot configures the calendar; the second holds pads 1–3 for factory
entry. It sends identity, connection, input, calibration-presence, metadata,
and exit requests. It never sends the calibration-start or routing commands.

The QEMU diagnostic verifies the following against actual firmware replies and
RAM state:

| Path | Verified behavior |
| --- | --- |
| Startup | GPIO closures plus jig filename select mode 7 after calendar setup |
| Identity and connection | Universal identity response followed by `67 0C` |
| Inputs | Selectors 0–14 respond; 15 and 20 return fallback `0x7f` |
| Unsupported command | `67 0F 01` receives no reply during the observation interval |
| Calibration presence | Fresh fixture replies `67 0F 05 00` |
| Metadata | System bytes `0110`; selector 2 bytes `0100`; boot bytes masked erased data; selector 3 bytes `5EDA` for this fixture |
| Keepalive expiry | Session byte and connected flag both become zero |
| Reconnection and exit | Handshake succeeds again; `67 0F 13 00` replies `67 0E` and resets session |

The fixture checksum/metadata value is not a claim about a physical unit's
system-information checksum. Calibration and routing packet layouts remain
static-analysis findings; the diagnostic does not exercise their effects.

```sh
# Run from the repository root.
cargo run --offline -p factory-protocol-smoke -- \
  --qemu /private/tmp/l6-qemu-src/build/qemu-system-arm \
  --volatile --logs emulator/logs/factory-protocol-smoke
```

### Verification limits

- Real hardware acceptance and electrical test routing are unverified.
- The bootloader is absent; its startup combinations and USB capabilities
  cannot be inferred from this application image.
- No arbitrary memory read/write or RAM execution command was identified in
  this factory decoder.
- Audio/calibration measurement behavior is not modeled sufficiently to
  validate calibration results.
