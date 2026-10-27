//! Optional local Unix-socket control endpoint for a running emulator.
use crate::input::InputHub;
use l6fw::Model;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct ControlServer {
    path: PathBuf,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ControlServer {
    pub fn start(path: PathBuf, inputs: Arc<InputHub>, model: Model) -> Result<Self, String> {
        if fs::symlink_metadata(&path).is_ok() {
            return Err(format!(
                "control socket path already exists: {}",
                path.display()
            ));
        }
        let listener = UnixListener::bind(&path)
            .map_err(|error| format!("bind control socket {}: {error}", path.display()))?;
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(&path);
            return Err(format!("secure control socket {}: {error}", path.display()));
        }
        if let Err(error) = listener.set_nonblocking(true) {
            let _ = fs::remove_file(&path);
            return Err(format!("configure control socket: {error}"));
        }
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = stopping.clone();
        let thread = match thread::Builder::new()
            .name("l6-control-socket".into())
            .spawn(move || {
                while !thread_stopping.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => handle(stream, &inputs, model),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => {
                            tracing::warn!(%error, "control socket accept failed");
                            thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                let _ = fs::remove_file(&path);
                return Err(format!("start control socket thread: {error}"));
            }
        };
        tracing::info!(socket=%path.display(), "local emulator control enabled");
        Ok(Self {
            path,
            stopping,
            thread: Some(thread),
        })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn model_name(model: Model) -> &'static str {
    match model {
        Model::L6 => "l6",
        Model::L6max => "l6max",
    }
}

fn number(request: &Value, key: &str) -> Result<u64, String> {
    request
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{key} must be a nonnegative integer"))
}

fn signed(request: &Value, key: &str) -> Result<i32, String> {
    request
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("{key} must be a signed 32-bit integer"))
}

fn execute(request: &Value, inputs: &InputHub, model: Model) -> Result<Value, String> {
    let command = request
        .get("command")
        .and_then(Value::as_str)
        .ok_or("request needs a command")?;
    match command {
        "info" => {
            let main = inputs.main.snapshot();
            let panel = inputs.panel.snapshot();
            Ok(json!({
                "model": model_name(model),
                "guest_ms": panel.guest_ms,
                "powered_off": main.powered_off,
                "main_indicators": main.indicators,
                "panel_indicators": panel.indicators,
            }))
        }
        "tap" | "down" | "up" => {
            let target = number(request, "target")?;
            if target >= crate::controls::INPUT_COUNT as u64 {
                return Err(format!(
                    "button target must be less than {}",
                    crate::controls::INPUT_COUNT
                ));
            }
            let target = target as u32;
            match command {
                "tap" => {
                    let hold = number(request, "hold_ms")?;
                    if hold == 0 || hold > 60_000 {
                        return Err("hold_ms must be between 1 and 60000".into());
                    }
                    inputs
                        .tap(target, hold as u32)
                        .map_err(|error| error.to_string())?;
                }
                "down" => {
                    let duration = number(request, "duration_ms")?;
                    if duration == 0 || duration > 60_000 {
                        return Err("duration_ms must be between 1 and 60000".into());
                    }
                    inputs
                        .down(target, duration as u32)
                        .map_err(|error| error.to_string())?;
                }
                "up" => {
                    inputs.up(target).map_err(|error| error.to_string())?;
                }
                _ => unreachable!(),
            }
            Ok(json!({"target":target,"command":command}))
        }
        "encoder" => {
            let channel = number(request, "channel")?;
            let count = if model == Model::L6 { 6 } else { 8 };
            if channel == 0 || channel > count {
                return Err(format!(
                    "encoder channel must be 1..{count} for {}",
                    model_name(model)
                ));
            }
            let steps = signed(request, "steps")?;
            if steps == 0 || steps.unsigned_abs() > 1000 {
                return Err("steps must be between -1000 and 1000, excluding zero".into());
            }
            inputs
                .encoder(channel as u32 - 1, steps)
                .map_err(|error| error.to_string())?;
            Ok(json!({"channel":channel,"steps":steps}))
        }
        "analog" => {
            let channel = number(request, "channel")?;
            if channel == 0 || channel > 5 {
                return Err("analog channel must be 1..5".into());
            }
            let steps = signed(request, "steps")?;
            if steps == 0 || steps.unsigned_abs() > 1000 {
                return Err("steps must be between -1000 and 1000, excluding zero".into());
            }
            let index = 8 + channel as usize - 1;
            inputs
                .knob(index, steps)
                .map_err(|error| error.to_string())?;
            Ok(json!({"channel":channel,"steps":steps}))
        }
        "set-analog" => {
            let channel = number(request, "channel")?;
            let value = number(request, "value")?;
            if channel == 0 || channel > 5 || value > 1023 {
                return Err("channel must be 1..5 and value must be 0..1023".into());
            }
            inputs
                .set_analog_position(channel as usize - 1, value as u16)
                .map_err(|error| error.to_string())?;
            Ok(json!({"channel":channel,"value":value}))
        }
        _ => Err(format!("unknown control command {command:?}")),
    }
}

fn handle(mut stream: UnixStream, inputs: &InputHub, model: Model) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    let outcome = BufReader::new(&mut stream)
        .take(4097)
        .read_line(&mut line)
        .map_err(|error| error.to_string())
        .and_then(|_| {
            if line.len() > 4096 {
                return Err("request exceeds 4096 bytes".into());
            }
            let request: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
            execute(&request, inputs, model)
        });
    let reply = match outcome {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    let _ = writeln!(stream, "{reply}");
}
