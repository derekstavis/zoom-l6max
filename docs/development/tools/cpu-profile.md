# `cpu-profile`

`cpu-profile` measures QEMU process CPU use while sampling guest execution,
exception numbers, guest time, and startup indicators. It writes
`startup-indicators.tsv` into the engine log directory.

```sh
cargo run --offline --locked -p cpu-profile -- \
  --warmup-seconds 45 --measure-seconds 20 --sample-interval-ms 157 \
  --qemu /path/to/qemu-system-arm --volatile \
  --firmware-dir /path/to/unpacked-firmware --logs /tmp/l6-profile
```

Profile flags are `--warmup-seconds N`, `--measure-seconds N`,
`--sample-interval-ms N`, and `--skip-guest-clock`. Positive integer values
are required. Remaining arguments are passed to the shared host engine, so
use its firmware, QEMU, state, audio, and timing options. The guest-clock
check is enabled by default and fails if measured guest progression differs
from wall time by more than five percent.

Use a new `--logs` directory and disposable state. This profiler is a focused
development tool, not a test-suite substitute.

When a persistent `--state-dir` contains `main-nor.bin`, the installed MAIN in
that NOR is what boots. Supplying a different `--firmware-dir` does not replace
it, and `cpu-profile` does not check that the two images match. Before attributing
samples to a particular MAIN, verify that the selected image matches the bytes
installed in NOR (starting at offset `0x50000`), or prepare a matching disposable
state. Record the exact image, state, QEMU, profiler, and sample identities.
