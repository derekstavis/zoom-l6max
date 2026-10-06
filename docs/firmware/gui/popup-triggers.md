# Knob popup dismissal behavior

[Documentation index](../../README.md)

Investigation pointed to an unconditional detach in the patch's busy-hide
wrapper: idle recorder polling dismissed the popup even though stock busy-hide
would return immediately. The corrected package preserves that no-op; its
popup behavior was confirmed on an L6max.

## Patch rule

The periodic recorder callback at `0x8004da20` can call busy-hide while the
busy flag is zero. This is routine status refresh, not a request to dismiss a
notification. The wrapper detaches the popup only when the busy flag equals 1,
checked under the notification manager lock. A zero flag leaves it alone.

The recorder service is gated by storage and input state. Card-detect GPIO2.28,
its interrupt, and USDHC1 participate in card handling, but the immediate
popup dismissal came from a periodic GUI callback. See [SD and USB](../../emulator/usb.md)
for modeled hardware behavior and [notification lifetime](notification-lifecycle.md)
for ownership and expiry.

## Reproduction and validation

`popup-trigger-probe` opens LEVEL through GPIO encoder input and invokes the
original recorder callback through GDB. It does not fabricate busy state. On
the earlier wrapper, busy flag 0 accompanied a transition from active popup to
inactive, with the original bitmap restored before expiry.

```sh
# Requires an earlier affected build; run from the repository root.
cargo run --offline -p popup-trigger-probe -- \
  --firmware-dir firmware-knob-dialog-live-refresh --volatile \
  --logs emulator/logs/recorder-poll-trigger
```

The current concurrency regression invokes the real recorder callback with
busy flag 0 and checks pixels, allocation, and expiry. The current patch reports
1.12 and preserves the corrected wrapper. Physical hardware confirmed the
corrected behavior; the precise hardware task interleaving was not captured.

## Limits

- The probe invokes a callback through the debugger, bypassing its natural
  registration gates; it does not measure physical callback frequency.
- A forced full-scene clear exposed a repaint gap but did not establish the
  hardware dismissal cause. The earlier repaint workaround is not used.
- Direct reference inventories cannot rule out computed or aliased accesses.
