# Documentation

This repository is maintained for emulator developers and coding agents. Start
with the repository workflow, then use the testing or diagnostics guide for the
task at hand.

## Repository workflow

1. Read the repository [agent instructions](../AGENTS.md).
2. Check [current development status](development/current-status.md) for known
   limits and qualification state.
3. Use the [testing workflow](development/testing.md) for repeatable test runs,
   or [diagnostics](development/diagnostics.md) to collect focused runtime
   observations.
4. Read the [scenario format](development/scenario-format.md) before extending
   the consolidated test driver.

## Development references

- [CI and releases](development/ci.md): automated checks, app artifacts, and tag publishing.
- [Tool index](development/tools.md): command references for emulator, firmware, and packaging tools.
- [Contributing](../CONTRIBUTING.md): conventions, review, and evidence standards.
- [Patch conventions](../patches/README.md): layout, hook guards, and adding patches.
- [MIDI sync implementation](../patches/midi-sync/README.md): hook, getters, combining, validation.
- [Knob patch implementation](../patches/knob-dialog/README.md): payload design, synchronization, placement, and validation.

## Emulator and firmware references

| Page | Topics |
| --- | --- |
| [Architecture](emulator/architecture.md) | Crate responsibilities, process ownership, build and launch |
| [Native frontend](emulator/frontend.md) | Shared display frames, components, input timing, LEDs, CPU use |
| [Headless host](emulator/host.md) | Engine options, lifecycle, services |
| [QEMU board](emulator/board.md) | Dual-chip model, peripheral coverage, diagnostics |
| [Audio](emulator/audio.md) | SAI output, output fidelity, live two-process timing |
| [Controls and indicators](emulator/controls.md) | GPIO matrix, encoders, ADC knobs, LED wiring |
| [Persistent storage](emulator/storage.md) | NOR regions, panel flash, version records, RTC |
| [SD and USB](emulator/usb.md) | Card detection, CoreMIDI and ALSA MIDI, USB file transfers, limits |
| [Power lifecycle](emulator/power.md) | GPIO power input, shutdown, restart, validation |
| [Peripheral audit](emulator/manual-audit.md) | Register behavior and coverage against the reference manual |
| [Panel audit](emulator/panel-manual-audit.md) | STM32G031 panel memory, GPIO, and timer coverage against RM0444 |

The [firmware overview](firmware/README.md) summarizes supported revisions,
model differences, patch contracts, updates, and MIDI protocols. Focused pages
cover [updates](firmware/updates.md), [MIDI protocols](firmware/protocols/editor-midi.md),
[GUI notifications](firmware/gui/notification-lifecycle.md), and
[parameter values](firmware/gui/parameter-values.md).

## User and operator guides

These pages cover common setup and usage workflows for the emulator and its
companion tools:

- [Getting started](getting-started.md): prerequisites, first boot, controls, device state, and troubleshooting.
- [macOS app](guides/macos-app.md): firmware selection, packaged builds, and local storage.
- [Linux](guides/linux.md): packages, build, ALSA MIDI ports, and verified scope.
- [L6 and L6max compatibility](guides/model-compatibility.md): model detection, simulator coverage, and current patch/update limits.
- [Firmware packages](guides/firmware-packages.md): supported input, extraction, lossless repacking, and checksum regeneration.
- [Knob value patch](guides/knob-patch.md): build, run, and validate version 1.12.
- [MIDI state sync patch](guides/midi-sync-patch.md): request model-specific mixer state over MIDI.
- [MIDI editor](editor.md): SwiftUI app, demo mode, and hardware/emulator transport.

## Reading behavior claims

- **Compatibility findings** describe behavior scoped to the identified model
  and firmware revision. Assigned function names are project names.
- **Emulator evidence** describes behavior exercised in the modeled peripherals.
  Debugger stops affect timing, and replacement models have explicit limits.
- **Hardware evidence** describes a particular tested payload or observation.
  It does not establish every scheduling interleaving or update policy.
- **Unknowns and unsupported behavior** are listed on the relevant reference page.
