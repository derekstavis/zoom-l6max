# `l6patch`

`l6patch` applies revision-guarded patch sets to an unpacked firmware
directory and writes a new output directory. The input is never edited.

```sh
cargo run --offline --locked -p l6patch -- INPUT_UNPACKED_DIR OUTPUT_DIR
cargo run --offline --locked -p l6patch -- INPUT_UNPACKED_DIR OUTPUT_DIR \
  --patch knob-dialog --patch midi-sync --update-version 0113
```

Supported names are `knob-dialog`, `midi-sync`, and `din-mixer-control`.
Without `--patch`, the defaults are `knob-dialog` for L6max and `midi-sync` for
L6. Repeat `--patch` to combine compatible patches. L6 currently supports only
`midi-sync`. The output directory must not exist. A requested update version
is exactly four digits; without one the tool selects the version appropriate
to the chosen patch combination. `l6fw repack` preserves the payload checksum
by default; explicitly request `--recompute-checksum` after payload edits.

The tool checks package model and revision guards before writing. Read the
individual patch guide for model support, behavior, and validation commands.
