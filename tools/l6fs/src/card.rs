//! Create a card or edit its settings without replacing existing media.
use std::{ffi::OsString, path::Path};

const CREATE_USAGE: &str = "l6fs card create PATH --model l6|l6max [--file CARD_PATH=HOST_PATH]... [--sound-pad PAD=FILE[,mode=one-shot|loop|hold][,level=0..59]]... [--no-system-file]";
const BUILD_USAGE: &str = "l6fs card build PATH --spec SPEC.toml";
const SET_PADS_USAGE: &str = "l6fs card set-pads CARD --model l6|l6max --sound-pad PAD=FILE[,mode=one-shot|loop|hold][,level=0..59]...";
const IMPORT_USAGE: &str = "l6fs card import PATH --directory FOLDER --model l6|l6max";

pub fn run(args: Vec<OsString>) -> Result<(), Box<dyn std::error::Error>> {
    if args
        .first()
        .is_some_and(|arg| matches!(arg.to_str(), Some("help" | "--help" | "-h")))
    {
        println!(
            "Usage: {CREATE_USAGE}\n       l6fs card demo PATH --firmware-package PACKAGE\n       {IMPORT_USAGE}\n       {BUILD_USAGE}\n       {SET_PADS_USAGE}"
        );
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "import") {
        if args.len() != 6 || args[2] != "--directory" || args[4] != "--model" {
            return Err(IMPORT_USAGE.into());
        }
        sd_image::from_directory(Path::new(&args[1]), Path::new(&args[3]), model(&args[5])?)?;
        println!("Created card from folder: {}", args[1].to_string_lossy());
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "demo") {
        if args.len() != 4 || args[2] != "--firmware-package" {
            return Err("Usage: l6fs card demo PATH --firmware-package PACKAGE".into());
        }
        sd_image::create(Path::new(&args[1]), Path::new(&args[3]))?;
        println!("Created 4 GiB demo card: {}", args[1].to_string_lossy());
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "build") {
        if args.len() != 4 || args[2] != "--spec" {
            return Err(BUILD_USAGE.into());
        }
        let spec = Path::new(&args[3]);
        sd_image::spec::CardSpec::load(spec)?
            .build(Path::new(&args[1]), spec.parent().unwrap_or(Path::new(".")))?;
        println!("Created card: {}", args[1].to_string_lossy());
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "set-pads") {
        return set_pads(&args[1..]);
    }
    if args.first().is_none_or(|arg| arg != "create") || args.len() < 2 {
        return Err(CREATE_USAGE.into());
    }
    create(&args[1], &args[2..])
}

fn text(value: &OsString) -> Result<&str, String> {
    value
        .to_str()
        .ok_or_else(|| "arguments must be valid Unicode".to_string())
}

fn create(path: &OsString, args: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let mut selected = None;
    let mut contents = sd_image::Contents::default();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        if flag == "--no-system-file" {
            contents.omit_system_file = true;
            continue;
        }
        let value = args.next().ok_or(CREATE_USAGE)?;
        match text(flag)? {
            "--model" => selected = Some(model(value)?),
            "--file" => {
                let (card, host) = text(value)?
                    .split_once('=')
                    .ok_or("--file expects CARD_PATH=HOST_PATH")?;
                contents.files.push((card.into(), host.into()));
            }
            "--sound-pad" => contents.sound_pads.push(text(value)?.parse()?),
            _ => return Err(CREATE_USAGE.into()),
        }
    }
    let selected = selected.ok_or(CREATE_USAGE)?;
    sd_image::create_with(Path::new(path), selected, &contents)?;
    println!("Created 4 GiB FAT32 card: {}", path.to_string_lossy());
    Ok(())
}

fn set_pads(args: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let (card, args) = args.split_first().ok_or(SET_PADS_USAGE)?;
    let mut selected = None;
    let mut pads = Vec::new();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(SET_PADS_USAGE)?;
        match text(flag)? {
            "--model" => selected = Some(model(value)?),
            "--sound-pad" => pads.push(text(value)?.parse()?),
            _ => return Err(SET_PADS_USAGE.into()),
        }
    }
    sd_image::pad_settings::apply(Path::new(card), selected.ok_or(SET_PADS_USAGE)?, &pads)?;
    println!("Updated pad settings on {}", card.to_string_lossy());
    Ok(())
}

fn model(value: &OsString) -> Result<l6fw::Model, String> {
    match value.to_str() {
        Some("l6") => Ok(l6fw::Model::L6),
        Some("l6max") => Ok(l6fw::Model::L6max),
        _ => Err("--model must be l6 or l6max".into()),
    }
}
