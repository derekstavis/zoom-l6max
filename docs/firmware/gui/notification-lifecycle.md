# Notification synchronization and knob dialog lifecycle

[Documentation index](../../README.md)

## Current design

The knob-value dialog is a layer-4 scene, not a stock notification. It leaves
the notification manager, its counting semaphore at `0x8020a2a0`, its queue and
its timer unchanged, and never takes the semaphore.

A channel encoder turn applies the original mixer setter first, then publishes
the channel/mode tag through a relaxed atomic byte mailbox. The mixer change
never waits for the GUI. DisplayCycle2 callback slot 5 consumes the mailbox,
reads the current mixer value and draws the dialog. Stock slots 1, 3 and 4 stay
available to text scrolling, busy animation and notifications.

The first valid turn claims slot 5 and stores a small state record in
bitmap-table slot 0, which is a null sentinel in stock firmware. The record and
callback last until reset. Expiry and window navigation restore the borrowed
text resource 179, widgets 7 and 33 and their backgrounds; entry 36, the stock
notification frame, is never modified. Before each draw the callback rechecks
the borrowed descriptors and declines if any of them changed.

A stock notification can draw over the dialog. Its expiry must leave the
scene's callback, the resource pointer table and the resource text unchanged.
The `knob-dialog` emulator case checks this. It also holds the semaphore
unavailable during first use and refresh, and verifies that both updates still
render. See the [patch documentation](../../../patches/knob-dialog/README.md)
for the exact checks.

No stock firmware race is claimed. The scene borrows GUI descriptors, so it
needs the restore and recheck discipline above.

## Earlier design

The first version showed the value as a stock notification. It wrapped the
semaphore with a 36-byte heap sidecar for same-task nesting and tagged the
sidecar pointer into `0x8020a2a0`. Locked veneers guarded stock show, tick,
pop and busy operations, and a 1,788-byte heap allocation held the popup's
resources. The `popup-concurrency` emulator case exercised that sidecar,
recorder polling, expiry-boundary input, queue overflow and busy pause/resume.
The scene replaced those hooks, and the case was retired afterwards, since
none of the state it checks exists in the current patch.
