# Firmware packages

[Documentation index](../README.md)

`l6fw` extracts and repacks the observed L6max update format without changing
its component offsets. Supply firmware locally; neither packed nor extracted
manufacturer firmware is distributed here.

## Supported input

The investigated stock package is L6max **1.10**, 1,737,216 bytes:

| Input | SHA-256 |
| --- | --- |
| `L6max.bin` | `4e20f92c9c5131ed082c0feec2b2d7d0445fb576296f11f6dace56a4fc0c9b47` |
| Extracted `main_firmware.bin` | `980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461` |

The package tool validates the observed fixed layout and lengths. It is not a
general extractor for all ZOOM firmware. The knob patch adds a strict main
image hash guard; address-based findings in the reference docs apply to that
revision.

## Extract and verify a lossless round trip

From the repository root, use new output paths:

```sh
cargo run -p l6fw -- unpack /path/to/L6max.bin unpacked
cargo run -p l6fw -- repack unpacked L6max-repacked.bin
cmp /path/to/L6max.bin L6max-repacked.bin
```

A matching `cmp` exits successfully with no output. The extracted directory
contains executable main and panel images, headers, trailers, padding, and
`manifest.json` with original hashes. Repacking reports changed components;
the original input is never modified.

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

The eight-byte secondary trailer contains the little-endian panel image length
(`0x3ad8`) and ASCII version `0100`. Main firmware compares the installed panel
version in NOR at physical offset `0x1f6ffc` with panel flash at `0x08003ffc`
and can program the panel over UART. The [storage map](../emulator/storage.md)
explains installed records; the [update reference](../firmware/updates.md)
explains the handoff and panel programming.

## Limits

- Other package layouts are unsupported and rejected.
- A valid byte-sum checksum does not establish bootloader acceptance.
- Hardware installation of the corrected knob payload does not establish
  acceptance of arbitrary modifications or interrupted-update recovery.
- Generated binaries, manifests, and derived firmware contents stay local in
  ignored directories.
