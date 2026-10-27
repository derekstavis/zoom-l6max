# `qemu-run`

`qemu-run` provides low-level timed input and memory probing for focused
emulator investigations. For repeatable assertions use `l6test`; for
structured observations use `l6dbg`.

```sh
cargo run --offline --locked -p qemu-run -- \
  --qemu /path/to/qemu-system-arm --firmware-dir /path/to/unpacked-firmware \
  --seconds 30 --button play:1000:1200 --probe 0x80000000:64
```

Optional flags are `--firmware-dir DIR`, `--audio-device SPEC`, `--seconds N`,
`--logs DIR`, `--sd-image FILE`, `--trace-sd`, `--service-audio-queue`,
repeatable `--button NAME:START_MS:END_MS`, `--panel-key ROW:COL:START_MS:END_MS`,
`--main-key NAME:START_MS:END_MS`, `--panel-probe ADDRESS:SIZE`, and
`--probe ADDRESS:SIZE`. Addresses and sizes accept decimal or `0x` hexadecimal
where appropriate. Button names include `menu`, `play`, `record`, `tap`, and
`sound-pad-1..4`; model validation rejects unverified mappings. Run with
`--help` for the compact usage line.

Probe output and logs are investigation artifacts. Do not treat a successful
process exit as a regression qualification.
