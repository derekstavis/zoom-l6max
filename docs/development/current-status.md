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
- L6 USB MIDI IN needs the `usb` scenario's 22 s wait on spec-built state.
  Enumerated when its controller starts (about 22 guest seconds), the L6 does
  not prime MIDI IN or send the encoder CC.
- L6 boot intermittently stalls: after 5–10 guest seconds MAIN sends a
  panel-link frame only every 1.2–1.3 guest seconds, and the encoder sweep does
  not complete within 120 s. Observed in 1 of 11 cold boots with the USB cable
  attached at power-on, and in 2 of 3 `power` restarts with the generated L6
  demo card (sound-pad samples) attached; 0 of 3 with an empty card.
- L6max `storage` fails with the prepared card `ceca7b3c…cd4e27`: after
  removal and reinsertion the top status bar differs from the home checkpoint,
  also when the case starts after 40 guest seconds.
- In the emulator, L6 sound-pad lamps light only when the USB cable is
  attached at power-on. With the generated demo card, pads 1–3 light at about
  32.5, 35.9 and 39.2 guest seconds and pad 4 does not; ejecting the card
  leaves them lit, and the guest clock stops about 7 s after reinsertion.
- On L6, `select-mode` waits while firmware restores the LEVEL and Hall lamps,
  several seconds after the rings.
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

## Current results

Headless, `--jobs 4`, QEMU `dd5cb794…77f0f4d4` built from this tree, stock
MAIN images L6 `827c7c5c…c03799` and L6max `980d9227…8d44461`, specs
`l6-settings-defaults.toml` `23b7f60e…8e630e3` and `l6max-setup-complete.toml`
`d3dedc75…6c36ee485`, prepared cards L6 `72748162…49ef67d3` (empty FAT32) and
L6max `ceca7b3c…cd4e27`:

| Selection | Result |
| --- | --- |
| `--suite quick` | PASS |
| `--case boot` | PASS; L6max ready at about 6 guest seconds |
| `--suite lifecycle` | PASS (L6 `power` 3 of 3 with the empty card) |
| `--suite devices` | PASS except L6max `storage` (above) |

USB MIDI IN: firmware flushes an IN transfer the host does not collect within
about 100 guest ms. `UsbHost` polls a configured MIDI interface's IN endpoint
continuously and queues what it receives. `usb-midi` requires the model's
channel-1 encoder CC. Firmware starts its USB device controller at about 22
guest seconds on L6, and on L6max at about 4 s, or 42 s with the cable attached
at power-on. `usb-descriptors` waits for it. L6max rejects SYNCHRONIZE CACHE as
an invalid opcode; the USB storage client treats that as nothing to flush.

The shared panel, audio, storage, power and USB cases run through the same
driver. L6max panel checks and both models' audio callback, SD and power cases
have passed for the tested inputs; L6max persistence and USB storage also have
passing results. MIDI-sync and direct USB checks have passing scenarios for
both models. Generated knob-dialog fixtures passed the knob-dialog and
popup-concurrency cases. These results do not establish untested UI, physical wiring or
audio behavior.

The headless `state-spec` case passed for L6max with stock 1.10 MAIN
`980d9227…8d44461`, spec `l6max-setup-complete.toml` `d3dedc75…6c36ee485` and an
external QEMU `1942dc36…02fb8d7e`. Built by `l6nor` without input, the state
booted to the recorder in 68–71 s of wall time. The same case took 162 s through
the `state-l6max-setup` recipe. Both runs produced a byte-identical MAIN NOR,
panel flash and option bytes. A control spec that left `setup_required` set
failed at the setup-flag check and opened Date/Time.

Scenario steps marked `setup_input` (startup prompt confirmations and the
L6 `ch4-mono` pair) are skipped on spec-built state. The following ran headless
with QEMU `8efab2ec…6855f3fbf`, built from this tree's sources, stock MAIN
images unless noted, and specs `l6max-setup-complete.toml` and
`l6-settings-defaults.toml`. The baseline is a saved L6max template with presses,
or for L6 no spec with presses:

| Case | Spec state, presses skipped | Baseline |
| --- | --- | --- |
| `state-spec` | PASS, 75 s | — |
| Quick suite (L6max) | PASS, 89 s | PASS, 99 s |
| `storage` (L6max) | PASS, 96 s | PASS, 108 s |
| `power` (L6max) | PASS, 150 s | PASS, 160 s |
| `l6max-din-mixer-control` | PASS twice, 87 s | PASS, 97 s |
| L6 `boot` (`midi-sync` patched, spec without a bank) | PASS, 64 s | — |
| L6 `l6-direct-midi-sync` (`midi-sync` patched, spec without a bank) | PASS, 229 s | PASS, 225 s |
| L6 `boot` (stock, final spec with `auto_power_off = 0`) | PASS | — |
| `usb` (both models) | PASS with the current driver (see Current results) | — |
| `l6max-direct-midi-sync` (stock) | FAIL, 3/94 MIDI-sync replies | Same failure |
| `l6max-direct-midi-sync` (`midi-sync` patched) | FAIL, encoder CC not received | Same failure |

`mixer-ready` and other settle checks ignore only the L6max TAP lamp, which
flashes at the effect tempo. The DIN mixer check takes its DIN baseline after
the USB-driven ring has settled; it previously sampled partial ring updates.
With a loaded L6 settings bank holding `auto_power_off = 1`, the default, the
emulated L6 waited for two `ch4-mono` presses before its boot LED sweep and
missed the readiness deadline. With 0 it booted without input, so the L6 spec
writes a defaults bank with `auto_power_off = 0`. Without any bank the firmware
did not wait; why the setting gates startup only with a loaded bank is
unresolved. The L6 settings catalog's default payload matched the firmware's
defaulted RAM image, and an `l6nor`-written L6 bank was loaded as written. The
L6 `usb` and patched `l6-direct-midi-sync` cases have not been rerun with the
final L6 spec.

Prepared L6max fixtures assume the documented SD Cancel/Confirm sequence.
Boot floors are 18 guest seconds for L6 and 40 for L6max on template and
fresh state; spec-built state skips them. The driver pins the
external QEMU executable, but its build-source provenance remains unattested.
