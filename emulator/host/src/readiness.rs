//! Observable startup readiness shared by native and headless diagnostics.
use crate::indicators::rings;
use l6fw::Model;

#[derive(Default, Debug)]
pub struct BootReadiness {
    pub all_lit_seen: bool,
    candidate: Option<[u32; 8]>,
    stable_since: u32,
}
impl BootReadiness {
    pub fn observe(&mut self, model: Model, guest_ms: u32, rows: [u32; 8]) -> bool {
        let count = if model == Model::L6 { 6 } else { 8 };
        let current = rings(rows);
        if current[..count].iter().all(|mask| *mask == 0x1ff) {
            self.all_lit_seen = true;
            self.candidate = None;
        } else if self.all_lit_seen {
            if self.candidate != Some(current) {
                self.candidate = Some(current);
                self.stable_since = guest_ms;
            }
        }
        // Preserve the validated setup floors until the firmware setup screens
        // have semantic recognizers. Wall time never substitutes for this clock.
        let floor = if model == Model::L6 { 18_000 } else { 40_000 };
        guest_ms >= floor
            && self.candidate.is_some()
            && guest_ms.wrapping_sub(self.stable_since) >= 1_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicators::RING_PINS;
    fn rows(count: usize) -> [u32; 8] {
        let mut rows = [0; 8];
        for pins in RING_PINS.iter().take(count) {
            for &(row, column) in pins {
                rows[row] |= 1 << (column ^ 7);
            }
        }
        rows
    }
    #[test]
    fn initial_dark_and_partial_sweeps_cannot_establish_readiness() {
        let mut boot = BootReadiness::default();
        assert!(!boot.observe(Model::L6max, 50_000, [0; 8]));
        assert!(!boot.observe(Model::L6max, 60_000, rows(6)));
        assert!(!boot.observe(Model::L6max, 70_000, [0; 8]));
    }
    #[test]
    fn model_ring_count_and_stability_are_enforced() {
        let mut boot = BootReadiness::default();
        assert!(!boot.observe(Model::L6, 20_000, rows(6)));
        assert!(!boot.observe(Model::L6, 21_000, [0; 8]));
        assert!(!boot.observe(Model::L6, 21_999, [0; 8]));
        assert!(boot.observe(Model::L6, 22_000, [0; 8]));
        assert!(!boot.observe(Model::L6, 22_100, rows(1)));
        assert!(boot.observe(Model::L6, 23_100, rows(1)));
    }
}
