//! QEMU-only capture publication protocol; it does not provide quiescence.
use core::sync::atomic::{AtomicU32, Ordering};

const PHASE: u32 = 3;
const WAIT: u32 = 0;
const COPYING: u32 = 1;
const DONE: u32 = 2;
const CANCELLED: u32 = 3;
// These bits are only interpreted in CANCELLED, where no step is retained.
const WRITER_ACTIVE: u32 = 4;
const REARM_REQUESTED: u32 = 8;
const STEP_MASK: u32 = 0x3c;
const EPOCH_MASK: u32 = !0x3f;
const EPOCH_INCREMENT: u32 = 0x40;

/// The control word and all copied memory must outlive both tasks.
pub struct CaptureProtocol<'a>(pub &'a AtomicU32);

impl CaptureProtocol<'_> {
    pub fn claim(&self) -> Option<(u32, usize)> {
        let state = self.0.load(Ordering::Acquire);
        // Step 15 is reserved for an installer reading a completed snapshot.
        if state & PHASE != WAIT || state & STEP_MASK == STEP_MASK { return None; }
        let claim = state | COPYING;
        self.0.compare_exchange(state, claim, Ordering::AcqRel, Ordering::Acquire).ok()?;
        Some((claim, ((state & STEP_MASK) >> 2) as usize))
    }

    /// Hold a completed snapshot against reuse while installation copies it.
    pub fn claim_install(&self) -> Option<u32> {
        let state = self.0.load(Ordering::Acquire);
        self.claim_install_for(state)
    }

    /// Hold only the completed snapshot identified by this exact token.
    pub fn claim_install_for(&self, state: u32) -> Option<u32> {
        if state & (PHASE | STEP_MASK) != DONE | STEP_MASK { return None; }
        let claim = (state & !PHASE) | COPYING;
        self.0.compare_exchange(state, claim, Ordering::AcqRel, Ordering::Acquire).ok()?;
        Some(claim)
    }

    /// Token a fresh single-step capture will have when published.
    pub fn pending_done_token(&self) -> Option<u32> {
        let state = self.0.load(Ordering::Acquire);
        if state & (PHASE | STEP_MASK) != WAIT || state & EPOCH_MASK == EPOCH_MASK { return None; }
        Some(state | STEP_MASK | DONE)
    }

    pub fn finish(&self, claim: u32, next_step: usize, complete: bool) -> bool {
        if next_step > 15 || claim & PHASE != COPYING { return false; }
        let next = (claim & EPOCH_MASK) | ((next_step as u32) << 2)
            | if complete { DONE } else { WAIT };
        if self.0.compare_exchange(claim, next, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return true;
        }
        // A cancelled writer must retire before a new capture can use its buffer.
        let cancelled_epoch = (claim & EPOCH_MASK).checked_add(EPOCH_INCREMENT).unwrap_or(EPOCH_MASK);
        let mut state = self.0.load(Ordering::Acquire);
        while state & (EPOCH_MASK | PHASE | WRITER_ACTIVE) == cancelled_epoch | CANCELLED | WRITER_ACTIVE {
            let rearm = state & REARM_REQUESTED != 0 && cancelled_epoch != EPOCH_MASK;
            let next = cancelled_epoch | if rearm { WAIT } else { CANCELLED };
            match self.0.compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(actual) => state = actual,
            }
        }
        false
    }

    /// Invalidates an incomplete claim; a completed snapshot may be retained.
    pub fn cancel(&self, preserve_done: bool) -> bool {
        self.cancel_matching(None, preserve_done)
    }

    /// Invalidate only the generation identified by a capture token.
    pub fn cancel_epoch(&self, token: u32, preserve_done: bool) -> bool {
        self.cancel_matching(Some(token & EPOCH_MASK), preserve_done)
    }

    fn cancel_matching(&self, expected_epoch: Option<u32>, preserve_done: bool) -> bool {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            if expected_epoch.is_some_and(|epoch| state & EPOCH_MASK != epoch) { return false; }
            if preserve_done && state & PHASE == DONE { return false; }
            let epoch = state & EPOCH_MASK;
            let already_cancelled = state & PHASE == CANCELLED;
            let next_epoch = if already_cancelled { epoch } else { epoch.checked_add(EPOCH_INCREMENT).unwrap_or(EPOCH_MASK) };
            let writer_active = state & PHASE == COPYING || already_cancelled && state & WRITER_ACTIVE != 0;
            let next = next_epoch | CANCELLED | if writer_active { WRITER_ACTIVE } else { 0 };
            match self.0.compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return true,
                Err(actual) => state = actual,
            }
        }
    }

    /// Rearms only after a selector explicitly enters AINR. An active writer
    /// retires first; epoch exhaustion remains cancelled until reset.
    pub fn arm(&self) {
        let mut state = self.0.load(Ordering::Acquire);
        while state & PHASE == CANCELLED && state & EPOCH_MASK != EPOCH_MASK {
            let next = if state & WRITER_ACTIVE != 0 { state | REARM_REQUESTED } else { state & EPOCH_MASK };
            match self.0.compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(actual) => state = actual,
            }
        }
    }

    /// Start a new AINR visit with a fresh generation. A completed snapshot
    /// from an earlier visit is not evidence of the current processor state.
    pub fn restart(&self) {
        self.cancel(false);
        self.arm();
    }

    pub fn done(&self) -> bool { self.0.load(Ordering::Acquire) & PHASE == DONE }

    pub fn current_done(&self, token: u32) -> bool {
        token & (PHASE | STEP_MASK) == DONE | STEP_MASK && self.0.load(Ordering::Acquire) == token
    }

    pub fn current_claim(&self, claim: u32) -> bool {
        claim & PHASE == COPYING && self.0.load(Ordering::Acquire) == claim
    }

    pub fn needs_reset(&self) -> bool {
        self.0.load(Ordering::Acquire) & (PHASE | STEP_MASK) != WAIT
    }
}
