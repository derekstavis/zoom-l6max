# Firmware patches

Each patch lives in its own directory named for the feature. Commit source and
reproduction instructions here; firmware packages and generated payloads stay
in ignored output directories.

## Catalog

| Patch | Supported models | Purpose | Builder |
| --- | --- | --- | --- |
| [knob-dialog](knob-dialog/README.md) | L6max 1.10 | Show channel encoder values in a custom scene using the stock GUI renderer | `l6patch` |
| [midi-sync](midi-sync/README.md) | L6max 1.10; L6 1.10 | Send the channel mixer state as Control Change on request | `l6patch` |
| [din-mixer-control](din-mixer-control/README.md) | L6max 1.10 | Let the physical MIDI IN/OUT jacks control the mixer when Mixer Control via MIDI is on, as documented but never implemented | `l6patch` |

The emulator can run any of these on the stock 1.10 firmware, from **File →
Firmware Patches…**, `patches` in `config.toml` or `--patch`. It applies the same
precompiled templates as the browser patcher, so it needs no cross-compiler. See
[getting started](../docs/getting-started.md).

## Directory layout

```text
patches/
  README.md
  api/
    patchrt.rs         # Shared no_std patch runtime and macros
    l6/                # L6 1.10 declarations, grouped by subsystem
    l6max/             # L6max 1.10 declarations, grouped by subsystem
  <patch-name>/
    README.md            # Overview when the patch has multiple profiles
    common/              # Optional code shared across model profiles
    l6/                  # Optional L6 profile
    l6max/               # Matching profile when L6 is also supported
      README.md
      src/               # Payload, firmware bindings and helpers
      linker/            # Linker script when injecting compiled code
```

`midi-sync/l6/` and `midi-sync/l6max/` contain the separate firmware profiles,
including their payload and linker scripts. Shared firmware bindings and
hookable-site catalogs live under `api/<model>/` and are selected for the
matching MAIN image.

Create only the files the patch needs. A change to existing instructions may
not need a compiled payload or linker script. Keep patch-specific bindings
with that patch and use the builder-generated continuation when hook metadata
declares one.
Payloads include [`api/patchrt.rs`](api/patchrt.rs) for shared patch runtime
support. Model-specific typed firmware declarations and hook catalogs are
grouped under [`api/`](api/README.md).
The [Rust patch payload API guide](../docs/development/patch-api.md) explains
the function attributes, shared hook catalog, and generated hook metadata.
The builder generates displaced-code continuations for entry hooks, a
register-preserving veneer for each static offset hook, and call dispatchers
for call-site hooks. Keep remaining hand-written assembly for loader dispatch
and other architecture-specific logic scoped to the matching model and
firmware revision.

## Heap headroom

Both 1.10 MAIN images run a FreeRTOS heap_4 allocator: 511,984 usable bytes on
L6 and 511,992 on L6max. Emulator observations, from a setup-complete state:

| Model | Firmware and activity | Free | Lowest free | Allocations | Frees |
| --- | --- | --- | --- | --- | --- |
| L6 | Stock: idle, recording a take, playing it back | 163,104 | 163,104 | 203 → 205 | 1 → 3 |
| L6 | midi-sync: idle, recording, playback | 163,104 | 163,104 | 203 → 205 | 1 → 3 |
| L6 | midi-sync with encoder turns, recording and playback (`l6-patch-heap`) | 163,104 | 163,104 | 205 | 3 |
| L6max | Stock: idle, recording a take, playing it back | 160,008 | 160,008 | 197 | 0 |
| L6max | Stock after switching to the USB card reader | 157,568 | 157,568 | — | — |
| L6max | All three patches: idle, recording, playback | 159,960 | 159,960 | 198 | 0 |
| L6max | All three patches with knob popups, recording and playback (`l6max-patch-heap`) | 159,960 | 158,160 | 199 | 1 |

L6 allocates and frees one block each when recording starts and when playback
starts; L6max allocates only at start-up in these runs. A patch that allocates
from this heap shares it with the firmware, so measure its lowest point while
the patch is busy:

- `l6dbg`'s `heap` step reports the counters at any point in a recipe.
  [`heap-load-l6.toml`](../tools/l6dbg/recipes/heap-load-l6.toml) and
  [`heap-load-l6max.toml`](../tools/l6dbg/recipes/heap-load-l6max.toml) take
  them at idle, while recording and during playback; add `--patch NAME` to
  measure a patch.
- `l6test`'s `expect-heap` step fails a scenario whose lowest point falls below
  `minimum_free`. The `l6-patch-heap` and `l6max-patch-heap` cases run every
  patch for their model together and require 128 KiB to stay free; extend
  them, or add a case, for a patch that allocates.

Both read the counters only after checking that the model's `pvPortMalloc` and
`vPortFree` match their 1.10 code in guest memory (L6 `0x8006de78` and
`0x80073f48`, L6max `0x80085478` and `0x8008e588`), so they work on any patched
image that leaves the allocator alone. Sound-pad playback and card-reader
transfers have not been measured.

## Adding a patch

1. Create `patches/<patch-name>/` and describe its purpose, supported input
   firmware hash, changed code/data, build command, and validation commands.
2. Put payload code in `src/` and placement rules in `linker/`. Document space
   limits and any RAM allocation or persistence behavior. Check heap use as
   described in [heap headroom](#heap-headroom).
3. Add a Rust builder crate under `tools/<patch-builder>/` (or explicitly
   extend an existing builder). Validate the input hash and displaced-region hashes,
   reject overlapping or out-of-slot writes, and require a new output directory.
4. Write generated payloads, patched images, packages, and `patch.json` into
   that output directory. Record input/output hashes, version markers, placement,
   and hook addresses so the artifact can be traced to its source.
5. Document emulator checks and hardware findings separately. List known
   limitations as bullets. Add the patch to the catalog above.

Run host commands from the repository root to use its pinned Rust toolchain.
Use a root output directory named `firmware-<patch-name>-<revision>/`, already
covered by `.gitignore`; never place extracted proprietary firmware in `patches/`.
Read any displaced instruction bytes from that local input during compilation;
commit the veneer logic and validation metadata instead of copied firmware bytes.

The builder supports `knob-dialog` and `midi-sync` on L6max, and only
`midi-sync` on L6. If `--patch` is omitted, firmware detection selects
`knob-dialog` on L6max and `midi-sync` on L6. Explicit `--patch` values remain
model-checked, and the builder rejects an unsupported selection. `--patch` may
repeat for compatible profiles. L6max payloads start at `0x80108000`; the L6
payload starts aligned after the exact supported MAIN image and consumes erased
`padding_before_secondary` bytes. Each linker script takes its base from
`PAYLOAD_BASE`. The builder checks hook regions against the input and refuses
overlapping hooks. Overlap checks do not prove independence: before combining
another patch, verify shared resources (RAM, heap, locks, callbacks) and input
hash expectations, and document the combination in each patch's README.
