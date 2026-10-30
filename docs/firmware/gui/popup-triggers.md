# Knob popup dismissal behavior

[Documentation index](../../README.md)

The L6max knob-value patch keeps its popup visible during idle recorder
refreshes. The periodic recorder callback can request a busy notification
update while the busy flag is zero; that update leaves the popup in place. An
active busy notification can dismiss the popup under the notification manager
lock. See [notification lifetime](notification-lifecycle.md) for ownership and
expiry.

The `popup-concurrency` test checks the recorder callback, popup pixels,
allocation, and expiry in the emulator. The behavior has also been observed
on an L6max with the patched firmware. Use the [testing workflow](../../development/testing.md)
to configure a matching patched fixture and run the case.
