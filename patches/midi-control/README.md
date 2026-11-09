# MIDI Control

Control the L6max recorder and blue-page buttons over MIDI, and report
recorder/page state back to the configured USB or DIN MIDI output.

The patch hooks the MIDI handler, four recorder start/stop callbacks, and the
main button event handler. Incoming transport messages post ordinary stock UI
events. The state request reports the 94 mixer parameters using the stock CC
map, followed by raw CC 115 for recorder state. Raw CC 116 reports blue-page
button presses.

## MIDI messages

All messages use the configured MIDI channel.

| CC | Action |
|---:|---|
| 107 | Tap REC |
| 108 | Tap PLAY/STOP |
| 111 | Tap previous |
| 112 | Tap the blue page selected by value 0–9 |
| 127 | Send all 94 mixer parameters, then recorder state |

For CC 107, 108, 111, and 127, values 64–127 trigger; values 0–63 are
consumed without another action. Unrecognized messages continue through the
stock MIDI handler.

| Outgoing CC | Value | When |
|---:|---|---|
| 115 | 0 stopped, 1 recording, 2 playing | After stock recorder start/stop callbacks |
| 116 | 0–9, matching CC 112 | When a blue page button is pressed |

## Compatibility and limits

MIDI Control can be combined with MIDI Sync. Their shared MIDI entry hook is
chained in filename order when loaded from SD, or patch catalog order when
built into firmware. It can also be combined with knob-dialog and
din-mixer-control. It requires L6max MAIN 1.10. The dynamic bundle builder
can emit it as a six-site module; the resident loader supports atomic
multi-site registration.

The current implementation has compile-time ABI checks for all six function
entries and verifies the displaced firmware bytes against the shared hook
catalog. Recorder state reporting uses the stock sender tail to bypass CC-map
translation while retaining stock channel and output routing.
