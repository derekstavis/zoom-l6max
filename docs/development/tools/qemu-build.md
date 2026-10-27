# `qemu-build`

Build the pinned QEMU source configured by this repository and print the path
to `qemu-system-arm`:

```sh
cargo run --offline --locked -p qemu-build --
cargo run --offline --locked -p qemu-build -- --minimal
cargo run --offline --locked -p qemu-build -- /path/to/qemu-source
```

The optional source path overrides the repository’s configured source tree.
`--minimal` selects the minimal build configuration. The command prints the
resulting executable path for use with `--qemu` or the local test-driver
configuration. See [QEMU build setup](../../../emulator/qemu/README.md).
