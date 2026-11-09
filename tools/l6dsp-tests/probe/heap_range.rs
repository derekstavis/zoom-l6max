//! QEMU-only allocator bounds observed in L6max firmware 1.10.

/// Check an aligned payload against the observed heap header and end sentinel.
pub fn valid_heap_allocation(address: usize, len: usize) -> bool {
    len != 0 && address >= 0x808a4150 && address % 8 == 0
        && address.checked_add(len).is_some_and(|end| end <= 0x80921140)
}
