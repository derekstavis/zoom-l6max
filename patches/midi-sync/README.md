# MIDI sync patch

MIDI sync lets a controller request the mixer's current settings with Control
Change 127 and a value of 64 or higher on the configured MIDI channel. The
patch sends the values through the model's existing mapped Control Change
path, so the configured channel and CC map continue to apply.

This patch has separate, symmetric profiles for the supported 1.10 firmware:

- [L6max](l6max/README.md): eight strips and six global settings.
- [L6](l6/README.md): six strips and nine mapped groups.

The profile directories contain their own firmware bindings, hook metadata,
payload, linker script, and CPU simulator. The request predicate and volatile
message reader are shared in [`common/sync_request.rs`](common/sync_request.rs).
The builder selects the profile from the firmware image.

## Validation

Use the profile-specific validation notes for the corresponding firmware and
CPU simulator. Emulator scenarios are listed in the repository's
[testing guide](../../docs/development/testing.md); each result is scoped to its
firmware, fixture, and backend.
