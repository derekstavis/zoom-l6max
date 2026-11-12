//! Export of guarded runtime modules with per-hook entry offsets.
use firmware_patch_compile::Relocation;
use sha2::{Digest, Sha256};

/// Encode complete guarded hook regions followed by the linked payload.
/// The caller must first apply the existing revision and hook guards.
pub fn build(
    stock: &[u8],
    patched: &[u8],
    base: u32,
    payload_base: u32,
    init: u32,
    regions: &[(u32, usize)],
) -> Result<Vec<u8>, String> {
    let payload_offset = payload_base
        .checked_sub(base)
        .ok_or("payload before image")? as usize;
    if patched.len() < stock.len() || payload_offset < stock.len() || payload_offset > patched.len()
    {
        return Err("invalid payload placement".into());
    }
    if patched[stock.len()..payload_offset]
        .iter()
        .any(|&b| b != 0xff)
    {
        return Err("non-erased bytes before payload".into());
    }
    let mut records = Vec::new();
    let mut ordered = regions.to_vec();
    ordered.sort_unstable_by_key(|&(address, _)| address);
    let count = u32::try_from(ordered.len()).map_err(|_| "too many records")?;
    let mut previous_end = 0;
    for (address, len) in ordered {
        let start = address.checked_sub(base).ok_or("hook precedes image")? as usize;
        let end = start.checked_add(len).ok_or("hook range overflow")?;
        if len == 0 || start < previous_end || end > stock.len() {
            return Err("invalid or overlapping guarded hook region".into());
        }
        if stock[previous_end..start] != patched[previous_end..start] {
            return Err("firmware change is outside guarded hook regions".into());
        }
        let offset = u32::try_from(start).map_err(|_| "offset too large")?;
        let len = u32::try_from(len).map_err(|_| "record too large")?;
        records.extend_from_slice(&offset.to_le_bytes());
        records.extend_from_slice(&len.to_le_bytes());
        records.extend_from_slice(&stock[start..end]);
        records.extend_from_slice(&patched[start..end]);
        previous_end = end;
    }
    if stock[previous_end..] != patched[previous_end..stock.len()] {
        return Err("firmware change is outside guarded hook regions".into());
    }
    let payload = &patched[payload_offset..];
    let mut output = Vec::new();
    output.extend_from_slice(b"L6SB");
    output.extend_from_slice(&4u32.to_le_bytes());
    output.extend_from_slice(&base.to_le_bytes());
    output.extend_from_slice(
        &u32::try_from(stock.len())
            .map_err(|_| "image too large")?
            .to_le_bytes(),
    );
    output.extend_from_slice(&payload_base.to_le_bytes());
    output.extend_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_| "payload too large")?
            .to_le_bytes(),
    );
    output.extend_from_slice(&count.to_le_bytes());
    output.extend_from_slice(&Sha256::digest(stock));
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&init.to_le_bytes());
    output.extend_from_slice(&records);
    output.extend_from_slice(payload);
    let checksum = patch_loader_core::bundle::checksum(&output).ok_or("short header")?;
    output[60..64].copy_from_slice(&checksum.to_le_bytes());
    Ok(output)
}

/// Build a writable-RAM module bundle. Each record stores a guarded stock hook
/// region and the linked payload entry offset in its replacement field; the
/// installed persistent loader publishes that entry through its RAM dispatch
/// table instead of writing firmware flash.
#[allow(dead_code)]
pub fn build_dynamic(
    stock: &[u8],
    base: u32,
    payload_base: u32,
    payload: &[u8],
    init_offset: u32,
    handlers: &[(u32, Vec<u8>, u32)],
    relocations: &[Relocation],
) -> Result<Vec<u8>, String> {
    if payload.is_empty()
        || init_offset & 1 != 0
        || (init_offset != 0 && init_offset as usize >= payload.len())
    {
        return Err("invalid dynamic module payload or initializer".into());
    }
    let mut ordered = handlers.to_vec();
    ordered.sort_unstable_by_key(|item| item.0);
    let mut records = Vec::new();
    let mut previous_end = 0usize;
    for (address, original, entry_offset) in ordered {
        let start = address
            .checked_sub(base)
            .ok_or("dynamic hook precedes firmware image")? as usize;
        let end = start
            .checked_add(original.len())
            .ok_or("dynamic hook range overflow")?;
        if original.len() < 4
            || start < previous_end
            || end > stock.len()
            || stock[start..end] != original
            || entry_offset as usize >= payload.len()
        {
            return Err("invalid, overlapping or unguarded dynamic hook".into());
        }
        records.extend_from_slice(&(start as u32).to_le_bytes());
        records.extend_from_slice(&(original.len() as u32).to_le_bytes());
        records.extend_from_slice(&original);
        records.extend_from_slice(&entry_offset.to_le_bytes());
        records.resize(records.len() + original.len() - 4, 0);
        previous_end = end;
    }
    let payload_len = u32::try_from(payload.len()).map_err(|_| "payload too large")?;
    let mut relocation_table = Vec::with_capacity(relocations.len() * 12);
    let mut relocation_sites = Vec::with_capacity(relocations.len() * 2);
    for relocation in relocations {
        let (kind, first, second) = match relocation {
            Relocation::Abs32 { offset } => (1u32, *offset, 0u32),
            Relocation::ThumbMov32 {
                low_offset,
                high_offset,
            } => (2u32, *low_offset, *high_offset),
        };
        let sites = if kind == 1 {
            [(first, 4), (0, 0)]
        } else {
            [(first, 4), (second, 4)]
        };
        for (offset, width) in sites.into_iter().filter(|(_, width)| *width != 0) {
            let end = offset
                .checked_add(width)
                .ok_or("relocation range overflow")?;
            if end > payload_len || offset & 1 != 0 {
                return Err("relocation outside payload or misaligned".into());
            }
            relocation_sites.push((offset, end));
        }
        if kind == 2 && first.abs_diff(second) > 16 {
            return Err("distant MOVW/MOVT relocation pair".into());
        }
        relocation_table.extend_from_slice(&kind.to_le_bytes());
        relocation_table.extend_from_slice(&first.to_le_bytes());
        relocation_table.extend_from_slice(&second.to_le_bytes());
    }
    relocation_sites.sort_unstable();
    if relocation_sites
        .windows(2)
        .any(|pair| pair[0].1 > pair[1].0)
    {
        return Err("overlapping relocations".into());
    }
    let mut output = Vec::new();
    output.extend_from_slice(b"L6SB");
    output.extend_from_slice(&6u32.to_le_bytes());
    output.extend_from_slice(&base.to_le_bytes());
    output.extend_from_slice(&(stock.len() as u32).to_le_bytes());
    output.extend_from_slice(&payload_base.to_le_bytes());
    output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    output.extend_from_slice(&(handlers.len() as u32).to_le_bytes());
    output.extend_from_slice(&Sha256::digest(stock));
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&init_offset.to_le_bytes());
    output.extend_from_slice(
        &u32::try_from(relocations.len())
            .map_err(|_| "too many relocations")?
            .to_le_bytes(),
    );
    output.extend_from_slice(&records);
    output.extend_from_slice(&relocation_table);
    output.extend_from_slice(payload);
    let checksum = patch_loader_core::bundle::checksum(&output).ok_or("short header")?;
    output[60..64].copy_from_slice(&checksum.to_le_bytes());
    Ok(output)
}

/// Encode one dynamic module with a streamed code image and a larger writable
/// allocation. Bytes between `code_len` and `memory_len` are zeroed by the
/// loader and are not stored in the SD file.
pub fn build_dynamic_v1(
    payload: &[u8],
    memory_len: usize,
    hooks: &[(u32, u32, u32)],
    relocations: &[Relocation],
) -> Result<Vec<u8>, String> {
    if payload.is_empty()
        || memory_len < payload.len()
        || memory_len > u32::MAX as usize
        || hooks.len() > u16::MAX as usize
        || relocations.len() > u16::MAX as usize
    {
        return Err("invalid dynamic module header values".into());
    }
    let mut ordered_hooks = hooks.to_vec();
    // Stable order is significant when one composed module has several
    // handlers for the same function. Their records form one hook chain.
    ordered_hooks.sort_by_key(|entry| entry.0);
    if ordered_hooks.iter().any(|(address, offset, next_offset)| {
        address & 1 != 0
            || offset & 1 != 0
            || *offset as usize >= payload.len()
            || (*next_offset != u32::MAX
                && (next_offset & 3 != 0 || *next_offset as usize + 4 > payload.len()))
    }) {
        return Err("invalid function-entry hook".into());
    }

    let mut relocation_table = Vec::with_capacity(relocations.len() * 12);
    for relocation in relocations {
        let (kind, first, second) = match relocation {
            Relocation::Abs32 { offset } => (1u32, *offset, 0u32),
            Relocation::ThumbMov32 {
                low_offset,
                high_offset,
            } => (2u32, *low_offset, *high_offset),
        };
        let offsets = if kind == 1 {
            [Some(first), None]
        } else {
            [Some(first), Some(second)]
        };
        for offset in offsets.into_iter().flatten() {
            if offset & 1 != 0 || offset as usize + 4 > payload.len() {
                return Err("relocation outside or misaligned in patch code".into());
            }
        }
        if kind == 2 && (second <= first || second - first > 16) {
            return Err("invalid Thumb MOVW/MOVT relocation pair".into());
        }
        relocation_table.extend_from_slice(&kind.to_le_bytes());
        relocation_table.extend_from_slice(&first.to_le_bytes());
        relocation_table.extend_from_slice(&second.to_le_bytes());
    }

    let mut output = Vec::with_capacity(
        patch_loader_core::dynamic::HEADER_LEN_V1
            + payload.len()
            + hooks.len() * 12
            + relocations.len() * 12,
    );
    output.extend_from_slice(b"L6SB");
    output.extend_from_slice(&patch_loader_core::dynamic::FORMAT_VERSION.to_le_bytes());
    output.extend_from_slice(&patch_loader_core::dynamic::LOADER_ABI.to_le_bytes());
    output.extend_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_| "dynamic module too large")?
            .to_le_bytes(),
    );
    output.extend_from_slice(&(memory_len as u32).to_le_bytes());
    output.extend_from_slice(&(hooks.len() as u16).to_le_bytes());
    output.extend_from_slice(&(relocations.len() as u16).to_le_bytes());
    output.extend_from_slice(&0u32.to_le_bytes()); // reserved
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(payload);
    for (address, entry_offset, next_offset) in ordered_hooks {
        output.extend_from_slice(&address.to_le_bytes());
        output.extend_from_slice(&entry_offset.to_le_bytes());
        output.extend_from_slice(&next_offset.to_le_bytes());
    }
    output.extend_from_slice(&relocation_table);
    let checksum = patch_loader_core::dynamic::checksum(&output).ok_or("short v1 file")?;
    output[24..28].copy_from_slice(&checksum.to_le_bytes());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_guarded_edits_and_fixed_address_payload() {
        let stock = [1, 2, 3, 4];
        let patched = [1, 7, 3, 8, 0xff, 0xff, 9, 10];
        let bundle = build(
            &stock,
            &patched,
            0x1000,
            0x1006,
            0,
            &[(0x1000, 2), (0x1002, 2)],
        )
        .unwrap();
        assert_eq!(&bundle[..4], b"L6SB");
        assert_eq!(u32::from_le_bytes(bundle[24..28].try_into().unwrap()), 2);
        let mut rebuilt = stock.to_vec();
        let mut p = 68;
        for _ in 0..2 {
            let offset = u32::from_le_bytes(bundle[p..p + 4].try_into().unwrap()) as usize;
            let len = u32::from_le_bytes(bundle[p + 4..p + 8].try_into().unwrap()) as usize;
            p += 8;
            assert_eq!(&rebuilt[offset..offset + len], &bundle[p..p + len]);
            rebuilt[offset..offset + len].copy_from_slice(&bundle[p + len..p + 2 * len]);
            p += 2 * len;
        }
        rebuilt.resize(6, 0xff);
        rebuilt.extend_from_slice(&bundle[p..]);
        assert_eq!(rebuilt, patched);
    }
    #[test]
    fn rejects_non_erased_gap() {
        assert!(build(&[1], &[1, 0, 2], 0x1000, 0x1002, 0, &[]).is_err());
    }

    #[test]
    fn emits_v1_module_with_address_hooks() {
        let payload = [0x00, 0xbf, 0x70, 0x47, 0, 0, 0, 0];
        let file = build_dynamic_v1(&payload, 0x48000, &[(0x8000_1000, 0, 4)], &[]).unwrap();
        assert_eq!(file.len(), 28 + payload.len() + 12);
        assert_eq!(&file[..4], b"L6SB");
        assert_eq!(u16::from_le_bytes(file[4..6].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(file[6..8].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(file[8..12].try_into().unwrap()), 8);
        assert_eq!(
            u32::from_le_bytes(file[12..16].try_into().unwrap()),
            0x48000
        );
        assert_eq!(u16::from_le_bytes(file[16..18].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(file[18..20].try_into().unwrap()), 0);
        assert_eq!(&file[28..36], &payload);
        assert_eq!(
            u32::from_le_bytes(file[36..40].try_into().unwrap()),
            0x8000_1000
        );
        assert_eq!(
            patch_loader_core::dynamic::checksum(&file),
            Some(u32::from_le_bytes(file[24..28].try_into().unwrap()))
        );
        let mut corrupt = file;
        corrupt[25] ^= 1;
        assert_ne!(
            patch_loader_core::dynamic::checksum(&corrupt),
            Some(u32::from_le_bytes(corrupt[24..28].try_into().unwrap()))
        );
    }

    #[test]
    fn emits_v1_multi_site_hook_records() {
        let payload = [
            0x00, 0xbf, 0x70, 0x47, 0, 0, 0, 0, 0, 0xbf, 0x70, 0x47, 0, 0, 0, 0,
        ];
        let hooks = [(0x8000_1000, 0, 4), (0x8000_2000, 8, 12)];
        let file = build_dynamic_v1(&payload, 0x48000, &hooks, &[]).unwrap();
        assert_eq!(u16::from_le_bytes(file[16..18].try_into().unwrap()), 2);
        assert_eq!(file.len(), 28 + payload.len() + hooks.len() * 12);
        assert_eq!(
            u32::from_le_bytes(
                file[28 + payload.len()..32 + payload.len()]
                    .try_into()
                    .unwrap()
            ),
            0x8000_1000
        );
        assert_eq!(
            u32::from_le_bytes(
                file[40 + payload.len()..44 + payload.len()]
                    .try_into()
                    .unwrap()
            ),
            0x8000_2000
        );
    }

    #[test]
    fn preserves_composed_handler_order_for_same_function() {
        let payload = [0u8; 16];
        let hooks = [(0x8000_1000, 8, 12), (0x8000_1000, 0, 4)];
        let file = build_dynamic_v1(&payload, payload.len(), &hooks, &[]).unwrap();
        let first = patch_loader_core::dynamic::HEADER_LEN_V1 + payload.len();
        let second = first + patch_loader_core::dynamic::HOOK_RECORD_LEN;

        assert_eq!(
            u32::from_le_bytes(file[first..first + 4].try_into().unwrap()),
            0x8000_1000
        );
        assert_eq!(
            u32::from_le_bytes(file[first + 4..first + 8].try_into().unwrap()),
            8
        );
        assert_eq!(
            u32::from_le_bytes(file[second + 4..second + 8].try_into().unwrap()),
            0
        );
    }

    #[test]
    fn emits_v1_state_length_without_storing_zero_fill() {
        let payload = [0x00, 0xbf, 0x70, 0x47];
        let file = build_dynamic_v1(&payload, 0x48000, &[(0x8000_1000, 0, 0)], &[]).unwrap();
        assert_eq!(file.len(), 28 + payload.len() + 12);
        assert_eq!(
            u16::from_le_bytes(file[4..6].try_into().unwrap()),
            patch_loader_core::dynamic::FORMAT_VERSION
        );
        assert_eq!(u32::from_le_bytes(file[8..12].try_into().unwrap()), 4);
        assert_eq!(
            u32::from_le_bytes(file[12..16].try_into().unwrap()),
            0x48000
        );
        assert_eq!(&file[28..32], &payload);
        assert_eq!(
            patch_loader_core::dynamic::checksum(&file),
            Some(u32::from_le_bytes(file[24..28].try_into().unwrap()))
        );
    }

    #[test]
    fn rejects_uncovered_changes_and_overlapping_guards() {
        let stock = [1, 2, 3, 4];
        let patched = [1, 7, 3, 8, 0xff, 0xff, 9, 10];
        assert!(build(&stock, &patched, 0x1000, 0x1006, 0, &[(0x1000, 2)]).is_err());
        assert!(
            build(
                &stock,
                &patched,
                0x1000,
                0x1006,
                0,
                &[(0x1000, 3), (0x1002, 2)]
            )
            .is_err()
        );
        assert!(build(&stock, &patched, 0x1000, 0x1006, 0, &[(0x0fff, 4)]).is_err());
    }

    #[test]
    fn v6_bundle_carries_and_applies_module_relocations() {
        use patch_loader_core::bundle::{self, Policy, Relocation as ParsedRelocation};

        let stock = [0u8; 16];
        let mut payload = Vec::new();
        payload.extend_from_slice(&0x2004u32.to_le_bytes());
        // Thumb MOVW r0, #0x2008 followed by MOVT r0, #0.
        payload.extend_from_slice(&[0x42, 0xf2, 0x08, 0x00, 0xc0, 0xf2, 0x00, 0x00]);
        let relocations = [
            Relocation::Abs32 { offset: 0 },
            Relocation::ThumbMov32 {
                low_offset: 4,
                high_offset: 8,
            },
        ];
        let file = build_dynamic(&stock, 0x1000, 0x2000, &payload, 0, &[], &relocations).unwrap();
        let policy = Policy {
            firmware_sha256: Sha256::digest(stock).into(),
            image_base: 0x1000,
            image_len: stock.len(),
            payload_base: 0x2000,
            max_payload: 32,
            max_records: 0,
            max_relocations: 4,
            compare_guards: true,
        };
        let bundle = bundle::validate(&file, &stock, &policy).unwrap();
        assert_eq!(bundle.version(), 6);
        assert_eq!(bundle.relocations().count(), 2);
        let mut loaded = bundle.payload().to_vec();
        assert_eq!(
            bundle::relocate(&mut loaded, bundle.relocations(), 0x2000, 0x3000),
            Ok(())
        );
        assert_eq!(&loaded[..4], &0x3004u32.to_le_bytes());
        assert_eq!(
            bundle.relocations().next(),
            Some(ParsedRelocation::Abs32 { offset: 0 })
        );
        assert_eq!(&loaded[4..8], &[0x43, 0xf2, 0x08, 0x00]);
        assert_eq!(&loaded[8..12], &[0xc0, 0xf2, 0x00, 0x00]);
    }

    #[test]
    fn consumer_validates_guards_and_reconstructs_image() {
        use patch_loader_core::bundle::{self, Error, Policy};
        let stock = [1, 2, 3, 4];
        let patched = [1, 7, 3, 8, 0xff, 0xff, 9, 10];
        let mut file = build(
            &stock,
            &patched,
            0x1000,
            0x1006,
            0,
            &[(0x1000, 2), (0x1002, 2)],
        )
        .unwrap();
        let policy = Policy {
            firmware_sha256: Sha256::digest(stock).into(),
            image_base: 0x1000,
            image_len: stock.len(),
            payload_base: 0x1006,
            max_payload: 2,
            max_records: 2,
            max_relocations: 0,
            compare_guards: true,
        };
        let bundle = bundle::validate(&file, &stock, &policy).unwrap();
        let mut result = stock.to_vec();
        for edit in bundle.edits() {
            result[edit.offset..edit.offset + edit.replacement.len()]
                .copy_from_slice(edit.replacement);
        }
        result.resize(6, 0xff);
        result.extend_from_slice(bundle.payload());
        assert_eq!(result, patched);
        assert!(matches!(
            bundle::validate(&file, &[1, 0, 3, 4], &policy),
            Err(Error::GuardMismatch)
        ));
        // Byte 0 is unchanged by the patch but belongs to its first hook.
        assert!(matches!(
            bundle::validate(&file, &[0, 2, 3, 4], &policy),
            Err(Error::GuardMismatch)
        ));
        file.pop();
        assert!(matches!(
            bundle::validate(&file, &stock, &policy),
            Err(Error::Invalid)
        ));
    }

    #[test]
    fn sequential_reader_handles_short_reads_and_rejects_damage() {
        use patch_loader_core::bundle::{self, Error, Policy, ReadError, Reader};
        struct Chunks<'a>(&'a [u8]);
        impl Reader for Chunks<'_> {
            type Error = ();
            fn read(&mut self, destination: &mut [u8]) -> Result<usize, ()> {
                let n = self.0.len().min(destination.len()).min(3);
                destination[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let stock = [1, 2, 3, 4];
        let patched = [1, 7, 3, 4, 0xff, 0xff, 9, 10];
        let mut file = build(&stock, &patched, 0x1000, 0x1006, 0x1006, &[(0x1000, 2)]).unwrap();
        let policy = Policy {
            firmware_sha256: Sha256::digest(stock).into(),
            image_base: 0x1000,
            image_len: stock.len(),
            payload_base: 0x1006,
            max_payload: 2,
            max_records: 1,
            max_relocations: 0,
            compare_guards: true,
        };
        let mut buffer = [0; 128];
        let bytes = bundle::read_bundle(&mut Chunks(&file), &mut buffer, 2, 1, 0).unwrap();
        assert_eq!(
            bundle::validate(bytes, &stock, &policy).unwrap().init(),
            Some(0x1006)
        );
        assert_eq!(
            bundle::read_bundle(&mut Chunks(&file[..file.len() - 1]), &mut buffer, 2, 1, 0),
            Err(ReadError::Truncated)
        );
        let last = file.len() - 1;
        file[last] ^= 1;
        assert!(matches!(
            bundle::validate(&file, &stock, &policy),
            Err(Error::Invalid)
        ));
        file[last] ^= 1;
        file[64..68].copy_from_slice(&0x1008u32.to_le_bytes());
        let crc = bundle::checksum(&file).unwrap();
        file[60..64].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            bundle::validate(&file, &stock, &policy),
            Err(Error::Invalid)
        ));
    }
}
