# Firmware patches

Each patch lives in its own directory named for the feature. Commit source and
reproduction instructions here; firmware packages and generated payloads stay
in ignored output directories.

## Catalog

| Patch | Supported models | Purpose | Builder |
| --- | --- | --- | --- |
| [knob-dialog](knob-dialog/README.md) | L6max 1.10 | Show channel encoder values using the stock notification renderer | `l6patch` |
| [midi-sync](midi-sync/README.md) | L6max 1.10; L6 1.10 | Send the channel mixer state as Control Change on request | `l6patch` |
| [din-mixer-control](din-mixer-control/README.md) | L6max 1.10 | Let the physical MIDI IN/OUT jacks control the mixer when Mixer Control via MIDI is on, as documented but never implemented | `l6patch` |

## Directory layout

```text
patches/
  README.md
  <patch-name>/
    README.md
    src/                 # Payload, firmware bindings, assembly, helpers
    linker/              # Linker script when injecting compiled code
    hooks.tsv            # Original region hashes/lengths and hook targets
```

`midi-sync/l6/` contains the separate L6 1.10 hook, firmware bindings,
trampoline and linker script. The native builder selects it from the MAIN
reset vector; the L6max profile remains at the patch root.

Create only the files the patch needs. A change to existing instructions may
not need a compiled payload or linker script. Keep patch-specific bindings and
trampolines with that patch: their addresses depend on the target firmware.

## Adding a patch

1. Create `patches/<patch-name>/` and describe its purpose, supported input
   firmware hash, changed code/data, build command, and validation commands.
2. Put payload code in `src/` and placement rules in `linker/`. Document space
   limits and any RAM allocation or persistence behavior.
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
