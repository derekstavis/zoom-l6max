# Power button and emulator lifecycle

[Documentation index](../README.md)

The UI and host expose one Power control, but L6 and L6max reach shutdown by
different paths.

| Surface | L6max | L6 |
| --- | --- | --- |
| Power key | Input ID 53; active-high MAIN GPIO2 bit 25 | Host input ID 53; no L6 firmware GPIO mapping is implemented |
| Firmware input path | Reader `0x80021d58`, inverted before the button scanner | Not yet established from L6 1.10 firmware analysis |
| Shutdown path | Firmware clears MAIN GPIO3 bit 3 after cleanup | QEMU board timer cuts virtual power after 500 ms |
| Power-off signal | GPIO3 DR_CLEAR at `0x401c0088`, mask `8` | Host `POWER_OFF` notification; no L6 GPIO3.3 edge |

On L6max, the firmware owns scanning, debounce, long-press recognition and
cleanup. The shutdown routine invokes the current window's power callback,
saves state, closes serial interfaces, disables peripheral rails, updates
indicators, waits for pending work, resets the panel, and finally drops power
hold. Firmware-update preparation writes `FW UPDT\0` through that window
callback before the final power cut.

The L6 1.10 firmware did not produce the L6max GPIO3.3 shutdown edge during
native testing, even though QEMU observed the Power input change. The L6 profile
therefore models its documented on/off switch as a board-level power cutoff
after a 500 ms press. This interval is an emulator approximation, not a measured
physical timing. Startup keys are held for 10 seconds into boot; the GUI queues
them as DOWN events while CPUs are paused, then resumes firmware.

The L6 MAIN image includes `EV_BTN_PWR_SW_%s` and
`EV_SYS_POWER_PWRSW_SHUTDOWN` event-name strings, but analysis found no direct
code reference to the system-event label. These strings do not establish which hardware input reaches the event
or whether the path is active in the stock firmware. The 500 ms cutoff remains
a synthetic fallback until that path is traced.

GPIO5 bit 1 is panel reset, rather than whole-device power-off. Serial close
requires idle transmit-complete status on unconnected LPUART ports; without
that modeled status, L6max preparation and shutdown can stall.

## Native UI

For L6max, hold Power until the firmware completes shutdown. The GPIO3.3 falling
edge publishes a runtime `POWER_OFF` notification and requests QEMU guest
shutdown. For L6, a 500 ms press triggers the profile's virtual power cutoff. In
either case the UI drops IPC resources, blanks the display/LEDs and keeps the
window open.

The L6 faceplate's rendered POWER lamp follows the emulator's virtual
powered/off state. This is host presentation; the lamp's physical electrical
drive has not been traced to an L6 firmware output.

Click Power again to start. The UI creates a new engine with the same options,
flash/RTC state and SD card. The replacement SD installer runs before QEMU when
an update is pending. Physical switch positions and analog knob positions are
restored. On L6, startup buttons can be selected while off; the host queues
their pressed levels before resuming either firmware CPU.

Closing the window quits the application. It does not substitute for the
firmware Power hold or create an update marker.

## Validation

```sh
# Run from the repository root with smoke.local.toml configured.
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case power --backend headless --output /tmp/power-headless
cargo run --offline --locked -p l6test -- run --config smoke.local.toml \
  --case gui-power --backend gui --output /tmp/power-gui
```

The shared `power` scenario detects the model from the booted firmware. For L6max it checks
that a short press is ignored and a long hold reaches the firmware GPIO power
cut. For L6 it checks that the board-level cutoff occurs after 500 guest ms,
that guest time stops while off, and that MAIN and panel firmware restart. The
`gui-power` scenario checks L6max's two Confirm presses, long Power hold and
restart. On L6 it clicks Power, verifies the board enters its off state, arms
Record, restarts, and checks for a Record panel report after reboot. These
checks use volatile or isolated fixture state and do not move the OS pointer.

## Limits

- The L6 500 ms cutoff is an emulator approximation pending recovery of its
  firmware power-off callback.
- Startup-key delivery is tested with Record; other L6 startup chords still
  need model-specific verification.
- Analog supply voltages, capacitor discharge and PMIC sequencing are not modeled.
- `--volatile` discards flash and RTC state when the engine stops, including a
  Power cycle within the UI.
