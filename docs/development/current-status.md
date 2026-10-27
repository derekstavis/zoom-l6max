# Current development status

The [`l6test` driver](testing.md) is the entry point for model and backend tests.
The scenario catalog lists available cases and prerequisites. A past pass does
not qualify a changed binary, fixture, or scenario; run the relevant cases and
read their results.

## Known limits

- The full L6 panel scenario is unqualified on both headless and GPUI backends:
  late ring feedback can fail an unrelated-ring assertion, and startup has
  sometimes failed the readiness deadline. Exact numerical reversal is also
  unqualified: the L6 headless quick case also observed channel 2 at `0x003`
  while waiting for `0x001` after reversal. Do not mask these failures with
  longer delays or exemptions.
- Fresh USB enumeration succeeds, but the USB MIDI IN sequence is unreliable
  for both models. The L6 host MIDI endpoint readiness remains unresolved.
- L6 DIN sweep and DIN-to-USB forwarding remain unqualified: the former did
  not produce the expected indicator change, and the latter did not reach its
  probe in the fresh-boot run. L6 event dispatch also times out waiting for
  the expected semaphore call.
- L6 GUI scene save fails at Scene A; startup-key parity is unresolved. GUI
  bundle and native menu cases require configured packaged-app fixtures and
  live qualification.
- Recorder playback cases for both models require prepared state and card
  fixtures and are unqualified. Recorder capture-to-file is unresolved. L6
  sound-pad playback and the L6 editor-assigned pad scenario need live
  qualification. L6max Pad 1 playback after editor assignment has passed in
  the emulator; physical audio fidelity is not established.
- The L6 MAIN GPIO event assertions, L6 USB startup probe, L6max sound-pad
  key-bit assertions, L6max direct USB cable-isolation steps, and L6max
  DIN/editor scenarios need live qualification.

The shared panel, audio, storage, power and USB cases run through the same
driver. L6max panel checks and both models' audio callback, SD and power cases
have passed for the tested inputs; L6max persistence and USB storage also have
passing results. MIDI-sync and direct USB checks have passing scenarios for
both models. Generated knob-dialog fixtures passed the knob-dialog and
popup-concurrency cases. These results do not establish untested UI, physical wiring or
audio behavior.

Prepared L6max fixtures assume the documented SD Cancel/Confirm sequence.
Boot floors are 18 guest seconds for L6 and 40 for L6max. The driver pins the
external QEMU executable, but its build-source provenance remains unattested.
