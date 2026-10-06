# Power button and emulator lifecycle

[Documentation index](../README.md)

Investigation pointed to a board power-hold GPIO in the observed shutdown
sequence.

| Surface | Mapping |
| --- | --- |
| Power key | Main GPIO2 bit 25, active high, input ID 53 |
| Firmware reader | `0x80021d58`, inverted before the button scanner |
| Shutdown handler | System power event routine at `0x80037328` |
| Power hold | Main GPIO3 bit 3, high while powered |
| Power-hold setter | Output table index 1 → `0x80021ea8` |
| Final power cut | GPIO3 DR_CLEAR at `0x401c0088`, mask `8` |

The firmware owns scanning, debounce, long-press recognition and cleanup.
The shutdown routine invokes the current window's power callback, saves state,
closes serial interfaces, disables peripheral rails, updates indicators,
waits for pending work, resets the panel, and finally drops power hold.
Firmware-update preparation writes `FW UPDT\0` through that window callback
before the final power cut.

GPIO5 bit 1 is panel reset, rather than whole-device power-off. Serial close
requires idle transmit-complete status on unconnected LPUART ports; without
that modeled status, preparation and shutdown can stall.

## Native UI

Hold Power for about 3.5 seconds and release. A short click does not force
shutdown. The GPIO3.3 falling edge publishes a runtime `POWER_OFF` notification
and requests QEMU guest shutdown. The engine uses `-no-shutdown` so QEMU freezes
both CPUs and virtual timers while the host consumes the notification. The UI
then drops IPC resources and reaps QEMU. The display and LEDs are blank while
the window remains open.

Click Power again to start. The UI creates a new engine with the same options,
flash/RTC state and SD card. The replacement SD installer runs before QEMU
when an update is pending. Physical switch positions and analog knob positions
are restored. The Power-on click is handled by the host while both chips are
off; it is not delivered as a held GPIO input during firmware startup.

Closing the window quits the application. It does not substitute for the
firmware Power hold or create an update marker.

## Validation

```sh
# Run from the repository root.
cargo run -p power-smoke -- --volatile --logs emulator/logs/power-smoke
L6_UI_POWER_SMOKE=1 cargo run -p l6max-gui -- --volatile --no-sd --logs emulator/logs/ui-power-smoke
```

The headless check owns temporary persistent state: a short press is ignored,
a long press reaches the GPIO power cut, QMP reports `shutdown`, guest time
stops, and a new engine retains completed Date/Time setup. The native check
dispatches GPUI events without moving the OS pointer, holds the physical Power
hit zone, verifies cleared guest/display/input ownership, clicks Power, and
waits for a new live firmware display.

## Limits

- Analog supply voltages, capacitor discharge and PMIC sequencing are not modeled.
- `--volatile` discards flash and RTC state when the engine stops, including a
  Power cycle within the UI.
- Power-on button holds and multi-button recovery chords at boot are not modeled.
