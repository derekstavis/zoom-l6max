# `qemu-build`

Build the pinned QEMU source configured by this repository and print the path
to `qemu-system-arm`:

```sh
cargo run --offline --locked -p qemu-build --
cargo run --offline --locked -p qemu-build -- --minimal
cargo run --offline --locked -p qemu-build -- --plugins
cargo run --offline --locked -p qemu-build -- --optimized
cargo run --offline --locked -p qemu-build -- /path/to/qemu-source
```

The optional source path overrides the repository’s configured source tree.
`--minimal` selects the minimal build configuration; it keeps only the host
audio backend the native UI uses (CoreAudio on macOS, PulseAudio on Linux).
Builds disable TCG plugin
support by default, because its hooks cost CPU even when no plugin is loaded;
`--plugins` keeps it for [QEMU plugins](cpu-profile.md#qemu-plugins). Each
build reconfigures the staged tree, and test and host tools that rebuild a stale
QEMU use the default, so rebuild with `--plugins` after board-source changes
while plugins are needed. The command prints the
resulting executable path for use with `--qemu` or the local test-driver
configuration. See [QEMU build setup](../../../emulator/qemu/README.md).

## Optimized builds

`--optimized` builds with link-time optimization and the checked-in
profile-guided optimization (PGO) data for the host: `macos-arm64` or
`linux-x86_64` in `emulator/qemu/pgo/`. CI packages optimized builds. They
require Clang 21.1.8, which trained the profiles, and its LLD linker:

| Host | Compiler and linker |
| --- | --- |
| macOS | `brew install llvm@21 lld@21`; used from `/opt/homebrew/opt` by default |
| Linux | `clang` and `lld` 21.1.8, or set `CC`, `CXX` and `LLD`, for example `clang-21`, `clang++-21` and `/usr/bin/ld.lld-21` |

The build stops when the compiler reports another version, because a profile
from a different Clang no longer matches most functions. A host without a
profile builds with LTO alone. Standard builds keep the platform compiler and
skip LTO, so board-source rebuilds stay quick.

### Refreshing a profile

Refresh the host's profile after QEMU upgrades or larger board-model changes:

```sh
cargo run --locked -p qemu-build -- --train-pgo --config config.toml
```

The config names local stock firmware, as for `l6test`. The command builds an
instrumented minimal QEMU in `build-source/build-pgo-instrumented`, runs the
`l6max-audio-input`, `l6max-eq-audio-update` and `l6-audio-input` cases and a
60-second idle session for each model, merges the counts with `llvm-profdata`
(Homebrew `llvm@21` on macOS; set `LLVM_PROFDATA` to choose another), and writes
`emulator/qemu/pgo/<platform>.profdata.gz` and `<platform>.toml`. The profile
holds execution counts for QEMU functions only; commit both files. Linux
instrumented builds also need Clang's profiling runtime (`libclang-rt-21-dev`
on Ubuntu).
