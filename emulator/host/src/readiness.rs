//! Observable startup readiness shared by native and headless diagnostics.
use crate::indicators::{l6_mode_leds, rings};
use l6fw::Model;

#[derive(Default, Debug)]
pub struct BootReadiness {
    pub all_lit_seen: bool,
    candidate: Option<[u32; 8]>,
    stable_since: u32,
    /// State built from a state spec boots straight to its home screen, so
    /// there are no first-boot setup screens for the guest-time floor to
    /// wait out; the settled sweep alone establishes readiness.
    pub no_setup_screens: bool,
}
impl BootReadiness {
    pub fn new(no_setup_screens: bool) -> Self {
        Self {
            no_setup_screens,
            ..Default::default()
        }
    }
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
        // L6 lights its editing-mode lamp once MAIN has restored the mixer.
        // L6max first-boot setup shows its Date/Time editor at about 9 guest
        // seconds, after a card popup, with no lamp change; the floor covers it
        // until the screens have semantic recognizers. Wall time never
        // substitutes for this clock.
        let floor = match (self.no_setup_screens, model) {
            (false, Model::L6max) => 12_000,
            _ => 0,
        };
        guest_ms >= floor
            && self.candidate.is_some()
            && guest_ms.wrapping_sub(self.stable_since) >= 1_000
            && (model != Model::L6 || l6_mode_leds(rows).contains(&true))
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
        assert!(!boot.observe(Model::L6max, 20_000, rows(6)));
        assert!(!boot.observe(Model::L6max, 21_000, rows(8)));
        assert!(!boot.observe(Model::L6max, 21_100, [0; 8]));
        assert!(!boot.observe(Model::L6max, 22_099, [0; 8]));
        assert!(boot.observe(Model::L6max, 22_100, [0; 8]));
        assert!(!boot.observe(Model::L6max, 22_200, rows(1)));
        assert!(boot.observe(Model::L6max, 23_200, rows(1)));
    }
    #[test]
    fn l6_waits_for_its_mode_lamp() {
        let mut boot = BootReadiness::default();
        let level = |mut rows: [u32; 8]| {
            rows[4] |= 1 << 8;
            rows
        };
        assert!(!boot.observe(Model::L6, 2_000, rows(6)));
        assert!(!boot.observe(Model::L6, 3_000, rows(1)));
        assert!(!boot.observe(Model::L6, 4_000, rows(1)));
        assert!(boot.observe(Model::L6, 4_100, level(rows(1))));
    }
    #[test]
    fn state_without_setup_screens_needs_only_the_settled_sweep() {
        let mut floor = BootReadiness::default();
        let mut spec = BootReadiness::new(true);
        for boot in [&mut floor, &mut spec] {
            assert!(!boot.observe(Model::L6max, 3_200, rows(8)));
            assert!(!boot.observe(Model::L6max, 5_400, [0; 8]));
        }
        assert!(!floor.observe(Model::L6max, 6_400, [0; 8]));
        assert!(spec.observe(Model::L6max, 6_400, [0; 8]));
        assert!(floor.observe(Model::L6max, 12_000, [0; 8]));
    }
}
