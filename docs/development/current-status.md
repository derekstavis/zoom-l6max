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

Headless, `--jobs 1`, QEMU `fcba0693…0c0981da` built from this tree, stock
MAIN images L6 `827c7c5c…c03799` and L6max `980d9227…8d44461`, specs
`l6-settings-defaults.toml` `23b7f60e…8e630e3` and `l6max-setup-complete.toml`
`d3dedc75…6c36ee485`, and an empty 128 MiB FAT32 prepared card for both models
(`72748162…49ef67d3`). Wall times from two consecutive passes:

| Case | L6 | L6max |
| --- | --- | --- |
| `boot` | 11 s / 11 s | 10 s / 9 s |
| `controls` | 25 s / 24 s | 18 s / 18 s |
| `audio-path` | 8 s / 8 s | 1 s / 1 s |
| `storage` | 31 s / 31 s | 22 s / 21 s |
| `storage-empty` | 12 s / 11 s | 10 s / 9 s |
| `usb` | 16 s / 16 s | 30 s / 31 s |
| `usb-storage` | — | 30 s / 29 s |
| `power` | 24 s / 24 s | 28 s / 28 s |
| `persistence` | — | 101 s / 102 s |
| `state-spec` | — | 14 s / 14 s |

Every case passed in both passes. Suite wall times: `quick` 77/75 s, `devices`
249/244 s, `lifecycle` 213/213 s.

USB MIDI IN: firmware flushes an IN transfer the host does not collect within
about 100 guest ms. `UsbHost` polls a configured MIDI interface's IN endpoint
continuously and queues what it receives. `usb-midi` requires the model's
channel-1 encoder CC. Firmware starts its USB device controller at about 4
guest seconds on L6; `usb-descriptors` waits for it. L6max rejects SYNCHRONIZE
CACHE as an invalid opcode; the USB storage client treats that as nothing to
flush.

These results do not establish untested UI, physical wiring or audio behavior.
Generated knob-dialog fixtures passed the knob-dialog and popup-concurrency
cases. A `state-spec` state and the `state-l6max-setup` recipe produce a
byte-identical MAIN NOR, panel flash and option bytes; a spec that leaves
`setup_required` set opens Date/Time and fails the setup-flag check.

Scenario steps marked `setup_input` (startup prompt confirmations and the
L6 `ch4-mono` pair) are skipped on spec-built state. These cases were last run
headless with the earlier QEMU `8efab2ec…6855f3fbf`, which used half the current
instruction budget:

| Case | Spec state, presses skipped | Saved template or no spec, with presses |
| --- | --- | --- |
| `l6max-din-mixer-control` | PASS twice | PASS |
| L6 `boot` (`midi-sync` patched, spec without a bank) | PASS | — |
| L6 `l6-direct-midi-sync` (`midi-sync` patched, spec without a bank) | PASS | PASS |
| `l6max-direct-midi-sync` (stock) | FAIL, 3/94 MIDI-sync replies | Same failure |
| `l6max-direct-midi-sync` (`midi-sync` patched) | FAIL, encoder CC not received | Same failure |

Settle checks ignore only the L6max TAP lamp, which
flashes at the effect tempo. The DIN mixer check takes its DIN baseline after
the USB-driven ring has settled.
With a loaded L6 settings bank holding `auto_power_off = 1`, the default, the
emulated L6 waited for two `ch4-mono` presses before its boot LED sweep and
missed the readiness deadline. With 0 it booted without input, so the L6 spec
writes a defaults bank with `auto_power_off = 0`. Without any bank the firmware
did not wait; why the setting gates startup only with a loaded bank is
unresolved. The L6 settings catalog's default payload matched the firmware's
defaulted RAM image, and an `l6nor`-written L6 bank was loaded as written. The
patched L6 `l6-direct-midi-sync` case has not been rerun with the final L6
spec.

Prepared L6max fixtures assume the documented SD Cancel/Confirm sequence.
The L6max boot floor is 12 guest seconds on template and fresh state;
spec-built state skips it, and L6 has none. The driver pins the
external QEMU executable, but its build-source provenance remains unattested.
