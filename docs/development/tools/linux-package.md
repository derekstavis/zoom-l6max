# `linux-package`

`linux-package` creates a firmware-free Linux AppDir and, when AppImage tools
are available, an AppImage from the supplied QEMU executable.

```sh
cargo run --offline --locked -p linux-package -- /path/to/qemu-system-arm
cargo run --offline --locked -p linux-package -- /path/to/qemu-system-arm \
  --version 0.1.0 --debug --appimagetool /path/to/appimagetool --runtime /path/to/runtime
```

Options are `--version VERSION`, `--debug`, `--appimagetool PATH`, and
`--runtime PATH`. The last two select explicit AppImage tooling. Device
firmware is supplied by the user at runtime. See the
[Linux guide](../../guides/linux.md).
