use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};

const FORMAT: &str = "l6max-observed-v1";
const TOTAL_SIZE: usize = 0x1a8200;
fn segments(main_length: usize) -> [(&'static str, usize, usize); 10] {
    [
        ("system_header.bin", 0x000000, 0x000100),
        ("main_header.bin", 0x000100, 0x000200),
        ("main_firmware.bin", 0x000200, 0x200 + main_length),
        (
            "padding_before_secondary.bin",
            0x200 + main_length,
            0x1a31f8,
        ),
        ("main_trailer.bin", 0x1a31f8, 0x1a3200),
        ("secondary_firmware.bin", 0x1a3200, 0x1a6cd8),
        ("padding_after_secondary.bin", 0x1a6cd8, 0x1a71f8),
        ("secondary_trailer.bin", 0x1a71f8, 0x1a7200),
        ("final_padding.bin", 0x1a7200, 0x1a81c0),
        ("package_footer.bin", 0x1a81c0, 0x1a8200),
    ]
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn u32le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub fn check_layout(data: &[u8]) -> Result<(), String> {
    if data.len() != TOTAL_SIZE {
        return Err(format!("expected {TOTAL_SIZE} bytes, got {}", data.len()));
    }
    if !data.starts_with(b"L6max System Data") {
        return Err("missing L6max system header".into());
    }
    if !data[0x100..].starts_with(b"L6max Main Data") {
        return Err("missing L6max main header".into());
    }
    if u32le(data, 0x60) != 2
        || u32le(data, 0x64) != 0x40
        || &data[0x68..0x6c] != b"BOOT"
        || &data[0x74..0x78] != b"MAIN"
        || u32le(data, 0x6c) != 0
        || u32le(data, 0x70) != 0
        || u32le(data, 0x78) != 0x100
        || u32le(data, 0x7c) != (TOTAL_SIZE - 0x100) as u32
    {
        return Err("unexpected package table".into());
    }
    let main_length = u32le(data, 0x1a31f8) as usize;
    if !(8..=0x1a2ff8).contains(&main_length) || u32le(data, 0x1a81f8) as usize != main_length {
        return Err("main length exceeds its slot or disagrees with footer".into());
    }
    for (start, end, trailer, base) in [
        (0x200, 0x200 + main_length, 0x1a31f8, 0x80000000u32),
        (0x1a3200, 0x1a6cd8, 0x1a71f8, 0x08000000u32),
    ] {
        if u32le(data, trailer) != (end - start) as u32 {
            return Err(format!("image length at 0x{trailer:x} does not match"));
        }
        let stack = u32le(data, start);
        let reset = u32le(data, start + 4);
        if !(0x20000000..0x40000000).contains(&stack)
            || reset <= base
            || reset >= base + (end - start) as u32
            || reset & 1 == 0
        {
            return Err(format!("no plausible Cortex-M vector table at 0x{start:x}"));
        }
    }
    if data[0x1a31fc..0x1a3200] != data[0x1a81fc..0x1a8200] {
        return Err("main version markers disagree".into());
    }
    Ok(())
}

pub fn unpack(source: &Path, out: &Path) -> Result<(), String> {
    let data = fs::read(source).map_err(|e| e.to_string())?;
    check_layout(&data)?;
    fs::create_dir(out).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for (name, start, end) in segments(u32le(&data, 0x1a31f8) as usize) {
        let part = &data[start..end];
        fs::write(out.join(name), part).map_err(|e| e.to_string())?;
        entries.push(
            json!({"file":name,"offset":start,"size":end-start,"original_sha256":sha256(part)}),
        );
    }
    let manifest = json!({"format":FORMAT,"original_sha256":sha256(&data),"segments":entries});
    fs::write(
        out.join("manifest.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?
        ),
    )
    .map_err(|e| e.to_string())?;
    println!("Unpacked {} into {}", source.display(), out.display());
    println!(
        "Main version: {}",
        String::from_utf8_lossy(&data[0x1a31fc..0x1a3200])
    );
    println!(
        "Secondary version: {}",
        String::from_utf8_lossy(&data[0x1a71fc..0x1a7200])
    );
    Ok(())
}

pub fn repack(input: &Path, output: &Path, recompute_checksum: bool) -> Result<(), String> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(input.join("manifest.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if manifest["format"] != FORMAT {
        return Err("unsupported manifest format".into());
    }
    let entries = manifest["segments"]
        .as_array()
        .ok_or("manifest has no segment list")?;
    let trailer = fs::read(input.join("main_trailer.bin")).map_err(|e| e.to_string())?;
    if trailer.len() != 8 {
        return Err("main trailer must have eight bytes".into());
    }
    let main_length = u32le(&trailer, 0) as usize;
    if !(8..=0x1a2ff8).contains(&main_length) {
        return Err("main image exceeds its reserved slot".into());
    }
    let layout = segments(main_length);
    if entries.len() != layout.len() {
        return Err("manifest segment list does not match this layout".into());
    }
    let mut assembled = Vec::with_capacity(TOTAL_SIZE);
    let mut changed = Vec::new();
    for (entry, (name, start, end)) in entries.iter().zip(layout) {
        if entry["file"] != name
            || entry["offset"].as_u64() != Some(start as u64)
            || entry["size"].as_u64() != Some((end - start) as u64)
        {
            return Err(format!("manifest mismatch for {name}"));
        }
        let part = fs::read(input.join(name)).map_err(|e| e.to_string())?;
        if part.len() != end - start {
            return Err(format!(
                "{name}: expected {} bytes, got {}",
                end - start,
                part.len()
            ));
        }
        if entry["original_sha256"].as_str() != Some(&sha256(&part)) {
            changed.push(name);
        }
        assembled.extend_from_slice(&part);
    }
    check_layout(&assembled)?;
    if recompute_checksum {
        let sum = assembled[0x200..]
            .iter()
            .fold(0u32, |sum, &byte| sum.wrapping_add(byte as u32));
        let checksum = sum.to_be_bytes();
        if assembled[0x1fc..0x200] != checksum && !changed.contains(&"main_header.bin") {
            changed.push("main_header.bin");
        }
        assembled[0x1fc..0x200].copy_from_slice(&checksum);
    }
    let digest = sha256(&assembled);
    if changed.is_empty() && manifest["original_sha256"].as_str() != Some(&digest) {
        return Err("assembled hash disagrees with the manifest".into());
    }
    if output.exists() {
        return Err(format!("refusing to overwrite {}", output.display()));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| e.to_string())?;
    file.write_all(&assembled).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    println!(
        "Wrote {} ({} bytes; SHA-256 {digest})",
        output.display(),
        assembled.len()
    );
    if changed.is_empty() {
        println!("Byte-identical to the original package.");
    } else {
        println!("Changed segments: {}", changed.join(", "));
        if recompute_checksum {
            println!("MAIN byte-sum checksum regenerated. Device acceptance is unverified.");
        } else {
            println!(
                "Payload checksum is preserved; use --recompute-checksum after payload edits. Device acceptance is unverified."
            );
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(length: usize) -> Vec<u8> {
        let mut data = vec![0xff; TOTAL_SIZE];
        data[..17].copy_from_slice(b"L6max System Data");
        data[0x100..0x10f].copy_from_slice(b"L6max Main Data");
        for (offset, value) in [
            (0x60, 2u32),
            (0x64, 0x40),
            (0x6c, 0),
            (0x70, 0),
            (0x78, 0x100),
            (0x7c, 0x1a8100),
            (0x200, 0x20010000),
            (0x204, 0x80001001),
            (0x1a31f8, length as u32),
            (0x1a3200, 0x20008000),
            (0x1a3204, 0x08000101),
            (0x1a71f8, 0x3ad8),
            (0x1a81f8, length as u32),
        ] {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        data[0x68..0x6c].copy_from_slice(b"BOOT");
        data[0x74..0x78].copy_from_slice(b"MAIN");
        data[0x1a31fc..0x1a3200].copy_from_slice(b"0110");
        data[0x1a81fc..].copy_from_slice(b"0110");
        let sum = data[0x200..]
            .iter()
            .fold(0u32, |s, &b| s.wrapping_add(b as u32));
        data[0x1fc..0x200].copy_from_slice(&sum.to_be_bytes());
        data
    }
    #[test]
    fn layout_rejects_growth_into_panel_and_inconsistent_footer() {
        let mut data = fixture(0x107fe8);
        assert!(check_layout(&data).is_ok());
        data[0x1a31f8..0x1a31fc].copy_from_slice(&0x1a3000u32.to_le_bytes());
        assert!(check_layout(&data).is_err());
        data[0x1a31f8..0x1a31fc].copy_from_slice(&0x107fe9u32.to_le_bytes());
        assert!(check_layout(&data).is_err());
    }
    #[test]
    fn original_and_expanded_main_roundtrip_without_changing_package_boundaries() {
        let directory =
            std::env::temp_dir().join(format!("l6-package-roundtrip-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        for length in [0x107fe8, 0x109404] {
            let bytes = fixture(length);
            let source = directory.join(format!("source-{length}.bin"));
            let unpacked = directory.join(format!("unpacked-{length}"));
            let packed = directory.join(format!("packed-{length}.bin"));
            fs::write(&source, &bytes).unwrap();
            unpack(&source, &unpacked).unwrap();
            assert_eq!(
                fs::metadata(unpacked.join("main_firmware.bin"))
                    .unwrap()
                    .len(),
                length as u64
            );
            repack(&unpacked, &packed, true).unwrap();
            assert_eq!(fs::read(&packed).unwrap(), bytes);
            assert!(repack(&unpacked, &packed, true).is_err());
            let mut manifest: Value =
                serde_json::from_slice(&fs::read(unpacked.join("manifest.json")).unwrap()).unwrap();
            manifest["segments"][3]["offset"] = json!(0x1a3200);
            fs::write(
                unpacked.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            assert!(repack(&unpacked, &directory.join("bad.bin"), true).is_err());
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

pub mod update;
