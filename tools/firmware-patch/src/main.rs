//! Reproducible, revision-guarded firmware experiment. Never edits its input.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn run(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
fn branch(from: u32, to: u32, link: bool) -> [u8; 4] {
    let displacement = to.wrapping_sub(from + 4);
    assert!(displacement as i32 >= -16_777_216 && (displacement as i32) < 16_777_216);
    assert_eq!(displacement & 1, 0);
    let s = (displacement >> 24) & 1;
    let j1 = (!(displacement >> 23) ^ s) & 1;
    let j2 = (!(displacement >> 22) ^ s) & 1;
    let first = 0xf000 | (s << 10) | ((displacement >> 12) & 0x3ff);
    let second = (if link { 0xd000 } else { 0x9000 })
        | (j1 << 13)
        | (j2 << 11)
        | ((displacement >> 1) & 0x7ff);
    let mut bytes = [0; 4];
    bytes[..2].copy_from_slice(&(first as u16).to_le_bytes());
    bytes[2..].copy_from_slice(&(second as u16).to_le_bytes());
    bytes
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if args.len() != 2 && !(args.len() == 4 && args[2] == Path::new("--update-version")) {
        return Err(
            "Usage: firmware-patch INPUT_UNPACKED_DIR OUTPUT_DIR [--update-version FOUR_DIGITS]"
                .into(),
        );
    }
    let version = args
        .get(3)
        .map(|v| v.to_str().ok_or("version must be UTF-8"))
        .transpose()?
        .unwrap_or("0112");
    if version.len() != 4 || !version.bytes().all(|b| b.is_ascii_digit()) {
        return Err("update version must be exactly four ASCII digits, e.g. 0112".into());
    }
    let source = args[0].canonicalize()?;
    if args[1].exists() {
        return Err("output must be a new directory".into());
    }
    let mut firmware = fs::read(source.join("main_firmware.bin"))?;
    let original_length = firmware.len();
    let digest = format!("{:x}", Sha256::digest(&firmware));
    if digest != "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461" {
        return Err("unsupported main firmware SHA-256".into());
    }
    assert_eq!(
        &firmware[0x36c74..0x36c78],
        &branch(0x80036c74, 0x80053b28, true)
    );
    assert_eq!(&firmware[0x40c..0x410], &0x8007eea9u32.to_le_bytes());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../patches/knob-dialog")
        .canonicalize()?;
    fs::create_dir_all(&args[1])?;
    let output = args[1].canonicalize()?;
    let elf = output.join("payload.elf");
    let blob = output.join("payload.bin");
    // Assembly trampolines read displaced instructions from the user's image.
    // Validate hook regions before compiling or modifying the output image.
    let hook_table = fs::read_to_string(root.join("hooks.tsv"))?;
    for line in hook_table
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let columns = line.split('\t').collect::<Vec<_>>();
        let address = u32::from_str_radix(columns[0], 16)?;
        let length: usize = columns[2].parse()?;
        let offset = (address - 0x80000000) as usize;
        if format!("{:x}", Sha256::digest(&firmware[offset..offset + length])) != columns[3] {
            return Err(format!("hook guard failed: {}", columns[1]).into());
        }
    }
    run(Command::new("rustc")
        .current_dir(&source)
        .arg(root.join("src/payload.rs"))
        // Keep embedded source paths stable when organizing payload modules.
        .arg(format!(
            "--remap-path-prefix={}={}",
            root.join("src").display(),
            root.display()
        ))
        .args([
            "--edition=2024",
            "--target=thumbv7em-none-eabihf",
            "-C",
            "opt-level=z",
            "-C",
            "panic=abort",
            "-C",
            "relocation-model=static",
            "-C",
            "linker=/opt/homebrew/bin/ld.lld",
            "-C",
        ])
        .arg(format!(
            "link-arg=-T{}",
            root.join("linker/link.ld").display()
        ))
        .arg("-o")
        .arg(&elf))?;
    let symbols = run(Command::new("/opt/homebrew/opt/llvm/bin/llvm-nm").arg(&elf))?;
    let symbol = |name: &str| -> Result<u32, Box<dyn std::error::Error>> {
        let line = symbols
            .lines()
            .find(|line| line.split_whitespace().last() == Some(name))
            .ok_or_else(|| format!("missing patch entry: {name}"))?;
        Ok(u32::from_str_radix(line.split_whitespace().next().unwrap(), 16)? & !1)
    };
    run(Command::new("/opt/homebrew/opt/llvm/bin/llvm-objcopy")
        .args(["-O", "binary"])
        .arg(&elf)
        .arg(&blob))?;
    let payload = fs::read(blob)?;
    if 0x108000 + payload.len() > 0x1a2ff8 {
        return Err("patch exceeds observed application slot".into());
    }
    if original_length > 0x108000 {
        return Err("payload would overwrite original firmware".into());
    }
    let padding = fs::read(source.join("padding_before_secondary.bin"))?;
    let consumed = 0x108000 + payload.len() - original_length;
    if consumed > padding.len() || padding[..consumed].iter().any(|&b| b != 0xff) {
        return Err("payload would overwrite non-erased reserved bytes".into());
    }
    firmware.resize(0x108000, 0xff);
    firmware.extend(&payload);
    firmware[0x36c74..0x36c78].copy_from_slice(&branch(0x80036c74, symbol("knob_changed")?, true));
    let mut hooks = Vec::new();
    for line in hook_table.lines() {
        if line.starts_with("#") || line.is_empty() {
            continue;
        }
        let columns = line.split("\t").collect::<Vec<_>>();
        let address = u32::from_str_radix(columns[0], 16)?;
        let length: usize = columns[2].parse()?;
        let offset = (address - 0x80000000) as usize;
        let target = symbol(columns[1])?;
        firmware[offset..offset + 4].copy_from_slice(&branch(address, target, false));
        for chunk in firmware[offset + 4..offset + length].chunks_exact_mut(2) {
            chunk.copy_from_slice(&[0, 0xbf]);
        }
        hooks.push(serde_json::json!({"address":address,"symbol":columns[1],"target":target,"displaced_length":length,"displaced_sha256":columns[3]}));
    }
    for (address, name, target) in [
        (0x8000648e, "notification_initialize_hook", 0x8005f600),
        (0x80006126, "semaphore_take_hook", 0x80091048),
        (0x800062c0, "semaphore_give_hook", 0x80090ad0),
    ] {
        let offset = (address - 0x80000000) as usize;
        if firmware[offset..offset + 4] != branch(address, target, true) {
            return Err(format!("call guard failed: {name}").into());
        }
        firmware[offset..offset + 4].copy_from_slice(&branch(address, symbol(name)?, true));
        hooks.push(serde_json::json!({"address":address,"symbol":name,"target":symbol(name)?}));
    }
    // Preserve the fixed package slots, consuming only erased main padding.
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), output.join(entry.file_name()))?;
        }
    }
    fs::write(output.join("main_firmware.bin"), &firmware)?;
    fs::write(
        output.join("padding_before_secondary.bin"),
        &padding[consumed..],
    )?;
    let mut trailer = fs::read(output.join("main_trailer.bin"))?;
    trailer[..4].copy_from_slice(&(firmware.len() as u32).to_le_bytes());
    trailer[4..8].copy_from_slice(version.as_bytes());
    fs::write(output.join("main_trailer.bin"), trailer)?;
    let mut footer = fs::read(output.join("package_footer.bin"))?;
    footer[0x38..0x3c].copy_from_slice(&(firmware.len() as u32).to_le_bytes());
    footer[0x3c..0x40].copy_from_slice(version.as_bytes());
    fs::write(output.join("package_footer.bin"), footer)?;
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json"))?)?;
    manifest["segments"][2]["size"] = serde_json::json!(firmware.len());
    manifest["segments"][3]["offset"] = serde_json::json!(0x200 + firmware.len());
    manifest["segments"][3]["size"] = serde_json::json!(padding.len() - consumed);
    fs::write(
        output.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    let update = output.join("L6max.bin");
    l6fw::repack(&output, &update, true)?;
    let packed = fs::read(&update)?;
    l6fw::update::validate(&packed)?;
    let report = serde_json::json!({"source_sha256":digest,"patched_sha256":format!("{:x}",Sha256::digest(&firmware)),"image_length":firmware.len(),"payload_address":"0x80108000","knob_changed":symbol("knob_changed")?,"notification_tick":symbol("notification_tick_hook")?,"hooks":hooks,"synchronization":"existing counting semaphore with heap sidecar for same-task nesting; kernel receives untagged handle","knob_presentation":"coalesced copied value serviced by notification callback; mixer hook never waits for rendering","state_storage":"original firmware heap, allocated per popup and released on restoration","update_package":"L6max.bin","update_sha256":format!("{:x}",Sha256::digest(&packed)),"version":String::from_utf8_lossy(&packed[0x1a31fc..0x1a3200]),"usage":"Repacked update for emulator validation; physical bootloader acceptance unverified"});
    fs::write(
        output.join("patch.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "{} bytes of Rust payload; patched firmware: {}",
        payload.len(),
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::branch;
    #[test]
    fn thumb_branches_match_original_firmware_instructions() {
        assert_eq!(
            branch(0x80036c74, 0x80053b28, true),
            [0x1c, 0xf0, 0x58, 0xff]
        );
        assert_eq!(
            branch(0x80053b54, 0x80053710, false),
            [0xff, 0xf7, 0xdc, 0xbd]
        );
    }
}

#[cfg(test)]
#[path = "../../../patches/knob-dialog/src/values.rs"]
mod values;
