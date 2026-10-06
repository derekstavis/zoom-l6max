use l6fw::{repack, unpack};
use std::{env, path::Path, process::ExitCode};
fn run() -> Result<(), String> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [command, source, output] if command == "unpack" => {
            unpack(Path::new(source), Path::new(output))
        }
        [command, input, output] if command == "repack" => {
            repack(Path::new(input), Path::new(output), false)
        }
        [command, input, output, flag] if command == "repack" && flag == "--recompute-checksum" => {
            repack(Path::new(input), Path::new(output), true)
        }
        _ => Err(
            "usage: cargo run --bin l6fw -- <unpack SOURCE DIR | repack DIR OUTPUT [--recompute-checksum]>"
                .into(),
        ),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
