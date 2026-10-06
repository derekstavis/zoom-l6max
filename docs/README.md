# Documentation

Start with [getting started](getting-started.md) to run the emulator, or
[firmware packages](guides/firmware-packages.md) to inspect a local update file.
Commands use the repository root unless a page says otherwise.

## Guides

- [macOS app](guides/macos-app.md): firmware selection, packaged builds, and local storage.

- [Getting started](getting-started.md): prerequisites, first boot, controls,
  device state, and troubleshooting.
- [Firmware packages](guides/firmware-packages.md): supported input, extraction,
  lossless repacking, and checksum regeneration.
- [Knob value patch](guides/knob-patch.md): build, run, and validate version 1.12.
- [MIDI editor](editor.md): SwiftUI app, demo mode, and hardware/emulator transport.

## Emulator reference

| Page | Topics |
| --- | --- |
| [Architecture](emulator/architecture.md) | Crate responsibilities, process ownership, build and launch |
| [Native frontend](emulator/frontend.md) | Shared display frames, components, input timing, LEDs, CPU use |
| [Headless host](emulator/host.md) | Engine options, lifecycle, services |
| [QEMU board](emulator/board.md) | Dual-chip model, peripheral coverage, diagnostics |
| [Controls and indicators](emulator/controls.md) | GPIO matrix, encoders, ADC knobs, LED wiring |
| [Persistent storage](emulator/storage.md) | NOR regions, panel flash, version records, RTC |
| [SD and USB](emulator/usb.md) | Card detection, CoreMIDI, USB file transfers, limits |
| [Power lifecycle](emulator/power.md) | GPIO power input, shutdown, restart, validation |

## Firmware behavior and patches

The [firmware overview](firmware/README.md) introduces chip responsibilities,
the supported revision, and evidence boundaries.

| Area | Reference |
| --- | --- |
| Updates | [Checks, storage handoff, panel programming, installer model](firmware/updates.md) |
| Notifications | [Ownership and lifetime](firmware/gui/notification-lifecycle.md), [dismissal behavior](firmware/gui/popup-triggers.md) |
| Values | [EQ units, percentages, pan/balance](firmware/gui/parameter-values.md) |
| MIDI | [Editor protocol](firmware/protocols/editor-midi.md), [factory protocol](firmware/protocols/factory-midi.md) |

## Development

- [Contributing](../CONTRIBUTING.md): repository conventions and change workflow.
- [CI and releases](development/ci.md): automated checks, app artifacts, and tag publishing.
- [Testing](development/testing.md): unit checks, GPIO/UI integration, patches,
  storage, and protocol diagnostics.
- [Tool catalog](development/tools.md): standalone Rust binaries.
- [Patch conventions](../patches/README.md): layout, hook guards, and adding patches.
- [Knob patch implementation](../patches/knob-dialog/README.md): payload design,
  synchronization, placement, and detailed validation.

## Reading the findings

- **Static evidence** describes bytes, decoded instructions, or inferred call
  shapes; assigned names are not recovered manufacturer symbols.
- **Emulator evidence** describes behavior exercised in the modeled peripherals.
  Debugger stops affect timing, and replacement models have explicit limits.
- **Hardware evidence** describes a particular tested payload or observation.
  It does not establish every scheduling interleaving or update policy.
- **Unknowns and unsupported behavior** are listed as bullets in the relevant page.
