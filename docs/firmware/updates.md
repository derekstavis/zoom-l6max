# Firmware update behavior

[Documentation index](../README.md)

The emulator supports the L6 and L6max 1.10 package layouts. Supply firmware
locally; the repository does not distribute firmware images. See the
[firmware package guide](../guides/firmware-packages.md) for validation,
unpacking, and repacking.

On L6max, the application validates an SD update package and presents an
Execute/Cancel confirmation. Execute prepares the update and requests a power
cycle. On the next start, the installer updates MAIN, and the application
updates the panel when its version differs. The
[official update guide](https://zoomcorp.com/media/documents/L6max_Firmware_Update_Guide_EN.pdf)
describes the device's update controls and indicators.

## Checksum and signing

The supported package uses a 32-bit additive checksum for the MAIN section.
`l6fw repack --recompute-checksum` updates that value after an edit; a lossless
repack preserves the stored checksum. The known application check is an
integrity check. Additional validation by the physical bootloader remains
unverified.

## Replacement installer model

The emulator uses a replacement installer for the SD-to-NOR step. The native
persistent UI enables it when a card is attached; headless tools use
`--update-bootloader`. It operates on isolated or explicitly selected device
state, requires the matching `L6.BIN` or `L6max.BIN` on the card, validates the
supported package layout, and preserves boot and calibration/settings regions.
The installer reads the SD card without modifying it.

```sh
cargo run -p l6max-gui -- /path/to/qemu-system-arm \
  --state-dir emulator/state/update-device --sd-image /path/to/card.img \
  --update-bootloader
```

Select Execute in the firmware UI, power off, then start the device again with
the same state and card. Closing the window alone is not a firmware power-off
request. The installer supports an unpartitioned FAT volume or the first FAT
partition in an MBR.

## Validation and limits

```sh
cargo run --offline --locked -p l6test -- run \
  --config smoke.local.toml --case firmware-update --model all
```

The scenario uses disposable state and media and checks package rejection,
installation, persistence, and preserved storage. Emulator installation does
not establish the physical bootloader's acceptance rules, recovery behavior,
signature policy, or interrupted-update behavior. BOOT-component replacement
and physical timing of flash operations are not modeled.
