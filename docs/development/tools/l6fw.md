# `l6fw`

`l6fw` unpacks and repacks L6 and L6max firmware update packages. The model is
detected from the package header.

```sh
cargo run --offline --locked -p l6fw -- unpack PACKAGE OUTPUT_DIRECTORY
cargo run --offline --locked -p l6fw -- repack INPUT_DIRECTORY OUTPUT_PACKAGE
cargo run --offline --locked -p l6fw -- repack INPUT_DIRECTORY OUTPUT_PACKAGE --recompute-checksum
```

Unpack into a new directory before editing. Repack writes a new package and
validates the expected firmware files. `--recompute-checksum` explicitly
regenerates the package checksum; otherwise the original checksum policy is
preserved. Keep firmware packages and extracted firmware out of Git.

`l6fw` handles package structure. To apply supported firmware modifications,
see [`l6patch`](l6patch.md) and the patch-specific guides.
