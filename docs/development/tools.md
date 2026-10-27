# Tool index

This page is an index. Each command page documents its full command syntax and
options. Run commands from the repository root unless that page says otherwise.

## Emulator and device tools

| Tool | Purpose | Reference |
| --- | --- | --- |
| `l6test` | Qualified scenarios, suites, GUI probes, and L6max screen GIF capture | [l6test](tools/l6test.md) |
| `l6dbg` | TOML observations and focused input, MIDI, UART-startup, and popup probes | [l6dbg](tools/l6dbg.md) |
| `l6ctl` | Control a running emulator through its local Unix socket | [l6ctl](tools/l6ctl.md) |
| `l6fs` | Create SD images and access SD files through USB storage | [l6fs](tools/l6fs.md) |
| `l6fw` | Inspect, unpack and rebuild firmware packages | [l6fw](tools/l6fw.md) |
| `qemu-run` | Low-level scripted QEMU input, memory probes and timed runs | [qemu-run](tools/qemu-run.md) |
| `cpu-profile` | Measure CPU use, guest clock, sampled PCs and exceptions | [cpu-profile](tools/cpu-profile.md) |

For the smoke driver’s TOML files, see [fixture and catalog configuration](testing.md#fixture-and-catalog-toml)
and the complete [scenario and state-recipe schema](scenario-format.md).
The diagnostic recipe schema is documented in the [l6dbg reference](tools/l6dbg.md#toml-recipe-schema).

## Build, patch and packaging tools

| Tool | Purpose | Reference |
| --- | --- | --- |
| `l6patch` | Apply supported, revision-guarded firmware patches | [l6patch](tools/l6patch.md) |
| `qemu-build` | Build the repository’s pinned QEMU fork | [qemu-build](tools/qemu-build.md) |
| `macos-package` | Package a macOS `.app` without firmware | [macos-package](tools/macos-package.md) |
| `linux-package` | Package a Linux AppDir/AppImage without firmware | [linux-package](tools/linux-package.md) |
| `firmware-patch-wasm-prebuild` | Maintainer build helper for browser patch templates | [WASM template prebuild](tools/firmware-patch-wasm-prebuild.md) |

`smoke-core`, `sd-image`, `l6max-diagnostics`, patch compiler crates, and media
libraries are workspace implementation crates, not standalone user commands.
Their owning command pages link to the subsystem guides and qualification cases
when more context is needed.
