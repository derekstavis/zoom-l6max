#![no_std]
//! Allocation-free parsing for experimental SD patch bundles.
#[cfg(test)]
extern crate std;

/// Experimental fixed-address bundles emitted by `l6patch`.
///
/// Validation only: publishing edits to a live firmware image requires a
/// platform-specific synchronization and cache protocol that is not yet known.
pub mod bundle {
    const HEADER: usize = 68;
    const RELOCATABLE_HEADER: usize = 72;
    const RELOCATION_RECORD: usize = 12;

    /// CRC-32/ISO-HDLC, excluding the stored checksum word at bytes 60..64.
    /// Detects accidental corruption; it does not authenticate a bundle.
    pub fn checksum(file: &[u8]) -> Option<u32> {
        if file.len() < HEADER || (word(file, 4) == 6 && file.len() < RELOCATABLE_HEADER) {
            return None;
        }
        let mut crc = !0u32;
        for &byte in file[..60].iter().chain(file[64..].iter()) {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        Some(!crc)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Error {
        Truncated,
        Invalid,
        Incompatible,
        GuardMismatch,
        TooLarge,
        Relocation,
    }

    /// A sequential SD file reader. A zero-byte read denotes end of file.
    pub trait Reader {
        type Error;
        fn read(&mut self, destination: &mut [u8]) -> Result<usize, Self::Error>;

        /// Confirm there are no bytes after the declared body. Readers whose
        /// underlying API reports EOF as an error can override this method.
        fn finish(&mut self) -> Result<(), ReadError> {
            let mut trailing = [0u8; 1];
            match self.read(&mut trailing) {
                Ok(0) => Ok(()),
                Ok(_) => Err(ReadError::Invalid),
                Err(_) => Err(ReadError::Io),
            }
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum ReadError {
        Io,
        Truncated,
        Invalid,
        TooLarge,
    }

    fn read_exact<R: Reader>(reader: &mut R, mut dst: &mut [u8]) -> Result<(), ReadError> {
        while !dst.is_empty() {
            let count = reader.read(dst).map_err(|_| ReadError::Io)?;
            if count == 0 {
                return Err(ReadError::Truncated);
            }
            if count > dst.len() {
                return Err(ReadError::Invalid);
            }
            dst = &mut dst[count..];
        }
        Ok(())
    }

    /// Read a bounded bundle into caller-owned memory. Nothing is activated;
    /// pass the returned slice to `validate` before using any edits or code.
    pub fn read_bundle<'a, R: Reader>(
        reader: &mut R,
        buffer: &'a mut [u8],
        max_payload: usize,
        max_records: u32,
        max_relocations: u32,
    ) -> Result<&'a [u8], ReadError> {
        if buffer.len() < HEADER {
            return Err(ReadError::TooLarge);
        }
        read_exact(reader, &mut buffer[..HEADER])?;
        let version = word(buffer, 4);
        if &buffer[..4] != b"L6SB" || !matches!(version, 4 | 5 | 6) {
            return Err(ReadError::Invalid);
        }
        let header_len = if version == 6 {
            let end = RELOCATABLE_HEADER;
            let bytes = buffer.get_mut(HEADER..end).ok_or(ReadError::TooLarge)?;
            read_exact(reader, bytes)?;
            end
        } else {
            HEADER
        };
        let payload_len = word(buffer, 20) as usize;
        let count = word(buffer, 24);
        let relocation_count = if version == 6 { word(buffer, 68) } else { 0 };
        if payload_len > max_payload || count > max_records || relocation_count > max_relocations {
            return Err(ReadError::TooLarge);
        }
        let mut pos = header_len;
        for _ in 0..count {
            let end = pos.checked_add(8).ok_or(ReadError::TooLarge)?;
            let bytes = buffer.get_mut(pos..end).ok_or(ReadError::TooLarge)?;
            read_exact(reader, bytes)?;
            let len = word(buffer, pos + 4) as usize;
            pos = end;
            let end = pos
                .checked_add(len.checked_mul(2).ok_or(ReadError::TooLarge)?)
                .ok_or(ReadError::TooLarge)?;
            let bytes = buffer.get_mut(pos..end).ok_or(ReadError::TooLarge)?;
            read_exact(reader, bytes)?;
            pos = end;
        }
        let relocation_bytes = (relocation_count as usize)
            .checked_mul(RELOCATION_RECORD)
            .ok_or(ReadError::TooLarge)?;
        let end = pos
            .checked_add(relocation_bytes)
            .ok_or(ReadError::TooLarge)?;
        let bytes = buffer.get_mut(pos..end).ok_or(ReadError::TooLarge)?;
        read_exact(reader, bytes)?;
        pos = end;
        let end = pos.checked_add(payload_len).ok_or(ReadError::TooLarge)?;
        let bytes = buffer.get_mut(pos..end).ok_or(ReadError::TooLarge)?;
        read_exact(reader, bytes)?;
        Ok(&buffer[..end])
    }

    pub struct Policy {
        pub firmware_sha256: [u8; 32],
        pub image_base: u32,
        pub image_len: usize,
        pub payload_base: u32,
        pub max_payload: usize,
        pub max_records: u32,
        pub max_relocations: u32,
        /// Dynamic bundles validate source hook bytes against the local patch
        /// catalog during construction; the installed image contains dispatch
        /// stubs at those addresses, so runtime validation skips this compare.
        pub compare_guards: bool,
    }

    pub struct Validated<'a> {
        records: &'a [u8],
        relocations: &'a [u8],
        payload: &'a [u8],
        count: u32,
        relocation_count: u32,
        init: Option<u32>,
        version: u32,
    }

    fn word(data: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
    }

    /// Checks all bounds, ordered non-overlapping edits and current bytes.
    /// `firmware` may include unrelated persistent loader changes; each edit
    /// must still match its own stock bytes. The policy identity is trusted
    /// metadata supplied by a revision-guarded installer, not a runtime hash.
    pub fn validate<'a>(
        file: &'a [u8],
        firmware: &[u8],
        policy: &Policy,
    ) -> Result<Validated<'a>, Error> {
        if file.len() < HEADER {
            return Err(Error::Truncated);
        }
        let version = word(file, 4);
        if &file[..4] != b"L6SB" || !matches!(version, 4 | 5 | 6) {
            return Err(Error::Invalid);
        }
        if checksum(file) != Some(word(file, 60)) {
            return Err(Error::Invalid);
        }
        let image_len = word(file, 12) as usize;
        let payload_len = word(file, 20) as usize;
        let count = word(file, 24);
        let init = word(file, 64);
        let relocation_count = if version == 6 {
            if file.len() < RELOCATABLE_HEADER {
                return Err(Error::Truncated);
            }
            word(file, 68)
        } else {
            0
        };
        if word(file, 8) != policy.image_base
            || image_len != policy.image_len
            || word(file, 16) != policy.payload_base
            || file[28..60] != policy.firmware_sha256
            || firmware.len() < image_len
        {
            return Err(Error::Incompatible);
        }
        if payload_len > policy.max_payload
            || count > policy.max_records
            || relocation_count > policy.max_relocations
        {
            return Err(Error::TooLarge);
        }
        if init != 0 {
            let len = u32::try_from(payload_len).map_err(|_| Error::TooLarge)?;
            let valid = if version == 4 {
                let end = policy
                    .payload_base
                    .checked_add(len)
                    .ok_or(Error::TooLarge)?;
                init >= policy.payload_base && init < end
            } else {
                init < len
            };
            if init & 1 != 0 || !valid {
                return Err(Error::Invalid);
            }
        }
        let header_len = if version == 6 {
            RELOCATABLE_HEADER
        } else {
            HEADER
        };
        let mut pos = header_len;
        let mut last_end = 0usize;
        for _ in 0..count {
            let end_header = pos.checked_add(8).ok_or(Error::Invalid)?;
            if end_header > file.len() {
                return Err(Error::Truncated);
            }
            let offset = word(file, pos) as usize;
            let len = word(file, pos + 4) as usize;
            let end = offset.checked_add(len).ok_or(Error::Invalid)?;
            let end_record = end_header
                .checked_add(len.checked_mul(2).ok_or(Error::Invalid)?)
                .ok_or(Error::Invalid)?;
            if len == 0 || offset < last_end || end > image_len {
                return Err(Error::Invalid);
            }
            if end_record > file.len() {
                return Err(Error::Truncated);
            }
            if version >= 5 {
                let replacement_len = end_header + len;
                if len < 4 {
                    return Err(Error::Invalid);
                }
                let entry = word(file, replacement_len);
                if entry & 1 != 0 || entry as usize >= payload_len {
                    return Err(Error::Invalid);
                }
            }
            if policy.compare_guards && firmware[offset..end] != file[end_header..end_header + len]
            {
                return Err(Error::GuardMismatch);
            }
            last_end = end;
            pos = end_record;
        }
        let relocation_start = pos;
        let relocation_len = (relocation_count as usize)
            .checked_mul(RELOCATION_RECORD)
            .ok_or(Error::TooLarge)?;
        let relocation_end = relocation_start
            .checked_add(relocation_len)
            .ok_or(Error::TooLarge)?;
        if relocation_end > file.len() {
            return Err(Error::Truncated);
        }
        let payload_end = relocation_end
            .checked_add(payload_len)
            .ok_or(Error::Invalid)?;
        if payload_end != file.len() {
            return Err(Error::Invalid);
        }
        let payload = &file[relocation_end..payload_end];
        let mut last_relocation_end = 0usize;
        for index in 0..relocation_count as usize {
            let record = relocation_start + index * RELOCATION_RECORD;
            let kind = word(file, record);
            let first = word(file, record + 4) as usize;
            let second = word(file, record + 8) as usize;
            let (start, end) = match kind {
                1 => {
                    if second != 0 {
                        return Err(Error::Invalid);
                    }
                    (first, first.checked_add(4).ok_or(Error::Invalid)?)
                }
                2 => {
                    if second <= first
                        || first.checked_add(4).is_none_or(|low_end| low_end > second)
                    {
                        return Err(Error::Invalid);
                    }
                    (first, second.checked_add(4).ok_or(Error::Invalid)?)
                }
                _ => return Err(Error::Invalid),
            };
            if start & 1 != 0
                || end > payload_len
                || start < last_relocation_end
                || (kind == 2 && second - first > 16)
            {
                return Err(Error::Invalid);
            }
            last_relocation_end = end;
        }
        Ok(Validated {
            records: &file[header_len..relocation_start],
            relocations: &file[relocation_start..relocation_end],
            payload,
            count,
            relocation_count,
            init: if init == 0 { None } else { Some(init) },
            version,
        })
    }

    impl<'a> Validated<'a> {
        pub fn payload(&self) -> &'a [u8] {
            self.payload
        }
        pub fn init(&self) -> Option<u32> {
            self.init
        }
        pub fn version(&self) -> u32 {
            self.version
        }
        pub fn relocations(&self) -> Relocations<'a> {
            Relocations {
                bytes: self.relocations,
                remaining: self.relocation_count,
            }
        }
        pub fn edits(&self) -> Edits<'a> {
            Edits {
                bytes: self.records,
                remaining: self.count,
            }
        }
    }

    /// Apply the bundle's load bias to module-internal absolute references.
    /// The file image remains unchanged, so callers may use it as the source
    /// while relocating the newly allocated copy in place.
    pub fn relocate<I: Iterator<Item = Relocation>>(
        image: &mut [u8],
        relocations: I,
        linked_base: u32,
        loaded_base: u32,
    ) -> Result<(), Error> {
        let linked_end = linked_base
            .checked_add(u32::try_from(image.len()).map_err(|_| Error::Relocation)?)
            .ok_or(Error::Relocation)?;
        let delta = i64::from(loaded_base) - i64::from(linked_base);
        for relocation in relocations {
            match relocation {
                Relocation::Abs32 { offset } => {
                    let end = offset.checked_add(4).ok_or(Error::Relocation)?;
                    let bytes = image.get_mut(offset..end).ok_or(Error::Relocation)?;
                    let target = u32::from_le_bytes(bytes.try_into().unwrap());
                    if !(linked_base..=linked_end).contains(&(target & !1)) {
                        return Err(Error::Relocation);
                    }
                    let relocated = (i64::from(target) + delta)
                        .try_into()
                        .map_err(|_| Error::Relocation)?;
                    bytes.copy_from_slice(&u32::to_le_bytes(relocated));
                }
                Relocation::ThumbMov32 {
                    low_offset,
                    high_offset,
                } => {
                    let (low_reg, low) = read_thumb_mov(image, low_offset, 0xf240)?;
                    let (high_reg, high) = read_thumb_mov(image, high_offset, 0xf2c0)?;
                    if low_reg != high_reg {
                        return Err(Error::Relocation);
                    }
                    let target = (u32::from(high) << 16) | u32::from(low);
                    if !(linked_base..=linked_end).contains(&(target & !1)) {
                        return Err(Error::Relocation);
                    }
                    let relocated: u32 = (i64::from(target) + delta)
                        .try_into()
                        .map_err(|_| Error::Relocation)?;
                    write_thumb_mov(image, low_offset, 0xf240, low_reg, relocated as u16)?;
                    write_thumb_mov(
                        image,
                        high_offset,
                        0xf2c0,
                        high_reg,
                        (relocated >> 16) as u16,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn read_thumb_mov(image: &[u8], offset: usize, opcode: u16) -> Result<(u8, u16), Error> {
        let end = offset.checked_add(4).ok_or(Error::Relocation)?;
        let instruction = image.get(offset..end).ok_or(Error::Relocation)?;
        let first = u16::from_le_bytes(instruction[..2].try_into().unwrap());
        let second = u16::from_le_bytes(instruction[2..].try_into().unwrap());
        if first & 0xfbf0 != opcode {
            return Err(Error::Relocation);
        }
        let immediate = ((first & 0x000f) << 12)
            | ((first & 0x0400) << 1)
            | ((second & 0x7000) >> 4)
            | (second & 0x00ff);
        Ok((((second >> 8) & 0xf) as u8, immediate))
    }

    fn write_thumb_mov(
        image: &mut [u8],
        offset: usize,
        opcode: u16,
        register: u8,
        immediate: u16,
    ) -> Result<(), Error> {
        let end = offset.checked_add(4).ok_or(Error::Relocation)?;
        let instruction = image.get_mut(offset..end).ok_or(Error::Relocation)?;
        let first = u16::from_le_bytes(instruction[..2].try_into().unwrap());
        let second = u16::from_le_bytes(instruction[2..].try_into().unwrap());
        if first & 0xfbf0 != opcode || ((second >> 8) & 0xf) as u8 != register {
            return Err(Error::Relocation);
        }
        let first = (first & !0x040f) | ((immediate >> 12) & 0xf) | ((immediate >> 1) & 0x0400);
        let second = (second & !0x70ff) | ((immediate << 4) & 0x7000) | (immediate & 0x00ff);
        instruction[..2].copy_from_slice(&first.to_le_bytes());
        instruction[2..].copy_from_slice(&second.to_le_bytes());
        Ok(())
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Relocation {
        Abs32 {
            offset: usize,
        },
        ThumbMov32 {
            low_offset: usize,
            high_offset: usize,
        },
    }

    pub struct Relocations<'a> {
        bytes: &'a [u8],
        remaining: u32,
    }
    impl Iterator for Relocations<'_> {
        type Item = Relocation;
        fn next(&mut self) -> Option<Self::Item> {
            if self.remaining == 0 {
                return None;
            }
            let kind = word(self.bytes, 0);
            let first = word(self.bytes, 4) as usize;
            let second = word(self.bytes, 8) as usize;
            self.bytes = &self.bytes[RELOCATION_RECORD..];
            self.remaining -= 1;
            Some(match kind {
                1 => Relocation::Abs32 { offset: first },
                2 => Relocation::ThumbMov32 {
                    low_offset: first,
                    high_offset: second,
                },
                _ => unreachable!("relocations are validated before iteration"),
            })
        }
    }

    pub struct Edit<'a> {
        pub offset: usize,
        pub original: &'a [u8],
        pub replacement: &'a [u8],
    }
    pub struct Edits<'a> {
        bytes: &'a [u8],
        remaining: u32,
    }
    impl<'a> Iterator for Edits<'a> {
        type Item = Edit<'a>;
        fn next(&mut self) -> Option<Self::Item> {
            if self.remaining == 0 {
                return None;
            }
            let offset = word(self.bytes, 0) as usize;
            let len = word(self.bytes, 4) as usize;
            let original = &self.bytes[8..8 + len];
            let replacement = &self.bytes[8 + len..8 + 2 * len];
            self.bytes = &self.bytes[8 + 2 * len..];
            self.remaining -= 1;
            Some(Edit {
                offset,
                original,
                replacement,
            })
        }
    }
}

/// Compact, one-patch-per-file dynamic module stream (format 7).
///
/// The fixed header is 24 bytes. Code starts immediately after it, followed by
/// fixed 8-byte hook records and 12-byte relocation records. The CRC covers the
/// complete file except the checksum word itself. No whole-file staging buffer
/// is needed: code is read into its final allocation and metadata is processed
/// one bounded record at a time.
pub mod dynamic {
    use super::bundle::{self, ReadError, Reader, Relocation};

    pub const HEADER_LEN: usize = 24;
    pub const FORMAT_VERSION: u16 = 8;
    pub const LOADER_ABI: u16 = 2;
    pub const HOOK_RECORD_LEN: usize = 12;
    pub const RELOCATION_RECORD_LEN: usize = 12;
    /// Maximum number of distinct hook sites the resident loader can index.
    /// Keep this synchronized with the fixed registry reserved by Patch loader.
    // The current L6max catalog exposes seven dynamic sites. Keep one spare
    // chain and bound per-module loader stack use while covering MIDI Control's
    // six registrations.
    pub const MAX_FUNCTION_CHAINS: usize = 8;
    /// Maximum hook records accepted in one patch file. This covers the
    /// current multi-site patch prototypes while keeping loader stack use
    /// bounded.
    pub const MAX_HOOKS_PER_MODULE: usize = 8;
    pub const MAX_HOOKS_PER_FUNCTION: usize = 8;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct HookLink {
        pub entry: u32,
        /// Address of this module's writable continuation pointer.
        pub next_slot: u32,
        /// Zero-based position in the filename-sorted load sequence.
        pub order: u32,
        pub module_base: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct FunctionHookChain {
        pub address: u32,
        pub count: u32,
        pub links: [HookLink; MAX_HOOKS_PER_FUNCTION],
    }

    impl Default for FunctionHookChain {
        fn default() -> Self {
            Self {
                address: 0,
                count: 0,
                links: [HookLink::default(); MAX_HOOKS_PER_FUNCTION],
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ChainError {
        DifferentFunction,
        Full,
        MissingContinuation,
    }

    pub trait PatchFilename {
        fn name_len(&self) -> usize;
        fn name_byte(&self, index: usize) -> u8;
    }

    impl PatchFilename for [u8] {
        fn name_len(&self) -> usize {
            self.len()
        }
        fn name_byte(&self, index: usize) -> u8 {
            self[index]
        }
    }

    fn numeric_prefix<N: PatchFilename + ?Sized>(name: &N) -> Option<(usize, usize)> {
        let dash = (0..name.name_len()).find(|&index| name.name_byte(index) == b'-')?;
        if dash == 0 || !(0..dash).all(|index| name.name_byte(index).is_ascii_digit()) {
            return None;
        }
        let mut first = 0;
        while first + 1 < dash && name.name_byte(first) == b'0' {
            first += 1;
        }
        Some((first, dash - first))
    }

    /// Compare patch names: decimal prefixes before `-` sort numerically and
    /// before unnumbered names; ties use case-insensitive then bytewise order.
    pub fn compare_patch_filenames<L: PatchFilename + ?Sized, R: PatchFilename + ?Sized>(
        left: &L,
        right: &R,
    ) -> core::cmp::Ordering {
        use core::cmp::Ordering;
        match (numeric_prefix(left), numeric_prefix(right)) {
            (Some((left_start, left_len)), Some((right_start, right_len))) => {
                let numeric = left_len.cmp(&right_len).then_with(|| {
                    (0..left_len)
                        .map(|i| left.name_byte(left_start + i))
                        .cmp((0..right_len).map(|i| right.name_byte(right_start + i)))
                });
                if numeric != Ordering::Equal {
                    return numeric;
                }
            }
            (Some(_), None) => return Ordering::Less,
            (None, Some(_)) => return Ordering::Greater,
            (None, None) => {}
        }
        let folded = (0..left.name_len())
            .map(|i| left.name_byte(i).to_ascii_lowercase())
            .cmp((0..right.name_len()).map(|i| right.name_byte(i).to_ascii_lowercase()));
        if folded != Ordering::Equal {
            return folded;
        }
        (0..left.name_len())
            .map(|i| left.name_byte(i))
            .cmp((0..right.name_len()).map(|i| right.name_byte(i)))
    }

    impl FunctionHookChain {
        /// Append a hook in already-sorted filename order. Returns the prior
        /// tail's continuation slot, which the loader links to the new entry.
        pub fn append(
            &mut self,
            address: u32,
            entry: u32,
            next_slot: u32,
            order: u32,
            module_base: u32,
        ) -> Result<Option<u32>, ChainError> {
            if address == 0 || entry == 0 || next_slot == 0 {
                return Err(ChainError::MissingContinuation);
            }
            if self.count == 0 {
                self.address = address;
            } else if self.address != address {
                return Err(ChainError::DifferentFunction);
            }
            let index = self.count as usize;
            if index >= self.links.len() {
                return Err(ChainError::Full);
            }
            let previous_slot = (index > 0).then(|| self.links[index - 1].next_slot);
            self.links[index] = HookLink {
                entry,
                next_slot,
                order,
                module_base,
            };
            self.count += 1;
            Ok(previous_slot)
        }

        pub fn head(&self) -> Option<u32> {
            (self.count != 0).then_some(self.links[0].entry)
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Hook {
        pub address: u32,
        pub entry_offset: u32,
        /// Optional aligned code offset containing a writable `next` function
        /// pointer. `u32::MAX` marks a replacement that cannot call through.
        pub next_slot_offset: u32,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Header {
        pub code_len: usize,
        pub hook_count: usize,
        pub relocation_count: usize,
        pub init_offset: u32,
        pub crc32: u32,
    }

    #[derive(Debug)]
    pub struct Pending {
        pub header: Header,
        crc_state: u32,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Error {
        Read(ReadError),
        Invalid,
        TooLarge,
        Relocation,
        Checksum,
    }

    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn read_exact<R: Reader>(reader: &mut R, mut dst: &mut [u8]) -> Result<(), Error> {
        while !dst.is_empty() {
            let count = reader.read(dst).map_err(|_| Error::Read(ReadError::Io))?;
            if count == 0 {
                return Err(Error::Read(ReadError::Truncated));
            }
            if count > dst.len() {
                return Err(Error::Read(ReadError::Invalid));
            }
            dst = &mut dst[count..];
        }
        Ok(())
    }

    fn crc_update(mut crc: u32, bytes: &[u8]) -> u32 {
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        crc
    }

    /// Read and validate the fixed header before the caller allocates code.
    pub fn read_header<R: Reader>(
        reader: &mut R,
        max_code_len: usize,
        max_hooks: usize,
        max_relocations: usize,
    ) -> Result<Pending, Error> {
        let mut header = [0u8; HEADER_LEN];
        read_exact(reader, &mut header)?;
        if &header[..4] != b"L6SB"
            || u16_at(&header, 4) != FORMAT_VERSION
            || u16_at(&header, 6) != LOADER_ABI
        {
            return Err(Error::Invalid);
        }
        let code_len = u32_at(&header, 8) as usize;
        let hook_count = u16_at(&header, 12) as usize;
        let relocation_count = u16_at(&header, 14) as usize;
        let init_offset = u32_at(&header, 16);
        let expected_crc = u32_at(&header, 20);
        if code_len == 0
            || code_len > max_code_len
            || hook_count == 0
            || hook_count > max_hooks
            || relocation_count > max_relocations
            || (init_offset != 0 && (init_offset & 1 != 0 || init_offset as usize >= code_len))
        {
            return Err(Error::TooLarge);
        }
        Ok(Pending {
            header: Header {
                code_len,
                hook_count,
                relocation_count,
                init_offset,
                crc32: expected_crc,
            },
            // Header checksum bytes are deliberately omitted from CRC input.
            crc_state: crc_update(!0u32, &header[..20]),
        })
    }

    /// Read one v8 body directly into its final code allocation. Relocations
    /// are applied as records arrive; hooks remain unpublished until CRC pass.
    pub fn read_body<R: Reader>(
        reader: &mut R,
        pending: Pending,
        code: &mut [u8],
        hooks: &mut [Hook],
        linked_base: u32,
        loaded_base: u32,
    ) -> Result<Header, Error> {
        let header = pending.header;
        if code.len() != header.code_len || hooks.len() < header.hook_count {
            return Err(Error::TooLarge);
        }
        let mut crc = pending.crc_state;
        read_exact(reader, code)?;
        crc = crc_update(crc, code);

        let mut hook_bytes = [0u8; HOOK_RECORD_LEN];
        for index in 0..header.hook_count {
            read_exact(reader, &mut hook_bytes)?;
            crc = crc_update(crc, &hook_bytes);
            let hook = Hook {
                address: u32_at(&hook_bytes, 0),
                entry_offset: u32_at(&hook_bytes, 4),
                next_slot_offset: u32_at(&hook_bytes, 8),
            };
            if hook.address & 1 != 0
                || hook.entry_offset & 1 != 0
                || hook.entry_offset as usize >= header.code_len
                || (hook.next_slot_offset != u32::MAX
                    && (hook.next_slot_offset & 3 != 0
                        || hook.next_slot_offset as usize + 4 > header.code_len))
                || hooks[..index]
                    .iter()
                    .any(|prior| prior.address == hook.address)
            {
                return Err(Error::Invalid);
            }
            hooks[index] = hook;
        }

        let mut relocation_bytes = [0u8; RELOCATION_RECORD_LEN];
        for _ in 0..header.relocation_count {
            read_exact(reader, &mut relocation_bytes)?;
            crc = crc_update(crc, &relocation_bytes);
            let kind = u32_at(&relocation_bytes, 0);
            let first = u32_at(&relocation_bytes, 4) as usize;
            let second = u32_at(&relocation_bytes, 8) as usize;
            let relocation = match kind {
                1 if second == 0 => Relocation::Abs32 { offset: first },
                2 => Relocation::ThumbMov32 {
                    low_offset: first,
                    high_offset: second,
                },
                _ => return Err(Error::Relocation),
            };
            bundle::relocate(code, core::iter::once(relocation), linked_base, loaded_base)
                .map_err(|_| Error::Relocation)?;
        }
        if !crc != header.crc32 {
            return Err(Error::Checksum);
        }
        reader.finish().map_err(Error::Read)?;
        Ok(header)
    }

    /// CRC-32/ISO-HDLC for a complete v8 file, excluding bytes 20..24.
    pub fn checksum(file: &[u8]) -> Option<u32> {
        if file.len() < HEADER_LEN || &file[..4] != b"L6SB" {
            return None;
        }
        let crc = crc_update(!0u32, &file[..20]);
        Some(!crc_update(crc, &file[HEADER_LEN..]))
    }
}

#[cfg(test)]
mod dynamic_stream_tests {
    extern crate std;
    use super::bundle::{ReadError, Reader};
    use super::dynamic::{self, Hook};
    use std::vec::Vec;

    struct Chunks<'a> {
        bytes: &'a [u8],
        chunk: usize,
    }
    impl Reader for Chunks<'_> {
        type Error = ();
        fn read(&mut self, dst: &mut [u8]) -> Result<usize, Self::Error> {
            let count = dst.len().min(self.chunk).min(self.bytes.len());
            dst[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }

    fn fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"L6SB");
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&[4, 0, 0, 0, 0, 0xbf, 0x70, 0x47, 0, 0, 0, 0]);
        bytes.extend_from_slice(&0x8000_1000u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let checksum = dynamic::checksum(&bytes).unwrap();
        bytes[20..24].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    #[test]
    fn streams_code_metadata_and_relocations_without_a_staging_buffer() {
        let bytes = fixture();
        assert_eq!(
            dynamic::checksum(&bytes),
            Some(u32::from_le_bytes(bytes[20..24].try_into().unwrap()))
        );
        let mut reader = Chunks {
            bytes: &bytes,
            chunk: 3,
        };
        let pending = dynamic::read_header(&mut reader, 32, 4, 8).unwrap();
        let mut code = [0; 12];
        let mut hooks = [Hook::default(); 4];
        let header =
            dynamic::read_body(&mut reader, pending, &mut code, &mut hooks, 0, 0x2000).unwrap();
        assert_eq!(header.code_len, 12);
        assert_eq!(header.hook_count, 1);
        assert_eq!(header.relocation_count, 1);
        assert_eq!(
            hooks[0],
            Hook {
                address: 0x8000_1000,
                entry_offset: 4,
                next_slot_offset: 8,
            }
        );
        assert_eq!(u32::from_le_bytes(code[..4].try_into().unwrap()), 0x2004);
    }

    #[test]
    fn fixed_function_chain_preserves_append_order_and_rejects_overflow() {
        let mut chain = dynamic::FunctionHookChain::default();
        assert_eq!(
            chain.append(0x8000_1000, 0x2001, 0x2100, 0, 0x2000),
            Ok(None)
        );
        assert_eq!(
            chain.append(0x8000_1000, 0x2201, 0x2300, 1, 0x2200),
            Ok(Some(0x2100))
        );
        assert_eq!(chain.head(), Some(0x2001));
        assert_eq!(chain.count, 2);
        assert_eq!(chain.links[1].entry, 0x2201);
        assert_eq!(chain.links[1].order, 1);
        assert_eq!(
            chain.append(0x8000_2000, 0x2401, 0x2500, 2, 0x2400),
            Err(dynamic::ChainError::DifferentFunction)
        );
        for index in 2..dynamic::MAX_HOOKS_PER_FUNCTION {
            chain
                .append(
                    0x8000_1000,
                    0x3001 + (index as u32) * 0x100,
                    0x3100,
                    index as u32,
                    0x3000,
                )
                .unwrap();
        }
        assert_eq!(
            chain.append(0x8000_1000, 0x4001, 0x4100, 8, 0x4000),
            Err(dynamic::ChainError::Full)
        );
    }

    #[test]
    fn patch_filename_priorities_use_numeric_order_and_stable_ties() {
        use core::cmp::Ordering;
        let compare = |left: &[u8], right: &[u8]| dynamic::compare_patch_filenames(left, right);
        assert_eq!(
            compare(b"2-MIDI.BIN" as &[u8], b"10-DIN.BIN"),
            Ordering::Less
        );
        assert_eq!(compare(b"02-MIDI.BIN", b"2-DIN.BIN"), Ordering::Less);
        assert_eq!(compare(b"9-LAST.BIN", b"MIDI.BIN"), Ordering::Less);
        assert_eq!(compare(b"abc.BIN", b"ABC.BIN"), Ordering::Greater);
    }

    #[test]
    fn streaming_parser_rejects_corruption_and_bad_counts() {
        let mut bytes = fixture();
        bytes[20] ^= 0x80;
        let mut reader = Chunks {
            bytes: &bytes,
            chunk: 5,
        };
        let pending = dynamic::read_header(&mut reader, 32, 4, 8).unwrap();
        let mut code = [0; 12];
        let mut hooks = [Hook::default(); 4];
        assert_eq!(
            dynamic::read_body(&mut reader, pending, &mut code, &mut hooks, 0, 0x2000),
            Err(dynamic::Error::Checksum)
        );

        let bytes = fixture();
        let mut reader = Chunks {
            bytes: &bytes,
            chunk: 7,
        };
        assert_eq!(
            dynamic::read_header(&mut reader, 4, 4, 8).unwrap_err(),
            dynamic::Error::TooLarge
        );

        let mut bytes = fixture();
        bytes.push(0);
        let mut reader = Chunks {
            bytes: &bytes,
            chunk: 64,
        };
        let pending = dynamic::read_header(&mut reader, 32, 4, 8).unwrap();
        let mut code = [0; 12];
        let mut hooks = [Hook::default(); 4];
        assert_eq!(
            dynamic::read_body(&mut reader, pending, &mut code, &mut hooks, 0, 0x2000),
            Err(dynamic::Error::Read(ReadError::Invalid))
        );
    }
}
