//! FreeRTOS heap counters of a running L6max 1.10 MAIN, stock or patched.
//!
//! Patches change the MAIN image hash, so the counters are guarded by the
//! allocator code itself: `pvPortMalloc` and `vPortFree` must match their
//! 1.10 bytes in guest memory before the counter addresses are trusted.
use crate::qmp::QmpClient;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// heap_4 region size configured by the 1.10 allocator, in bytes.
pub const L6MAX_HEAP_BYTES: u32 = 512_000;
/// `xFreeBytesRemaining`, followed by `xMinimumEverFreeBytesRemaining`,
/// `xNumberOfSuccessfulAllocations` and `xNumberOfSuccessfulFrees`.
const COUNTERS: u32 = 0x8020_ab1c;
const ALLOCATOR: [(&str, u32, usize, &str); 2] = [
    (
        "pvPortMalloc",
        0x8008_5478,
        0x1c4,
        "553dd1e9cbb9f58c30b57f91827a1e627fa34e55d1d8419f9a4c09bb0ec77d8e",
    ),
    (
        "vPortFree",
        0x8008_e588,
        0xd4,
        "2a2649e2ee66a717ac2051c30853d14190add0aab9dcffcdd253e154d9fcf7d2",
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Heap {
    pub size: u32,
    pub free: u32,
    pub minimum_ever_free: u32,
    pub allocations: u32,
    pub frees: u32,
}

impl Heap {
    fn decode(counters: &[u8]) -> Result<Self, String> {
        let word = |i: usize| -> Result<u32, String> {
            counters
                .get(i * 4..i * 4 + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .ok_or_else(|| "short heap counter read".to_string())
        };
        let heap = Self {
            size: L6MAX_HEAP_BYTES,
            free: word(0)?,
            minimum_ever_free: word(1)?,
            allocations: word(2)?,
            frees: word(3)?,
        };
        if heap.free > heap.size || heap.minimum_ever_free > heap.free {
            return Err(format!("implausible heap counters: {heap:?}"));
        }
        Ok(heap)
    }

    /// Bytes allocated at the lowest point since boot.
    pub fn peak_used(&self) -> u32 {
        self.size - self.minimum_ever_free
    }
}

/// Verify the L6max 1.10 allocator in guest memory, then read its counters.
/// A patch that replaces either allocator routine is refused.
pub fn read_l6max(qmp: &QmpClient) -> Result<Heap, String> {
    for (name, address, length, expected) in ALLOCATOR {
        let code = qmp
            .read_memory(0, address, length)
            .map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(&code)) != expected {
            return Err(format!(
                "{name} at {address:#010x} differs from L6max 1.10; heap counters are unknown for this image"
            ));
        }
    }
    let counters = qmp
        .read_memory(0, COUNTERS, 16)
        .map_err(|e| e.to_string())?;
    Heap::decode(&counters)
}

#[cfg(test)]
mod tests {
    use super::Heap;

    #[test]
    fn decodes_and_checks_counters() {
        let bytes: Vec<u8> = [160_008u32, 150_000, 197, 3]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let heap = Heap::decode(&bytes).unwrap();
        assert_eq!(heap.peak_used(), 362_000);
        assert_eq!(heap.allocations, 197);
        let inverted: Vec<u8> = [100u32, 200, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert!(Heap::decode(&inverted).is_err());
        assert!(Heap::decode(&bytes[..12]).is_err());
    }
}
