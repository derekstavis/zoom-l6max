use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};

const L6MAX_FORMAT: &str = "l6max-observed-v1";
const L6_FORMAT: &str = "l6-observed-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    L6,
    L6max,
}

#[derive(Clone, Copy)]
struct Profile {
    model: Model,
    label: &'static [u8],
    format: &'static str,
    total_size: usize,
    board_mask: u32,
    main_base: u32,
    main_trailer: usize,
    panel_start: usize,
    panel_trailer: usize,
}

const L6: Profile = Profile {
    model: Model::L6,
    label: b"L6",
    format: L6_FORMAT,
    total_size: 0x395200,
    board_mask: 1,
    main_base: 0x80001000,
    main_trailer: 0x2851f8,
    panel_start: 0x285200,
    panel_trailer: 0x2951f8,
};
const L6MAX: Profile = Profile {
    model: Model::L6max,
    label: b"L6max",
    format: L6MAX_FORMAT,
    total_size: 0x1a8200,
    board_mask: 0x40,
    main_base: 0x80000000,
    main_trailer: 0x1a31f8,
    panel_start: 0x1a3200,
    panel_trailer: 0x1a71f8,
};

fn segments_for(profile: Profile, main_length: usize) -> Vec<(&'static str, usize, usize)> {
    let (main_trailer, panel_start, panel_trailer, total_size) = (
        profile.main_trailer,
        profile.panel_start,
        profile.panel_trailer,
        profile.total_size,
    );
    let panel_end = panel_trailer + 8;
    let package_footer = total_size - 0x40;
    if profile.model == Model::L6max {
        vec![
            ("system_header.bin", 0x000000, 0x000100),
            ("main_header.bin", 0x000100, 0x000200),
            ("main_firmware.bin", 0x000200, 0x200 + main_length),
            (
                "padding_before_secondary.bin",
                0x200 + main_length,
                main_trailer,
            ),
            ("main_trailer.bin", main_trailer, panel_start),
            ("secondary_firmware.bin", panel_start, 0x1a6cd8),
            ("padding_after_secondary.bin", 0x1a6cd8, panel_trailer),
            ("secondary_trailer.bin", panel_trailer, panel_end),
            ("final_padding.bin", panel_end, package_footer),
            ("package_footer.bin", package_footer, total_size),
        ]
    } else {
        vec![
            ("system_header.bin", 0, 0x100),
            ("main_header.bin", 0x100, 0x200),
            ("main_firmware.bin", 0x200, 0x200 + main_length),
            (
                "padding_before_secondary.bin",
                0x200 + main_length,
                main_trailer,
            ),
            ("main_trailer.bin", main_trailer, panel_start),
            ("secondary_firmware.bin", panel_start, 0x288680),
            ("padding_after_secondary.bin", 0x288680, panel_trailer),
            ("secondary_trailer.bin", panel_trailer, panel_end),
            ("resource_region.bin", panel_end, package_footer),
            ("package_footer.bin", package_footer, total_size),
        ]
    }
}

/// The package's segment layout for a given main-image length — offsets are
/// fixed, only `main_firmware.bin`'s end (and everything after it, up to the
/// trailer) shifts. Exposed so callers that resize `main_firmware.bin`
/// in-memory (rather than through `unpack`/`repack`'s own files) can recompute
/// a manifest's `size`/`offset` fields for the two affected segments.
pub fn layout(main_length: usize) -> [(&'static str, usize, usize); 10] {
    let v = segments_for(L6MAX, main_length);
    v.try_into().expect("L6max has ten package segments")
}

pub fn layout_for(model: Model, main_length: usize) -> Vec<(&'static str, usize, usize)> {
    segments_for(profile(model), main_length)
}

fn profile(model: Model) -> Profile {
    match model {
        Model::L6 => L6,
        Model::L6max => L6MAX,
    }
}

fn detect(data: &[u8]) -> Result<Profile, String> {
    if data.starts_with(b"L6max System Data") {
        Ok(L6MAX)
    } else if data.starts_with(b"L6 System Data") {
        Ok(L6)
    } else {
        Err("unknown system header (expected L6 or L6max)".into())
    }
}

/// Identify a complete update package from its system header, after checking
/// its layout and the MAIN payload byte-sum checksum.
pub fn model_from_package(data: &[u8]) -> Result<Model, String> {
    check_layout(data)?;
    let actual = data[0x200..]
        .iter()
        .fold(0u32, |sum, &byte| sum.wrapping_add(u32::from(byte)));
    if u32::from_be_bytes(data[0x1fc..0x200].try_into().unwrap()) != actual {
        return Err("MAIN payload byte-sum checksum mismatch".into());
    }
    Ok(detect(data)?.model)
}

/// Identify an extracted MAIN image from its reset vector. This is also the
/// discriminator used by QEMU at boot: L6 starts at 0x80001000 and L6max at
/// 0x80000000.
pub fn model_from_main(data: &[u8]) -> Result<Model, String> {
    if data.len() < 8 {
        return Err("MAIN image is too short to contain a vector table".into());
    }
    let reset = u32le(data, 4);
    let stack = u32le(data, 0);
    if !(0x20000000..0x40000000).contains(&stack) || reset & 1 == 0 {
        return Err(format!(
            "invalid MAIN vector table ({stack:#010x}, {reset:#010x})"
        ));
    }
    if (0x80001000..0x80200000).contains(&(reset & !1)) {
        Ok(Model::L6)
    } else if (0x80000000..0x80001000).contains(&(reset & !1)) {
        Ok(Model::L6max)
    } else {
        Err(format!("unknown MAIN reset vector {reset:#010x}"))
    }
}

/// Identify the firmware image installed in the simulator's 2 MiB MAIN NOR.
/// The application image starts at offset 0x50000 in the device flash; this
/// is the image QEMU executes when persistent device state is enabled.
pub fn model_from_main_nor(data: &[u8]) -> Result<Model, String> {
    const MAIN_IMAGE_OFFSET: usize = 0x50000;
    let main = data
        .get(MAIN_IMAGE_OFFSET..)
        .ok_or("MAIN NOR is too short to contain the installed firmware")?;
    model_from_main(main)
}

fn profile_for_manifest(manifest: &Value) -> Result<Profile, String> {
    match manifest["format"].as_str() {
        Some(L6_FORMAT) => Ok(L6),
        Some(L6MAX_FORMAT) => Ok(L6MAX),
        _ => Err("unsupported manifest format".into()),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn u32le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub fn check_layout(data: &[u8]) -> Result<(), String> {
    let p = detect(data)?;
    if data.len() != p.total_size {
        return Err(format!(
            "{}: expected {} bytes, got {}",
            String::from_utf8_lossy(p.label),
            p.total_size,
            data.len()
        ));
    }
    let mut main_label = p.label.to_vec();
    main_label.extend_from_slice(b" Main Data");
    if !data[0x100..].starts_with(&main_label) {
        return Err("missing MAIN header".into());
    }
    if u32le(data, 0x60) != 2
        || u32le(data, 0x64) != p.board_mask
        || &data[0x68..0x6c] != b"BOOT"
        || &data[0x74..0x78] != b"MAIN"
        || u32le(data, 0x6c) != 0
        || u32le(data, 0x70) != 0
        || u32le(data, 0x78) != 0x100
        || u32le(data, 0x7c) != (p.total_size - 0x100) as u32
    {
        return Err("unexpected package table".into());
    }
    let main_length = u32le(data, p.main_trailer) as usize;
    if !(8..=p.main_trailer - 0x200).contains(&main_length)
        || u32le(data, p.total_size - 8) as usize != main_length
    {
        return Err("main length exceeds its slot or disagrees with footer".into());
    }
    let panel_length = u32le(data, p.panel_trailer) as usize;
    if !(8..=p.panel_trailer - p.panel_start).contains(&panel_length) {
        return Err("panel image exceeds its reserved slot".into());
    }
    for (start, end, trailer, base) in [
        (0x200, 0x200 + main_length, p.main_trailer, p.main_base),
        (
            p.panel_start,
            p.panel_start + panel_length,
            p.panel_trailer,
            0x08000000u32,
        ),
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
    if data[p.main_trailer + 4..p.main_trailer + 8] != data[p.total_size - 4..p.total_size]
        || !data[p.panel_trailer + 4..p.panel_trailer + 8]
            .iter()
            .all(u8::is_ascii_digit)
    {
        return Err("main version markers disagree".into());
    }
    Ok(())
}

/// Splits a validated package into its named segments and a fresh manifest,
/// without touching the filesystem. Shared by `unpack` (writes files) and
/// `unpack_bytes` (keeps everything in memory, for the in-browser patcher).
fn unpack_core(data: &[u8]) -> Result<(Value, Vec<(&'static str, Vec<u8>)>), String> {
    check_layout(data)?;
    let p = detect(data)?;
    let mut entries = Vec::new();
    let mut parts = Vec::new();
    for (name, start, end) in segments_for(p, u32le(data, p.main_trailer) as usize) {
        let part = data[start..end].to_vec();
        entries.push(
            json!({"file":name,"offset":start,"size":end-start,"original_sha256":sha256(&part)}),
        );
        parts.push((name, part));
    }
    let manifest = json!({"format":p.format,"model":if p.model == Model::L6 {"L6"} else {"L6max"},"original_sha256":sha256(data),"segments":entries});
    Ok((manifest, parts))
}

pub fn unpack(source: &Path, out: &Path) -> Result<(), String> {
    let data = fs::read(source).map_err(|e| e.to_string())?;
    let (manifest, parts) = unpack_core(&data)?;
    fs::create_dir(out).map_err(|e| e.to_string())?;
    for (name, part) in &parts {
        fs::write(out.join(name), part).map_err(|e| e.to_string())?;
    }
    fs::write(
        out.join("manifest.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?
        ),
    )
    .map_err(|e| e.to_string())?;
    println!("Unpacked {} into {}", source.display(), out.display());
    let p = detect(&data)?;
    println!(
        "Model: {}",
        if p.model == Model::L6 { "L6" } else { "L6max" }
    );
    println!(
        "Main version: {}",
        String::from_utf8_lossy(&data[p.main_trailer + 4..p.main_trailer + 8])
    );
    println!(
        "Panel version: {}",
        String::from_utf8_lossy(
            &data[detect(&data)?.panel_trailer + 4..detect(&data)?.panel_trailer + 8]
        )
    );
    Ok(())
}

/// In-memory equivalent of `unpack`: validates `data` and returns its fresh
/// manifest plus a name → bytes map of every segment, with nothing written to
/// disk. Used by the WASM patcher, which never has a filesystem.
pub fn unpack_bytes(
    data: &[u8],
) -> Result<(Value, std::collections::BTreeMap<String, Vec<u8>>), String> {
    let (manifest, parts) = unpack_core(data)?;
    Ok((
        manifest,
        parts.into_iter().map(|(n, b)| (n.to_string(), b)).collect(),
    ))
}

/// Assembles a package from its manifest and segments, reading each named
/// segment through `read`. Shared by `repack` (reads files) and
/// `repack_bytes` (reads from an in-memory map). Returns the assembled bytes
/// and the list of segments that changed since the manifest's own
/// `original_sha256` was recorded.
fn repack_core(
    manifest: &Value,
    read: impl Fn(&str) -> Result<Vec<u8>, String>,
    recompute_checksum: bool,
) -> Result<(Vec<u8>, Vec<&'static str>), String> {
    let p = profile_for_manifest(manifest)?;
    if manifest["model"] != if p.model == Model::L6 { "L6" } else { "L6max" } {
        return Err("manifest model and format disagree".into());
    }
    let entries = manifest["segments"]
        .as_array()
        .ok_or("manifest has no segment list")?;
    let trailer = read("main_trailer.bin")?;
    if trailer.len() != 8 {
        return Err("main trailer must have eight bytes".into());
    }
    let main_length = u32le(&trailer, 0) as usize;
    if !(8..=p.main_trailer - 0x200).contains(&main_length) {
        return Err("main image exceeds its reserved slot".into());
    }
    let layout = segments_for(p, main_length);
    if entries.len() != layout.len() {
        return Err("manifest segment list does not match this layout".into());
    }
    let mut assembled = Vec::with_capacity(p.total_size);
    let mut changed = Vec::new();
    for (entry, (name, start, end)) in entries.iter().zip(layout) {
        if entry["file"] != name
            || entry["offset"].as_u64() != Some(start as u64)
            || entry["size"].as_u64() != Some((end - start) as u64)
        {
            return Err(format!("manifest mismatch for {name}"));
        }
        let part = read(name)?;
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
    Ok((assembled, changed))
}

pub fn repack(input: &Path, output: &Path, recompute_checksum: bool) -> Result<(), String> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(input.join("manifest.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let (assembled, changed) = repack_core(
        &manifest,
        |name| fs::read(input.join(name)).map_err(|e| e.to_string()),
        recompute_checksum,
    )?;
    let digest = sha256(&assembled);
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

/// In-memory equivalent of `repack`: assembles the final package bytes from a
/// manifest and a name → bytes map, with nothing read from or written to
/// disk. Used by the WASM patcher.
pub fn repack_bytes(
    manifest: &Value,
    segments: &std::collections::BTreeMap<String, Vec<u8>>,
    recompute_checksum: bool,
) -> Result<Vec<u8>, String> {
    let (assembled, _changed) = repack_core(
        manifest,
        |name| {
            segments
                .get(name)
                .cloned()
                .ok_or_else(|| format!("missing segment {name}"))
        },
        recompute_checksum,
    )?;
    Ok(assembled)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(model: Model, main_length: usize) -> Vec<u8> {
        let p = profile(model);
        let mut data = vec![0xff; p.total_size];
        let mut system = p.label.to_vec();
        system.extend_from_slice(b" System Data");
        let mut main = p.label.to_vec();
        main.extend_from_slice(b" Main Data");
        data[..system.len()].copy_from_slice(&system);
        data[0x100..0x100 + main.len()].copy_from_slice(&main);
        for (offset, value) in [
            (0x60, 2u32),
            (0x64, p.board_mask),
            (0x6c, 0),
            (0x70, 0),
            (0x78, 0x100),
            (0x7c, (p.total_size - 0x100) as u32),
            (0x200, 0x20220000),
            (0x204, p.main_base + 9),
            (p.main_trailer, main_length as u32),
            (p.panel_start, 0x20002000),
            (p.panel_start + 4, 0x08000009),
            (p.panel_trailer, 0x40),
            (p.total_size - 8, main_length as u32),
        ] {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        data[0x68..0x6c].copy_from_slice(b"BOOT");
        data[0x74..0x78].copy_from_slice(b"MAIN");
        data[p.main_trailer + 4..p.main_trailer + 8].copy_from_slice(b"0110");
        data[p.panel_trailer + 4..p.panel_trailer + 8].copy_from_slice(b"0100");
        data[p.total_size - 4..].copy_from_slice(b"0110");
        if model == Model::L6 {
            data[p.panel_trailer + 8..p.panel_trailer + 16].copy_from_slice(b"RESOURCE");
        }
        let sum = data[0x200..]
            .iter()
            .fold(0u32, |s, &b| s.wrapping_add(b as u32));
        data[0x1fc..0x200].copy_from_slice(&sum.to_be_bytes());
        data
    }

    #[test]
    fn both_models_roundtrip_in_memory_and_keep_model_metadata() {
        for model in [Model::L6, Model::L6max] {
            let data = fixture(model, 0x40);
            check_layout(&data).unwrap();
            assert_eq!(model_from_package(&data).unwrap(), model);
            let (manifest, parts) = unpack_bytes(&data).unwrap();
            assert_eq!(
                manifest["model"],
                if model == Model::L6 { "L6" } else { "L6max" }
            );
            assert_eq!(repack_bytes(&manifest, &parts, true).unwrap(), data);
            if model == Model::L6 {
                assert_eq!(&parts["resource_region.bin"][..8], b"RESOURCE");
            }
        }
    }

    #[test]
    fn both_models_roundtrip_on_disk() {
        for model in [Model::L6, Model::L6max] {
            let id = if model == Model::L6 { "l6" } else { "l6max" };
            let root = std::env::temp_dir().join(format!("l6fw-{id}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir(&root).unwrap();
            let source = root.join("source.bin");
            let unpacked = root.join("parts");
            let packed = root.join("roundtrip.bin");
            let bytes = fixture(model, 0x40);
            fs::write(&source, &bytes).unwrap();
            unpack(&source, &unpacked).unwrap();
            repack(&unpacked, &packed, true).unwrap();
            assert_eq!(fs::read(&packed).unwrap(), bytes);
            assert!(repack(&unpacked, &packed, true).is_err());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn l6max_resized_main_still_repackages_for_patch_callers() {
        let original = fixture(Model::L6max, 0x40);
        let (mut manifest, mut parts) = unpack_bytes(&original).unwrap();
        let grown = 0x50;
        parts
            .get_mut("main_firmware.bin")
            .unwrap()
            .extend_from_slice(&[0xaa; 16]);
        for (name, start, end) in layout(grown) {
            let index = manifest["segments"]
                .as_array()
                .unwrap()
                .iter()
                .position(|e| e["file"] == name)
                .unwrap();
            manifest["segments"][index]["offset"] = json!(start);
            manifest["segments"][index]["size"] = json!(end - start);
        }
        parts.insert(
            "padding_before_secondary.bin".into(),
            parts["padding_before_secondary.bin"][16..].to_vec(),
        );
        let mut trailer = parts["main_trailer.bin"].clone();
        trailer[..4].copy_from_slice(&(grown as u32).to_le_bytes());
        parts.insert("main_trailer.bin".into(), trailer);
        let mut footer = parts["package_footer.bin"].clone();
        footer[0x38..0x3c].copy_from_slice(&(grown as u32).to_le_bytes());
        parts.insert("package_footer.bin".into(), footer);
        let assembled = repack_bytes(&manifest, &parts, true).unwrap();
        assert_eq!(u32le(&assembled, L6MAX.main_trailer) as usize, grown);
        check_layout(&assembled).unwrap();
    }

    #[test]
    fn rejects_wrong_masks_lengths_and_vector_addresses() {
        for model in [Model::L6, Model::L6max] {
            let p = profile(model);
            let good = fixture(model, 0x40);
            assert!(check_layout(&good).is_ok());
            let mut bad = good.clone();
            bad[0x64..0x68].copy_from_slice(&0u32.to_le_bytes());
            assert!(check_layout(&bad).is_err());
            let mut bad = good.clone();
            bad[0x204..0x208].copy_from_slice(&0x80000008u32.to_le_bytes());
            assert!(check_layout(&bad).is_err());
            let mut bad = good.clone();
            bad[p.total_size - 8..p.total_size - 4].copy_from_slice(&0x99u32.to_le_bytes());
            assert!(check_layout(&bad).is_err());
            assert!(check_layout(&good[..good.len() - 1]).is_err());
        }
    }

    #[test]
    fn identifies_model_from_extracted_main_reset_vector() {
        for (model, reset) in [(Model::L6, 0x80001415u32), (Model::L6max, 0x80000415u32)] {
            let mut image = vec![0; 8];
            image[..4].copy_from_slice(&0x20220000u32.to_le_bytes());
            image[4..8].copy_from_slice(&reset.to_le_bytes());
            assert_eq!(model_from_main(&image).unwrap(), model);
        }
    }

    #[test]
    fn identifies_model_from_installed_main_nor_image() {
        let mut nor = vec![0xff; 0x200000];
        for (model, reset) in [(Model::L6, 0x80001415u32), (Model::L6max, 0x80000415u32)] {
            nor[0x50000..0x50004].copy_from_slice(&0x20220000u32.to_le_bytes());
            nor[0x50004..0x50008].copy_from_slice(&reset.to_le_bytes());
            assert_eq!(model_from_main_nor(&nor).unwrap(), model);
        }
        assert!(model_from_main_nor(&nor[..0x50007]).is_err());
    }

    #[test]
    fn main_model_detection_enforces_vector_boundaries() {
        for (model, reset) in [
            (Model::L6max, 0x80000fffu32),
            (Model::L6, 0x80001001u32),
            (Model::L6, 0x801fffffu32),
        ] {
            let mut image = vec![0; 8];
            image[..4].copy_from_slice(&0x20220000u32.to_le_bytes());
            image[4..8].copy_from_slice(&reset.to_le_bytes());
            assert_eq!(model_from_main(&image).unwrap(), model);
        }
    }

    #[test]
    fn main_model_detection_rejects_invalid_vectors() {
        let mut image = vec![0; 8];
        image[..4].copy_from_slice(&0x20220000u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x80001415u32.to_le_bytes());
        assert!(model_from_main(&image[..7]).is_err());

        for (stack, reset) in [
            (0x1fffffffu32, 0x80001415u32),
            (0x40000000, 0x80001415),
            (0x20220000, 0x80001410),
            (0x20220000, 0x7ffffffd),
            (0x20220000, 0x80200001),
        ] {
            image[..4].copy_from_slice(&stack.to_le_bytes());
            image[4..8].copy_from_slice(&reset.to_le_bytes());
            assert!(
                model_from_main(&image).is_err(),
                "{stack:#010x} {reset:#010x}"
            );
        }
    }
}

pub mod update;
