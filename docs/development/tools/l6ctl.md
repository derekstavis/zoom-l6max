# `l6ctl`

`l6ctl` sends input commands to an already running emulator. Start the emulator
with an explicit socket path:

```sh
cargo run -p l6max-gui -- --control-socket /tmp/l6-control.sock
```

Then run commands in another terminal:

```sh
cargo run -p l6ctl -- --socket /tmp/l6-control.sock info
cargo run -p l6ctl -- --socket /tmp/l6-control.sock button channel.1.mute
cargo run -p l6ctl -- --socket /tmp/l6-control.sock down play 500
cargo run -p l6ctl -- --socket /tmp/l6-control.sock up play
cargo run -p l6ctl -- --socket /tmp/l6-control.sock encoder 1 -2
cargo run -p l6ctl -- --socket /tmp/l6-control.sock analog 1 3
cargo run -p l6ctl -- --socket /tmp/l6-control.sock set-analog 1 512
```

`--socket PATH` can be omitted when `L6_CONTROL_SOCKET` is set. The socket is
created with current-user-only permissions and is removed when the emulator
exits. `info` reports model, guest time, power state, and indicator snapshots.

`button` (alias `tap`) presses and releases after 50 ms by default; use
`--hold-ms MS` to change it. `down NAME|TARGET DURATION_MS` presses for a
bounded duration, while `up NAME|TARGET` releases it. Names are semantic names
from the shared host control map (for example `channel.1.mute`, `mode.level`,
or `recorder.menu`); numeric GPIO targets `0..53` are also accepted.
`encoder CHANNEL STEPS` uses one-based channels and signed detents.
`analog CHANNEL STEPS` moves one of five knobs by signed steps; `set-analog`
sets a knob position from `0..1023`.

Use `cargo run -p l6ctl -- --help` for the syntax summary. The endpoint is
opt-in: an emulator started without `--control-socket` exposes no control
socket.
