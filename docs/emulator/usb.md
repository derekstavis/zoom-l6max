# SD storage and USB

[Documentation index](../README.md)

Investigation pointed to separate board card detection, the RT1052 USDHC
controller, and a ChipIdea USB device controller. The firmware owns card
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
and `insert_sd`, wrapped as `Engine::eject_sd`/`insert_sd`. The slot itself
is always inherited (an `-drive if=sd,id=l6-sd` with no `file=` when no card
is given at boot, like the real card-edge connector is always present), so
both work at any time, including inserting a card that was never attached at
boot — no engine restart needed either way, mirroring the [USB and DIN MIDI
jacks' own hotplug](#hotplug).

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
pairs on macOS, as soon as the engine starts:

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

### Hotplug

Both the USB cable and the DIN MIDI jacks are hotpluggable at any time,
matching real hardware — no engine restart needed. `--usb` and `--midi` only
pick the state at startup; the native app's **Connections** menu ("Connect
USB", "Disconnect USB", "Connect MIDI", "Disconnect MIDI") toggles each
independently while running, and `Engine::connect_usb`/`disconnect_usb`/
`connect_midi`/`disconnect_midi` do the same from code (`engine.rs`).

The emulated ChipIdea USB device and the LPUART3 socket are always inherited
by the QEMU child process, like the real chip's USB and UART silicon is
always physically present; only whether the host side is actively bridging
them is a runtime toggle. For USB, disconnecting sends the board model's own
`connected = false` protocol message (the same one `enumerate()` sends
`true` for on connect) and stops the 3 CoreMIDI ports, without closing the
inherited socket, so a later connect reuses it instead of restarting QEMU.
For DIN MIDI, which has no plug-detection on the real jack either, "hotplug"
only means opening or closing the native `L6max MIDI In/Out Port`; the UART
socket is drained continuously either way so the guest's transmit side never
blocks, and bytes arriving while disconnected are simply dropped, like an
unplugged cable.

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
[Linux guide](../guides/linux.md). `midi-smoke` with `L6_MIDI_DIRECT=1`
exercises the same firmware path over USB without any host MIDI system.

The mixer-control round trip is verified with a separate host MIDI client (CoreMIDI on macOS, ALSA on Linux):
a channel-1 encoder produces CC `0x51` on cable 1, and host CC `0x51` with
value 100 changes the firmware-driven channel-1 GPIO LED ring.

## Physical MIDI IN/OUT (DIN/TRS) jacks

The main MCU's LPUART3 drives the physical MIDI IN/OUT (3.5 mm TRS) jacks.
Run with `--midi` (independent of `--usb`) to connect this host MIDI port,
`L6max MIDI In/Out Port (Emulator)`, as soon as the engine starts; it needs
no USB MIDI interface state and does not wait for enumeration — see
[Hotplug](#hotplug) above for toggling it, and the 3 USB ports, at runtime
instead. Zoom's own documentation says the jacks accept the same
mixer-control Control Change as the USB Mixer Control Port; see
[the faders feature in the Zoomie editor repository](https://github.com/derekstavis/zoom-l6-control)
for why this matters for wireless mixer control.

This has none of the USB MIDI framing: it is one plain MIDI byte stream in
each direction, with no cable number, carried over an inherited socket
(`L6_MAIN_MIDI_UART_FD`) exactly like `L6_USB_FD`, rather than through
QEMU's own chardev layer (the project's established pattern for ad hoc
host/guest links; see `engine.rs`). [`din_midi.rs`](../../emulator/host/src/din_midi.rs)
reuses the same `HostMidi` back ends as the USB cables (CoreMIDI/ALSA), now
parameterized by a port name list instead of the fixed three USB names, so
a single extra port costs no new platform code.

### Current behavior

- The stock firmware's boot sequence sets LPUART3's CTRL register TE, RE and
  RIE (bit19, bit18, bit21), initializing the port for both directions with
  receive interrupts, unconditionally — not gated behind any menu setting. A
  byte sent to the CoreMIDI port reaches the model's socket handler (`l6 main
  LPUART3 (MIDI jacks) RX` in the trace log).
- LPUART1-8 are IRQn 20-27 in the i.MX RT1052 CMSIS header (`MIMXRT1052.h`),
  sequential from LPUART1 = 20, so LPUART3 = 22; `unpacked/main_firmware.bin`'s
  own vector table holds a distinct, non-default handler there, the same
  shape as every other LPUART's entry. RX pulses that line through
  `qdev_get_gpio_in(.., 22)` when CTRL.RIE is set, once per byte that becomes
  the pending one (including refills from the queue, each its own RDRF edge
  on real hardware); TX pulses it symmetrically on CTRL.TIE.
- Only LPUART1 (the panel link) and LPUART3 (the MIDI jacks) are driven by
  firmware. LPUART2 and 4-8 have distinct, non-default vector table entries
  too (the SDK wires all eight uniformly), but firmware never writes to
  their CTRL registers, so the interrupt handling above does not apply to
  them; the existing "unconnected, idle" stub for them stands.
- Neither direction of this port reaches the mixer, in any state of
  `mixerControlViaMidi`. A channel-1 LEVEL CC (`0xB0 0x51 100`, the same
  message that moves the USB Mixer Control Port's GPIO ring) sent to this
  port produces no change, and the same holds for a Note On matching a
  sound pad's configured MIDI note.
  - The IN jack is an **unconditional MIDI-to-USB bridge**: a message sent to
    it arrives byte-for-byte on the host's `L6max MIDI I/O Port` (USB cable
    0, *not* the Mixer Control cable), regardless of `mixerControlViaMidi`.
    The setting only toggles an extra, functionally inert debug-mirroring
    step; it does not gate the bridge.
  - The OUT direction's root cause was a gap in this project's own emulator,
    not the firmware: firmware and its generic LPUART driver fill the
    LPUART3 ring buffer correctly and enable the transmit interrupt, which
    real hardware answers immediately (the transmit-empty flag is always set
    when idle), but this board model's `CTRL`-register write handler never
    pulsed the IRQ on that enable — only reactively, after a `DATA` write
    already happened. Fixed by pulsing the IRQ when `TIE` newly transitions
    high, mirroring logic this model already had for LPUART1 and for the RX
    side's `RIE`; a channel-1 mute press now produces `B0 5D 7F` on the DIN
    OUT jack, matching the USB Mixer Control Port echo exactly.
  - One narrow case on the IN side, a fixed 3-byte Note On sequence, is
    gated by the factory test jig's connect/disconnect lifecycle, not by
    anything reachable in normal use.
  - Zoom's own documentation says the jacks accept the same mixer-control
    Control Change as the USB Mixer Control Port; that does not match this
    emulator's current behavior in either direction, including with Mixer
    Control via MIDI turned on through the editor's own session (identity,
    global setting dump, then the `mixerControlViaMidi` parameter change) —
    see `din-editor-mixer-smoke`.

Only cable 1 does this. With `L6_MIDI_DIRECT=1 L6_MIDI_SEND_CABLE=<n>`,
`midi-smoke` sends the same CC on another cable: on cable 1 the ring goes from
`0x1` to `0x1ff`; on cables 0, 2 and an unused cable 5 it only reaches `0x3`,
so the firmware ignores mixer CC there. A host that exposes only the first
USB MIDI port (a CME WIDI Uhost does) cannot drive the mixer.

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

# Native panel with SD storage and host MIDI ports (CoreMIDI or ALSA).
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
