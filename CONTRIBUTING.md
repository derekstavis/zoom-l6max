# Contributing

Contributions to the emulator, editor, firmware tools, and documentation are
welcome. Start with [getting started](docs/getting-started.md) and the
[architecture guide](docs/emulator/architecture.md). The
[documentation index](docs/README.md) links the detailed reference maps.

## Choose the right part of the project

| Change | Location |
| --- | --- |
| Device registers, interrupt behavior, UART, GPIO, DMA | `emulator/qemu/` |
| Engine lifecycle, IPC, display publication, persistence, USB services | `emulator/host/` |
| Native components, layout, gestures, visual indicators | `emulator/gui/` |
| MIDI editor and protocol library | `editor/` |
| Firmware payloads, bindings, linker placement, hook guards | `patches/` |
| Standalone CLI or diagnostic | `tools/<tool-name>/` |
| Guides, architecture, behavioral specifications | `docs/` |

The host library is independent of GPUI. Keep UI concerns in the GUI crate and
shared device behavior in the host or model. Input timing belongs to the guest
clock; indicator state should come from the modeled hardware outputs.

## Development workflow

1. Describe the behavior being changed and its current evidence. For a model
   change, identify the relevant registers, interrupt, or firmware path.
2. Make a focused change and update its guide/reference when behavior changes.
3. Run formatting and relevant checks from the repository root:

   ```sh
   cargo fmt --all --check
   cargo test -p l6max-host --lib
   ```

   The [testing guide](docs/development/testing.md) maps diagnostics to changes.
   Rebuild custom QEMU before testing model changes. GUI dispatch and framebuffer
   behavior require the corresponding integration check.
4. In the pull request, describe what changed, why, what was tested, and any
   remaining limits. Include firmware hashes when a finding depends on a revision.

For reports, include the host OS, command, relevant version/hash, expected and
actual behavior, and a minimal reproduction. Share interpreted logs or screenshots
when useful; exclude raw firmware, RAM dumps, and derived binary exports.

## Firmware and contribution conventions

- Supply firmware locally. Do not commit manufacturer firmware, extracted
  components, generated payloads/packages, RAM dumps, or vendor artwork.
- Put runtime output in `emulator/logs/` and persistent state in
  `emulator/state/`. Keep generated investigation artifacts local and ignored.
- Use new ignored `firmware-<patch-name>-<revision>/` output directories for
  patch builds; preserve input files and record source/output hashes.
- Follow the [patch conventions](patches/README.md): guard hooks, placement,
  supported revision, resource ownership, and validation before adding a patch.
- Keep public documentation and patch bindings consistent. Use reviewed
  upstream FreeRTOS names for established matches.
- Distinguish static evidence, emulator behavior, hardware observations, and
  hypotheses. A tested payload does not establish compatibility for every release.

Contributions must be independently written. Document observable hardware and
protocol behavior, compatibility addresses, and ABI definitions needed by the
emulator, patches, and probes. Include evidence and tests for modeled behavior;
do not submit manufacturer source code or extracted firmware resources.
Builds and tests must work using this repository and its public dependencies.

## Documentation conventions

Keep the README focused on purpose and getting started. Put practical guides in
`docs/guides/`, emulator behavior in `docs/emulator/`, firmware behavior in
`docs/firmware/`, and reproduction workflows in `docs/development/`.

Lead detailed pages with the finding and its scope. Preserve addresses, hashes,
method, evidence, and unresolved context. Prefer “investigation pointed to”
over chronological progress reports. List unsupported or unverified behavior
as bullets. Use relative links and commands without personal checkout paths.
Add new pages to the [documentation index](docs/README.md).

## Licensing

Original project contributions use [MIT](LICENSE). QEMU model changes retain
GPL-2.0-or-later, and bundled public assets retain their upstream licenses.
Preserve notices and consult [NOTICE.md](NOTICE.md) when adding dependencies
or assets.

For binary previews and release requirements, see the
[macOS packaging guide](docs/guides/macos-app.md).
