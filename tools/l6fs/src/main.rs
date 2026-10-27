mod card;
mod usb;

use std::ffi::OsString;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().and_then(|arg| arg.into_string().ok());
    let args = args.collect::<Vec<OsString>>();
    match command.as_deref() {
        Some("card") => card::run(args),
        Some("usb") => usb::run(args),
        Some("help" | "--help" | "-h") | None => {
            println!(
                "Usage:\n  l6fs card create PATH [--size-mib MIB]\n  l6fs card demo PATH --firmware-package PACKAGE\n  l6fs card import PATH --directory FOLDER\n  l6fs usb --sd-image CARD.img [engine options] -- list [DIR] | get SD_PATH HOST_PATH | put HOST_PATH SD_PATH\n\nCard commands create FAT32 media or populate a demo card. USB commands access a card through the emulator's USB mass-storage path."
            );
            Ok(())
        }
        Some(other) => Err(format!("unknown l6fs command {other:?}; use `l6fs --help`").into()),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("l6fs: {error}");
        std::process::exit(1);
    }
}
