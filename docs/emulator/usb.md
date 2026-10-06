# SD storage and USB

[Documentation index](../README.md)

Investigation pointed to separate board card detection, the RT1052 USDHC
controller, and a ChipIdea USB device controller. The firmware owns card
initialization, its filesystem, USB descriptors, MIDI processing, and SCSI
commands. Host requests reach those handlers through modeled peripherals.

## SD card

In the macOS bundle, **File → Use SD Card Folder…** imports a folder into a
private persistent card and restarts the guest. See the
[app guide](../guides/macos-app.md#use-a-folder-as-the-sd-card) for semantics and limits.
From a source checkout, create the same folder snapshot explicitly:

```sh
cargo run -p sd-image -- /path/to/card.img --directory /path/to/sd-folder
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
and `insert_sd` for an engine started with an attached image.

RT1052 USDHC derives its clock and power from the SoC. Firmware waits for
PRES_STATE.SDSTB before programming SYS_CTRL, whose low three bits are reserved
on this controller. QEMU's SDHCI clock model instead requires an internal
clock enable bit. The board adapter retains the firmware-visible divider and
timeout fields, supplies stable SoC clock and 3.3 V power, and translates
reset requests. RSTA/RSTC/RSTD and INITA self-clear. Card commands, DMA and
controller interrupts continue through upstream QEMU.

Validation covers CMD0/8, ACMD41, identification, card selection, CMD17/18
reads, filesystem startup, and removal/reinsertion. With the tested FAT32
image, the recorder changes from `No SD Card` to `No File`. Removing the
medium restores `No SD Card`; reinserting it restores the original recorder
frame and produces a new initialization/read sequence.

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

## Native MIDI

Run the native app with `--usb` to expose three CoreMIDI source/destination
pairs on macOS:

- `L6max MIDI I/O Port (Emulator)`
- `L6max Mixer Control Port (Emulator)`
- `for L6 Editor Port (Emulator)`

These correspond to the [documented USB MIDI ports](https://zoomcorp.com/manuals/l6max-en/).
The CoreMIDI callback queues bytes to a worker, which preserves running status
and SysEx across packet boundaries and converts them into USB MIDI 1.0 event
packets. Firmware-generated endpoint data takes the reverse path. The worker
continues consuming MIDI IN even when no host application is connected.
Closing the engine cancels socket operations, joins the worker and disposes
its endpoints.

The mixer-control round trip is verified with a separate CoreMIDI client:
a channel-1 encoder produces CC `0x51` on cable 1, and host CC `0x51` with
value 100 changes the firmware-driven channel-1 GPIO LED ring.

## Host file access

The `usb-files` tool starts its own engine, accepts the startup dialogs using
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
cargo run -p sd-image -- /path/to/card.img 64

# Native panel with SD storage and macOS MIDI ports.
cargo run -p l6max-gui -- --sd-image /path/to/card.img --usb

# File commands own a separate guest; the card image has one QEMU owner at a time.
cargo run -p usb-files -- --sd-image /path/to/card.img -- list /
cargo run -p usb-files -- --sd-image /path/to/card.img -- get /TRACK.WAV /path/to/track.wav
cargo run -p usb-files -- --sd-image /path/to/card.img -- put /path/to/track.wav /TRACK.WAV
```

`put` replaces the named SD file. Firmware and card images remain local and
are excluded from Git. USB file access has been validated by creating,
writing, reading and deleting a 1537-byte file, crossing sector boundaries.
Separate `put` and `get` runs also verified byte-identical readback after
restarting the guest with the same card image.

## Validation

```sh
cargo test -p l6max-host --lib
cargo run -p storage-smoke -- --logs emulator/logs/sd-absent
cargo run -p storage-smoke -- --sd-image /path/to/test-card.img --logs emulator/logs/sd-hotplug
cargo run -p usb-smoke -- --logs emulator/logs/usb-enumeration
cargo run -p usb-smoke -- --sd-image /path/to/test-card.img --logs emulator/logs/usb-storage
cargo run -p midi-smoke -- --logs emulator/logs/native-midi
cargo run -p usb-files -- --sd-image /path/to/test-card.img -- roundtrip
```

The storage USB smoke writes back an unchanged sector. `roundtrip` creates a
uniquely named temporary test file and deletes it after readback. Integration
checks use guest-clock input durations and published frames; they do not move
the host mouse. Logs and captured screens are excluded from Git.

## Current limits

- Finder/OS USB disk mounting is not implemented; host file access uses
  `usb-files`.
- USB audio streaming and physical DIN/TRS MIDI I/O are not modeled. The mixer
  MIDI control port is verified; complete L6 Editor compatibility is unverified.
- CoreMIDI bridging is macOS-only. USB endpoint and storage tools use Unix IPC.
- USB transactions consume complete dTDs or short OUT stages, up to 16 KiB.
  Full-packet partial dTD transfers and isochronous transfers are unsupported.
- USDHC clock, initialization clocks, and regulator transitions are functional
  approximations rather than cycle-accurate electrical timing.
- GPIO interrupt modeling here covers the SD detect input, not every GPIO pin.
