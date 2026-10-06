//! Validation policy for the emulator's replacement MAIN-only installer.
use std::io;
const PACKAGE_SIZE: usize = 0x1a8200;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
/// Validate before changing any installed bytes. Extra restrictions describe
/// this model's supported layout, not additional checks proved in ZOOM's code.
pub fn validate(package: &[u8]) -> io::Result<&[u8]> {
    if package.len() != PACKAGE_SIZE {
        return Err(invalid("unsupported package length"));
    }
    if package[0x60] != 2 {
        return Err(invalid(
            "only MAIN-only packages are supported; BOOT replacement is unavailable",
        ));
    }
    if package[0x64] & (1 << 6) == 0 {
        return Err(invalid("package excludes board classification 6"));
    }
    if le32(package, 0x78) != 0x100 || le32(package, 0x7c) != 0x1a8100 {
        return Err(invalid("unsupported MAIN layout"));
    }
    if !package[0x100..].starts_with(b"L6max Main Data") {
        return Err(invalid("invalid MAIN section label"));
    }
    let payload = &package[0x200..];
    let actual = payload
        .iter()
        .fold(0u32, |sum, &b| sum.wrapping_add(b as u32));
    let expected = u32::from_be_bytes(package[0x1fc..0x200].try_into().unwrap());
    if actual != expected {
        return Err(invalid("MAIN byte-sum checksum mismatch"));
    }
    // These consistency checks are model policy for the known fixed layout.
    if le32(package, 0x1a31f8) != le32(package, 0x1a81f8)
        || le32(package, 0x1a31f8) > 0x1a2ff8
        || le32(package, 0x1a71f8) > 0x3ff8
        || package[0x1a31fc..0x1a3200] != package[0x1a81fc..0x1a8200]
    {
        return Err(invalid("inconsistent image trailers"));
    }
    Ok(payload)
}
