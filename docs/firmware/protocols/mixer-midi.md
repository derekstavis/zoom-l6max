# Mixer MIDI: Control Change

[Documentation index](../../README.md)

The L6max sends a Control Change for each mixer change made on the panel, and
applies Control Change received on the mixer-control port. This page describes
that behavior and the request added by the [midi-sync patch](../../../patches/midi-sync/README.md).
Addresses apply to the [supported main revision](../README.md#supported-revision).

Stock firmware applies received Control Change only on the USB Mixer Control
port, despite a documented setting (`mixerControlViaMidi`) that is supposed to
extend this to the physical MIDI IN/OUT jacks. The
[din-mixer-control patch](../../../patches/din-mixer-control/README.md) adds
that missing routing, applying the same Control Change handling described
here to a message received on the DIN jack.

## Controller numbers

Each controllable value has a parameter identifier (`00`-`5D`). The CC map
(`midiCCNumber` in the [editor protocol](editor-midi.md)) assigns a controller
number to each. With the default map, the eight physical channels of each
blue-button setting use eight consecutive controllers:

| Setting | Identifiers | Default controllers |
| --- | --- | --- |
| HIGH | `00`-`07` | `01`-`08` |
| FREQ | `08`-`0F` | `0B`-`12` |
| MID | `10`-`17` | `15`-`1C` |
| LOW | `18`-`1F` | `21`-`28` |
| SUB-MIX | `20`-`27` | `29`-`30` |
| AUX1 | `28`-`2F` | `31`-`38` |
| AUX2 | `30`-`37` | `39`-`40` |
| EFX | `38`-`3F` | `41`-`48` |
| PAN | `40`-`47` | `49`-`50` |
| LEVEL | `48`-`4F` | `51`-`58` |

Values are 0-127 control positions, the same values the knob popup reads. A
channel-1 LEVEL change therefore sends controller `51`. Identifiers `50`-`5D`
are button-type values:

| Setting | Identifier | Default controller |
| --- | --- | --- |
| MUTE, channels 1-8 | `50`-`57` | `5D`, `5E`, `5F`, `66`-`6A` |
| CH 5 MONO x2 | `58` | `6D` |
| CH 6 MONO x2 | `59` | `6E` |
| USB 1/2 | `5A` | `71` |
| USB 3/4 | `5B` | `72` |
| EFX TYPE | `5C` | `75` |
| COMPRESSOR | `5D` | `77` |

The stock firmware only sends changes; it does not report values set before a
controller connected.

## State request (midi-sync patch)

| Message | Effect |
| --- | --- |
| Control Change 127, value 64-127, on the configured MIDI channel | Device sends all 94 parameter identifiers — the 10 continuous settings and MUTE for every channel, then the 6 button-type globals (CH 5 MONO x2, CH 6 MONO x2, USB 1/2, USB 3/4, EFX TYPE, COMPRESSOR — identifiers `58`-`5D`) — as Control Change using the current CC map |

Controller 127 is outside the assignable range, so it cannot collide with a
mixer controller. Values below 64 are ignored, so a button release does nothing.
Replies use the same port, channel and CC map as live changes. Replies to a
request are sent in channel order then setting order within a channel
(MUTE last, identifier `50`-`57`), then the 6 globals in identifier order.
Full coverage: every one of the 94 stock parameter identifiers is included.

## Evidence and limits

Findings are checked with a CPU-level simulation of the patched handler; see the
[patch validation](../../../patches/midi-sync/README.md#validation). The default
controller numbers combine the default map with the identifier layout and match
the emulator's documented channel-1 controller `51`. Hardware behavior, button
identifiers and the receive gating are unverified.
