# `l6fs`

`l6fs` combines FAT card creation and file transfer through the emulated USB
mass-storage device.

## Create SD cards

```sh
cargo run -p l6fs -- card create emulator/state/blank.img --model l6max
cargo run -p l6fs -- card demo emulator/state/demo.img --firmware-package /path/to/L6max.bin
cargo run -p l6fs -- card import emulator/state/imported.img --directory /path/to/card-folder --model l6max
```

Every command creates a 4 GiB FAT32 image with 32 KiB clusters, which the firmware
accepts for recording; see [card layout](../../emulator/usb.md#card-layout). Images
are sparse, so they use only the disk space their files need. `create` makes an empty
card, `demo` adds the package's demo audio, and `import` copies a host directory.
All three add the model's system file and refuse to overwrite an existing image.

`create` can also define the card's contents:

```sh
cargo run -p l6fs -- card create emulator/state/pads.img --model l6max \\
  --file SOUND_PAD/PAD4/KICK.WAV=/path/to/kick.wav \\
  --sound-pad 4=SOUND_PAD/PAD4/KICK.WAV,mode=loop,level=40
```

`--file` copies a host file to a card path, and `--sound-pad` writes the model's pad
settings file. `--no-system-file` leaves the system file out. `card set-pads CARD --model MODEL --sound-pad ...` updates assignments on an
existing card, and `card build PATH --spec SPEC.toml` builds a
[card spec](../testing.md).

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
