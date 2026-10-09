# Firmware patches

Each patch lives in its own directory named for the feature. Commit source and
reproduction instructions here; firmware packages and generated payloads stay
in ignored output directories.

## Catalog

| Patch | Target | Purpose | Builder |
| --- | --- | --- | --- |
| [knob-dialog](knob-dialog/README.md) | Main firmware | Show channel encoder values using the stock notification renderer | `firmware-patch` |
| [midi-sync](midi-sync/README.md) | Main firmware | Send the full channel mixer state as Control Change on request | `firmware-patch` |

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

The builder supports `knob-dialog` and `midi-sync`, and `--patch` may repeat to
combine them. It places payloads back to back from `0x80108000` in catalog
order (each linker script takes its base from `PAYLOAD_BASE`), checks every hook
region against the input, and refuses overlapping hook regions. Overlap checks do
not prove independence: before combining another patch, verify shared
resources (RAM, heap, locks, callbacks) and input hash expectations, and
document the combination in each patch's README.
