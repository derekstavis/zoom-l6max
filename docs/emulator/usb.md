# SD storage and USB

[Documentation index](../README.md)

The board models card detection, the RT USDHC controller, and a USB device
controller. The firmware owns card
initialization, its filesystem, USB descriptors, MIDI processing, and SCSI
commands. Host requests reach those handlers through modeled peripherals.

## SD card

In the macOS bundle, **File → Use SD Card Folder…** imports a folder into a
private persistent card and inserts it live, like inserting a card into the
slot, with no engine restart; **File → Eject SD Card** removes it the same
way. See the
[app guide](../guides/macos-app.md#use-a-folder-as-the-sd-card) for semantics and limits.
From a source checkout, create the same folder snapshot explicitly:

```sh
cargo run -p l6fs -- card import /path/to/card.img --directory /path/to/sd-folder
cargo run -p l6max-gui -- /path/to/qemu-system-arm --sd-image /path/to/card.img
```

| Surface | Mapping |
| --- | --- |
| Card detect | GPIO2 pin 28, active high |
| GPIO2 upper-bank IRQ | NVIC 83 |
| USDHC1 | `0x402c0000`, NVIC 110 |
| Card transport | Upstream QEMU SD card, SDHCI/IMX USDHC, ADMA and block backend |

The card-detect setup at `0x80022a90` samples GPIO2 and invokes its insertion
callback when bit 28 is high. Firmware also enables that pin's interrupt.
The board samples QEMU SDBus medium state at the existing virtual millisecond
tick, updates the input, latches GPIO ISR, and raises IRQ 83 when unmasked.
GPIO ISR is write-one-to-clear. QMP media replacement therefore changes both
the card model and the detect pin. The Rust QAPI client provides `eject_sd`
and `insert_sd`, wrapped as `Engine::eject_sd`/`insert_sd`. The slot itself
is always inherited (an `-drive if=sd,id=l6-sd` with no `file=` when no card
is given at boot, like the real card-edge connector is always present), so
both work at any time, including inserting a card that was never attached at
boot — no engine restart needed either way, mirroring the [USB and DIN MIDI
jacks' own hotplug](#hotplug).

RT USDHC derives its clock and power from the SoC. Firmware waits for
PRES_STATE.SDSTB before programming SYS_CTRL, whose low three bits are reserved
on this controller. QEMU's SDHCI clock model instead requires an internal
clock enable bit. The board adapter retains the firmware-visible divider and
timeout fields, supplies stable SoC clock and 3.3 V power, and translates
reset requests. RSTA/RSTC/RSTD and INITA self-clear. Card removal does not
gate that clock, so a command issued to an empty slot ends with a command
timeout error. PROT_CTRL WECINT/WECINS/WECRM enable wakeup events only; the
adapter keeps them readable but out of the interrupt decision, so card
insertion and removal interrupt only through INT_SIGNAL_EN. Card commands, DMA
and controller interrupts continue through upstream QEMU.

### Card layout

Observed on L6max firmware 1.10 through the emulator, by pressing Record:

| Card | Result |
| --- | --- |
| FAT16 with 2 KiB clusters, FAT32 with 4 or 8 KiB clusters | `Invalid SD Card!` |
| FAT16, 16 KiB clusters, no `ZOOM_L6max.sys` | Opens the SD Card menu, which also blocks pad input |
| FAT16, 16 KiB clusters, with `ZOOM_L6max.sys` | Records |
| exFAT, 32 GiB or smaller | `Invalid SD Card!` |
| exFAT, 33 GiB or larger, 128 KiB clusters, with `ZOOM_L6max.sys` | Records |

Cards with small clusters still boot and play assigned pads. WAV files need the
FAT archive attribute to appear in the firmware's file list. L6 has not been
tested against this table.

### Validation

Validation covers CMD0/8, ACMD41, identification, card selection, CMD17/18
reads, filesystem startup, and removal/reinsertion. With the tested FAT32
image, the recorder changes from `No SD Card` to `No File`. Removing the
medium restores `No SD Card`; reinserting it restores the original recorder
frame and produces a new initialization/read sequence. A card that holds a
recorder project returns with a new project named from the current clock
instead of the project selected at power-on.

## USB device controller

| Surface | Mapping |
| --- | --- |
| ChipIdea USB1 | `0x402e0000`, NVIC 113 |
| Queue-head list | Firmware programs ENDPTLISTADDR, observed `0x20007000` |
| Queue heads / dTDs | 64-byte / 32-byte guest-memory DMA structures |
| Normal device | VID `1686`, PID `08f5`, configuration 507 bytes, 5 interfaces |
| USB MIDI | Interface 4; bulk OUT 3, IN 4; 3 cables |
| File-transfer device | Adds mass-storage interface 5; bulk IN 5, OUT 6 |

Upstream QEMU's ChipIdea implementation models the host controller. The
board's device model handles reset, setup packets, endpoint prime/flush,
DMA transfer completion, stalls, write-one-to-clear status, OTG VBUS status,
and USB IRQ delivery. SOF interrupts run on QEMU virtual time at 1 ms; FRINDEX
uses 125 microsecond increments. It advertises high-speed device operation.

A dedicated inherited Unix socket carries host setup/IN/OUT transactions and
replies. It uses no socket pathname or shared RAM file. QEMU reads and writes
guest dQH/dTD buffers through the CPU address space, then updates controller
status and interrupts. Rust does not substitute descriptors or execute
firmware class handlers on the host.

The firmware exposes USB audio and MIDI descriptors in normal mode. Selecting
`USB File Transfer` in the recorder menu adds the mass-storage interface.
The firmware's menu entry is index 5, after Project, Recorder Mode, Sound Pad,
Mixer and SD Card. The transition stops and restarts the USB controller;
host enumeration allows that transition and bus-reset processing to finish.
The device model tracks a host SETUP until an EP0 transfer completes it. When
firmware flushes every endpoint or resets the controller first, as its bus-reset
handling does, later EP0 tokens return ECONNRESET and the host resends the
control transfer from SETUP.

L6 and L6max both enumerate through the direct USB device socket. On L6, the
USB startup wrapper, initializer, and PHY gate run at about 3.1 seconds in a
volatile QEMU boot, followed by controller-register writes. The direct L6 USB
smoke reads the 18-byte device descriptor and 598-byte configuration, accepts
`SET_CONFIGURATION`, and receives an encoder-generated MIDI packet. The
model-aware direct MIDI smoke also passes the host-to-device path: L6 emits
CC 83 for the tested encoder, and host CC 81 changes its channel-1 indicator
ring from `0x1` to `0x3`. L6max retains its CC 81 encoder mapping and its ring
changes from `0x1` to `0x1ff`. These are QEMU results; physical USB behavior
has not been tested.

## Native MIDI

Run the native app with `--usb` to expose three CoreMIDI source/destination
pairs on macOS. The MAIN firmware reset vector selects the model-specific
names automatically:

- L6: `L6 MIDI I/O Port (Emulator)`, `L6 Mixer Control Port (Emulator)`, and
  `for L6 Editor Port (Emulator)`.
- L6max: `L6max MIDI I/O Port (Emulator)`, `L6max Mixer Control Port
  (Emulator)`, and `for L6 Editor Port (Emulator)`.

These correspond to the documented USB MIDI ports in the [L6 operation
manual](https://zoomcorp.com/manuals/l6-en/) and [L6max operation
manual](https://zoomcorp.com/manuals/l6max-en/).
The CoreMIDI callback queues bytes to a worker, which preserves running status
and SysEx across packet boundaries and converts them into USB MIDI 1.0 event
packets. Firmware-generated endpoint data takes the reverse path. The worker
continues consuming MIDI IN even when no host application is connected.
Closing the engine cancels socket operations, joins the worker and disposes
its endpoints.

The ports appear after firmware completes USB enumeration; startup can take
longer in host wall time than the guest-time initialization. Clients should
wait for the endpoints. `--auto-usb-unlock` supplies the L6 startup inputs used
by the direct MIDI-sync scenario; basic Editor connectivity does not require it.

### Hotplug

Both the USB cable and the DIN MIDI jacks are hotpluggable at any time,
matching real hardware — no engine restart needed. `--usb` and `--midi` only
pick the state at startup; the native app's **Connections** menu ("Connect
USB", "Disconnect USB", "Connect MIDI", "Disconnect MIDI") toggles each
independently while running, and `Engine::connect_usb`/`disconnect_usb`/
`connect_midi`/`disconnect_midi` do the same from code (`engine.rs`).

Disconnecting USB removes its host endpoints and reconnecting enumerates the
guest again. Disconnecting DIN MIDI closes its host port; guest output is
drained and discarded until the port is reconnected.

### Host MIDI back ends

The worker is written once, against the `HostMidi` trait in
`emulator/host/src/host_midi/`: `open` creates the three endpoints, `read`
drains messages host applications sent (per cable), and `write` delivers device
bytes to a cable's subscribers. Back ends are selected by `cfg`:

| Platform | Back end | Endpoints |
| --- | --- | --- |
| macOS | CoreMIDI | Virtual sources and destinations |
| Linux | ALSA sequencer | One client, `L6max Emulator`, three duplex subscribable ports (`aconnect -l`) |
| Other | none | The worker still drains the device and reports "Host MIDI unavailable" |

Dropping a back end removes its endpoints. A new platform adds one file that
implements `HostMidi`. USB framing, chunking and cable routing stay in
`usb_midi.rs` and are unit-tested with an in-memory back end. Opening
`/dev/snd/seq` may need `audio` group membership; see the
[Linux guide](../guides/linux.md). The `l6max-direct-midi-sync` driver case
exercises USB MIDI without a host MIDI system and checks the mixer-control
cable routing.

The mixer-control round trip is verified with a separate host MIDI client (CoreMIDI on macOS, ALSA on Linux):
a channel-1 encoder produces CC `0x51` on cable 1, and host CC `0x51` with
value 100 changes the firmware-driven channel-1 GPIO LED ring.

## Physical MIDI IN/OUT (DIN/TRS) jacks

Use `--midi` to expose the model's MIDI IN/OUT port independently of USB.
The native app can connect or disconnect the port from **Connections** while
the emulator is running. L6 uses MAIN LPUART2 and L6max uses LPUART3.

The L6max MIDI input forwards messages to the USB MIDI I/O port. Stock
L6max firmware does not route that input to mixer controls; the optional
[DIN mixer-control patch](../../patches/din-mixer-control/README.md) adds that
route. L6 forwards tested DIN messages to USB, but its DIN mixer-control and
forwarding scenarios need further qualification. See
[current status](../development/current-status.md) for the current limits.

## Host file access

The `l6fs usb` tool starts its own engine, accepts the startup dialogs using
GPIO button events, selects USB File Transfer, enumerates the device, and
uses USB Bulk-Only Transport. SCSI INQUIRY identifies `ZOOM / L6max SD R&W /
1.00`. TEST UNIT READY and REQUEST SENSE handle startup unit attention before
READ CAPACITY. READ(10) and WRITE(10) reach the firmware SD driver. The Rust
FAT filesystem accesses a cached, seekable USB disk; dirty sectors are flushed
before shutdown. Both superfloppy FAT volumes and FAT MBR partitions are
supported.

From the repository root:

```sh
# Create an empty FAT32 raw card. Existing files are never overwritten.
cargo run -p l6fs -- card create /path/to/card.img --size-mib 64

# Native panel with SD storage and host MIDI ports (CoreMIDI or ALSA).
cargo run -p l6max-gui -- --sd-image /path/to/card.img --usb

# File commands own a separate guest; the card image has one QEMU owner at a time.
cargo run -p l6fs -- usb --sd-image /path/to/card.img -- list /
cargo run -p l6fs -- usb --sd-image /path/to/card.img -- get /TRACK.WAV /path/to/track.wav
cargo run -p l6fs -- usb --sd-image /path/to/card.img -- put /path/to/track.wav /TRACK.WAV
```

`put` replaces the named SD file. Firmware and card images remain local and
are excluded from Git. USB file access has been validated by creating,
writing, reading and deleting a 1537-byte file, crossing sector boundaries.
Separate `put` and `get` runs also verified byte-identical readback after
restarting the guest with the same card image.

## Validation

```sh
cargo test -p l6max-host --lib
cargo run --offline --locked -p l6test -- run --case storage-empty --model l6 --config smoke.local.toml
cargo run --offline --locked -p l6test -- run --case storage --model l6max --config smoke.local.toml
cargo run --offline --locked -p l6test -- run --case usb --model l6max --config smoke.local.toml
cargo run --offline --locked -p l6test -- run --case usb-storage --model l6max --config smoke.local.toml
```

The `usb-storage` driver case writes back an unchanged sector and checks the
FAT-level round trip by creating a temporary test file, reading it back
byte-for-byte, and deleting it. Integration checks use guest-clock input
durations and published frames; they do not move the host mouse. Logs and
captured screens are excluded from Git.

## Current limits

- Finder/OS USB disk mounting is not implemented; host file access uses
  `l6fs usb`.
- USB audio streaming is not modeled. The mixer MIDI control port is
  verified; complete L6 Editor compatibility is unverified. The physical
  MIDI IN/OUT (DIN/TRS) jacks are modeled, including the receive/transmit
  interrupt (see above); whether firmware applies mixer Control Change
  received there is not yet confirmed.
- Host MIDI bridging uses CoreMIDI on macOS and the ALSA sequencer on Linux;
  other platforms have no bridge (see the back-end table above). The USB link
  to QEMU is a Unix socket pair with inherited descriptors, so Windows is not
  supported.
- USB transactions consume complete dTDs or short OUT stages, up to 16 KiB.
  Full-packet partial dTD transfers and isochronous transfers are unsupported.
- USDHC clock, initialization clocks, and regulator transitions are functional
  approximations rather than cycle-accurate electrical timing.
- GPIO interrupt modeling here covers the SD detect input, not every GPIO pin.
