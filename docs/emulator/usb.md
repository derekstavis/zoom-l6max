# SD storage and USB

[Documentation index](../README.md)

Investigation pointed to separate board card detection, the RT USDHC
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

The three ports do not appear the instant the engine starts: `MidiBridge::run`
(`usb_midi.rs`) only calls `P::open` once `usb.enumerate()` succeeds with a
MIDI endpoint, i.e. once firmware has actually completed USB enumeration —
before that its status is "Waiting for firmware USB ...". From a cold
`--volatile` boot this has taken as long as ~90 seconds in practice (not the
~3.1 seconds the guest-side register writes in [USB device
controller](#usb-device-controller) above complete by); callers that connect
immediately after starting the engine should poll rather than assume the
ports exist. `l6max-host --auto-usb-unlock` (below) is unrelated to this
delay — it doesn't make enumeration any faster, it just also isn't needed to
make it happen at all; basic USB MIDI editor connect works fine with nothing
but patience.

A long-running headless instance can mirror `l6-direct-midi-sync`'s scenario
steps by passing `l6max-host --auto-usb-unlock`, which presses the panel's
CH4 MONO button twice at the same guest-ms thresholds that scenario uses,
instead of needing them clicked by hand in the GUI window. Live testing found
this is **not** required for basic USB MIDI editor connect or for pad file /
settings round trips — those all work with plain `--usb` once enumeration
finishes. What `l6-direct-midi-sync` actually needs the presses for (its own
later steps use the DIN jacks and ring-pin sweeps, not just the editor port)
remains unconfirmed; treat `--auto-usb-unlock` as a working but
not-yet-understood tool, not a general connectivity prerequisite.

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
only means opening or closing the model's native MIDI IN/OUT port (`L6 MIDI
In/Out Port` or `L6max MIDI In/Out Port`); the UART
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
[Linux guide](../guides/linux.md). The `l6max-direct-midi-sync` driver case
exercises USB MIDI without a host MIDI system and checks the mixer-control
cable routing.

The mixer-control round trip is verified with a separate host MIDI client (CoreMIDI on macOS, ALSA on Linux):
a channel-1 encoder produces CC `0x51` on cable 1, and host CC `0x51` with
value 100 changes the firmware-driven channel-1 GPIO LED ring.

## Physical MIDI IN/OUT (DIN/TRS) jacks

L6max routes the physical MIDI IN/OUT (3.5 mm TRS) jacks through MAIN LPUART3;
L6 uses MAIN LPUART2. Run with `--midi` (independent of `--usb`) to connect
this host MIDI port,
the model's MIDI IN/OUT port (`L6 MIDI In/Out Port (Emulator)` or
`L6max MIDI In/Out Port (Emulator)`), as soon as the engine starts; it needs
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

- L6max 1.10's boot sequence sets LPUART3's CTRL register TE, RE and
  RIE (bit19, bit18, bit21), initializing the port for both directions with
  receive interrupts, unconditionally — not gated behind any menu setting.
  L6 1.10 initializes its DIN candidate LPUART2 later, at about 950 ms after
  the audio profile handshake; CTRL becomes `0x002c0000`, with RE and RIE set.
  The model's host RX trace includes CTRL and RIE, distinguishing socket
  arrival from an enabled guest receiver.
- The L6 firmware uses IRQn 20-27 for LPUART1-8, sequential from LPUART1 = 20
  (so LPUART3 = 22); QEMU uses those firmware-observed values. The L6 chip is
  confirmed as RT1041, while L6max silicon remains unverified.
  `unpacked/main_firmware.bin`'s
  own vector table holds a distinct, non-default handler there, the same
  shape as every other LPUART's entry. RX pulses that line through
  `qdev_get_gpio_in(.., 22)` when CTRL.RIE is set, once per byte that becomes
  the pending one (including refills from the queue, each its own RDRF edge
  on real hardware); TX pulses it symmetrically on CTRL.TIE.
- L6max firmware drives LPUART1 (the panel link) and LPUART3 (the MIDI
  jacks). L6 drives its panel link through LPUART7 (SDK instance 6, IRQ 26)
  and initializes LPUART2 for its DIN jack through the static DIN initializer
  at `0x8002f8a0`. Stimulated L6 QEMU runs reach the UART7 IRQ handler and
  read panel reports from its DATA register. A 128-CC L6 run confirms all
  bytes reach LPUART2 with RE/RIE set, but the mixer ring does not change and
  all 384 input bytes are read back from LPUART2's DATA register, the raw
  ring's backing bytes match, and its write/read indices both reach 384. The
  firmware receive task drains the sweep, but the channel-1 ring does not
  change; the downstream mixer effect remains under investigation. LPUART4-6
  and LPUART8 have distinct vector entries, but no
  relevant CTRL setup has been observed for them.
- In L6max stock firmware, neither direction of this port reaches the mixer,
  in any state of `mixerControlViaMidi`. A channel-1 LEVEL CC
  (`0xB0 0x51 100`, the
  same message that moves the USB Mixer Control Port's GPIO ring) sent to this
  port produces no change, and the same holds for a Note On matching a
  sound pad's configured MIDI note.
  - The IN jack is an **unconditional MIDI-to-USB bridge**: a message sent to
    it arrives byte-for-byte on the host's `L6max MIDI I/O Port` (USB cable
    0, *not* the Mixer Control cable), regardless of `mixerControlViaMidi`.
    The setting only toggles an extra, functionally inert debug-mirroring
    step; it does not gate the bridge.
  - A focused L6 1.10 probe sent a Control Change, Note On, realtime Clock,
    and short SysEx to DIN IN. Each arrived unchanged on USB MIDI I/O cable 0;
    other message forms and setting-dependent behavior have not been tested.
    Run it with:

    ```sh
    cargo run --offline --locked -p l6test -- run --case l6-din-usb-forwarding --model l6 --config smoke.local.toml
    ```
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
  - Zoom documents mixer-control Control Changes over the MIDI jacks. L6max
    stock firmware leaves DIN IN on its factory-jig path; the
    [`din-mixer-control` patch](../../patches/din-mixer-control/README.md)
    routes received messages through the USB Mixer Control handler. On L6,
    stock firmware maps DIN CCs into its Main event queue. The L6
    `l6-din-sweep` driver case checks the 384 UART/ring bytes and traces the
    expected 66 Main submissions and queue reads, setter activity, mixer RAM,
    and indicator changes. Current stock runs still fail at Main queue
    consumption/state change, so L6 DIN mixer behavior remains unqualified;
    do not apply the L6max patch to L6 without further evidence.

Only cable 1 does this. `l6max-direct-midi-sync` sends the same CC on cables 0,
2, and 5: cable 1 changes the ring from `0x1` to `0x1ff`; the other cables
only reach `0x3`. A host that exposes only the first USB MIDI port (a CME WIDI
Uhost does) cannot drive the mixer.

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
