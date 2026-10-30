# Factory protocol scenario

[Documentation index](../../README.md)

The `factory-protocol` scenario exercises the emulator's L6max inspection
session through its USB MIDI interface. It uses disposable state and media;
the scenario does not start calibration or change hardware routing.

```toml
[[fixture]]
name = "factory-protocol"
model = "l6max"
firmware_package = "/path/to/L6max.bin"
```

```sh
cargo run --offline --locked -p l6test -- run --case factory-protocol \
  --model l6max --config smoke.local.toml
```

The test covers session connection, input queries, metadata, keep-alive expiry,
and reconnection. Its results describe the emulated guest and do not establish
physical test-jig behavior or calibration accuracy. See the
[testing workflow](../../development/testing.md) for fixture requirements and
artifacts.
