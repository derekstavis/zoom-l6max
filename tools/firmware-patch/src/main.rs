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
const HOMEBREW_LLD: &str = "/opt/homebrew/bin/ld.lld";
const HOMEBREW_NM: &str = "/opt/homebrew/opt/llvm/bin/llvm-nm";
const HOMEBREW_OBJCOPY: &str = "/opt/homebrew/opt/llvm/bin/llvm-objcopy";
/// The active toolchain's `bin` directory, which holds `rust-lld` and, with the
/// `llvm-tools` component (listed in `rust-toolchain.toml`), `llvm-nm` and
/// `llvm-objcopy`.
fn toolchain_bin() -> Option<PathBuf> {
    let sysroot = run(Command::new("rustc").args(["--print", "sysroot"])).ok()?;
    fs::read_dir(PathBuf::from(sysroot.trim()).join("lib/rustlib"))
        .ok()?
        .flatten()
        .map(|host| host.path().join("bin"))
        .find(|bin| bin.join("rust-lld").is_file())
}
/// An explicit `variable` override, then a Homebrew LLVM install, then the Rust
/// toolchain's own copy.
fn llvm_tool(
    variable: &str,
    homebrew: &str,
    bundled: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = std::env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    if Path::new(homebrew).exists() {
        return Ok(PathBuf::from(homebrew));
    }
    toolchain_bin()
        .map(|bin| bin.join(bundled))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            format!("{bundled} not found; run `rustup component add llvm-tools` or set {variable}")
                .into()
        })
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
/// Patches in catalog order. Output never depends on the order of `--patch` flags.
const PATCHES: [&str; 3] = ["knob-dialog", "midi-sync", "din-mixer-control"];
const FIRST_PAYLOAD: u32 = 0x80108000;
const SLOT_END: u32 = 0x801a2ff8;
const USAGE: &str = "Usage: firmware-patch INPUT_UNPACKED_DIR OUTPUT_DIR [--patch NAME]... [--update-version FOUR_DIGITS]\n\
Patches: knob-dialog (default), midi-sync, din-mixer-control. Repeat --patch to combine them.";

struct Options {
    source: PathBuf,
    output: PathBuf,
    patches: Vec<&'static str>,
    version: String,
}
fn parse_args() -> Result<Options, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(source), Some(output)) = (args.next(), args.next()) else {
        return Err(USAGE.into());
    };
    let mut requested = Vec::new();
    let mut version = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--patch" => requested.push(
                *PATCHES
                    .iter()
                    .find(|name| **name == value)
                    .ok_or_else(|| format!("unknown patch {value}\n{USAGE}"))?,
            ),
            "--update-version" => version = Some(value),
            _ => return Err(USAGE.into()),
        }
    }
    let patches = if requested.is_empty() {
        vec!["knob-dialog"]
    } else {
        PATCHES
            .into_iter()
            .filter(|n| requested.contains(n))
            .collect()
    };
    // The original knob-only build keeps its published marker; anything else is newer.
    let version = version.unwrap_or_else(|| {
        if patches == ["knob-dialog"] {
            "0112"
        } else {
            "0113"
        }
        .into()
    });
    if version.len() != 4 || !version.bytes().all(|b| b.is_ascii_digit()) {
        return Err("update version must be exactly four ASCII digits, e.g. 0112".into());
    }
    Ok(Options {
        source: PathBuf::from(source),
        output: PathBuf::from(output),
        patches,
        version,
    })
}
/// Guard a hook table against the supplied image; returns its parsed rows.
fn read_hooks(
    firmware: &[u8],
    root: &Path,
) -> Result<Vec<(u32, String, usize, String)>, Box<dyn std::error::Error>> {
    let mut hooks = Vec::new();
    for line in fs::read_to_string(root.join("hooks.tsv"))?
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
        hooks.push((
            address,
            columns[1].to_string(),
            length,
            columns[3].to_string(),
        ));
    }
    Ok(hooks)
}
/// Compile one patch's payload at `base`; returns its bytes and symbol table.
fn compile(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let elf = output.join(format!("{name}.payload.elf"));
    let blob = output.join(format!("{name}.payload.bin"));
    run(Command::new("rustc")
        .current_dir(source)
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
        ])
        .arg(format!(
            "linker={}",
            llvm_tool("L6_LD_LLD", HOMEBREW_LLD, "rust-lld")?.display()
        ))
        .arg("-C")
        .arg(format!("link-arg=--defsym=PAYLOAD_BASE={base:#x}"))
        .arg("-C")
        .arg(format!(
            "link-arg=-T{}",
            root.join("linker/link.ld").display()
        ))
        .arg("-o")
        .arg(&elf))?;
    let symbols = run(Command::new(llvm_tool("L6_LLVM_NM", HOMEBREW_NM, "llvm-nm")?).arg(&elf))?;
    run(Command::new(llvm_tool(
        "L6_LLVM_OBJCOPY",
        HOMEBREW_OBJCOPY,
        "llvm-objcopy",
    )?)
    .args(["-O", "binary"])
    .arg(&elf)
    .arg(&blob))?;
    Ok((fs::read(blob)?, symbols))
}
fn symbol(symbols: &str, name: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some(name))
        .ok_or_else(|| format!("missing patch entry: {name}"))?;
    Ok(u32::from_str_radix(line.split_whitespace().next().unwrap(), 16)? & !1)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_args()?;
    let source = options.source.canonicalize()?;
    if options.output.exists() {
        return Err("output must be a new directory".into());
    }
    let version = options.version.as_str();
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
    if original_length > 0x108000 {
        return Err("payload would overwrite original firmware".into());
    }
    // Validate every hook region before compiling or modifying the output image.
    let roots = options
        .patches
        .iter()
        .map(|name| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../patches")
                .join(name)
                .canonicalize()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let tables = roots
        .iter()
        .map(|root| read_hooks(&firmware, root))
        .collect::<Result<Vec<_>, _>>()?;
    let mut claimed = Vec::new();
    for (address, name, length, _) in tables.iter().flatten() {
        if claimed
            .iter()
            .any(|&(a, l): &(u32, usize)| *address < a + l as u32 && a < *address + *length as u32)
        {
            return Err(format!("hook overlaps another patch: {name}").into());
        }
        claimed.push((*address, *length));
    }
    fs::create_dir_all(&options.output)?;
    let output = options.output.canonicalize()?;
    let padding = fs::read(source.join("padding_before_secondary.bin"))?;
    // Payloads are placed back to back, each aligned to 16 bytes.
    let mut end = FIRST_PAYLOAD;
    let mut applied = Vec::new();
    let mut knob_report = serde_json::Map::new();
    for (index, (name, root)) in options.patches.iter().zip(&roots).enumerate() {
        let base = end;
        let (payload, symbols) = compile(&source, root, &output, name, base)?;
        let hooks = &tables[index];
        if base + payload.len() as u32 > SLOT_END {
            return Err("patch exceeds observed application slot".into());
        }
        let offset = (base - 0x80000000) as usize;
        let consumed = offset + payload.len() - original_length;
        if consumed > padding.len() || padding[..consumed].iter().any(|&b| b != 0xff) {
            return Err("payload would overwrite non-erased reserved bytes".into());
        }
        firmware.resize(offset, 0xff);
        firmware.extend(&payload);
        end = (base + payload.len() as u32 + 15) & !15;
        let mut applied_hooks = Vec::new();
        if *name == "knob-dialog" {
            firmware[0x36c74..0x36c78].copy_from_slice(&branch(
                0x80036c74,
                symbol(&symbols, "knob_changed")?,
                true,
            ));
        }
        for (address, hook, length, hash) in hooks {
            let offset = (address - 0x80000000) as usize;
            let target = symbol(&symbols, hook)?;
            firmware[offset..offset + 4].copy_from_slice(&branch(*address, target, false));
            for chunk in firmware[offset + 4..offset + length].chunks_exact_mut(2) {
                chunk.copy_from_slice(&[0, 0xbf]);
            }
            applied_hooks.push(serde_json::json!({"address":address,"symbol":hook,"target":target,"displaced_length":length,"displaced_sha256":hash}));
        }
        if *name == "knob-dialog" {
            for (address, call, target) in [
                (0x8000648e, "notification_initialize_hook", 0x8005f600),
                (0x80006126, "semaphore_take_hook", 0x80091048),
                (0x800062c0, "semaphore_give_hook", 0x80090ad0),
            ] {
                let offset = (address - 0x80000000) as usize;
                if firmware[offset..offset + 4] != branch(address, target, true) {
                    return Err(format!("call guard failed: {call}").into());
                }
                let redirect = symbol(&symbols, call)?;
                firmware[offset..offset + 4].copy_from_slice(&branch(address, redirect, true));
                applied_hooks
                    .push(serde_json::json!({"address":address,"symbol":call,"target":redirect}));
            }
            knob_report.insert(
                "knob_changed".into(),
                symbol(&symbols, "knob_changed")?.into(),
            );
            knob_report.insert(
                "notification_tick".into(),
                symbol(&symbols, "notification_tick_hook")?.into(),
            );
        }
        applied.push(serde_json::json!({"name":name,"payload_address":format!("{base:#x}"),"payload_length":payload.len(),"hooks":applied_hooks}));
    }
    let consumed = firmware.len() - original_length;
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
    let mut report = serde_json::json!({"source_sha256":digest,"patched_sha256":format!("{:x}",Sha256::digest(&firmware)),"image_length":firmware.len(),"payload_address":format!("{FIRST_PAYLOAD:#x}"),"patches":applied,"update_package":"L6max.bin","update_sha256":format!("{:x}",Sha256::digest(&packed)),"version":String::from_utf8_lossy(&packed[0x1a31fc..0x1a3200]),"usage":"Repacked update for emulator validation; physical bootloader acceptance unverified"});
    if options.patches.contains(&"knob-dialog") {
        let fields = report.as_object_mut().unwrap();
        fields.extend(knob_report);
        fields.insert("hooks".into(), applied[0]["hooks"].clone());
        fields.insert("synchronization".into(), "existing counting semaphore with heap sidecar for same-task nesting; kernel receives untagged handle".into());
        fields.insert("knob_presentation".into(), "coalesced copied value serviced by notification callback; mixer hook never waits for rendering".into());
        fields.insert(
            "state_storage".into(),
            "original firmware heap, allocated per popup and released on restoration".into(),
        );
    }
    fs::write(
        output.join("patch.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "{} bytes of Rust payload ({}); patched firmware: {}",
        firmware.len() - 0x108000,
        options.patches.join(" + "),
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

#[cfg(test)]
#[path = "../../../patches/midi-sync/src/parameters.rs"]
mod parameters;
