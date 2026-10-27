# `macos-package`

`macos-package` creates a firmware-free macOS application bundle around a
QEMU binary.

```sh
cargo run --offline --locked -p macos-package -- /path/to/qemu-system-arm
cargo run --offline --locked -p macos-package -- /path/to/qemu-system-arm \
  --debug --output /tmp/Zoomie.app --version 0.1.0
```

Options are `--debug`, `--output DIRECTORY`, and `--version VERSION`. The
default output is selected by the packager. The QEMU binary is bundled; device
firmware is supplied by the user at runtime. See the
[macOS application guide](../../guides/macos-app.md).
