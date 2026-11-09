//! Callback admission model. This is not connected to firmware hooks.
//!
//! A lease must be acquired before loading a callback pointer or borrowing
//! its state or shared scratch, and held until all accesses are complete.
//! Full live and file-mix leases are mutually exclusive because the stock
//! paths share effect state and scratch. A separate modeled live-bypass lease
//! permits only work proven independent of file mixing. Closing this gate does
//! not stop DMA or code that does not participate in it.
use core::sync::atomic::{AtomicU32, Ordering};

const LIVE: u32 = 1;
const FILE_MIX: u32 = 2;
const CLOSED: u32 = 4;
const RESERVED: u32 = 8;

#[derive(Clone, Copy)]
pub enum CallbackPath {
    Live,
    FileMix,
}

pub struct CallbackAdmission {
    state: AtomicU32,
}

impl CallbackAdmission {
    pub const fn new() -> Self {
        Self { state: AtomicU32::new(0) }
    }

    /// Makes one strong atomic admission attempt. A closed, occupied, or
    /// invalid state is rejected; callers must handle rejection safely.
    /// The target's atomic implementation may retry and needs deadline qualification.
    pub fn try_enter(&self, path: CallbackPath) -> Option<CallbackLease<'_>> {
        let bit = match path { CallbackPath::Live => LIVE, CallbackPath::FileMix => FILE_MIX };
        let state = self.state.load(Ordering::Acquire);
        if state != 0 && state != RESERVED { return None; }
        self.state.compare_exchange(state, state | bit, Ordering::AcqRel, Ordering::Acquire)
            .ok().map(|_| CallbackLease { gate: self, bit })
    }

    /// Claims the live side while file mixing owns the effect. The caller may
    /// only use resources proven disjoint from file mixing and must not load
    /// or invoke the effect callback. Acquire before touching live scratch.
    /// This model does not establish those resource boundaries in firmware.
    pub fn try_enter_live_bypass(&self) -> Option<LiveBypassLease<'_>> {
        let state = self.state.load(Ordering::Acquire);
        if state != FILE_MIX && state != (FILE_MIX | RESERVED) { return None; }
        self.state.compare_exchange(state, state | LIVE, Ordering::AcqRel, Ordering::Acquire)
            .ok().map(|_| LiveBypassLease { gate: self })
    }

    /// Stops new leases. Only one selector can hold the close guard; a caller
    /// must wait for `is_drained` before touching callback-owned state. The
    /// compare-exchange loop and target atomics need deadline qualification.
    pub fn try_close(&self) -> Option<CloseGuard<'_>> {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & !(LIVE | FILE_MIX) != 0 { return None; }
            match self.state.compare_exchange_weak(state, state | CLOSED, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(CloseGuard { gate: self }),
                Err(actual) => state = actual,
            }
        }
    }

    /// Reserves the next selector transition while admitting callbacks. One
    /// failed atomic attempt rejects the request before the stock fade starts.
    /// No firmware hook or deadline is established by this model.
    pub fn try_reserve_selector(&self) -> Option<SelectorReservation<'_>> {
        let state = self.state.load(Ordering::Acquire);
        if state & !(LIVE | FILE_MIX) != 0 { return None; }
        self.state.compare_exchange(state, state | RESERVED, Ordering::AcqRel, Ordering::Acquire)
            .ok().map(|_| SelectorReservation { gate: self })
    }
}

/// Exclusive selector intent; audio remains admitted until `close`.
#[must_use]
pub struct SelectorReservation<'a> {
    gate: &'a CallbackAdmission,
}

impl SelectorReservation<'_> {
    /// Prevent new leases after the fade; existing leases must still drain.
    pub fn close(&mut self) {
        self.gate.state.fetch_or(CLOSED, Ordering::AcqRel);
    }

    pub fn is_drained(&self) -> bool {
        self.gate.state.load(Ordering::Acquire) == (RESERVED | CLOSED)
    }

    /// Admit callbacks again without allowing a competing selector. Call only
    /// after all state changes protected by the closure are complete, or when
    /// abandoning a closure before making any changes. The model cannot prove
    /// that a firmware call site satisfies this requirement.
    pub fn resume_callbacks(&mut self) {
        self.gate.state.fetch_and(!CLOSED, Ordering::Release);
    }
}

impl Drop for SelectorReservation<'_> {
    fn drop(&mut self) {
        self.gate.state.fetch_and(!(RESERVED | CLOSED), Ordering::Release);
    }
}

/// Owns only the modeled live bypass, not the effect or file-mix state.
#[must_use]
pub struct LiveBypassLease<'a> {
    gate: &'a CallbackAdmission,
}

impl Drop for LiveBypassLease<'_> {
    fn drop(&mut self) {
        self.gate.state.fetch_and(!LIVE, Ordering::Release);
    }
}

#[must_use]
pub struct CallbackLease<'a> {
    gate: &'a CallbackAdmission,
    bit: u32,
}

impl Drop for CallbackLease<'_> {
    fn drop(&mut self) {
        self.gate.state.fetch_and(!self.bit, Ordering::Release);
    }
}

#[must_use]
pub struct CloseGuard<'a> {
    gate: &'a CallbackAdmission,
}

impl CloseGuard<'_> {
    /// True only after all admitted paths have released their leases.
    pub fn is_drained(&self) -> bool {
        self.gate.state.load(Ordering::Acquire) == CLOSED
    }
}

impl Drop for CloseGuard<'_> {
    fn drop(&mut self) {
        self.gate.state.fetch_and(!CLOSED, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_state_rejects_both_callback_paths() {
        let gate = CallbackAdmission::new();
        gate.state.store(16, Ordering::Relaxed);
        assert!(gate.try_enter(CallbackPath::Live).is_none());
        assert!(gate.try_enter(CallbackPath::FileMix).is_none());
        assert!(gate.try_enter_live_bypass().is_none());
    }

    #[test]
    fn selector_reservation_keeps_audio_open_until_close_and_drains_existing_work() {
        let gate = CallbackAdmission::new();
        let live = gate.try_enter(CallbackPath::Live).unwrap();
        let mut selector = gate.try_reserve_selector().unwrap();
        assert!(gate.try_reserve_selector().is_none());
        assert!(gate.try_close().is_none());
        assert!(!selector.is_drained());
        drop(live);
        let file = gate.try_enter(CallbackPath::FileMix).unwrap();
        let bypass = gate.try_enter_live_bypass().unwrap();
        selector.close();
        assert!(!selector.is_drained());
        assert!(gate.try_enter(CallbackPath::Live).is_none());
        assert!(gate.try_reserve_selector().is_none());
        drop(file);
        assert!(!selector.is_drained());
        drop(bypass);
        assert!(selector.is_drained());
        drop(selector);
        assert!(gate.try_enter(CallbackPath::Live).is_some());
    }

    #[test]
    fn cancelling_reservation_preserves_active_leases_and_reopens_gate() {
        for close_first in [false, true] {
            let gate = CallbackAdmission::new();
            let live = gate.try_enter(CallbackPath::Live).unwrap();
            let mut selector = gate.try_reserve_selector().unwrap();
            if close_first { selector.close(); }
            drop(selector);
            assert!(gate.try_enter(CallbackPath::FileMix).is_none());
            drop(live);
            assert!(gate.try_enter(CallbackPath::FileMix).is_some());
        }
    }

    #[test]
    fn resumed_callbacks_do_not_release_selector_reservation() {
        let gate = CallbackAdmission::new();
        let mut selector = gate.try_reserve_selector().unwrap();
        selector.close();
        assert!(selector.is_drained());
        selector.resume_callbacks();
        assert!(!selector.is_drained());
        assert!(gate.try_reserve_selector().is_none());
        assert!(gate.try_close().is_none());
        let live = gate.try_enter(CallbackPath::Live).unwrap();
        selector.close();
        assert!(!selector.is_drained());
        drop(live);
        assert!(selector.is_drained());
        selector.resume_callbacks();
        drop(selector);
        assert!(gate.try_reserve_selector().is_some());
    }

    #[test]
    fn bypass_excludes_other_live_work_and_drains_in_either_order() {
        for file_finishes_first in [false, true] {
            let gate = CallbackAdmission::new();
            assert!(gate.try_enter_live_bypass().is_none());
            let file = gate.try_enter(CallbackPath::FileMix).unwrap();
            let bypass = gate.try_enter_live_bypass().unwrap();
            assert!(gate.try_enter_live_bypass().is_none());
            assert!(gate.try_enter(CallbackPath::Live).is_none());
            let close = gate.try_close().unwrap();
            assert!(!close.is_drained());
            assert!(gate.try_enter_live_bypass().is_none());
            if file_finishes_first {
                drop(file);
                assert!(!close.is_drained());
                drop(bypass);
            } else {
                drop(bypass);
                assert!(!close.is_drained());
                drop(file);
            }
            assert!(close.is_drained());
            drop(close);
            assert!(gate.try_enter(CallbackPath::Live).is_some());
        }
    }

    #[test]
    fn bypass_outlives_file_without_allowing_a_new_owner() {
        let gate = CallbackAdmission::new();
        let file = gate.try_enter(CallbackPath::FileMix).unwrap();
        let bypass = gate.try_enter_live_bypass().unwrap();
        drop(file);
        assert!(gate.try_enter(CallbackPath::FileMix).is_none());
        assert!(gate.try_enter(CallbackPath::Live).is_none());
        assert!(gate.try_enter_live_bypass().is_none());
        drop(bypass);
        assert!(gate.try_enter(CallbackPath::FileMix).is_some());
    }
}
