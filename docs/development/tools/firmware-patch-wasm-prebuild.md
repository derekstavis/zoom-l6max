# `firmware-patch-wasm-prebuild`

This maintainer tool builds the static templates consumed by the browser-based
patcher. It needs local unpacked L6max and L6 firmware directories and writes
generated output under the repository's `web` workspace. Generated templates
are build artifacts and should not be edited by hand.

```sh
cargo run --offline --locked -p firmware-patch-wasm-prebuild -- \
  /path/to/unpacked-l6max /path/to/unpacked-l6
```

Run it only when intentionally refreshing the browser patch templates. It
validates the input firmware revisions and emits a manifest and per-patch
template metadata. Keep source firmware local and review generated output
before including any build artifact in a change.
