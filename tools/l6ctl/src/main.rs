use l6fw::Model;
use l6max_host::controls::control_target;
use serde_json::{Value, json};
use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
};

fn request(socket: &PathBuf, message: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        format!(
            "cannot connect to {}: {error}; launch the emulator with `--control-socket {}`",
            socket.display(),
            socket.display()
        )
    })?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .map_err(|error| error.to_string())?;
    writeln!(stream, "{message}").map_err(|error| error.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|error| error.to_string())?;
    let reply: Value = serde_json::from_str(&reply).map_err(|error| error.to_string())?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(reply
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("control request failed")
            .to_owned());
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

fn target(model: Model, name: &str) -> Result<u32, String> {
    if let Ok(target) = name.parse::<u32>() {
        return (target < 54)
            .then_some(target)
            .ok_or_else(|| "numeric button target must be 0..53".into());
    }
    control_target(model, name)
}

fn parse_int<T: std::str::FromStr>(value: Option<&String>, name: &str) -> Result<T, String> {
    value
        .ok_or_else(|| format!("missing {name}"))?
        .parse()
        .map_err(|_| format!("invalid {name}"))
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let mut socket = env::var_os("L6_CONTROL_SOCKET").map(PathBuf::from);
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--socket" {
            if index + 1 >= args.len() {
                return Err("--socket requires a path".into());
            }
            socket = Some(PathBuf::from(args.remove(index + 1)));
            args.remove(index);
        } else {
            index += 1;
        }
    }
    let command = args.first().map(String::as_str).unwrap_or("help");
    if matches!(command, "help" | "--help" | "-h") {
        println!(
            "Usage: l6ctl [--socket PATH] info\n       l6ctl [--socket PATH] button NAME|TARGET [--hold-ms MS]\n       l6ctl [--socket PATH] down NAME|TARGET DURATION_MS\n       l6ctl [--socket PATH] up NAME|TARGET\n       l6ctl [--socket PATH] encoder CHANNEL STEPS\n       l6ctl [--socket PATH] analog CHANNEL STEPS\n       l6ctl [--socket PATH] set-analog CHANNEL VALUE\n\nConnects to a running emulator started with --control-socket PATH. Button names use the semantic names documented by l6test; numeric targets 0..53 are also accepted. Encoder channels are one-based. Analog channels are 1..5."
        );
        return Ok(());
    }
    let socket = socket.ok_or("provide --socket PATH or set L6_CONTROL_SOCKET")?;
    if command == "info" {
        println!(
            "{}",
            serde_json::to_string_pretty(&request(&socket, json!({"command":"info"}))?).unwrap()
        );
        return Ok(());
    }
    let info = request(&socket, json!({"command":"info"}))?;
    let model = match info.get("model").and_then(Value::as_str) {
        Some("l6") => Model::L6,
        Some("l6max") => Model::L6max,
        Some(other) => return Err(format!("emulator returned unknown model {other:?}")),
        None => return Err("emulator info omitted model".into()),
    };
    let message = match command {
        "button" | "tap" | "down" | "up" => {
            let target = target(
                model,
                args.get(1).ok_or("button command needs a name or target")?,
            )?;
            match command {
                "button" | "tap" => {
                    let mut hold_ms = 50u32;
                    if args.len() == 4 && args[2] == "--hold-ms" {
                        hold_ms = parse_int(args.get(3), "hold duration")?;
                    } else if args.len() != 2 {
                        return Err("Usage: l6ctl button NAME|TARGET [--hold-ms MS]".into());
                    }
                    json!({"command":"tap","target":target,"hold_ms":hold_ms})
                }
                "down" => {
                    if args.len() != 3 {
                        return Err("Usage: l6ctl down NAME|TARGET DURATION_MS".into());
                    }
                    let duration_ms: u32 = parse_int(args.get(2), "duration")?;
                    json!({"command":"down","target":target,"duration_ms":duration_ms})
                }
                "up" => {
                    if args.len() != 2 {
                        return Err("Usage: l6ctl up NAME|TARGET".into());
                    }
                    json!({"command":"up","target":target})
                }
                _ => unreachable!(),
            }
        }
        "encoder" => {
            if args.len() != 3 {
                return Err("Usage: l6ctl encoder CHANNEL STEPS".into());
            }
            let channel: u32 = parse_int(args.get(1), "channel")?;
            let steps: i32 = parse_int(args.get(2), "steps")?;
            json!({"command":"encoder","channel":channel,"steps":steps})
        }
        "analog" => {
            if args.len() != 3 {
                return Err("Usage: l6ctl analog CHANNEL STEPS".into());
            }
            let channel: u32 = parse_int(args.get(1), "channel")?;
            let steps: i32 = parse_int(args.get(2), "steps")?;
            json!({"command":"analog","channel":channel,"steps":steps})
        }
        "set-analog" => {
            if args.len() != 3 {
                return Err("Usage: l6ctl set-analog CHANNEL VALUE".into());
            }
            let channel: u32 = parse_int(args.get(1), "channel")?;
            let value: u16 = parse_int(args.get(2), "value")?;
            json!({"command":"set-analog","channel":channel,"value":value})
        }
        other => return Err(format!("unknown command {other:?}; use `l6ctl --help`")),
    };
    println!(
        "{}",
        serde_json::to_string(&request(&socket, message)?).unwrap()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("l6ctl: {error}");
        std::process::exit(2);
    }
}
