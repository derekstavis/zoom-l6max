//! QEMU-only composition for a refresh into separately owned scratch.
//! It does not establish source quiescence, memory ownership, or timing.
use core::sync::atomic::{AtomicU32, Ordering};

use crate::boundary_pause::BoundaryPause;
use crate::handoff_protocol::CaptureProtocol;

/// Check both retained ranges against the observed L6max 1.10 allocator arena.
pub fn valid_heap_scratch(address: usize, snapshot: usize) -> bool {
    let len = crate::boundary_pause::CAPTURE_BYTES;
    crate::heap_range::valid_heap_allocation(address, len)
        && crate::heap_range::valid_heap_allocation(snapshot, len)
        && (address + len <= snapshot || address >= snapshot + len)
}

#[derive(Clone, Copy)]
pub struct RefreshRequest {
    pause: u32,
    done: u32,
}

pub struct RefreshHandoff<'a> {
    pub pause: &'a AtomicU32,
    pub capture: &'a AtomicU32,
}

impl RefreshHandoff<'_> {
    /// Begin a new generation only when the previous writer has retired.
    pub fn request(&self) -> Option<RefreshRequest> {
        let capture = CaptureProtocol(self.capture);
        capture.restart();
        let done = capture.pending_done_token()?;
        let pause = BoundaryPause(self.pause).request()?;
        Some(RefreshRequest { pause, done })
    }

    /// Claim the exact pending boundary before touching the scratch buffer.
    pub fn claim(&self) -> Option<(u32, u32)> {
        let pause = BoundaryPause(self.pause);
        let request = self.pause.load(Ordering::Acquire);
        if !pause.acknowledge(request) { return None; }
        let capture = CaptureProtocol(self.capture);
        match capture.claim() {
            Some((claim, 0)) => Some((request, claim)),
            Some((claim, _)) => {
                capture.finish(claim, 0, false);
                pause.finish(request);
                None
            }
            None => {
                pause.finish(request);
                None
            }
        }
    }

    /// Publish only if this boundary and capture generation are still valid.
    pub fn publish(&self, pause: u32, claim: u32) -> bool {
        BoundaryPause(self.pause).publish_capture(pause, &CaptureProtocol(self.capture), claim)
    }

    /// A reader owns scratch until it releases this exact completed claim.
    pub fn claim_completed(&self, request: RefreshRequest) -> Option<u32> {
        CaptureProtocol(self.capture).claim_install_for(request.done)
    }

    pub fn release(&self, claim: u32) -> bool {
        CaptureProtocol(self.capture).finish(claim, 15, true)
    }

    /// A stale timeout cannot invalidate a later capture generation.
    pub fn cancel(&self, request: RefreshRequest) {
        BoundaryPause(self.pause).finish(request.pause);
        CaptureProtocol(self.capture).cancel_epoch(request.done, false);
    }
}
