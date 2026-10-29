//! The firmware's two-slot settings-bank protocol over a NOR image.
//!
//! Both models use the same bank format; only the slot addresses, payload
//! size and the NOR device holding them differ, so these functions operate on
//! whichever image the catalog names.

use crate::catalog::Catalog;

/// An active settings bank: which slot holds it and its generation marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveBank {
    pub slot: usize,
    pub marker: u32,
}

fn marker(storage: &[u8], slot: usize) -> u32 {
    u32::from_le_bytes(storage[slot..slot + 4].try_into().unwrap())
}

/// Select the bank the firmware loads: markers 1–3 are valid; with both valid
/// the higher wins except that 1 follows 3.
pub fn active(storage: &[u8], catalog: &Catalog) -> Option<ActiveBank> {
    let [a, b] = catalog.bank.slots;
    let (ma, mb) = (marker(storage, a), marker(storage, b));
    let valid = |m: u32| (1..=3).contains(&m);
    let pick = |slot: usize, marker| Some(ActiveBank { slot, marker });
    match (valid(ma), valid(mb)) {
        (false, false) => None,
        (true, false) => pick(0, ma),
        (false, true) => pick(1, mb),
        (true, true) => {
            let a_newer = if mb < ma {
                !(ma == 3 && mb == 1)
            } else {
                ma == 1 && mb == 3
            };
            if a_newer { pick(0, ma) } else { pick(1, mb) }
        }
    }
}

pub fn payload<'a>(storage: &'a [u8], catalog: &Catalog) -> Option<&'a [u8]> {
    let active = active(storage, catalog)?;
    let start = catalog.bank.slots[active.slot] + catalog.bank.payload_offset;
    Some(&storage[start..start + catalog.bank.payload_size])
}

/// Commit a settings payload with the firmware's store sequence: erase the
/// inactive slot, program the payload, write the next marker last, then
/// erase the previously active slot.
pub fn store(storage: &mut [u8], catalog: &Catalog, payload: &[u8]) -> Result<ActiveBank, String> {
    if payload.len() != catalog.bank.payload_size {
        return Err("settings payload has the wrong size".into());
    }
    let [a, b] = catalog.bank.slots;
    let size = catalog.bank.slot_size;
    if a.max(b) + size > storage.len() {
        return Err("settings bank slots exceed the NOR image".into());
    }
    let current = active(storage, catalog);
    let marker: u32 = match current.map(|a| a.marker) {
        Some(1) => 2,
        Some(2) => 3,
        _ => 1,
    };
    let target = if current.is_some_and(|a| a.slot == 0) {
        1
    } else {
        0
    };
    let (new, old) = if target == 0 { (a, b) } else { (b, a) };
    storage[new..new + size].fill(0xff);
    let start = new + catalog.bank.payload_offset;
    storage[start..start + payload.len()].copy_from_slice(payload);
    storage[new..new + 4].copy_from_slice(&marker.to_le_bytes());
    storage[old..old + size].fill(0xff);
    Ok(ActiveBank {
        slot: target,
        marker,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog, nor::MAIN_NOR_SIZE};

    #[test]
    fn selection_matches_the_firmware_bank_choice() {
        let catalog = &catalog::embedded()[0];
        let [a, b] = catalog.bank.slots;
        let mut nor = vec![0xff; MAIN_NOR_SIZE];
        assert_eq!(active(&nor, catalog), None);
        let mut set = |ma: u32, mb: u32| {
            nor[a..a + 4].copy_from_slice(&ma.to_le_bytes());
            nor[b..b + 4].copy_from_slice(&mb.to_le_bytes());
            active(&nor, catalog).map(|x| (x.slot, x.marker))
        };
        assert_eq!(set(1, 0xffffffff), Some((0, 1)));
        assert_eq!(set(0xffffffff, 2), Some((1, 2)));
        assert_eq!(set(1, 2), Some((1, 2)));
        assert_eq!(set(2, 1), Some((0, 2)));
        assert_eq!(set(3, 1), Some((1, 1)));
        assert_eq!(set(1, 3), Some((0, 1)));
        assert_eq!(set(2, 2), Some((1, 2)));
    }

    #[test]
    fn store_alternates_slots_and_wraps_markers() {
        for catalog in catalog::embedded() {
            let mut nor = vec![0xff; catalog.bank.slots[1] + catalog.bank.slot_size];
            let data = vec![0x5a; catalog.bank.payload_size];
            let mut seen = Vec::new();
            for _ in 0..4 {
                let bank = store(&mut nor, &catalog, &data).unwrap();
                assert_eq!(active(&nor, &catalog), Some(bank));
                assert_eq!(payload(&nor, &catalog).unwrap(), data.as_slice());
                let other = catalog.bank.slots[1 - bank.slot];
                assert!(
                    nor[other..other + catalog.bank.slot_size]
                        .iter()
                        .all(|&b| b == 0xff)
                );
                seen.push((bank.slot, bank.marker));
            }
            assert_eq!(seen, [(0, 1), (1, 2), (0, 3), (1, 1)]);
        }
    }
}
