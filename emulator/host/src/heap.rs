//! FreeRTOS heap counters of a running 1.10 MAIN, stock or patched.
//!
//! Patches change the MAIN image hash, so the counters are guarded by the
//! allocator code itself: `pvPortMalloc` and `vPortFree` must match their
//! 1.10 bytes in guest memory before the counter addresses are trusted.
use crate::qmp::QmpClient;
use l6fw::Model;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// One allocator routine: name, entry, length and SHA-256 of its bytes.
type Routine = (&'static str, u32, usize, &'static str);

struct Layout {
    /// Usable heap_4 bytes, as the allocator's first call sets them free.
    size: u32,
    /// `xFreeBytesRemaining`, followed by `xMinimumEverFreeBytesRemaining`,
    /// `xNumberOfSuccessfulAllocations` and `xNumberOfSuccessfulFrees`.
    counters: u32,
    allocator: [Routine; 2],
}

const L6: Layout = Layout {
    size: 0x7cff0,
    counters: 0x801f_9020,
    allocator: [
        (
            "pvPortMalloc",
            0x8006_de78,
            0x1c8,
            "a25462aa95a97b6f238b0aa8f51b2f612f54abb8965a965332e48113ac16bb8f",
        ),
        (
            "vPortFree",
            0x8007_3f48,
            0xd8,
            "f95e7e19bcdde5d236cf123ecaa2e87f1b604a66d5ca32056696e17c9716eada",
        ),
    ],
};

const L6MAX: Layout = Layout {
    size: 0x7cff8,
    counters: 0x8020_ab1c,
    allocator: [
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
    ],
};

fn layout(model: Model) -> &'static Layout {
    match model {
        Model::L6 => &L6,
        Model::L6max => &L6MAX,
    }
}

/// Usable heap bytes on a model's 1.10 MAIN.
pub fn size(model: Model) -> u32 {
    layout(model).size
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Heap {
    pub size: u32,
    pub free: u32,
    pub minimum_ever_free: u32,
    pub allocations: u32,
    pub frees: u32,
}

impl Heap {
    fn decode(size: u32, counters: &[u8]) -> Result<Self, String> {
        let word = |i: usize| -> Result<u32, String> {
            counters
                .get(i * 4..i * 4 + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .ok_or_else(|| "short heap counter read".to_string())
        };
        let heap = Self {
            size,
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

/// Verify the model's 1.10 allocator in guest memory, then read its counters.
/// A patch that replaces either allocator routine is refused.
pub fn read(qmp: &QmpClient, model: Model) -> Result<Heap, String> {
    let layout = layout(model);
    for (name, address, length, expected) in layout.allocator {
        let code = qmp
            .read_memory(0, address, length)
            .map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(&code)) != expected {
            return Err(format!(
                "{name} at {address:#010x} differs from 1.10; heap counters are unknown for this image"
            ));
        }
    }
    let counters = qmp
        .read_memory(0, layout.counters, 16)
        .map_err(|e| e.to_string())?;
    Heap::decode(layout.size, &counters)
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
        let heap = Heap::decode(0x7cff8, &bytes).unwrap();
        assert_eq!(heap.peak_used(), 361_992);
        assert_eq!(heap.allocations, 197);
        let inverted: Vec<u8> = [100u32, 200, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert!(Heap::decode(0x7cff8, &inverted).is_err());
        assert!(Heap::decode(0x7cff8, &bytes[..12]).is_err());
    }
}
