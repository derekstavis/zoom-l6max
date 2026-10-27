# `l6fs`

`l6fs` combines FAT card creation and file transfer through the emulated USB
mass-storage device.

## Create SD cards

```sh
cargo run -p l6fs -- card create emulator/state/blank.img --size-mib 64
cargo run -p l6fs -- card demo emulator/state/demo.img --firmware-package /path/to/L6max.bin
cargo run -p l6fs -- card import emulator/state/imported.img --directory /path/to/card-folder
```

`create` produces an empty FAT32 image. Size defaults to 64 MiB and must be a
power of two from 64 MiB through 1 TiB. It refuses to overwrite an existing
image. `demo` creates a 128 MiB card populated with the package’s demo audio.
`import` creates a card from a host directory. Both populated-card commands
also refuse to overwrite the target.

## Read and write through USB storage

```sh
cargo run -p l6fs -- usb --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-firmware --sd-image /path/to/card.img -- list /
cargo run -p l6fs -- usb --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-firmware --sd-image /path/to/card.img -- get /TRACK.WAV /tmp/track.wav
cargo run -p l6fs -- usb --volatile --qemu /path/to/qemu-system-arm \
  --firmware-dir /path/to/unpacked-firmware --sd-image /path/to/card.img -- put /tmp/track.wav /TRACK.WAV
```

Everything before `--` is passed to the host engine. Everything after it is the
file command: `list [DIR]`, `get SD_PATH HOST_PATH`, or `put HOST_PATH SD_PATH`.
The firmware must be in USB File Transfer mode. `put` replaces/creates the
destination file; use a disposable card when experimenting. `--volatile` keeps
device state temporary while the selected card remains the explicit image.

Run `cargo run -p l6fs -- --help` or use `card --help` / `usb --help` for short
syntax summaries. See [USB storage behavior](../../emulator/usb.md).
