//! Pure, I/O-free firmware-splicing logic shared by the native `l6patch`
//! CLI (which also compiles payloads via `rustc`/`lld`/`llvm-objcopy`, a step
//! that stays native-only) and the browser-based WASM patcher (which applies
//! payloads precompiled ahead of time). Every function here only moves bytes
//! around in memory; none of it touches the filesystem or shells out.
use sha2::{Digest, Sha256};

/// Patches in catalog order. Output never depends on the order patches are requested in.
pub const PATCHES: [&str; 3] = ["knob-dialog", "midi-sync", "din-mixer-control"];
pub const FIRST_PAYLOAD: u32 = 0x80108000;
pub const SLOT_END: u32 = 0x801a2ff8;
/// SHA-256 of the supported stock L6max MAIN firmware image (1.10).
pub const EXPECTED_FIRMWARE_SHA256: &str =
    "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461";
/// SHA-256 of the supported stock L6 MAIN firmware image (1.10).
pub const L6_EXPECTED_FIRMWARE_SHA256: &str =
    "827c7c5cb2a43cc3a011d4556462b8cb9539242f817ebeed7727d66a36c03799";
pub const L6_MAIN_BASE: u32 = 0x80001000;
/// End of the observed L6 MAIN slot, immediately before its trailer area.
pub const L6_SLOT_END: u32 = 0x80285ff8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareModel {
    L6,
    L6max,
}

/// The L6 payload starts immediately after its exact supported MAIN image,
/// aligned for Thumb payload placement. Its large erased package padding
/// supplies the bytes consumed when the MAIN segment grows.
pub fn first_payload_for(model: FirmwareModel, main_length: usize) -> u32 {
    match model {
        FirmwareModel::L6 => {
            let end = L6_MAIN_BASE + main_length as u32;
            (end + 15) & !15
        }
        FirmwareModel::L6max => FIRST_PAYLOAD,
    }
}

/// Encodes a Thumb-2 BL (`link: true`) or B.W (`link: false`) branch from `from` to `to`.
pub fn branch(from: u32, to: u32, link: bool) -> [u8; 4] {
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

/// Detects the model from the MAIN reset vector.
pub fn detect_firmware_model(firmware: &[u8]) -> Result<FirmwareModel, String> {
    let reset_vector = firmware
        .get(4..8)
        .ok_or("firmware is too short to contain a reset vector")?;
    match u32::from_le_bytes(reset_vector.try_into().unwrap()) {
        0x80001415 => Ok(FirmwareModel::L6),
        0x80000415 => Ok(FirmwareModel::L6max),
        value => Err(format!("unsupported firmware reset vector: {value:#010x}")),
    }
}

/// Confirms `firmware` is byte-identical to the supported stock image for its
/// detected model, returning a descriptive error instead of panicking (the
/// inputs — user uploads in the browser or local files for the CLI — are
/// untrusted enough to deserve a clean error, not a crash).
pub fn verify_firmware(firmware: &[u8]) -> Result<(), String> {
    verify_firmware_for(detect_firmware_model(firmware)?, firmware)
}

pub fn verify_firmware_for(model: FirmwareModel, firmware: &[u8]) -> Result<(), String> {
    let digest = format!("{:x}", Sha256::digest(firmware));
    let (expected, reset_vector, min_len) = match model {
        FirmwareModel::L6 => (L6_EXPECTED_FIRMWARE_SHA256, 0x80001415u32, 8),
        FirmwareModel::L6max => (EXPECTED_FIRMWARE_SHA256, 0x80000415u32, 0x410),
    };
    if digest != expected {
        return Err(format!(
            "unsupported main firmware SHA-256: expected {expected}, got {digest}"
        ));
    }
    if firmware.len() < min_len {
        return Err("firmware is too short to be the expected image".into());
    }
    if u32::from_le_bytes(firmware[4..8].try_into().unwrap()) != reset_vector {
        return Err("firmware does not match the expected reset vector".into());
    }
    if model == FirmwareModel::L6 {
        return Ok(());
    }
    if firmware[0x40c..0x410] != 0x8007eea9u32.to_le_bytes() {
        return Err("firmware does not match the expected reset support vector".into());
    }
    if firmware[0x36c74..0x36c78] != branch(0x80036c74, 0x80053b28, true) {
        return Err("firmware does not match the expected knob-dialog call site".into());
    }
    if firmware.len() > 0x108000 {
        return Err("payload would overwrite original firmware".into());
    }
    Ok(())
}

/// One hooked instruction: a branch overwrites the first 4 bytes at `address`,
/// and any remaining bytes up to `length` are filled with Thumb NOPs
/// (`0xbf00`), matching how `l6patch` neutralizes a wider displaced
/// region than the branch itself occupies.
pub struct HookEdit {
    pub address: u32,
    pub target: u32,
    pub length: usize,
}

/// `knob-dialog`'s three `bl` call-site redirects: before patching, the call
/// at `address` must still target `original` (a guard against a stock image
/// that doesn't call what this patch expects), then is rewritten to `target`.
pub struct CallSiteEdit {
    pub address: u32,
    pub original: u32,
    pub target: u32,
}

/// Everything needed to splice one already-compiled patch payload into a
/// firmware image: its hook edits, and (for `knob-dialog` only) the extra
/// entry-point edit and call-site redirects.
pub struct PatchEdits {
    pub hooks: Vec<HookEdit>,
    pub knob_dialog_entry: Option<u32>,
    pub knob_dialog_call_sites: Vec<CallSiteEdit>,
}

impl PatchEdits {
    pub fn new(hooks: Vec<HookEdit>) -> Self {
        Self {
            hooks,
            knob_dialog_entry: None,
            knob_dialog_call_sites: Vec::new(),
        }
    }
}

/// Result of successfully applying one patch: where its payload landed, and
/// the next 16-byte-aligned base address for whatever patch comes after it.
pub struct AppliedPatch {
    pub base: u32,
    pub payload_len: usize,
    pub next_base: u32,
}

/// Places one already-compiled, position-dependent payload at `base` in
/// `firmware` (growing it with `0xff` filler as needed) and applies its hook
/// and call-site edits. `original_length` is the firmware's length before any
/// patch in this run was applied; `padding` is the untouched
/// `padding_before_secondary.bin` content read before any patch grew the
/// image. Mirrors `l6patch`'s per-patch loop exactly, minus compiling.
pub fn apply_patch(
    firmware: &mut Vec<u8>,
    original_length: usize,
    base: u32,
    payload: &[u8],
    edits: &PatchEdits,
    padding: &[u8],
) -> Result<AppliedPatch, String> {
    apply_patch_with_map(
        firmware,
        original_length,
        base,
        payload,
        edits,
        padding,
        0x80000000,
        SLOT_END,
    )
}

pub fn apply_patch_with_map(
    firmware: &mut Vec<u8>,
    original_length: usize,
    base: u32,
    payload: &[u8],
    edits: &PatchEdits,
    padding: &[u8],
    firmware_base: u32,
    slot_end: u32,
) -> Result<AppliedPatch, String> {
    if base + payload.len() as u32 > slot_end {
        return Err("patch exceeds observed application slot".into());
    }
    if base < firmware_base {
        return Err("payload address precedes the loaded MAIN image".into());
    }
    let offset = (base - firmware_base) as usize;
    let consumed = offset + payload.len() - original_length;
    if consumed > padding.len() || padding[..consumed].iter().any(|&b| b != 0xff) {
        return Err("payload would overwrite non-erased reserved bytes".into());
    }
    firmware.resize(offset, 0xff);
    firmware.extend_from_slice(payload);
    if let Some(target) = edits.knob_dialog_entry {
        let off = (0x80036c74 - firmware_base) as usize;
        firmware[off..off + 4].copy_from_slice(&branch(0x80036c74, target, true));
    }
    for hook in &edits.hooks {
        let off = (hook.address - firmware_base) as usize;
        firmware[off..off + 4].copy_from_slice(&branch(hook.address, hook.target, false));
        for chunk in firmware[off + 4..off + hook.length].chunks_exact_mut(2) {
            chunk.copy_from_slice(&[0, 0xbf]);
        }
    }
    for call in &edits.knob_dialog_call_sites {
        let off = (call.address - firmware_base) as usize;
        if firmware[off..off + 4] != branch(call.address, call.original, true) {
            return Err(format!("call guard failed at {:#x}", call.address));
        }
        firmware[off..off + 4].copy_from_slice(&branch(call.address, call.target, true));
    }
    let next_base = (base + payload.len() as u32 + 15) & !15;
    Ok(AppliedPatch {
        base,
        payload_len: payload.len(),
        next_base,
    })
}

/// Writes the new main image length and update-version marker into a copy of
/// `main_trailer.bin` (8 bytes: little-endian length, then 4 ASCII digits)
/// and `package_footer.bin` (same two fields at its own fixed offsets).
pub fn write_image_length_and_version(
    trailer: &mut [u8],
    footer: &mut [u8],
    length: u32,
    version: &str,
) {
    trailer[..4].copy_from_slice(&length.to_le_bytes());
    trailer[4..8].copy_from_slice(version.as_bytes());
    footer[0x38..0x3c].copy_from_slice(&length.to_le_bytes());
    footer[0x3c..0x40].copy_from_slice(version.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_firmware_verification_detects_both_models_from_reset_vector() {
        for (reset_vector, expected) in [
            (0x80001415u32, FirmwareModel::L6),
            (0x80000415u32, FirmwareModel::L6max),
        ] {
            let mut firmware = vec![0; 8];
            firmware[4..8].copy_from_slice(&reset_vector.to_le_bytes());
            assert_eq!(detect_firmware_model(&firmware), Ok(expected));
            let error = verify_firmware(&firmware).unwrap_err();
            let expected_digest = match expected {
                FirmwareModel::L6 => L6_EXPECTED_FIRMWARE_SHA256,
                FirmwareModel::L6max => EXPECTED_FIRMWARE_SHA256,
            };
            assert!(error.contains(expected_digest), "{error}");
        }
    }

    #[test]
    fn generic_firmware_verification_rejects_unknown_and_short_images() {
        assert!(
            detect_firmware_model(&[0; 7])
                .unwrap_err()
                .contains("too short")
        );
        let mut firmware = vec![0; 8];
        firmware[4..8].copy_from_slice(&0x12345679u32.to_le_bytes());
        assert!(
            detect_firmware_model(&firmware)
                .unwrap_err()
                .contains("unsupported firmware reset vector")
        );
    }

    #[test]
    fn thumb_branches_encode_synthetic_targets() {
        assert_eq!(branch(0x1000, 0x1004, true), [0x00, 0xf0, 0x00, 0xf8]);
        assert_eq!(branch(0x1000, 0x1004, false), [0x00, 0xf0, 0x00, 0xb8]);
    }

    #[test]
    fn apply_patch_grows_firmware_and_writes_a_hook_branch() {
        // Just under FIRST_PAYLOAD's offset (0x108000), so growth is small.
        let mut firmware = vec![0u8; 0x107ff0];
        let original_length = firmware.len();
        let padding = vec![0xffu8; 0x10000];
        let edits = PatchEdits::new(vec![HookEdit {
            address: 0x80000100,
            target: 0x80108010,
            length: 6,
        }]);
        let payload = vec![0xaa; 16];
        let applied = apply_patch(
            &mut firmware,
            original_length,
            FIRST_PAYLOAD,
            &payload,
            &edits,
            &padding,
        )
        .unwrap();
        assert_eq!(applied.base, FIRST_PAYLOAD);
        assert_eq!(applied.next_base, 0x80108010);
        assert_eq!(
            &firmware[0x100..0x104],
            &branch(0x80000100, 0x80108010, false)
        );
        assert_eq!(&firmware[0x104..0x106], &[0, 0xbf]);
        assert_eq!(&firmware[0x108000..], &payload[..]);
    }

    #[test]
    fn l6_payload_and_hook_offsets_use_the_l6_loaded_base() {
        let mut firmware = vec![0u8; 0x20];
        let padding = vec![0xff; 0x100];
        assert_eq!(
            first_payload_for(FirmwareModel::L6max, 0x1081e8),
            FIRST_PAYLOAD
        );
        let base = first_payload_for(FirmwareModel::L6, firmware.len());
        assert_eq!(base, 0x80001020);
        let target = base;
        let edits = PatchEdits::new(vec![HookEdit {
            address: 0x80001010,
            target,
            length: 4,
        }]);
        let payload = [0xaa; 16];
        let applied = apply_patch_with_map(
            &mut firmware,
            0x20,
            base,
            &payload,
            &edits,
            &padding,
            L6_MAIN_BASE,
            L6_SLOT_END,
        )
        .unwrap();
        assert_eq!(applied.base, base);
        assert_eq!(applied.next_base, 0x80001030);
        assert_eq!(&firmware[0x10..0x14], &branch(0x80001010, target, false));
        assert_eq!(&firmware[0x20..], &payload);
    }

    #[test]
    fn apply_patch_rejects_non_erased_padding() {
        let mut firmware = vec![0u8; 0x107ff0];
        let original_length = firmware.len();
        let mut padding = vec![0xffu8; 0x10000];
        padding[5] = 0x00; // falls within the small region this patch would consume
        let edits = PatchEdits::new(Vec::new());
        let payload = vec![0xaa; 16];
        assert!(
            apply_patch(
                &mut firmware,
                original_length,
                FIRST_PAYLOAD,
                &payload,
                &edits,
                &padding
            )
            .is_err()
        );
    }
}
