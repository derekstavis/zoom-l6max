# L6 Editor MIDI protocol

[Documentation index](../../README.md)

Investigation pointed to a SysEx session between ZOOM L6 Editor and the main
firmware, carried on the third USB MIDI port. This map joins two sources:

- Main firmware, SHA-256
  `980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`
  (SYSTEM 1.10). Addresses are main MCU runtime addresses.
- ZOOM L6 Editor for macOS 2.0.0 (build 39), bundle `jp.co.zoom.l6editor`. It
  is a Flutter application (Dart 3.7.2, package `d414_app`) with a Swift
  CoreMIDI plugin, `midi_connection`. Its Dart symbols are not obfuscated.

Message names below are the application's own enum and method names. Firmware
function names are investigator assigned. Byte layouts come from the firmware
parser and reply builders and were compared with the application's builders
and parsers. The [factory protocol map](factory-midi.md) covers the separate
inspection session that shares this parser.

Evidence is marked as follows:

- **Probe**: observed from stock firmware in the emulator through emulated USB.
- **Static**: read from firmware or application code only.

The application itself has not been run against the emulator, and nothing here
has been captured from hardware.

## Transport

The device is USB MIDI class on interface 4 (bulk OUT 3, bulk IN 4) with three
cables; see [SD storage and USB](../../emulator/usb.md). The editor
session uses cable **2**, `for L6 Editor Port`. Everything is SysEx carried in
USB MIDI event packets. No other USB interface or vendor request is involved in
the editor session.

`midi_settings_receive_task` at `0x8003e5b0` accepts `F0`, then only `52` or
`7E` as the next byte, then data bytes until `F7`. A message longer than 594
bytes or one containing another status byte is dropped. Complete messages go to
`midi_sysex_dispatch` at `0x80040988`. Replies are queued by `0x80041f78` with
port argument 2.

### How the application finds the port

The plugin enumerates CoreMIDI sources and destinations (`MIDIGetSource`,
`MIDIGetDestination`), so virtual endpoints qualify. It builds each device name
from the endpoint's `kMIDIPropertyModel` and `kMIDIPropertyName` strings joined
by a space; which comes first was not established. The application then takes
the first endpoint per direction whose name:

- contains `L6`, and
- contains `Editor` or `MIDIIN3` (input) / `Editor` or `MIDIOUT3` (output).

The emulator's `for L6 Editor Port (Emulator)` endpoints satisfy this, and the
other two emulator ports do not. (Static.)

The plugin sends each message as one `MIDIPacketList` built in a 1024-byte
buffer with `MIDISend`. Incoming packets are forwarded to Dart, where
`_SysExReceiver` reassembles SysEx across packets. CoreMIDI setup-change
notifications are debounced by 500 ms before the application reconnects.

## Framing

Universal identity is standard. Everything else uses this envelope (hex), with
byte index 0 at `F0`:

```text
F0 52 00 00 <function> <data...> F7
```

`52` is the ZOOM manufacturer ID. The application requires bytes 2 and 3 to be
`00`; the firmware does not check them. All data bytes are 7-bit.

| Function | Application name | Direction |
| --- | --- | --- |
| `00` | Respondence (acknowledgment) | Device to host |
| `2A` | Global setting dump | Device to host |
| `2B` | `globalSettingDumpRequest` | Host to device |
| `31` | `parameterChange` | Both |
| `45` | `advPatchDataDump` | Both |
| `46` | `advPatchDataDumpRequest` | Host to device |
| `67` | `recorderEditorMessage` | Both |

Multi-byte encodings:

- **14-bit, low first**: `lo | hi << 7`. Used unless stated otherwise.
- **14-bit, high first**: `hi << 7 | lo`. Used only for file numbers.
- **64-bit**: ten bytes, seven bits each, least significant group first.
- **Packed bytes**: every seven source bytes become eight. The first carries the
  seven high bits (bit 6 belongs to the first source byte, bit 0 to the
  seventh); the next seven carry the low seven bits. Encoder `0x800872b8`,
  application `MidiUtil.decode7bitData`.
- **File names**: UTF-16 code units, low byte first, then packed as above.

## Session state

The byte at `0x805dca98` is the session state, shared with the factory session.

| State | Accepted | Effect |
| --- | --- | --- |
| 0 | `F0 7E dd 06 01 F7` | State 1; identity reply |
| 1 | Envelope + `2B` | State 2; global setting dump |
| 1 | Envelope + `52` | Event `0x56`; state unchanged; purpose not established |
| 1 | Envelope + `67 0B` | State 3, factory session |
| 1 | Any message whose second byte is not `52` | State 0 |
| 2 | `F0 7E dd 06 01 F7` | Identity reply; state unchanged |
| 2 | Envelope + `2B`, `31`, `45 05`, `46`, `67 01` | Editor commands below |

In state 0 an editor command is discarded without reply. (Probe: `2B` after a
lapsed session received nothing until identity was repeated.)

Requests are decoded in the MIDI task and posted as events to the main event
dispatcher at `0x800363b8`, which builds the replies. That dispatcher returns
early, with no reply, unless startup mode `0x80462d9c` is 7 or bit 1 of the
byte at `0x80200408` is set. On the recorder screen that byte read `06` and
every request below was answered. Which screens clear the bit is not mapped, so
identity may go unanswered during startup dialogs. (Static, except the recorder
observation.)

### Identity

```text
host    F0 7E 00 06 01 F7
device  F0 7E 00 06 02 52 72 00 0D 00 v0 v1 v2 v3 F7
```

The application sends device ID `00`. The firmware ignores that byte. `v0`–`v3`
are four ASCII digits of the system version: `30 31 31 30` for 1.10. (Probe.)

The application accepts the reply only when it is 15 bytes with device ID `00`,
manufacturer `52`, family `72 00`, a known model byte, and a following `00`.
Model `0D` is L6max; `0B` is L6. Any other reply raises its illegal-device
error. It displays the version as `1.10`.

### Connection sequence

`DeviceInitializationService` runs these steps in order. Each waits for its
reply, and a failure cancels initialization. (Static for ordering; each exchange
is Probe.)

| Step | Host sends | Expected reply |
| --- | --- | --- |
| Identity | `F0 7E 00 06 01 F7` | Identity reply |
| Global settings | `2B` | `2A` dump |
| Application version | `45 05` + four 14-bit numbers | `00 11` |
| SD card information | `67 01` | `67 00` + 26 bytes |
| File lists | `46 00`, `46 01`, `46 02` per pad | `45 00`, `45 01`, `45 02` |
| Clock | `31 00` + date and time | `00 00` |

After that it repeats keep-alive `31 0B` and expects `00 0B` each time.

### Timing

- The application's default reply timeout is 1 s per request. Its retry counts
  and its keep-alive interval were not recovered.
- The first `31 0B` starts periodic RTOS timer 9 at 1000 ms. Each `31 0B` sets a
  flag at `0x805dcaf8`. The callback at `0x800886f0` clears the flag, or counts
  a miss when it is not set. On the fifth consecutive miss it sets the session
  state to 0 and posts event `0xCE`. (Static.)
- Probe: the session survived 1.6 s of silence and was reset after 6.5 s.
- Without any keep-alive the watchdog never starts.

## Acknowledgments and errors

```text
F0 52 00 00 00 cc F7
```

`cc` shares numbering with the parameter types below. The firmware acknowledges
most settings with `00`. The application's reply matcher compares `cc` with a
value supplied per request and treats `40` as failure.

| `cc` | Sent after |
| --- | --- |
| `00` | Most accepted parameter changes |
| `04` | Recorder mode change |
| `05` | Sound pad file assignment or removal |
| `08` | Sound pad play button |
| `09` | SD card reader mode change |
| `0B` | Keep-alive |
| `11` | Application version accepted |
| `12` | Dialog answer handled |
| `40` | Failure; an error message follows |

A failure is two messages: `00 40`, then parameter change type `10`:

```text
F0 52 00 00 31 10 ee F7
```

Error codes `ee` emitted by this firmware, with triggers inferred from the
handlers (Static):

| `ee` | Trigger |
| --- | --- |
| `00` | SD card reader mode refused while the recorder is busy |
| `02` | Reset refused while the recorder is busy |
| `03` | Reset requested; awaits a dialog answer |
| `04` | Application version out of range |
| `05` | Application version is all zero |
| `06` | Pad assignment awaits a dialog answer |
| `07`, `08`, `09` | Pad assignment rejected; three distinct file checks |
| `0A` | Recorder mode change refused while the recorder is busy |
| `0B` | Other refusal |
| `0C` | Pad assignment refused for the pad currently in use |

Codes `03` and `06` double as dialog identifiers: the host answers with
parameter change `12`.

## Global setting dump

Request `F0 52 00 00 2B F7`. Reply builder `0x800412b8`, 182 bytes. The
application accepts an L6max dump when it is at least 182 bytes and the length
field is at least 174. (Probe for length and defaults; field names Static.)

| Index | Field | Fresh emulator state |
| --- | --- | --- |
| 4 | `2A` | |
| 5–6 | Payload length, 14-bit low first | `2E 01` (174) |
| 7 | `batteryType` | `00` |
| 8 | `autoPowerOff` | `01` |
| 9 | `mixerControlViaMidi` | `00` |
| 10 | `recorderMode` | `00` |
| 11 | `usbAudioIFMode` | `00` |
| 12 | `usbMixMinus` | `00` |
| 13 | `midiClockSyncSource` | `00` |
| 14–17 | `midiClockSyncEnable`, four entries | `00` each |
| 18–21 | `soundPadPlayMode`, pads 1–4 | `01` each |
| 22–25 | `soundPadLevel`, pads 1–4 | `31` each |
| 26–29 | Sound pad playing state, pads 1–4 | `00` each |
| 30 | `sdCardReaderMode` | `00` |
| 31 | `midiOutMode` | `00` |
| 32 | `midiChannel` | `00` |
| 33–126 | `midiCCNumber`, 94 entries; unassigned sent as `00` | See default map |
| 127–130 | Sound pad MIDI note, pads 1–4 | `3C 3E 40 41` |
| 131–134 | Sound pad note not mapped, pads 1–4 | `00` each |
| 135–154 | Effect parameters, ten 14-bit values | Below |
| 155 | `monitorPoint` | `01` |
| 156 | `subOutPoint` | `01` |
| 157–164 | AUX 1 pre/post, channels 1–8 | `01` each |
| 165–172 | AUX 2 pre/post, channels 1–8 | `01` each |
| 173–180 | Zero in this firmware | `00` each |
| 181 | `F7` | |

Effect values are ordered type 0 index 0, type 0 index 1, through type 4
index 1. The fresh values were 40, 50, 50, 50, 90, 30, 505, 35, 505, 50. For
types 3 and 4 the index-0 value is sent one higher than stored when the
selected variant is 0.

The application reads indices 157–180 as three groups of eight.

L6 uses a shorter dump (at least 139 bytes, length at least 131) with a
different layout. It was not mapped.

## Parameter change

```text
F0 52 00 00 31 tt <args...> F7
```

Host to device. The firmware checks every range shown and silently drops a
message that fails. Pad, channel, and index arguments are zero based. (Static,
except where marked.)

| `tt` | Name | Arguments and ranges | Reply |
| --- | --- | --- | --- |
| `00` | `dateTime` | year−2000 (0–99), month (1–12), day (1–31), hour (0–23), minute (0–59), second (0–59) | `00 00` (Probe) |
| `01` | `batteryType` | value 0–2 | `00 00` |
| `02` | `autoPowerOff` | value 0–1 | `00 00` |
| `03` | `mixerControlViaMidi` | value 0–1 | `00 00` |
| `04` | `recorderMode` | value 0–1 | `00 04`, then `67 00` |
| `05` | `soundPadAssignFileName` | pad 0–3, file number (high first), name length (low first), packed name | `00 05` or error |
| `06` | `soundPadPlayMode` | pad 0–3, mode 0–2 | `00 00` |
| `07` | `soundPadLevel` | pad 0–3, level 0–59 | `00 00` |
| `08` | `soundPadPlayBtn` | pad 0–3, 1 play / 0 stop | `00 08` conditionally |
| `09` | `sdCardReaderMode` | value 0–1 | `00 09` or error |
| `0A` | `factoryReset` | none | Error `03`, then dialog |
| `0B` | `keepAlive` | none | `00 0B` (Probe) |
| `0C` | `midiOutMode` | value 0–1 | `00 00` |
| `0D` | `midiChannel` | value 0–15 | `00 00` (Probe) |
| `0E` | `midiCCNumber` | 94 CC numbers | `00 00` |
| `0F` | `soundPadMidiNote` | pad 0–3, note, not-mapped 0–1 | `00 00` |
| `12` | `dialogYesNo` | dialog 0–12, answer 0 no / 1 yes / 2 other | See below |
| `13` | `efxParam` | type 0–5, index 0–1, 14-bit value | `00 00` |
| `14` | `auxPrePost` | channel 0–7, AUX 0–1, value 0–1 | `00 00` |
| `15` | `usbMixMinus` | value 0–1 | `00 00` |
| `16` | `midiClockSyncSource` | value 0–2 | `00 00` |
| `17` | `midiClockSyncEnable` | entry 0–3, value 0–1 | `00 00` |
| `18` | `usbAudioIFMode` | value 0–1 | `00 00`, then session reset |
| `19` | `monitorPoint` | value 0–3 | `00 00` |
| `1A` | `subOutPoint` | value 0–3 | `00 00` |

Types `10` (`error`) and `11` (`appVersion`) are ignored from the host. An
unknown type receives no reply. (Probe: `31 7F`.)

Notes:

- `05` with name length 0 removes the assignment. Otherwise the firmware
  compares the supplied name with its own file list entry before assigning.
- `08` is acted on in the MIDI task. It replies only for play, or for stop when
  the pad's play mode is 2.
- `09` with value 1 sends `45 03 pad 00` for each playing pad, replies `00 09`,
  stops the keep-alive timer, sets the session state to 0, and opens the card
  reader window. The host must repeat identity afterwards.
- `0A` never resets directly. The firmware replies `00 40` and error `03`. The
  host then sends `31 12 03 01`; the firmware resets settings, replies `00 12`,
  and sends `67 00`. Any other answer to dialog `03` replies `00 12` only.
- Dialog `06` continues a pending pad assignment for answer 1 or 2.
- `0E`: values above `77`, `00`, `20`, and `60`–`65` are stored as unassigned.
- `0F`: a note already used by another pad is removed from that pad.
- `18` replies, sets the session state to 0, and restarts the USB device about
  one second later. The host port disappears and returns.

Device to host, sent by `0x80041700` when a setting changes on the panel and by
the reset path. The argument layout matches the host form. Observed senders
cover types `03`, `04`, `13`, `15`, `16`, `18`, and `19`; other panel paths
were not traced. (Static.)

## Patch data dumps

Requests are `F0 52 00 00 46 kk <args> F7`; data is
`F0 52 00 00 45 kk <data> F7`.

| `kk` | Name | Request arguments | Data |
| --- | --- | --- | --- |
| `00` | `fileNum` | pad 0–3 | pad, file count (high first) |
| `01` | `fileName` | pad 0–3, file number (high first) | pad, file number (high first), name length (low first), packed name |
| `02` | `assignFileName` | pad 0–3 | Same shape as `01` |
| `03` | `soundPadState` | pad 0–3 | pad, 0 stopped / 1 playing |
| `04` | `midiCCNumberDefault` | none | 94 CC numbers; 101-byte message |
| `05` | `appVersion` | Host to device only | Four 14-bit numbers |
| `06` | `sdInsertedState` | Not requestable | 0 or 1 |
| `07` | `efxParam` | Not requestable | type, index, 14-bit value |
| `08` | `auxPrePost` | Not requestable | channel, AUX, value |

Name length is the count of packed bytes that follow. An unassigned pad returns
file number `7F 7F` and length 0: `45 02 pad 7F 7F 00 00`. (Probe.)

With no SD card, `46 00` returned count 0 for each pad. (Probe.) `46 01` was not
exercised.

`46 04` also restores the default CC map in the device before replying. The
default map on this firmware is `01`–`08`, `0B`–`12`, `15`–`1C`, `21`–`58`,
`5D`–`5F`, `66`–`6A`, `6D`, `6E`, `71`, `72`, `75`, `77`. (Probe.)

`45 05` carries the application version, 2.0.0.39 as
`45 05 02 00 00 00 00 00 27 00`. Any version that is nonzero and in range is
acknowledged with `00 11`. (Probe.)

While the session state is 2 the firmware also sends `03`, `06`, `07`, and `08`
unprompted when the corresponding state changes. (Static.)

## Recorder messages

Request `F0 52 00 00 67 01 F7`. Reply builder `0x80041800`, 32 bytes:

| Index | Field |
| --- | --- |
| 4–5 | `67 00` |
| 6 | Card state: 0 absent, 1 usable, 2 present but not usable (meaning of 2 inferred) |
| 7 | Remaining hours ÷ 100 |
| 8 | Remaining hours mod 100 (inferred) |
| 9 | Remaining minutes |
| 10 | Remaining seconds (inferred) |
| 11–20 | 64-bit byte count, used space |
| 21–30 | 64-bit byte count, card capacity |

With no card every data byte was `00`. (Probe.) The field split of the time
bytes and the meaning of the two counts are inferred from the builder and the
application's `maxCapacityStr`, `usageCapacityStr`, and `remainingTimeStr`. The
application also accepts a 31-byte form.

The firmware sends the same message unprompted when remaining time changes,
after a recorder mode change, and after a reset.

## Evidence

Packet layouts combine interpreted firmware/application findings and replies
from a stock-firmware emulator probe on USB MIDI cable 2. The probe used
volatile state and no SD card. Static findings and inferred fields are marked
above; detailed reconstruction notes are maintained separately from this
public protocol specification.

## Verification limits

- The application has not been connected to the emulator. Port matching and
  CoreMIDI behavior are static findings.
- Hardware behavior is unverified.
- The gate that suppresses replies outside eligible screens is identified but
  its screens are not mapped.
- File name transfer, pad assignment, and dialog flows were not exercised;
  there was no SD card in the probe.
- SD card reader mode and audio interface mode restart USB. Their effect on a
  live application session in the emulator is untested.
- L6 (non-max) layouts are noted only where the application distinguishes them.
- Application retry counts and keep-alive interval remain unknown.
