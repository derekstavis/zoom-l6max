# `firmware-patch-wasm-prebuild`

This maintainer tool builds the static templates consumed by the browser-based
patcher. It needs local unpacked L6max and L6 firmware directories and writes
generated output to `tools/firmware-patch-wasm/templates`. Generated templates
are build artifacts and should not be edited by hand.

```sh
cargo run --offline --locked -p firmware-patch-wasm-prebuild -- \
  /path/to/unpacked-l6max /path/to/unpacked-l6
```

Run it whenever a patch payload, hook registration, dependency, composition
rule or loader template input changes. The browser build consumes the checked-in
templates; compiling the Rust/WASM code does not regenerate them automatically.
The tool validates the input firmware revisions and emits a manifest and
per-patch template metadata. Keep source firmware local and review the generated
output before including it with the source change. Then run the `wasm-patch`
smoke case for both models to check the browser output against the native
builder; use the `sd-matrix` suite to validate SD loading and composed profiles.
See the [testing workflow](../testing.md#firmware-patch) for the commands.
