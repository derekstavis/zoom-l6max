# Factory protocol scenario

[Documentation index](../../README.md)

The `factory-protocol` scenario exercises the emulator's L6max inspection
session through its USB MIDI interface. It uses disposable state and media;
the scenario does not start calibration or change hardware routing.

```sh
cargo run --offline --locked -p l6test -- run --case factory-protocol \
  --model l6max --config config.toml
```

The test covers session connection, input queries, metadata, keep-alive expiry,
and reconnection. Its results describe the emulated guest and do not establish
physical test-jig behavior or calibration accuracy. See the
[testing workflow](../../development/testing.md) for configuration and
artifacts.
