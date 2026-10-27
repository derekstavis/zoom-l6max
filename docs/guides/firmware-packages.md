# Firmware packages

[Documentation index](../README.md)

`l6fw` detects and extracts the observed L6 and L6max package layouts, then
repackages unchanged components without changing their offsets. Supply firmware
locally; neither packed nor extracted manufacturer firmware is distributed here.

## Supported input

The two inspected stock packages are MAIN **0110**, panel **0100**:

| Model | Package size | Board mask | Package SHA-256 |
| --- | --- | --- | --- |
| L6 | `0x395200` | `0x01` | `64f1f36b8383176b5d841911fbf3c16205139a313ee88be906f313b1a98a33fb` |
| L6max | `0x1a8200` | `0x40` | `4e20f92c9c5131ed082c0feec2b2d7d0445fb576296f11f6dace56a4fc0c9b47` |

The package tool validates the model header, observed package table, image
boundaries, trailers, Cortex-M vectors and MAIN payload byte-sum checksum. It supports these two observed
layouts, not arbitrary ZOOM packages. The extracted L6max MAIN image hash is
`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`;
the L6 MAIN loads at `0x80001000` and L6max MAIN at `0x80000000`.

The emulator accepts either package, and its QEMU board detects the MAIN base
from the reset vector in the extracted image. The [compatibility guide](model-compatibility.md)
tracks the simulator, update, and patch support boundaries.

## Extract and verify a lossless round trip

From the repository root, use new output paths:

```sh
cargo run -p l6fw -- unpack /path/to/L6.BIN unpacked-l6
cargo run -p l6fw -- repack unpacked-l6 L6-repacked.BIN
cmp /path/to/L6.BIN L6-repacked.BIN

cargo run -p l6fw -- unpack /path/to/L6max.BIN unpacked-l6max
cargo run -p l6fw -- repack unpacked-l6max L6max-repacked.BIN
cmp /path/to/L6max.BIN L6max-repacked.BIN
```

A matching `cmp` exits successfully with no output. The extracted directory
contains executable main and panel images, headers, trailers, reserved bytes, and
`manifest.json` with original hashes. Repacking reports changed components;
the original input is never modified.

The supplied L6 1.10 package has also passed this full round trip locally,
including checksum recomputation; the resulting 3,756,544-byte package was
byte-identical to the source. The `l6fw` test suite exercises synthetic package
round trips for both L6 and L6max.

## Repack modified components

```sh
cargo run -p l6fw -- repack /path/to/modified-components L6max-modified.bin --recompute-checksum
```

The tool preserves component layout. Main firmware can grow within its
existing slot using the supported length records and padding rules; other
layout/length changes are rejected. See the
[package implementation](../../tools/l6fw/src/lib.rs) for enforced constraints.

The big-endian word at file offset `0x1fc` is the MAIN payload byte-sum checksum.
Plain repacking preserves it; `--recompute-checksum` regenerates it after edits.
This is an integrity check and does not sign firmware. No signature check was
identified in the application routines; the missing main bootloader may apply
additional checks. See the [update map](../firmware/updates.md#checksum-and-signing).

The eight-byte panel trailer contains the little-endian image length and ASCII
version. For the inspected packages, L6 uses offset `0x2951f8` and length
`0x3480`; L6max uses offset `0x1a71f8` and length `0x3ad8`. Both report panel
version `0100`. The NOR comparison at `0x1f6ffc`, panel flash version at
`0x08003ffc`, and UART programming sequence below the
[update reference](../firmware/updates.md) are L6max findings.

## Limits

- Other package layouts are unsupported and rejected. The replacement update
  emulator supports the observed L6 and L6max package layouts. The native and browser patch builders
  support the L6 `midi-sync` profile and the existing L6max profiles.
- A valid byte-sum checksum does not establish bootloader acceptance.
- Hardware installation of the corrected knob payload does not establish
  acceptance of arbitrary modifications or interrupted-update recovery.
- Generated binaries, manifests, and derived firmware contents stay local in
  ignored directories.

The L6 package's trailing resource bytes are written as `resource_region.bin`
and preserved as-is. The corresponding L6max reserved region remains in its
existing padding segment. The model field in each generated manifest is checked
against its format before repacking.
