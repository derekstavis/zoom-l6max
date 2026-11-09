#[path = "../../../patches/dual-efx/src/math.rs"]
pub mod math;
#[path = "../../../patches/dual-efx/src/velvet.rs"]
pub mod velvet;
#[path = "../../../patches/dual-efx/src/plateau.rs"]
pub mod plateau;
#[cfg(experimental_plateau_32k)]
#[path = "../../../patches/dual-efx/src/plateau_rate.rs"]
mod plateau_rate;
#[path = "../../../patches/dual-efx/src/routing.rs"]
pub mod routing;
#[path = "../../../patches/dual-efx/src/parameter_view.rs"]
pub mod parameter_view;
#[path = "../../../patches/dual-efx/src/parameters.rs"]
pub mod parameters;
#[path = "../../../patches/dual-efx/src/controls.rs"]
pub mod controls;
#[path = "../../../patches/dual-efx/src/admission.rs"]
pub mod admission;
#[path = "../probe/event_queue.rs"]
pub mod event_queue;
#[path = "../probe/handoff_protocol.rs"]
pub mod handoff_protocol;
#[path = "../probe/heap_range.rs"]
pub mod heap_range;
#[path = "../probe/boundary_pause.rs"]
pub mod boundary_pause;
#[path = "../probe/refresh_handoff.rs"]
pub mod refresh_handoff;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(experimental_plateau_32k)]
    #[test]
    fn plateau_32k_routed_converter_matches_explicit_core_and_resets() {
        let params = plateau::PlateauParameters { size: 4.0, predelay_ms: 500.0,
            mod_depth: 16.0, ..plateau::PlateauParameters::default() };
        let mut routed = Box::new(routing::IdlePlateau::new());
        let mut core = Box::new(plateau::PlateauReverb::new());
        let mut rate = plateau_rate::PlateauRate::new();
        routed.init(params);
        core.init(params);
        let mut peak = 0.0f32;
        let mut first = None;
        for frame in 0..72_000 {
            let input = if frame == 0 { 0.5 } else { 0.0 };
            let actual = routed.process_sample(input, input);
            let expected = rate.process(input, input, |l, r| core.process_sample(l, r));
            assert_eq!(actual, expected, "frame {frame}");
            let level = actual.0.abs().max(actual.1.abs());
            if level != 0.0 && first.is_none() { first = Some(frame); }
            peak = peak.max(level);
        }
        assert!(first.is_some_and(|frame| frame >= 23_999));
        assert!(peak > 0.0001);

        // Neither initialization nor clear may retain converter history.
        for reinitialize in [true, false] {
            routed.process_sample(0.5, 0.5);
            if reinitialize { routed.init(params); } else { routed.clear(); }
            for frame in 0..26_000 {
                assert_eq!(routed.process_sample(0.0, 0.0), (0.0, 0.0),
                    "stale output at frame {frame}, reinitialize={reinitialize}");
            }
        }
    }

    #[cfg(experimental_plateau_32k)]
    #[test]
    fn plateau_32k_inspector_model_accepts_full_range() {
        use parameters::{edit_encoder, snapshot_parameter, EfxParamId as Id, ParameterBank, ParameterHandoff};
        let mut original = Box::<routing::DualEfxProcessor>::new_uninit();
        let processor = routing::DualEfxProcessor::initialize_in_place(&mut original);
        assert_eq!(edit_encoder(processor, ParameterBank::Plateau, 0, i32::MAX), Some(Id::PlatPreDelay));
        assert_eq!(edit_encoder(processor, ParameterBank::Plateau, 1, i32::MAX), Some(Id::PlatSize));
        assert_eq!(processor.plateau.params.predelay_ms, 500.0);
        assert_eq!(processor.plateau.params.size, 4.0);
        let saved = ParameterHandoff::new();
        for id in [Id::PlatPreDelay, Id::PlatSize] {
            let snapshot = snapshot_parameter(id, processor);
            assert!(parameter_view::ParameterSnapshot::from_words(id as u32, snapshot.value.to_bits()).is_some());
            saved.record(id, processor);
        }
        let mut storage = Box::<routing::DualEfxProcessor>::new_uninit();
        let restored = routing::DualEfxProcessor::initialize_in_place(&mut storage);
        saved.restore(restored);
        assert_eq!(restored.plateau.params.predelay_ms, 500.0);
        assert_eq!(restored.plateau.params.size, 4.0);
        processor.plateau.init(restored.plateau.params);
        let mut peak = 0.0f32;
        for frame in 0..30_000 {
            let input = if frame == 0 { 0.5 } else { 0.0 };
            let actual = restored.plateau.process_sample(input, input);
            assert_eq!(actual, processor.plateau.process_sample(input, input), "frame {frame}");
            peak = peak.max(actual.0.abs()).max(actual.1.abs());
        }
        assert!(peak > 0.0001, "the restored predelay must eventually produce output");
    }

    #[cfg(experimental_plateau_32k)]
    #[test]
    fn plateau_32k_freeze_discards_converter_input_even_on_short_holds() {
        let params = plateau::PlateauParameters { predelay_ms: 0.0, diffusion: 0.0,
            ..plateau::PlateauParameters::default() };
        let mut reverb = Box::new(routing::IdlePlateau::new());
        for live in [false, true] {
            for pre_freeze_input in [false, true] {
                reverb.init(params);
                if pre_freeze_input {
                    // The first host frame has not yet reached the 32 kHz core.
                    assert_eq!(reverb.process_sample(0.5, 0.5), (0.0, 0.0));
                }
                let mut frozen = params;
                frozen.freeze = true;
                if live { reverb.set_live_parameters(frozen); } else { reverb.set_parameters(frozen); }
                assert_eq!(reverb.process_sample(0.75, 0.75), (0.0, 0.0));
                if live { reverb.set_live_parameters(params); } else { reverb.set_parameters(params); }
                for frame in 0..2_000 {
                    assert_eq!(reverb.process_sample(0.0, 0.0), (0.0, 0.0),
                        "frozen input replayed at frame {frame}, live={live}, pre={pre_freeze_input}");
                }
            }
        }
    }

    #[test]
    fn callback_admission_excludes_shared_scratch_and_rejects_duplicate_closes() {
        use admission::{CallbackAdmission, CallbackPath};
        let gate = CallbackAdmission::new();
        let live = gate.try_enter(CallbackPath::Live).unwrap();
        assert!(gate.try_enter(CallbackPath::FileMix).is_none());
        assert!(gate.try_enter(CallbackPath::Live).is_none());
        let close = gate.try_close().unwrap();
        assert!(gate.try_close().is_none());
        assert!(gate.try_enter(CallbackPath::Live).is_none(), "failed close reopened the gate");
        assert!(!close.is_drained());
        drop(live);
        assert!(close.is_drained());
        assert!(gate.try_enter(CallbackPath::FileMix).is_none());
        drop(close);
        let file = gate.try_enter(CallbackPath::FileMix).unwrap();
        assert!(gate.try_enter(CallbackPath::Live).is_none());
        let close = gate.try_close().unwrap();
        assert!(!close.is_drained());
        drop(file);
        assert!(close.is_drained());
        drop(close);
        assert!(gate.try_enter(CallbackPath::Live).is_some());
    }

    #[test]
    fn callback_admission_handles_timeout_and_pointer_load_after_close() {
        use admission::{CallbackAdmission, CallbackPath};
        use core::sync::atomic::{AtomicU32, Ordering};
        use std::sync::mpsc::sync_channel;
        use std::time::Duration;
        let gate = CallbackAdmission::new();
        let pointer = AtomicU32::new(7);
        let (entered_tx, entered_rx) = sync_channel(1);
        let (continue_tx, continue_rx) = sync_channel(1);
        let (loaded_tx, loaded_rx) = sync_channel(1);
        std::thread::scope(|scope| {
            let gate_ref = &gate;
            let pointer_ref = &pointer;
            scope.spawn(move || {
                let lease = gate_ref.try_enter(CallbackPath::Live).unwrap();
                entered_tx.send(()).unwrap();
                continue_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let old = pointer_ref.load(Ordering::Relaxed);
                drop(lease);
                loaded_tx.send(old).unwrap();
            });
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let timed_out = gate.try_close().unwrap();
            assert!(!timed_out.is_drained());
            drop(timed_out);
            assert!(gate.try_enter(CallbackPath::Live).is_none(), "active lease was lost on timeout");
            let close = gate.try_close().unwrap();
            assert!(!close.is_drained());
            continue_tx.send(()).unwrap();
            assert_eq!(loaded_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 7);
            assert!(close.is_drained());
            pointer.store(9, Ordering::Relaxed);
            drop(close);
        });
        let lease = gate.try_enter(CallbackPath::Live).unwrap();
        assert_eq!(pointer.load(Ordering::Relaxed), 9);
        drop(lease);
    }

    #[test]
    fn callback_admission_serializes_both_paths_during_concurrent_closes() {
        use admission::{CallbackAdmission, CallbackPath};
        use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Barrier;
        let gate = CallbackAdmission::new();
        let active = AtomicUsize::new(0);
        let changing = AtomicBool::new(false);
        let start = Barrier::new(5);
        std::thread::scope(|scope| {
            for path in [CallbackPath::Live, CallbackPath::Live,
                         CallbackPath::FileMix, CallbackPath::FileMix] {
                let (gate, active, changing, start) = (&gate, &active, &changing, &start);
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..3000 {
                        if let Some(lease) = gate.try_enter(path) {
                            assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                            assert!(!changing.load(Ordering::SeqCst));
                            std::thread::yield_now();
                            assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                            drop(lease);
                        }
                    }
                });
            }
            start.wait();
            for _ in 0..3000 {
                if let Some(close) = gate.try_close() {
                    if close.is_drained() {
                        changing.store(true, Ordering::SeqCst);
                        assert_eq!(active.load(Ordering::SeqCst), 0);
                        changing.store(false, Ordering::SeqCst);
                    }
                    drop(close);
                }
                std::thread::yield_now();
            }
        });
    }

    #[test]
    fn reserved_selector_excludes_competing_close_while_audio_keeps_running() {
        use admission::{CallbackAdmission, CallbackPath};
        use std::sync::mpsc::sync_channel;
        use std::time::Duration;
        let gate = CallbackAdmission::new();
        let (ready_tx, ready_rx) = sync_channel(1);
        let (finish_tx, finish_rx) = sync_channel(1);
        std::thread::scope(|scope| {
            let gate_ref = &gate;
            scope.spawn(move || {
                let mut selector = gate_ref.try_reserve_selector().unwrap();
                ready_tx.send(()).unwrap();
                finish_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                selector.close();
                assert!(selector.is_drained());
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(gate.try_reserve_selector().is_none());
            assert!(gate.try_close().is_none());
            let live = gate.try_enter(CallbackPath::Live).unwrap();
            drop(live);
            finish_tx.send(()).unwrap();
        });
        assert!(gate.try_close().is_some());
    }

    #[test]
    fn live_bypass_is_exclusive_and_selector_waits_for_it() {
        use admission::{CallbackAdmission, CallbackPath};
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Barrier;
        let gate = CallbackAdmission::new();
        let file = gate.try_enter(CallbackPath::FileMix).unwrap();
        let start = Barrier::new(3);
        let attempted = Barrier::new(3);
        let release = Barrier::new(3);
        let successes = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..2 {
                let (gate, start, attempted, release, successes) =
                    (&gate, &start, &attempted, &release, &successes);
                handles.push(scope.spawn(move || {
                    start.wait();
                    let lease = gate.try_enter_live_bypass();
                    if lease.is_some() { successes.fetch_add(1, Ordering::SeqCst); }
                    attempted.wait();
                    release.wait();
                    drop(lease);
                }));
            }
            start.wait();
            attempted.wait();
            assert_eq!(successes.load(Ordering::SeqCst), 1);
            let close = gate.try_close().unwrap();
            drop(file);
            assert!(!close.is_drained());
            assert!(gate.try_enter_live_bypass().is_none());
            release.wait();
            for handle in handles { handle.join().unwrap(); }
            assert!(close.is_drained());
            drop(close);
        });
        assert!(gate.try_enter(CallbackPath::Live).is_some());
    }

    #[test]
    fn ainr_boundary_pause_rejects_stale_acknowledgment_after_timeout() {
        use core::sync::atomic::AtomicU32;
        let word = AtomicU32::new(0);
        let pause = boundary_pause::BoundaryPause(&word);
        let old = pause.request().unwrap();
        assert!(pause.bypass(1));
        assert!(!pause.acknowledged(old));
        assert!(pause.finish(old));
        let new = pause.request().unwrap();
        assert!(!pause.acknowledge(old));
        assert!(!pause.acknowledged(new));
        assert!(!pause.acknowledged(old));
        assert!(!pause.finish(old));
        assert!(pause.acknowledge(new));
        assert!(pause.acknowledged(new));
        assert!(pause.bypass(1));
        assert!(!pause.bypass(2));
        assert!(!pause.acknowledged(new));
        assert!(!pause.finish(new));
        let following = pause.request().unwrap();
        assert!(pause.bypass(0));
        assert!(pause.acknowledged(following));
        assert!(pause.finish(following));
        assert!(!pause.bypass(0));
        let exhausted = AtomicU32::new(u32::MAX & !3);
        assert!(boundary_pause::BoundaryPause(&exhausted).request().is_none());
    }

    #[test]
    fn ainr_boundary_snapshot_packs_and_reconstructs_the_candidate_tail() {
        use boundary_pause::{CAPTURE_BYTES, CLONE_BYTES, DESCRIPTOR_OFFSET, SNAPSHOT_END, SNAPSHOT_START};
        let source: Vec<u8> = (0..SNAPSHOT_END).map(|i| (i.wrapping_mul(37) & 255) as u8).collect();
        let mut packed = vec![0xa5; CAPTURE_BYTES + 2];
        unsafe { boundary_pause::copy_snapshot(source.as_ptr(), packed.as_mut_ptr().add(1)); }
        assert_eq!(packed[0], 0xa5);
        assert_eq!(packed[CAPTURE_BYTES + 1], 0xa5);
        assert_eq!(&packed[1..=CAPTURE_BYTES], &source[SNAPSHOT_START..SNAPSHOT_END]);
        assert_eq!(&packed[1 + DESCRIPTOR_OFFSET..1 + DESCRIPTOR_OFFSET + 4], &source[0xba4c..0xba50]);

        let mut clone = vec![0xa5; CLONE_BYTES + 2];
        unsafe { boundary_pause::install_snapshot(packed.as_ptr().add(1), clone.as_mut_ptr().add(1)); }
        assert_eq!(clone[0], 0xa5);
        assert_eq!(clone[CLONE_BYTES + 1], 0xa5);
        let clone = &clone[1..=CLONE_BYTES];
        assert!(clone[..SNAPSHOT_START].iter().all(|&byte| byte == 0));
        assert_eq!(&clone[SNAPSHOT_START..SNAPSHOT_END], &source[SNAPSHOT_START..SNAPSHOT_END]);
        assert!(clone[SNAPSHOT_END..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn ainr_boundary_cannot_publish_after_pause_invalidation() {
        use core::sync::atomic::AtomicU32;
        let pause_word = AtomicU32::new(0);
        let capture_word = AtomicU32::new(0);
        let pause = boundary_pause::BoundaryPause(&pause_word);
        let capture = handoff_protocol::CaptureProtocol(&capture_word);
        let (unacknowledged_claim, _) = capture.claim().unwrap();
        let unacknowledged = pause.request().unwrap();
        assert!(!pause.publish_capture(unacknowledged, &capture, unacknowledged_claim));
        assert!(!capture.done());
        assert!(pause.finish(unacknowledged));
        let (claim, _) = capture.claim().unwrap();
        let request = pause.request().unwrap();
        assert!(pause.bypass(0));
        // The selector has just performed its last validity check.
        assert!(pause.acknowledged(request));
        // An unexpected callback arrives before publication.
        assert!(!pause.bypass(2));
        assert!(!pause.publish_capture(request, &capture, claim));
        assert!(!capture.done());
        let (retry, _) = capture.claim().unwrap();
        let next = pause.request().unwrap();
        assert!(pause.bypass(0));
        assert!(pause.publish_capture(next, &capture, retry));
        assert!(capture.done());
        assert!(!pause.bypass(0));
        assert!(capture.cancel(false));
        capture.arm();
        let (cancelled, _) = capture.claim().unwrap();
        let third = pause.request().unwrap();
        assert!(pause.bypass(0));
        assert!(capture.cancel(false));
        assert!(!pause.publish_capture(third, &capture, cancelled));
        assert!(!capture.done());
        assert!(!pause.bypass(0));
        capture.arm();
        let (timeout_claim, _) = capture.claim().unwrap();
        let timeout = pause.request().unwrap();
        assert!(pause.bypass(0));
        assert!(pause.finish(timeout));
        assert!(!pause.publish_capture(timeout, &capture, timeout_claim));
        assert!(!capture.done());
    }

    #[test]
    fn refresh_heap_ranges_are_bounded_aligned_and_disjoint() {
        use refresh_handoff::valid_heap_scratch as valid;
        let snapshot = 0x808fa040;
        let len = boundary_pause::CAPTURE_BYTES;
        assert!(valid(snapshot + len, snapshot));
        assert!(valid(snapshot - len, snapshot));
        assert!(!valid(snapshot, snapshot));
        assert!(!valid(snapshot + len - 8, snapshot));
        assert!(!valid(snapshot + len + 1, snapshot));
        assert!(!valid(0x808a4140, snapshot));
        assert!(valid(0x80921140 - len, snapshot));
        assert!(!valid(0x80921148 - len, snapshot));
        assert!(!valid(0x80921148, snapshot));
        assert!(!valid(snapshot + len, 0));
        assert!(!valid(usize::MAX - 7, snapshot));
        assert!(!valid(snapshot + len, usize::MAX - 7));
    }

    #[test]
    fn snapshot_heap_range_rejects_headers_sentinel_and_overflow() {
        use heap_range::valid_heap_allocation as valid;
        let len = boundary_pause::CAPTURE_BYTES;
        assert!(valid(0x808a4150, len));
        assert!(valid(0x80921140 - len, len));
        assert!(!valid(0x808a4148, len));
        assert!(!valid(0x808a4151, len));
        assert!(!valid(0x80921148 - len, len));
        assert!(!valid(usize::MAX - 7, len));
        assert!(!valid(0x808a4150, 0));
    }

    #[test]
    fn refresh_reclaim_keeps_primary_install_claim_until_free_finishes() {
        use core::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Barrier;
        let primary_word = AtomicU32::new(0);
        let primary = handoff_protocol::CaptureProtocol(&primary_word);
        let (writer, _) = primary.claim().unwrap();
        assert!(primary.finish(writer, 15, true));
        let installer = primary.claim_install().unwrap();
        // The refresh reader has finished; the primary claim still excludes
        // a selector that could otherwise acquire this scratch pointer.
        let slot = AtomicU32::new(0x80907c18);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let other = scope.spawn(|| {
                barrier.wait();
                assert!(primary.cancel(false));
                primary.arm();
                assert!(primary.claim().is_none(), "old installer still owns the primary claim");
                barrier.wait();
            });
            // Force cancellation between the validity check and pointer detach.
            assert!(primary.current_claim(installer));
            barrier.wait();
            barrier.wait();
            assert!(slot.compare_exchange(0x80907c18, 0, Ordering::AcqRel, Ordering::Acquire).is_ok());
            assert!(primary.claim().is_none(), "no new installer may use the detached buffer");
            other.join().unwrap();
        });
        assert!(!primary.finish(installer, 15, true));
        assert!(primary.claim().is_some(), "only retirement allows another capture");
    }

    #[test]
    fn refresh_retries_exclude_stale_writers_and_readers() {
        use core::sync::atomic::AtomicU32;
        let pause = AtomicU32::new(0);
        let capture = AtomicU32::new(0);
        let refresh = refresh_handoff::RefreshHandoff { pause: &pause, capture: &capture };

        // Cancellation before the audio task claims the request.
        let unclaimed = refresh.request().unwrap();
        refresh.cancel(unclaimed);
        assert!(refresh.claim().is_none());

        // A claimed writer must retire before its scratch is reused.
        let old = refresh.request().unwrap();
        let (boundary, writer) = refresh.claim().unwrap();
        refresh.cancel(old);
        assert!(refresh.claim_completed(old).is_none());
        assert!(refresh.request().is_none(), "timed-out writer still owns scratch");
        assert!(!refresh.publish(boundary, writer));

        // A late timeout cannot cancel a newer generation.
        let next = refresh.request().unwrap();
        refresh.cancel(old);
        let (boundary, writer) = refresh.claim().unwrap();
        assert!(refresh.publish(boundary, writer));
        assert!(refresh.claim_completed(old).is_none());
        let reader = refresh.claim_completed(next).unwrap();
        assert!(refresh.claim_completed(next).is_none());
        assert!(handoff_protocol::CaptureProtocol(&capture).current_claim(reader));

        // Scratch remains unavailable while an installer holds the reader.
        assert!(refresh.request().is_none(), "reader still owns scratch");
        assert!(!handoff_protocol::CaptureProtocol(&capture).current_claim(reader));
        assert!(!refresh.release(reader));
        let retry = refresh.request().unwrap();
        let (boundary, writer) = refresh.claim().unwrap();
        assert!(refresh.publish(boundary, writer));
        assert!(refresh.claim_completed(next).is_none());
        assert!(refresh.release(refresh.claim_completed(retry).unwrap()));
    }

    #[test]
    fn refresh_timeout_blocks_reuse_then_closed_gate_allows_retry() {
        use core::sync::atomic::AtomicU32;
        use std::sync::Barrier;
        let pause = AtomicU32::new(0);
        let capture = AtomicU32::new(0);
        let refresh = refresh_handoff::RefreshHandoff { pause: &pause, capture: &capture };
        let request = refresh.request().unwrap();
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let (boundary, writer) = refresh.claim().unwrap();
                barrier.wait();
                barrier.wait();
                assert!(!refresh.publish(boundary, writer));
            });
            barrier.wait();
            refresh.cancel(request);
            assert!(refresh.request().is_none());
            barrier.wait();
            worker.join().unwrap();
        });
        // After retirement, the outer audio boundary can service a retry
        // while the closed gate excludes live and file-mix state users.
        let gate = admission::CallbackAdmission::new();
        let active = gate.try_enter(admission::CallbackPath::Live).unwrap();
        let closed = gate.try_close().unwrap();
        assert!(!closed.is_drained());
        drop(active);
        assert!(closed.is_drained());
        let retry = refresh.request().unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                assert!(gate.try_enter(admission::CallbackPath::Live).is_none());
                assert!(gate.try_enter(admission::CallbackPath::FileMix).is_none());
                let (boundary, writer) = refresh.claim().unwrap();
                assert!(refresh.publish(boundary, writer));
            }).join().unwrap();
        });
        assert!(closed.is_drained());
        assert!(refresh.release(refresh.claim_completed(retry).unwrap()));
        drop(closed);
        assert!(gate.try_enter(admission::CallbackPath::Live).is_some());
    }

    #[test]
    fn ainr_capture_cancellation_rejects_stale_claims_across_rearm() {
        use core::sync::atomic::AtomicU32;
        let word = AtomicU32::new(0);
        let protocol = handoff_protocol::CaptureProtocol(&word);
        let (old_claim, step) = protocol.claim().unwrap();
        assert_eq!(step, 0);
        assert!(protocol.cancel(true));
        assert!(!protocol.finish(old_claim, 1, false));
        protocol.arm();
        let (new_claim, step) = protocol.claim().unwrap();
        assert_eq!(step, 0);
        assert!(!protocol.finish(old_claim, 15, true));
        assert!(!protocol.done());
        assert!(protocol.finish(new_claim, 1, false));
        let (claim, step) = protocol.claim().unwrap();
        assert_eq!(step, 1);
        assert!(protocol.finish(claim, 15, true));
        assert!(protocol.done());
        assert!(!protocol.cancel(true));
        assert!(protocol.done());
        assert!(protocol.cancel(false));
        assert!(!protocol.done());
        protocol.arm();
        assert_eq!(protocol.claim().unwrap().1, 0);
    }

    #[test]
    fn ainr_reentry_invalidates_completed_snapshot_and_waits_for_old_writer() {
        use core::sync::atomic::AtomicU32;
        let word = AtomicU32::new(0);
        let protocol = handoff_protocol::CaptureProtocol(&word);
        let (first, _) = protocol.claim().unwrap();
        assert!(protocol.finish(first, 15, true));
        let old_ready = (first & !0x3f) | (15 << 2) | 2;
        assert!(protocol.current_done(old_ready));

        protocol.restart();
        assert!(!protocol.current_done(old_ready));
        let (second, step) = protocol.claim().unwrap();
        assert_eq!(step, 0);
        protocol.restart();
        assert!(protocol.claim().is_none(), "old writer still owns snapshot");
        assert!(!protocol.finish(second, 15, true));
        let (third, step) = protocol.claim().unwrap();
        assert_eq!(step, 0);
        assert!(protocol.finish(third, 15, true));
        assert!(!protocol.current_done(old_ready));
    }

    #[test]
    fn ainr_capture_epoch_exhaustion_fails_closed() {
        use core::sync::atomic::AtomicU32;
        let word = AtomicU32::new(0xffff_ff80);
        let protocol = handoff_protocol::CaptureProtocol(&word);
        let (claim, _) = protocol.claim().unwrap();
        assert!(protocol.cancel(true));
        assert!(!protocol.finish(claim, 1, false));
        protocol.arm();
        assert!(protocol.claim().is_none());
        assert!(!protocol.done());
    }

    #[test]
    fn ainr_capture_waits_for_cancelled_writer_before_reusing_snapshot() {
        use core::sync::atomic::AtomicU32;
        let word = AtomicU32::new(0);
        let protocol = handoff_protocol::CaptureProtocol(&word);
        let (old, _) = protocol.claim().unwrap();
        assert!(protocol.cancel(true));
        protocol.arm();
        assert!(protocol.claim().is_none());
        assert!(!protocol.finish(old, 1, false));
        let (new, step) = protocol.claim().unwrap();
        assert_eq!(step, 0);
        assert!(protocol.cancel(true));
        protocol.arm();
        // A second cancellation revokes the pending rearm request.
        assert!(protocol.cancel(true));
        assert!(!protocol.finish(old, 1, false));
        assert!(protocol.claim().is_none());
        assert!(!protocol.finish(new, 1, false));
        assert!(protocol.claim().is_none());
        protocol.arm();
        assert_eq!(protocol.claim().unwrap().1, 0);
    }

    #[test]
    fn ainr_install_holds_snapshot_and_rejects_cancelled_generation() {
        use core::sync::atomic::AtomicU32;
        let word = AtomicU32::new(0);
        let protocol = handoff_protocol::CaptureProtocol(&word);
        let (capture, _) = protocol.claim().unwrap();
        assert!(protocol.finish(capture, 15, true));
        let install = protocol.claim_install().unwrap();
        assert!(protocol.claim_install().is_none());
        assert!(protocol.cancel(true));
        protocol.arm();
        assert!(protocol.claim().is_none());
        assert!(!protocol.finish(install, 15, true));
        assert!(!protocol.current_done((install & !3) | 2));
        let (capture, _) = protocol.claim().unwrap();
        assert!(protocol.finish(capture, 15, true));
        let install = protocol.claim_install().unwrap();
        assert!(protocol.finish(install, 15, true));
        let ready = (install & !3) | 2;
        assert!(protocol.current_done(ready));
        assert!(protocol.cancel(false));
        protocol.arm();
        let (capture, _) = protocol.claim().unwrap();
        assert!(protocol.finish(capture, 15, true));
        assert!(!protocol.current_done(ready));
    }

    #[test]
    fn control_queue_preserves_fifo_at_capacity_and_counter_wrap() {
        use core::sync::atomic::{AtomicU32, Ordering};
        use event_queue::{Queue, CAPACITY, WORDS_WITH_GENERATIONS};
        let words: [AtomicU32; WORDS_WITH_GENERATIONS] = [const { AtomicU32::new(0) }; WORDS_WITH_GENERATIONS];
        let start = u32::MAX - 31;
        words[0].store(start, Ordering::Relaxed);
        words[1].store(start, Ordering::Relaxed);
        let queue = Queue::new(&words, true);
        for n in 0..CAPACITY {
            assert_eq!(queue.push(n, || n ^ 0x55aa), Ok(start.wrapping_add(n + 1)));
        }
        assert_eq!(queue.push(999, || panic!("full queue requested generation")), Err(1));
        let (head, mut tail) = queue.batch().unwrap();
        for n in 0..CAPACITY {
            assert_ne!(tail, head);
            assert_eq!(queue.read(tail), (n, n ^ 0x55aa));
            tail = tail.wrapping_add(1);
            queue.commit(tail);
        }
        assert_eq!(tail, head);
        assert_eq!(queue.push(123, || 456), Ok(head.wrapping_add(1)));
        assert_eq!(queue.read(tail), (123, 456));
    }

    #[test]
    fn standalone_queue_does_not_access_generation_storage() {
        use core::sync::atomic::AtomicU32;
        use event_queue::{Queue, CAPACITY, HEADER_WORDS};
        let words: [AtomicU32; HEADER_WORDS + CAPACITY as usize] =
            [const { AtomicU32::new(0) }; HEADER_WORDS + CAPACITY as usize];
        let queue = Queue::new(&words, false);
        assert_eq!(queue.push(77, || panic!("standalone requested a generation")), Ok(1));
        let (head, tail) = queue.batch().unwrap();
        assert_eq!((head, tail), (1, 0));
        assert_eq!(queue.push(88, || 1), Ok(2));
        assert_eq!(queue.read(tail), (77, 0));
        queue.commit(tail + 1);
        assert_eq!(head, 1, "the original batch excludes the later event");
        assert_eq!(queue.batch(), Some((2, 1)));
        assert_eq!(queue.read(1), (88, 0));
    }

    #[test]
    fn control_queue_rejects_impossible_indices_without_consuming() {
        use core::sync::atomic::{AtomicU32, Ordering};
        use event_queue::{Queue, CAPACITY, WORDS_WITH_GENERATIONS};
        let words: [AtomicU32; WORDS_WITH_GENERATIONS] = [const { AtomicU32::new(0) }; WORDS_WITH_GENERATIONS];
        let queue = Queue::new(&words, true);
        for (head, tail) in [(65, 0), (0, 1), (u32::MAX, 0), (32, u32::MAX - 32)] {
            words[0].store(head, Ordering::Relaxed);
            words[1].store(tail, Ordering::Relaxed);
            assert_eq!(queue.batch(), None, "head {head}, tail {tail}");
            assert_eq!(words[1].load(Ordering::Relaxed), tail);
        }
        words[0].store(32, Ordering::Relaxed);
        words[1].store(32u32.wrapping_sub(CAPACITY), Ordering::Relaxed);
        assert_eq!(queue.batch(), Some((32, 32u32.wrapping_sub(CAPACITY))));
    }

    #[test]
    fn control_queue_publishes_events_with_their_generations_between_threads() {
        use core::sync::atomic::AtomicU32;
        use event_queue::{Queue, WORDS_WITH_GENERATIONS};
        let words: [AtomicU32; WORDS_WITH_GENERATIONS] = [const { AtomicU32::new(0) }; WORDS_WITH_GENERATIONS];
        let queue = Queue::new(&words, true);
        let mut mismatch = None;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for n in 0..100_000u32 {
                    while queue.push(n, || !n).is_err() { std::thread::yield_now(); }
                }
            });
            let mut expected = 0u32;
            while expected < 100_000 {
                let (head, mut tail) = queue.batch().unwrap();
                while tail != head {
                    let observed = queue.read(tail);
                    if observed != (expected, !expected) && mismatch.is_none() {
                        mismatch = Some((expected, observed));
                    }
                    expected += 1;
                    tail = tail.wrapping_add(1);
                    queue.commit(tail);
                }
                std::thread::yield_now();
            }
        });
        assert_eq!(mismatch, None);
    }

    fn paired_reverb_return() -> (Box<routing::DualEfxProcessor>, Box<routing::DualEfxProcessor>) {
        let mut actual = Box::new(routing::DualEfxProcessor::new());
        let mut expected = Box::new(routing::DualEfxProcessor::new());
        for processor in [&mut actual, &mut expected] {
            processor.init();
            processor.return_gain_a = 0.0;
        }
        (actual, expected)
    }

    #[test]
    fn shared_velvet_matches_independent_engines_across_path_switches() {
        let mut shared = Box::new(velvet::VelvetDecorrelator::new());
        let mut history = Box::new(velvet::MonoHistory::new());
        let mut direct = Box::new(velvet::VelvetDecorrelator::new());
        let mut cascade = Box::new(velvet::VelvetDecorrelator::new());
        for engine in [&mut shared, &mut direct, &mut cascade] {
            engine.init(velvet::Parameters::default());
        }
        let mut random = 7u32;
        for block in 0..2400 {
            if [10, 500, 1200].contains(&block) {
                let (length, density) = match block {
                    10 => (95.0, 3000.0),
                    500 => (8.0, 500.0),
                    _ => (35.0, 1700.0),
                };
                let mut params = direct.current;
                params.length_ms = length;
                params.density = density;
                params.transition_ms = 15.0;
                params.refresh_seconds = 0.1;
                direct.set_parameters(params);
                shared.set_parameters(params);
            }
            cascade.set_parameters(direct.current);
            for frame in 0..64 {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let a = if block % 300 < 200 { ((random >> 8) as f32 / 16777216.0 - 0.5) * 0.5 } else { 0.0 };
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let c = if block % 500 < 400 { ((random >> 8) as f32 / 16777216.0 - 0.5) * 0.3 } else { 0.0 };
                if (260..320).contains(&(block % 400)) {
                    let expected = direct.process_sample(a, c);
                    cascade.process_sample(0.0, 0.0);
                    let actual = shared.process_sample(a, c);
                    history.silence();
                    assert_eq!(actual, expected, "stereo block {block}, frame {frame}");
                } else {
                    let expected = (direct.process_sample(a, a), cascade.process_sample(c, c));
                    let actual = shared.process_shared_mono(&mut history, a, c);
                    assert_eq!(actual, expected, "routed block {block}, frame {frame}");
                }
            }
        }
    }

    #[test]
    fn velvet_amount_edits_fade_both_shared_histories() {
        let mut direct = Box::new(velvet::VelvetDecorrelator::new());
        let mut shared = Box::new(velvet::VelvetDecorrelator::new());
        let mut history = Box::new(velvet::MonoHistory::new());
        let mut params = velvet::Parameters::default();
        params.dynamic = false;
        params.amount = 0.0;
        for engine in [&mut direct, &mut shared] {
            engine.init(params);
            for channel in &mut engine.channels {
                channel.banks[0].count = 1;
                channel.banks[0].storage[0] = velvet::Tap { delay: 0, gain: 1.0 };
            }
        }
        for _ in 0..100 {
            assert_eq!(direct.process_sample(0.5, 0.5), (0.0, 0.0));
            assert_eq!(shared.process_shared_mono(&mut history, 0.5, 0.5), ((0.0, 0.0), (0.0, 0.0)));
        }
        params.amount = 1.0;
        direct.set_parameters(params);
        shared.set_parameters(params);
        let mut previous = 0.0f32;
        for frame in 0..64 {
            let stereo = direct.process_sample(0.5, 0.5);
            let (a, cascade) = shared.process_shared_mono(&mut history, 0.5, 0.5);
            assert_eq!(stereo, a);
            assert_eq!(a, cascade);
            assert!(stereo.0 >= previous && stereo.0 - previous < 0.02, "frame {frame}: {previous} -> {}", stereo.0);
            previous = stereo.0;
        }
        assert!(previous > 0.4);
        params.amount = 0.0;
        direct.set_parameters(params);
        shared.set_parameters(params);
        for frame in 0..72 {
            if frame == 8 {
                params.amount = 1.0;
                direct.set_parameters(params);
                shared.set_parameters(params);
            }
            let stereo = direct.process_sample(0.5, 0.5);
            let (a, cascade) = shared.process_shared_mono(&mut history, 0.5, 0.5);
            assert_eq!(stereo, a);
            assert_eq!(a, cascade);
            assert!((stereo.0 - previous).abs() < 0.02, "frame {frame}: {previous} -> {}", stereo.0);
            previous = stereo.0;
        }
        assert!(previous > 0.4);
        params.amount = 0.0;
        direct.set_parameters(params);
        direct.reset();
        for channel in &mut direct.channels {
            channel.banks[0].count = 1;
            channel.banks[0].storage[0] = velvet::Tap { delay: 0, gain: 1.0 };
        }
        assert_eq!(direct.process_sample(0.5, 0.5), (0.0, 0.0));
    }

    #[test]
    fn cascade_encoder_edits_ramp_reverb_input() {
        use parameters::{edit_encoder, ParameterBank};
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        let configure = |p: &mut routing::DualEfxProcessor| {
            p.init();
            let mut params = p.velvet.current;
            params.dynamic = false;
            p.velvet.init(params);
            for channel in &mut p.velvet.channels {
                channel.banks[0].count = 1;
                channel.banks[0].storage[0] = velvet::Tap { delay: 0, gain: 1.0 };
            }
        };
        configure(&mut processor);
        processor.process_bus_sample(0.0, 0.0, 0.5);
        let last_predelay = |p: &routing::DualEfxProcessor| {
            let d = &p.plateau.predelay;
            d.buffer[(d.write_pos + d.buffer.len() - 1) % d.buffer.len()]
        };
        let mut dc = plateau::OnePoleHP::new();
        dc.set_cutoff(20.0);
        let mut lp = plateau::OnePoleLP::new();
        lp.set_cutoff(14_080.0);
        let mut hp = plateau::OnePoleHP::new();
        hp.set_cutoff(13.75);
        let filter = |x, dc: &mut plateau::OnePoleHP, lp: &mut plateau::OnePoleLP, hp: &mut plateau::OnePoleHP| {
            hp.process(lp.process(dc.process(x)))
        };
        let sample = math::soft_clip(0.5);
        let mut previous = last_predelay(&processor);
        assert!((previous - filter(sample, &mut dc, &mut lp, &mut hp)).abs() < 1e-6);
        for _ in 0..64 {
            processor.process_bus_sample(0.0, 0.0, 0.5);
            previous = last_predelay(&processor);
            assert!((previous - filter(sample, &mut dc, &mut lp, &mut hp)).abs() < 1e-6);
        }
        edit_encoder(&mut processor, ParameterBank::Velvet, 7, -10).unwrap();
        let mut gain = 1.0f32;
        let mut step = -1.0 / 64.0;
        for frame in 0..80 {
            if frame == 20 {
                edit_encoder(&mut processor, ParameterBank::Velvet, 7, 10).unwrap();
                step = (1.0 - gain) / 64.0;
            }
            gain += step;
            processor.process_bus_sample(0.0, 0.0, 0.5);
            let next = last_predelay(&processor);
            assert!((next - previous).abs() < 0.02, "frame {frame}: {previous} -> {next}");
            assert!((next - filter(sample * gain, &mut dc, &mut lp, &mut hp)).abs() < 1e-6, "frame {frame}");
            previous = next;
        }
        edit_encoder(&mut processor, ParameterBank::Velvet, 7, -10).unwrap();
        step = -gain / 64.0;
        for frame in 0..64 {
            gain = if frame == 63 { 0.0 } else { gain + step };
            processor.process_bus_sample(0.0, 0.0, 0.5);
            let next = last_predelay(&processor);
            assert!(next <= previous && previous - next < 0.02, "fade-out frame {frame}");
            assert!((next - filter(sample * gain, &mut dc, &mut lp, &mut hp)).abs() < 1e-6, "fade-out frame {frame}");
            previous = next;
        }

        // The stereo-send path shares the ramp, and an explicit direct write
        // still takes effect immediately even if it interrupts an encoder edit.
        configure(&mut processor);
        dc.clear();
        lp.clear();
        hp.clear();
        let mut left = [0.0];
        let mut right = [0.0];
        processor.process_stereo_send(&[0.5], &[0.5], &mut left, &mut right, true, true);
        previous = last_predelay(&processor);
        assert!((previous - filter(0.5 + sample, &mut dc, &mut lp, &mut hp)).abs() < 1e-6);
        for _ in 0..64 {
            processor.process_stereo_send(&[0.5], &[0.5], &mut left, &mut right, true, true);
            previous = last_predelay(&processor);
            assert!((previous - filter(0.5 + sample, &mut dc, &mut lp, &mut hp)).abs() < 1e-6);
        }
        edit_encoder(&mut processor, ParameterBank::Velvet, 7, -10).unwrap();
        for frame in 0..8 {
            processor.process_stereo_send(&[0.5], &[0.5], &mut left, &mut right, true, true);
            let next = last_predelay(&processor);
            assert!((next - previous).abs() < 0.02, "stereo frame {frame}");
            assert!((next - filter(0.5 + sample * (1.0 - (frame + 1) as f32 / 64.0), &mut dc, &mut lp, &mut hp)).abs() < 1e-6);
            previous = next;
        }
        processor.cascade_gain = 0.25;
        processor.process_stereo_send(&[0.5], &[0.5], &mut left, &mut right, true, true);
        assert!((last_predelay(&processor) - filter(0.5 + sample * 0.25, &mut dc, &mut lp, &mut hp)).abs() < 1e-6);
    }

    #[test]
    fn shared_velvet_handles_different_history_positions() {
        let mut shared = Box::new(velvet::VelvetDecorrelator::new());
        let mut history = Box::new(velvet::MonoHistory::new());
        let mut direct = Box::new(velvet::VelvetDecorrelator::new());
        let mut cascade = Box::new(velvet::VelvetDecorrelator::new());
        let mut params = velvet::Parameters::default();
        params.dynamic = false;
        for engine in [&mut shared, &mut direct, &mut cascade] {
            engine.init(params);
        }
        // An independent stereo frame moves only the direct history cursor.
        shared.process_sample(0.0, 0.0);
        direct.process_sample(0.0, 0.0);
        for frame in 0..10_000 {
            let a = if frame == 0 { 0.5 } else { 0.0 };
            let c = if frame == 1 { -0.25 } else { 0.0 };
            let expected = (direct.process_sample(a, a), cascade.process_sample(c, c));
            let actual = shared.process_shared_mono(&mut history, a, c);
            assert_eq!(actual, expected, "frame {frame}");
        }
    }

    fn idle_reverb_pair() -> (Box<routing::IdlePlateau>, Box<plateau::PlateauReverb>) {
        let mut idle = Box::new(routing::IdlePlateau::new());
        let mut reference = Box::new(plateau::PlateauReverb::new());
        idle.init(plateau::PlateauParameters::default());
        reference.init(plateau::PlateauParameters::default());
        (idle, reference)
    }

    fn assert_idle_reverb_frame(idle: &mut routing::IdlePlateau,
                                reference: &mut plateau::PlateauReverb,
                                input: f32, frame: usize) {
        let actual = idle.process_sample(input, input);
        let expected = reference.process_sample(input, input);
        assert_eq!((actual.0.to_bits(), actual.1.to_bits()),
                   (expected.0.to_bits(), expected.1.to_bits()), "frame {frame}");
    }

    #[test]
    fn idle_reverb_matches_full_processing_through_edits_and_resume() {
        let (mut idle, mut reference) = idle_reverb_pair();
        for frame in 0..90_000 {
            if frame == 4_000 || frame == 17_000 || frame == 35_000 {
                let mut params = reference.params;
                params.size = if frame == 17_000 { 0.2 } else { 1.2 };
                params.predelay_ms = if frame == 4_000 { 190.0 } else { 0.0 };
                params.freeze = frame == 17_000;
                params.low_cut_hz = if frame == 17_000 { 1000.0 } else { 20.0 };
                params.high_cut_hz = if frame == 17_000 { 1000.0 } else { 16000.0 };
                idle.set_parameters(params);
                reference.set_parameters(params);
            }
            let input = if frame == 50_000 { 0.5 } else { 0.0 };
            assert_idle_reverb_frame(&mut idle, &mut reference, input, frame);
        }
    }

    #[test]
    fn idle_reverb_mutable_access_preserves_seeded_history() {
        let (mut idle, mut reference) = idle_reverb_pair();
        idle.predelay.write(0.75);
        reference.predelay.write(0.75);
        let mut peak = 0.0f32;
        for frame in 0..20_000 {
            let actual = idle.process_sample(0.0, 0.0);
            let expected = reference.process_sample(0.0, 0.0);
            assert_eq!(actual, expected, "frame {frame}");
            peak = peak.max(actual.0.abs()).max(actual.1.abs());
        }
        assert!(peak > 0.001, "test must observe the seeded history");
    }

    #[test]
    fn plateau_live_encoder_edits_match_full_parameter_updates() {
        use parameters::{edit_encoder, ParameterBank};
        let (mut fast, mut reference) = paired_reverb_return();
        let mut peak = 0.0f32;
        for frame in 0..100_000 {
            let slow = frame < 40_000;
            let elapsed = if slow { frame } else { frame - 40_000 };
            let period = if slow { 997 } else { 13 };
            if elapsed % period == 0 {
                let turn = elapsed / period;
                // The rapid sequence reverses predelay and diffusion before
                // their previous smoothing ramps have finished.
                let encoder = if slow { [0, 1, 2, 5, 6, 7, 3, 4][turn % 8] }
                    else { [0, 0, 5, 5, 1, 7, 6, 2, 3, 4][turn % 10] };
                let delta = if turn % 2 == 0 { 3 } else { -4 };
                let mut p = reference.plateau.params;
                match encoder {
                    0 => p.predelay_ms = math::clamp(p.predelay_ms + delta as f32 * 2.0, 0.0, 190.0),
                    1 => p.size += delta as f32 * 0.02,
                    2 => p.decay += delta as f32 * 0.02,
                    3 => p.low_cut_hz = math::clamp(p.low_cut_hz + delta as f32 * 20.0, 20.0, 1000.0),
                    4 => p.high_cut_hz = math::clamp(p.high_cut_hz + delta as f32 * 250.0, 1000.0, 16000.0),
                    5 => p.diffusion += delta as f32 * 0.05,
                    6 => p.mod_depth += delta as f32 * 0.25,
                    _ => p.freeze = delta > 0,
                }
                reference.plateau.set_parameters(p);
                assert!(edit_encoder(&mut fast, ParameterBank::Plateau, encoder, delta).is_some());
            }
            let input = if frame % 131 == 0 { 0.5 } else { 0.0 };
            let actual = fast.process_bus_sample(0.0, input, 0.0);
            let expected = reference.process_bus_sample(0.0, input, 0.0);
            assert_eq!(actual, expected, "frame {frame}, edit period {period}");
            peak = peak.max(actual.0.abs()).max(actual.1.abs());
        }
        assert!(peak > 0.001, "test must observe a reverb response");
    }

    #[test]
    fn plateau_live_edits_clamp_like_full_setter() {
        use parameters::{edit_encoder, ParameterBank};
        let (mut fast, mut reference) = paired_reverb_return();
        for (encoder, delta) in [(0, i32::MAX), (0, i32::MIN), (1, i32::MAX),
                                 (1, i32::MIN), (5, i32::MAX), (5, i32::MIN)] {
            let mut p = reference.plateau.params;
            let step = delta as f32;
            match encoder {
                0 => p.predelay_ms = math::clamp(p.predelay_ms + step * 2.0, 0.0, 190.0),
                1 => p.size += step * 0.02,
                5 => p.diffusion += step * 0.05,
                _ => unreachable!(),
            }
            reference.plateau.set_parameters(p);
            assert!(edit_encoder(&mut fast, ParameterBank::Plateau, encoder, delta).is_some());
            assert_eq!(fast.plateau.params.predelay_ms, reference.plateau.params.predelay_ms);
            assert_eq!(fast.plateau.params.size, reference.plateau.params.size);
            assert_eq!(fast.plateau.params.diffusion, reference.plateau.params.diffusion);
            if encoder == 1 && delta == i32::MIN {
                assert_eq!(fast.plateau.params.size, 0.01);
            }
            for frame in 0..128 {
                let input = if frame == 0 { 0.5 } else { 0.0 };
                assert_eq!(fast.process_bus_sample(0.0, input, 0.0),
                           reference.process_bus_sample(0.0, input, 0.0));
            }
        }
    }

    #[test]
    fn parameter_handoff_restores_each_latest_applied_value() {
        use parameters::{edit_encoder, format_parameter, snapshot_parameter, EfxParamId as Id, ParameterBank, ParameterHandoff};
        let mut source = Box::new(routing::DualEfxProcessor::new());
        let mut restored = Box::new(routing::DualEfxProcessor::new());
        source.init();
        restored.init();
        let handoff = ParameterHandoff::new();
        let mut ids = Vec::new();
        for bank in [ParameterBank::Velvet, ParameterBank::Plateau] {
            for encoder in 0..8 {
                let delta = if bank == ParameterBank::Velvet && (encoder == 5 || encoder == 7) { -1 } else { 1 };
                let id = edit_encoder(&mut source, bank, encoder, delta).unwrap();
                handoff.record(id, &source);
                ids.push(id);
            }
        }
        // A second edit to one slot must replace its earlier value.
        let id = edit_encoder(&mut source, ParameterBank::Plateau, 2, 2).unwrap();
        handoff.record(id, &source);
        handoff.restore(&mut restored);
        for id in ids {
            assert_eq!(snapshot_parameter(id, &restored).value.to_bits(),
                       snapshot_parameter(id, &source).value.to_bits(), "{id:?}");
        }
        // Applying the handoff to fresh state does not claim to restore tails.
        assert_eq!(restored.velvet.current.amount.to_bits(), source.velvet.current.amount.to_bits());
        assert_eq!(restored.plateau.params.size.to_bits(), source.plateau.params.size.to_bits());
        assert_eq!(restored.plateau.params.decay.to_bits(), source.plateau.params.decay.to_bits());
        assert_eq!(restored.plateau.params.diffusion.to_bits(), source.plateau.params.diffusion.to_bits());
        assert_eq!(restored.cascade_gain.to_bits(), source.cascade_gain.to_bits());

        // Refresh edits can also clamp the transition length, so they must
        // replace both stored values before the next selection.
        let transition = edit_encoder(&mut source, ParameterBank::Velvet, 6, 40).unwrap();
        handoff.record(transition, &source);
        let refresh = edit_encoder(&mut source, ParameterBank::Velvet, 4, -10).unwrap();
        handoff.record(refresh, &source);
        restored.init();
        handoff.restore(&mut restored);
        assert_eq!(source.velvet.current.transition_ms, 450.0);
        assert_eq!(restored.velvet.current.refresh_seconds, source.velvet.current.refresh_seconds);
        assert_eq!(restored.velvet.current.transition_ms, source.velvet.current.transition_ms);

        // Preserve the requested density across a parameter handoff.
        source.init();
        restored.init();
        let mut params = source.velvet.current;
        params.length_ms = 95.0;
        params.density = 3000.0;
        source.velvet.set_parameters(params);
        let displayed = snapshot_parameter(Id::VndDensity, &source);
        assert!((displayed.value - 3000.0).abs() < 0.01);
        let formatted = format_parameter(Id::VndDensity, &source);
        assert_eq!(&formatted.value_str[..formatted.value_len], b"3000.0 pps");
        let handoff = ParameterHandoff::new();
        handoff.record(Id::VndTimeSpread, &source);
        handoff.record(Id::VndDensity, &source);
        handoff.restore(&mut restored);
        assert_eq!(restored.velvet.current.density, 3000.0);
        let mut shorter = restored.velvet.current;
        shorter.length_ms = 10.0;
        restored.velvet.set_parameters(shorter);
        assert_eq!(snapshot_parameter(Id::VndDensity, &restored).value, 3000.0);
    }

    #[test]
    fn parameter_handoff_restores_plateau_before_the_first_audio_frame() {
        use parameters::{EfxParamId as Id, ParameterHandoff};
        let mut source = Box::new(routing::DualEfxProcessor::new());
        let mut restored = Box::new(routing::DualEfxProcessor::new());
        let mut expected = Box::new(routing::DualEfxProcessor::new());
        source.init();
        restored.init();
        expected.init();
        let mut params = source.plateau.params;
        params.predelay_ms = 0.0;
        params.diffusion = 0.0;
        params.low_cut_hz = 1000.0;
        params.high_cut_hz = 1000.0;
        source.plateau.set_parameters(params);
        let handoff = ParameterHandoff::new();
        for id in [Id::PlatPreDelay, Id::PlatDiffusion, Id::PlatLowCut, Id::PlatHighCut] {
            handoff.record(id, &source);
        }
        handoff.restore(&mut restored);
        expected.plateau.init(params);
        assert_eq!(restored.plateau.l_lp.a, expected.plateau.l_lp.a);
        assert_eq!(restored.plateau.l_apf1.gain, expected.plateau.l_apf1.gain);
        for frame in 0..16_000 {
            let input = if frame == 0 { 0.5 } else { 0.0 };
            assert_eq!(restored.plateau.process_sample(input, input),
                       expected.plateau.process_sample(input, input), "frame {frame}");
        }
    }

    #[test]
    fn parameter_handoff_replays_full_control_state_from_first_frame() {
        use parameters::{EfxParamId as Id, ParameterHandoff};
        let ids = [Id::VndAmount, Id::VndTimeSpread, Id::VndDensity, Id::VndDecay,
            Id::VndRefresh, Id::VndMode, Id::VndTransition, Id::PlatPreDelay,
            Id::PlatSize, Id::PlatDecay, Id::PlatLowCut, Id::PlatHighCut,
            Id::PlatDiffusion, Id::PlatModDepth, Id::PlatFreeze, Id::GlobalCascade];
        for case in 0..4 {
            let mut source = Box::new(routing::DualEfxProcessor::new());
            let mut actual = Box::new(routing::DualEfxProcessor::new());
            let mut expected = Box::new(routing::DualEfxProcessor::new());
            source.init();
            actual.init();
            expected.init();
            let mut velvet = source.velvet.current;
            velvet.amount = [0.0, 0.2, 0.8, 1.0][case];
            velvet.length_ms = [1.0, 25.0, 60.0, 100.0][case];
            velvet.density = [10.0, 500.0, 2000.0, 4000.0][case];
            velvet.decay_db = [-120.0, -60.0, -20.0, 0.0][case];
            velvet.refresh_seconds = [0.1, 0.5, 5.0, 60.0][case];
            velvet.transition_ms = [90.0, 100.0, 500.0, 2000.0][case];
            velvet.dynamic = case % 2 == 0;
            source.velvet.set_parameters(velvet);
            let mut plateau = source.plateau.params;
            plateau.predelay_ms = [0.0, 10.0, 100.0, 190.0][case];
            plateau.size = [0.2, 0.5, 1.0, 1.2][case];
            plateau.decay = [0.1, 0.4, 0.8, 0.999][case];
            plateau.low_cut_hz = [20.0, 80.0, 500.0, 1000.0][case];
            plateau.high_cut_hz = [1000.0, 6500.0, 12000.0, 16000.0][case];
            plateau.diffusion = [0.0, 0.25, 0.75, 1.0][case];
            plateau.mod_depth = [0.0, 4.0, 8.0, 16.0][case];
            plateau.freeze = case == 3;
            source.plateau.set_parameters(plateau);
            source.cascade_gain = [0.0, 0.25, 0.75, 1.0][case];
            let handoff = ParameterHandoff::new();
            for id in ids { handoff.record(id, &source); }
            handoff.restore(&mut actual);
            expected.velvet.init(source.velvet.current);
            expected.plateau.init(source.plateau.params);
            expected.cascade_gain = source.cascade_gain;
            let mut peak = 0.0f32;
            for frame in 0..14_000 {
                let a = if frame % 257 == 0 { 0.3 } else { 0.0 };
                let b = if frame % 113 == 0 { -0.2 } else { 0.0 };
                let cascade = if frame % 379 == 0 { 0.1 } else { 0.0 };
                let output = actual.process_bus_sample(a, b, cascade);
                assert_eq!(output, expected.process_bus_sample(a, b, cascade), "case {case}, frame {frame}");
                peak = peak.max(output.0.abs()).max(output.1.abs());
            }
            assert!(peak > 0.0001, "case {case} did not exercise output");
        }
    }

    #[test]
    fn velvet_refresh_encoder_covers_full_range_after_restore() {
        use parameters::{edit_encoder, snapshot_parameter, EfxParamId, ParameterBank, ParameterHandoff, ParameterSnapshot};
        let mut source_storage = Box::<routing::DualEfxProcessor>::new_uninit();
        let source = routing::DualEfxProcessor::initialize_in_place(&mut source_storage);
        assert_eq!(edit_encoder(source, ParameterBank::Velvet, 4, 110), Some(EfxParamId::VndRefresh));
        assert_eq!(source.velvet.current.refresh_seconds, 60.0);
        edit_encoder(source, ParameterBank::Velvet, 4, 1);
        assert_eq!(source.velvet.current.refresh_seconds, 60.0);
        let snapshot = snapshot_parameter(EfxParamId::VndRefresh, source);
        assert!(ParameterSnapshot::from_words(EfxParamId::VndRefresh as u32, snapshot.value.to_bits()).is_some());

        let handoff = ParameterHandoff::new();
        handoff.record(EfxParamId::VndRefresh, source);
        let mut restored_storage = Box::<routing::DualEfxProcessor>::new_uninit();
        let restored = routing::DualEfxProcessor::initialize_in_place(&mut restored_storage);
        handoff.restore(restored);
        assert_eq!(restored.velvet.current.refresh_seconds, 60.0);
        edit_encoder(restored, ParameterBank::Velvet, 4, -1);
        assert_eq!(restored.velvet.current.refresh_seconds, 59.5);
        edit_encoder(restored, ParameterBank::Velvet, 4, -200);
        assert_eq!(restored.velvet.current.refresh_seconds, 0.1);
    }

    #[test]
    fn velvet_density_display_tracks_the_generated_tap_count() {
        use parameters::{snapshot_parameter, EfxParamId, ParameterSnapshot};
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        for (spread, requested, expected_taps, expected_density) in [
            (1.0, 100.0, 2, 2000.0),
            (1.2, 3000.0, 4, 192_000.0 / 57.0),
            (10.0, 440.0, 4, 400.0),
            (95.0, 3000.0, 285, 3000.0),
            (1.125, 4000.0, 5, 240_000.0 / 54.0),
            (100.0, 4000.0, 400, 4000.0),
        ] {
            let mut params = processor.velvet.current;
            params.length_ms = spread;
            params.density = requested;
            processor.velvet.init(params);
            assert_eq!(processor.velvet.channels[0].banks[0].count, expected_taps);
            let snapshot = snapshot_parameter(EfxParamId::VndDensity, &processor);
            assert!((snapshot.value - expected_density).abs() < 0.01);
            assert_eq!(processor.velvet.current.density, requested);
            assert!(ParameterSnapshot::from_words(EfxParamId::VndDensity as u32, snapshot.value.to_bits()).is_some());
        }
    }

    #[test]
    fn efx_select_defers_short_press_and_distinguishes_hold() {
        use controls::EfxSelectAction as Action;
        let mut gesture = controls::EfxSelectGesture::default();
        assert_eq!(gesture.edge(true, 100, 750), Action::None);
        assert_eq!(gesture.edge(true, 200, 750), Action::None);
        assert_eq!(gesture.edge(false, 249, 750), Action::None);
        assert_eq!(gesture.edge(false, 250, 750), Action::None);
        assert_eq!(gesture.edge(true, 300, 750), Action::None);
        assert_eq!(gesture.edge(false, 400, 750), Action::SelectNext);
        assert_eq!(gesture.edge(true, 1000, 750), Action::None);
        assert_eq!(gesture.edge(false, 1750, 750), Action::ToggleInspector);
        assert_eq!(gesture.edge(true, u32::MAX - 99, 750), Action::None);
        assert_eq!(gesture.edge(false, 650, 750), Action::ToggleInspector);
        assert_eq!(gesture.edge(true, 2000, 750), Action::None);
        gesture.cancel();
        assert_eq!(gesture.edge(false, 2100, 750), Action::None);
    }

    #[test]
    fn efx_select_sequence_gaps_discard_unfinished_gestures() {
        use controls::EfxSelectAction as Action;
        let mut gesture = controls::EfxSelectGesture::default();
        let events = [
            // An ordinary short press.
            (true, 100, 1, Action::None), (false, 250, 2, Action::SelectNext),
            // The first release is lost; the next press starts afresh.
            (true, 1000, 3, Action::None), (true, 3000, 5, Action::None),
            (false, 3150, 6, Action::SelectNext),
            // A lost release and following press cannot create a hold.
            (true, 4000, 7, Action::None), (false, 6000, 10, Action::None),
            (true, 7000, 11, Action::None), (false, 8500, 12, Action::ToggleInspector),
            // An unassociated event cancels; tick and sequence can wrap.
            (true, 9000, 13, Action::None), (false, 10000, 0, Action::None),
            (true, u32::MAX - 99, u32::MAX, Action::None),
            (false, 650, 1, Action::ToggleInspector),
        ];
        for (pressed, tick, sequence, expected) in events {
            assert_eq!(gesture.sequenced_edge(pressed, tick, sequence, 750), expected,
                "unexpected gesture for sequence {sequence}");
        }
        // Switching between tagged and untagged sources cannot reuse a press.
        assert_eq!(gesture.sequenced_edge(true, 100, 2, 750), Action::None);
        assert_eq!(gesture.edge(false, 1000, 750), Action::None);
        assert_eq!(gesture.edge(true, 2000, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(false, 3000, 3, 750), Action::None);
    }

    #[test]
    fn efx_select_repeated_press_cannot_turn_a_short_press_into_a_hold() {
        use controls::EfxSelectAction as Action;
        let mut gesture = controls::EfxSelectGesture::default();
        // The release was lost before the decoder assigned sequence numbers.
        assert_eq!(gesture.sequenced_edge(true, 100, 1, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(true, 3000, 2, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(true, 3050, 3, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(false, 3150, 4, 750), Action::None);
        // A clean following gesture still works.
        assert_eq!(gesture.sequenced_edge(true, 4000, 5, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(false, 4100, 6, 750), Action::SelectNext);
        assert_eq!(gesture.sequenced_edge(true, 5000, 7, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(false, 6000, 8, 750), Action::ToggleInspector);
        // A detected gap discards ambiguity and permits a fresh press.
        assert_eq!(gesture.sequenced_edge(true, 7000, 9, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(true, 7100, 10, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(true, 8000, 12, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(false, 8100, 13, 750), Action::SelectNext);
        // Losing the tag explicitly cancels the ambiguous gesture.
        assert_eq!(gesture.sequenced_edge(true, 9000, 14, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(true, 9100, 15, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(false, 9200, 0, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(true, 10000, 20, 750), Action::None);
        assert_eq!(gesture.sequenced_edge(false, 10100, 21, 750), Action::SelectNext);

        let mut untagged = controls::EfxSelectGesture::default();
        assert_eq!(untagged.edge(true, 100, 750), Action::None);
        assert_eq!(untagged.edge(true, 3000, 750), Action::None);
        assert_eq!(untagged.edge(false, 3100, 750), Action::None);
        assert_eq!(untagged.edge(true, 4000, 750), Action::None);
        assert_eq!(untagged.edge(false, 4100, 750), Action::SelectNext);
    }

    #[test]
    fn panel_efx_decoder_requires_complete_matching_frames() {
        let mut decoder = controls::PanelEfxDecoder::default();
        let bytes = [0x91, 4, 6, 0x90, 4, 6, 0x91, 3, 6, 0x91, 4, 0xe1,
                     6, 0x91, 4, 0x90, 4, 6, 4, 6];
        let events: Vec<_> = bytes.into_iter().filter_map(|byte| decoder.feed(byte)).collect();
        assert_eq!(events, [true, false, false]);
    }

    #[test]
    fn inspector_encoders_edit_and_display_actual_parameters() {
        use parameters::{edit_encoder, format_parameter, EfxParamId as Id, ParameterBank as Bank};
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        let banks = [
            (Bank::Velvet, [Id::VndAmount, Id::VndTimeSpread, Id::VndDensity,
                Id::VndDecay, Id::VndRefresh, Id::VndMode, Id::VndTransition, Id::GlobalCascade]),
            (Bank::Plateau, [Id::PlatPreDelay, Id::PlatSize, Id::PlatDecay,
                Id::PlatLowCut, Id::PlatHighCut, Id::PlatDiffusion, Id::PlatModDepth, Id::PlatFreeze]),
        ];
        for (bank, ids) in banks {
            for (encoder, id) in ids.into_iter().enumerate() {
                let before = format_parameter(id, &processor);
                let delta = if id == Id::PlatFreeze { 1 } else { -1 };
                assert_eq!(edit_encoder(&mut processor, bank, encoder, delta), Some(id));
                let after = format_parameter(id, &processor);
                assert_ne!(&before.value_str[..before.value_len], &after.value_str[..after.value_len], "{id:?}");
            }
            assert_eq!(edit_encoder(&mut processor, bank, 8, 1), None);
            assert_eq!(edit_encoder(&mut processor, bank, 0, 0), None);
        }
        assert!(!processor.velvet.current.dynamic);
        assert!(processor.plateau.params.freeze);
        edit_encoder(&mut processor, Bank::Plateau, 7, -1);
        assert!(!processor.plateau.params.freeze);
        edit_encoder(&mut processor, Bank::Velvet, 2, i32::MAX);
        assert_eq!(processor.velvet.current.density, 4000.0);
        edit_encoder(&mut processor, Bank::Velvet, 1, i32::MAX);
        assert_eq!(processor.velvet.current.length_ms, 100.0);
        let displayed = format_parameter(Id::VndDensity, &processor);
        assert_eq!(&displayed.value_str[..displayed.value_len], b"4000.0 pps");
        edit_encoder(&mut processor, Bank::Plateau, 0, i32::MIN);
        assert_eq!(processor.plateau.params.predelay_ms, 0.0);
        let mut plateau = processor.plateau.params;
        plateau.predelay_ms = 190.0;
        processor.plateau.set_parameters(plateau);
        edit_encoder(&mut processor, Bank::Plateau, 0, -1);
        assert_eq!(processor.plateau.params.predelay_ms, 188.0);
        let displayed = format_parameter(Id::PlatPreDelay, &processor);
        assert_eq!(&displayed.value_str[..displayed.value_len], b"188.0 ms");
        edit_encoder(&mut processor, Bank::Plateau, 0, i32::MAX);
        assert_eq!(processor.plateau.params.predelay_ms, 190.0);
    }

    #[test]
    fn display_snapshot_does_not_borrow_live_dsp_state() {
        use parameters::{format_parameter, format_snapshot, snapshot_parameter, EfxParamId as Id, ParameterSnapshot};
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        let ids = [Id::VndAmount, Id::VndTimeSpread, Id::VndDensity, Id::VndDecay,
            Id::VndRefresh, Id::VndMode, Id::VndTransition, Id::PlatPreDelay,
            Id::PlatSize, Id::PlatDecay, Id::PlatLowCut, Id::PlatHighCut,
            Id::PlatDiffusion, Id::PlatModDepth, Id::PlatFreeze, Id::GlobalCascade];
        let snapshots = ids.map(|id| snapshot_parameter(id, &processor));
        let original = ids.map(|id| format_parameter(id, &processor));
        processor.velvet.current.amount = 0.0;
        processor.velvet.current.dynamic = false;
        processor.plateau.params.freeze = true;
        processor.cascade_gain = 0.0;
        for (snapshot, expected) in snapshots.into_iter().zip(original) {
            let copied = ParameterSnapshot::from_words(snapshot.id as u32, snapshot.value.to_bits()).unwrap();
            let actual = format_snapshot(copied);
            assert_eq!(actual.title, expected.title);
            assert_eq!(&actual.value_str[..actual.value_len], &expected.value_str[..expected.value_len]);
        }
        assert!(ParameterSnapshot::from_words(16, 0).is_none());
        assert!(ParameterSnapshot::from_words(Id::VndAmount as u32, f32::NAN.to_bits()).is_none());
        assert!(ParameterSnapshot::from_words(Id::PlatFreeze as u32, 0.5f32.to_bits()).is_none());
    }

    #[test]
    fn display_snapshot_rejects_values_outside_applied_parameter_ranges() {
        use parameter_view::{EfxParamId as Id, ParameterSnapshot};
        let ranges: [(Id, f32, f32); 14] = [
            (Id::VndAmount, 0.0, 100.0), (Id::VndTimeSpread, 1.0, 100.0),
            (Id::VndDensity, 10.0, 4600.0), (Id::VndDecay, -120.0, 0.0),
            (Id::VndRefresh, 0.1, 60.0), (Id::VndTransition, 1.0, 54000.0),
            (Id::PlatPreDelay, 0.0, plateau::MAX_PREDELAY_MS), (Id::PlatSize, 0.01 * 100.0, plateau::MAX_SIZE * 100.0),
            (Id::PlatDecay, 0.1 * 100.0, 0.999 * 100.0),
            (Id::PlatLowCut, 20.0, plateau::SAMPLE_RATE * 0.49),
            (Id::PlatHighCut, 20.0, plateau::SAMPLE_RATE * 0.49),
            (Id::PlatDiffusion, 0.0, 100.0), (Id::PlatModDepth, 0.0, 16.0),
            (Id::GlobalCascade, 0.0, 100.0),
        ];
        for (id, min, max) in ranges {
            for value in [min, max] {
                assert!(ParameterSnapshot::from_words(id as u32, value.to_bits()).is_some(), "{id:?}: {value}");
            }
            for value in [min - 1.0, max + 1.0, f32::MAX, f32::NAN] {
                assert!(ParameterSnapshot::from_words(id as u32, value.to_bits()).is_none(), "{id:?}: {value}");
            }
        }
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        for bank in [parameters::ParameterBank::Velvet, parameters::ParameterBank::Plateau] {
            for encoder in 0..8 {
                for delta in [i32::MIN, i32::MAX] {
                    let id = parameters::edit_encoder(&mut processor, bank, encoder, delta).unwrap();
                    let snapshot = parameters::snapshot_parameter(id, &processor);
                    assert!(ParameterSnapshot::from_words(id as u32, snapshot.value.to_bits()).is_some(),
                        "applied {id:?}: {}", snapshot.value);
                }
            }
        }
    }

    #[test]
    fn routing_display_decodes_only_valid_strips_and_routes() {
        use parameter_view::{DisplaySnapshot, format_display};
        for strip in 0..8 {
            for (mode, expected) in ["OFF", "A", "B", "A -> B"].iter().enumerate() {
                let snapshot = DisplaySnapshot::from_words(16 + strip, (mode as f32).to_bits()).unwrap();
                let formatted = format_display(snapshot);
                assert_eq!(formatted.title, format!("CH {} ROUTE", strip + 1));
                assert_eq!(&formatted.value_str[..formatted.value_len], expected.as_bytes());
            }
        }
        for (id, value) in [(25, 1.0), (25, f32::NAN), (26, 0.0), (24, 3.0), (16, -1.0), (16, 0.5), (16, 4.0), (16, f32::NAN)] {
            assert!(DisplaySnapshot::from_words(id, value.to_bits()).is_none());
        }
        for (raw, expected) in [(0.0f32, "PLATEAU PARAM"), (1.0f32, "ROUTING"), (2.0f32, "VELVET PARAM")] {
            let formatted = format_display(DisplaySnapshot::from_words(24, raw.to_bits()).unwrap());
            assert_eq!(formatted.title, "EFX CONTROLS");
            assert_eq!(&formatted.value_str[..formatted.value_len], expected.as_bytes());
        }
        let warning = format_display(DisplaySnapshot::from_words(25, 0.0f32.to_bits()).unwrap());
        assert_eq!(warning.title, "EFX INPUT");
        assert_eq!(&warning.value_str[..warning.value_len], b"BUSY - RETRY");
    }

    #[test]
    fn test_math_functions() {
        assert!((math::sin(0.0)).abs() < 1e-4);
        assert!((math::sin(math::PI_2) - 1.0).abs() < 1e-3);
        assert!((math::cos(0.0) - 1.0).abs() < 1e-3);
        assert!((math::cos(math::PI_2)).abs() < 1e-3);

        assert!((math::sqrt(4.0) - 2.0).abs() < 1e-4);
        assert!((math::sqrt(100.0) - 10.0).abs() < 1e-4);

        assert!((math::pow10(0.0) - 1.0).abs() < 1e-3);
        assert!((math::pow10(1.0) - 10.0).abs() < 0.1);
        assert!((math::db_to_gain(0.0) - 1.0).abs() < 1e-3);
        assert!((math::db_to_gain(-20.0) - 0.1).abs() < 0.01);
    }

    #[test]
    fn velvet_uniform_matches_double_precision_reference() {
        let mut actual_state = 0x91a2_b3c4_d5e6_f708;
        let mut reference_state = actual_state;
        for _ in 0..200_000 {
            let actual = velvet::uniform_signed(&mut actual_state);
            let bits = velvet::splitmix64(&mut reference_state) >> 11;
            let expected = (bits as f64 * (1.0 / 9007199254740992.0) * 2.0 - 1.0) as f32;
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        // Exercise rounding boundaries and both endpoints of the 53-bit domain.
        for exponent in 0..53 {
            let base = 1u64 << exponent;
            for bits in [base - 1, base, base + 1, (1u64 << 53) - base] {
                // Invert SplitMix64 so this case exercises uniform_signed itself.
                let mut state = bits << 11;
                state ^= state >> 31 ^ state >> 62;
                state = state.wrapping_mul(0x3196_42b2_d24d_8ec3);
                state ^= state >> 27 ^ state >> 54;
                state = state.wrapping_mul(0x96de_1b17_3f11_9089);
                state ^= state >> 30 ^ state >> 60;
                state = state.wrapping_sub(0x9e37_79b9_7f4a_7c15);
                let expected = (bits as f64 * (1.0 / 9007199254740992.0) * 2.0 - 1.0) as f32;
                let actual = velvet::uniform_signed(&mut state);
                assert_eq!(actual.to_bits(), expected.to_bits(), "bits={bits}");
            }
        }
    }

    #[test]
    fn sine_range_reduction_terminates_for_extreme_inputs() {
        for x in [f32::MAX, f32::MIN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let result = math::sin(x);
            assert!(result.is_finite() && result.abs() <= 1.001, "sin({x}) = {result}");
        }
        for x in [-10.0f32, -3.0, 0.0, 3.0, 10.0] {
            assert!((math::sin(x) - x.sin()).abs() < 0.002);
        }
    }

    #[test]
    fn plateau_lowpass_has_minus_three_db_at_its_cutoff() {
        for cutoff in [20.0_f64, 1000.0, 6500.0, 16000.0] {
            let mut filter = plateau::OnePoleLP::new();
            filter.set_cutoff(cutoff as f32);
            let mut input_energy = 0.0;
            let mut output_energy = 0.0;
            for frame in 0..(48_000 + 16_384) {
                let phase = std::f64::consts::TAU * cutoff * frame as f64 / 48_000.0;
                let input = phase.sin() as f32;
                let output = filter.process(input);
                if frame >= 16_384 {
                    input_energy += (input as f64).powi(2);
                    output_energy += (output as f64).powi(2);
                }
            }
            let gain = (output_energy / input_energy).sqrt();
            assert!((gain - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.015,
                    "cutoff {cutoff} Hz has gain {gain}");
        }
    }

    #[test]
    fn plateau_highpass_has_unity_passband_and_minus_three_db_cutoff() {
        for cutoff in [13.75_f64, 20.0, 1000.0, 6500.0, 16000.0, 23520.0] {
            let mut filter = plateau::OnePoleHP::new();
            filter.set_cutoff(cutoff as f32);
            let mut input_energy = 0.0;
            let mut output_energy = 0.0;
            for frame in 0..(48_000 + 16_384) {
                let phase = std::f64::consts::TAU * cutoff * frame as f64 / 48_000.0;
                let input = phase.sin() as f32;
                let output = filter.process(input);
                if frame >= 16_384 {
                    input_energy += (input as f64).powi(2);
                    output_energy += (output as f64).powi(2);
                }
            }
            let gain = (output_energy / input_energy).sqrt();
            assert!((gain - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.015,
                    "cutoff {cutoff} Hz has gain {gain}");
            filter.clear();
            for frame in 0..10_000 {
                let input = if frame % 2 == 0 { 1.0 } else { -1.0 };
                let output = filter.process(input);
                if frame == 9_999 {
                    assert!((output.abs() - 1.0).abs() < 0.001,
                            "Nyquist passband at {cutoff} Hz has gain {}", output.abs());
                }
            }
        }
    }

    #[test]
    fn plateau_cutoff_updates_preserve_independent_filter_histories() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        reverb.init(plateau::PlateauParameters::default());
        let mut lp = [plateau::OnePoleLP::new(), plateau::OnePoleLP::new(), plateau::OnePoleLP::new()];
        let mut hp = [plateau::OnePoleHP::new(), plateau::OnePoleHP::new(), plateau::OnePoleHP::new()];
        lp[0].set_cutoff(14_080.0);
        hp[0].set_cutoff(13.75);
        for (low, high) in [(80.0, 6500.0), (900.0, 16000.0), (100.0, 1000.0)] {
            let mut params = reverb.params;
            params.low_cut_hz = low;
            params.high_cut_hz = high;
            reverb.set_parameters(params);
            for _ in 0..64 {
                reverb.process_sample(0.0, 0.0);
                hp[0].process(lp[0].process(0.0));
            }
            // Preserve each tank filter's post-transition history separately.
            lp[1] = reverb.l_lp;
            lp[2] = reverb.r_lp;
            hp[1] = reverb.l_hp;
            hp[2] = reverb.r_hp;
            for frame in 0..512 {
                let inputs = [if frame % 37 == 0 { 0.5 } else { 0.0 },
                    if frame % 53 == 0 { -0.75 } else { 0.0 },
                    if frame % 71 == 0 { 1.0 } else { 0.0 }];
                let actual_lp = [&mut reverb.in_lp, &mut reverb.l_lp, &mut reverb.r_lp];
                for (i, filter) in actual_lp.into_iter().enumerate() {
                    assert_eq!(filter.process(inputs[i]), lp[i].process(inputs[i]));
                }
                let actual_hp = [&mut reverb.in_hp, &mut reverb.l_hp, &mut reverb.r_hp];
                for (i, filter) in actual_hp.into_iter().enumerate() {
                    assert_eq!(filter.process(inputs[i]), hp[i].process(inputs[i]));
                }
            }
        }
    }

    #[test]
    fn plateau_damping_edit_ramps_and_retargets_without_a_coefficient_jump() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.high_cut_hz = 1000.0;
        reverb.init(params);
        let initial = reverb.l_lp.a;
        params.high_cut_hz = 16000.0;
        reverb.set_parameters(params);
        assert_eq!(reverb.l_lp.a, initial, "the setter must not jump the filter");
        for _ in 0..20 { reverb.process_sample(0.0, 0.0); }
        let midway = reverb.l_lp.a;
        assert!(midway > initial && midway < 0.8);
        params.high_cut_hz = 1000.0;
        reverb.set_parameters(params);
        assert_eq!(reverb.l_lp.a, midway, "reversal must start from the applied coefficient");
        for _ in 0..64 { reverb.process_sample(0.0, 0.0); }
        assert_eq!(reverb.l_lp.a, initial);
        assert_eq!(reverb.r_lp.a, initial);
        params.high_cut_hz = 16000.0;
        reverb.set_parameters(params);
        reverb.clear();
        let mut target = plateau::OnePoleLP::new();
        target.set_cutoff(16000.0);
        assert_eq!(reverb.l_lp.a, target.a, "clear must apply the requested cutoff");
    }

    #[test]
    fn plateau_low_cut_ramps_without_changing_filter_history_at_the_edit() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.low_cut_hz = 20.0;
        reverb.init(params);
        let mut reference = plateau::OnePoleHP::new();
        reference.set_cutoff(20.0);
        assert_eq!(reverb.l_hp.process(0.5), reference.process(0.5));
        params.low_cut_hz = 1000.0;
        reverb.set_parameters(params);
        assert_eq!(reverb.l_hp.process(-0.2), reference.process(-0.2));
        for _ in 0..64 { reverb.process_sample(0.0, 0.0); }
        reverb.l_hp.clear();
        reference.clear();
        reference.set_cutoff(1000.0);
        assert_eq!(reverb.l_hp.process(0.5), reference.process(0.5));
    }

    #[test]
    fn nonfinite_input_does_not_poison_effect_state() {
        let mut actual_v = velvet::VelvetDecorrelator::new();
        let mut expected_v = velvet::VelvetDecorrelator::new();
        actual_v.init(velvet::Parameters::default());
        expected_v.init(velvet::Parameters::default());
        let mut actual_p = Box::new(plateau::PlateauReverb::new());
        let mut expected_p = Box::new(plateau::PlateauReverb::new());
        actual_p.init(plateau::PlateauParameters::default());
        expected_p.init(plateau::PlateauParameters::default());
        for i in 0..12000 {
            let signal = if i == 10 { 0.5 } else { 0.0 };
            let corrupt = if i == 0 { f32::NAN } else if i == 1 { f32::INFINITY } else { signal };
            assert_eq!(actual_v.process_sample(corrupt, corrupt), expected_v.process_sample(signal, signal));
            assert_eq!(actual_p.process_sample(corrupt, corrupt), expected_p.process_sample(signal, signal));
        }
        assert_eq!(math::clamp(f32::NAN, 0.0, 1.0), 0.0);
        assert_eq!(math::soft_clip(f32::NAN), 0.0);
    }

    #[test]
    fn plateau_zero_diffusion_bypasses_input_delays_and_keeps_histories_current() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 0.0;
        params.diffusion = 0.0;
        reverb.init(params);
        reverb.process_sample(0.5, 0.5);
        assert!(reverb.l_apf1.delay.buffer[0].abs() > 0.01 && reverb.r_apf1.delay.buffer[0].abs() > 0.01,
                "zero diffusion should reach both tanks without the input delay chain");
        for _ in 0..2500 { reverb.process_sample(0.0, 0.0); }
        assert!(reverb.in_apf4.delay.buffer.iter().any(|x| *x != 0.0),
                "bypassed diffusers must continue receiving input");

        params.diffusion = 0.5;
        reverb.set_parameters(params);
        for _ in 0..96 {
            let (l, r) = reverb.process_sample(0.0, 0.0);
            assert!(l.is_finite() && r.is_finite() && l.abs() <= 1.0 && r.abs() <= 1.0);
        }
    }

    #[test]
    fn plateau_diffusion_edit_avoids_first_frame_jump() {
        let mut changed = Box::new(plateau::PlateauReverb::new());
        let mut unchanged = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 0.0;
        changed.init(params);
        unchanged.init(params);
        let input = |frame: usize| ((frame * 7919 % 127) as f32 - 63.0) / 90.0;
        for frame in 0..10_000 {
            let sample = input(frame);
            assert_eq!(changed.process_sample(sample, sample), unchanged.process_sample(sample, sample));
        }
        params.diffusion = 0.0;
        changed.set_parameters(params);
        let sample = input(10_000);
        let actual = changed.process_sample(sample, sample);
        let reference = unchanged.process_sample(sample, sample);
        let difference = (actual.0 - reference.0).abs().max((actual.1 - reference.1).abs());
        assert!(difference < 0.002, "first-frame change {difference}");
    }

    #[test]
    fn plateau_diffusion_edits_ramp_allpass_gains_and_retarget() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        reverb.init(params);
        let initial = reverb.l_apf2.gain;
        assert_eq!(initial, 0.7 * params.diffusion);
        assert_eq!(reverb.l_apf1.gain, -initial);
        assert_eq!(reverb.r_apf1.gain, -initial);
        assert_eq!(reverb.r_apf2.gain, initial);
        assert_eq!([reverb.in_apf1.gain, reverb.in_apf2.gain, reverb.in_apf3.gain, reverb.in_apf4.gain],
                   [0.75, 0.75, 0.625, 0.625]);
        params.diffusion = 0.0;
        reverb.set_parameters(params);
        assert_eq!(reverb.l_apf2.gain, initial, "the setter must not jump the coefficient");
        for _ in 0..20 { reverb.process_sample(0.0, 0.0); }
        let halfway = reverb.l_apf2.gain;
        assert!(halfway > 0.0 && halfway < initial);
        params.diffusion = 1.0;
        reverb.set_parameters(params);
        assert_eq!(reverb.l_apf2.gain, halfway, "reversal must start at the applied gain");
        for _ in 0..256 { reverb.process_sample(0.0, 0.0); }
        assert_eq!([reverb.l_apf1.gain, reverb.l_apf2.gain, reverb.r_apf1.gain, reverb.r_apf2.gain],
                   [-0.7, 0.7, -0.7, 0.7]);
        assert_eq!([reverb.in_apf1.gain, reverb.in_apf2.gain, reverb.in_apf3.gain, reverb.in_apf4.gain],
                   [0.75, 0.75, 0.625, 0.625]);
        params.diffusion = 0.0;
        reverb.set_parameters(params);
        reverb.clear();
        assert_eq!([reverb.l_apf1.gain, reverb.l_apf2.gain, reverb.r_apf1.gain, reverb.r_apf2.gain],
                   [0.0; 4], "clear should apply the latest setting");
    }

    #[test]
    fn plateau_diffusion_changes_tank_response_after_input_diffusers() {
        let mut dry_tank = Box::new(plateau::PlateauReverb::new());
        let mut diffused_tank = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 0.0;
        params.diffusion = 1.0;
        dry_tank.init(params);
        diffused_tank.init(params);
        // Isolate the tank: keep both input-diffuser paths enabled and identical.
        params.diffusion = 0.5;
        dry_tank.set_parameters(params);
        for _ in 0..64 {
            dry_tank.process_sample(0.0, 0.0);
            diffused_tank.process_sample(0.0, 0.0);
        }
        let mut difference = 0.0f32;
        for frame in 0..32_000 {
            let input = if frame == 0 { 0.5 } else { 0.0 };
            let a = dry_tank.process_sample(input, input);
            let b = diffused_tank.process_sample(input, input);
            difference = difference.max((a.0 - b.0).abs()).max((a.1 - b.1).abs());
        }
        assert!(difference > 0.001, "tank diffusion did not change the impulse response: {difference}");
    }

    #[test]
    fn plateau_diffusion_bypass_tracks_edits_while_idle() {
        let mut idle = Box::new(routing::IdlePlateau::new());
        let mut full = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 0.0;
        idle.init(params);
        full.init(params);
        for frame in 0..3000 {
            match frame {
                0 | 35 | 220 => params.diffusion = 0.0,
                20 | 180 | 240 => params.diffusion = 0.5,
                _ => {}
            }
            if [0, 20, 35, 180, 220, 240].contains(&frame) {
                idle.set_parameters(params);
                full.set_parameters(params);
            }
            let input = if frame == 200 { 0.5 } else { 0.0 };
            assert_eq!(idle.process_sample(input, input), full.process_sample(input, input),
                       "frame {frame}");
        }
    }

    #[test]
    fn plateau_output_taps_use_full_history_independent_of_size() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.size = 0.2;
        params.freeze = true;
        reverb.init(params);

        // Plateau's seven tap offsets and signs, in 29,761 Hz samples.
        // The first value names the retained line (L/R delay 1, allpass 2,
        // or delay 2); the final value selects the output channel.
        let cases = [
            (0, 266.0f32, 1.0, false), (0, 2974.0, 1.0, false),
            (1, 1913.0, -1.0, false), (2, 1996.0, 1.0, false),
            (3, 1990.0, -1.0, false), (4, 187.0, -1.0, false),
            (5, 1066.0, -1.0, false),
            (3, 266.0, 1.0, true), (3, 2974.0, 1.0, true),
            (4, 1913.0, -1.0, true), (5, 1996.0, 1.0, true),
            (0, 1990.0, -1.0, true), (1, 187.0, -1.0, true),
            (2, 1066.0, -1.0, true),
        ];
        assert!((2974.0f32 * (48_000.0 / 29_761.0)) as usize > reverb.r_del1.length);
        for (line, offset, sign, right_channel) in cases {
            reverb.clear();
            let history: &mut [f32] = match line {
                0 => &mut reverb.l_del1.buffer,
                1 => &mut reverb.l_apf2.delay.buffer,
                2 => &mut reverb.l_del2.buffer,
                3 => &mut reverb.r_del1.buffer,
                4 => &mut reverb.r_apf2.delay.buffer,
                5 => &mut reverb.r_del2.buffer,
                _ => unreachable!(),
            };
            let tap = (offset * (48_000.0 / 29_761.0)) as usize;
            assert!(tap < history.len());
            let index = (1 + history.len() - tap) % history.len();
            history[index] = 0.5;
            let (left, right) = reverb.process_sample(0.0, 0.0);
            let mut dc = plateau::OnePoleHP::new();
            dc.set_cutoff(20.0);
            let expected = math::soft_clip(dc.process(sign * 0.5) * 0.5);
            let (actual, other) = if right_channel { (right, left) } else { (left, right) };
            assert!(other.abs() < 1e-7, "line {line}, tap {tap}: opposite channel {other}");
            assert!((actual - expected).abs() < 1e-6,
                    "line {line}, tap {tap}: actual {actual}, expected {expected}");
        }
    }

    #[test]
    fn plateau_first_tank_allpass_reaches_each_output() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.diffusion = 1.0;
        reverb.init(params);
        reverb.params.freeze = true;
        reverb.l_sum = 0.5;
        reverb.r_sum = -0.25;

        // PlateauNEVersio Dattorro.cpp, commit 4218b072e7420dd788cd3c98c36b3eba445f8886,
        // includes each first tank-allpass output in its matching channel.
        // All other tank taps start silent. With gain -0.7, the first
        // allpasses immediately contribute 0.35 and -0.175 respectively.
        let mut left_hp = plateau::OnePoleHP::new();
        let mut right_hp = plateau::OnePoleHP::new();
        left_hp.set_cutoff(20.0);
        right_hp.set_cutoff(20.0);
        let wet = reverb.params.wet;
        let expected = (
            math::soft_clip(left_hp.process(0.35) * 0.5) * wet,
            math::soft_clip(right_hp.process(-0.175) * 0.5) * wet,
        );
        let actual = reverb.process_sample(0.0, 0.0);
        assert!(
            (actual.0 - expected.0).abs() < 1e-6,
            "left: {actual:?} vs {expected:?}"
        );
        assert!(
            (actual.1 - expected.1).abs() < 1e-6,
            "right: {actual:?} vs {expected:?}"
        );
    }

    #[test]
    fn plateau_tank_delays_match_their_configured_ring_lengths() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.diffusion = 1.0;
        reverb.init(params);
        reverb.params.freeze = true;
        reverb.l_del1.length = 10;
        reverb.l_del2.length = 10;
        reverb.r_del1.length = 10;
        reverb.r_del2.length = 10;
        reverb.l_sum = 1.0;
        reverb.r_sum = 1.0;

        // With empty allpasses the first tank allpass produces an immediate
        // 0.7 impulse, then remains silent past these short test rings.
        for frame in 0..=20 {
            reverb.process_sample(0.0, 0.0);
            let l = reverb.l_apf2.delay.buffer[frame];
            let r = reverb.r_apf2.delay.buffer[frame];
            let expected = if frame == 10 { 0.7 } else { 0.0 };
            assert!((l - expected).abs() < 1e-6, "left first tank delay frame {frame}: {l}");
            assert!((r - expected).abs() < 1e-6, "right first tank delay frame {frame}: {r}");
            let feedback = if frame == 20 { -0.49 } else { 0.0 };
            assert!((reverb.l_sum - feedback).abs() < 1e-6, "left cross-feedback frame {frame}: {}", reverb.l_sum);
            assert!((reverb.r_sum - feedback).abs() < 1e-6, "right cross-feedback frame {frame}: {}", reverb.r_sum);
        }
    }

    #[test]
    fn extreme_finite_input_does_not_poison_effect_state() {
        let mut actual_v = velvet::VelvetDecorrelator::new();
        let mut expected_v = velvet::VelvetDecorrelator::new();
        actual_v.init(velvet::Parameters::default());
        expected_v.init(velvet::Parameters::default());
        let mut actual_p = Box::new(plateau::PlateauReverb::new());
        let mut expected_p = Box::new(plateau::PlateauReverb::new());
        actual_p.init(plateau::PlateauParameters::default());
        expected_p.init(plateau::PlateauParameters::default());
        for i in 0..12000 {
            let signal = if i == 10 { 0.5 } else { 0.0 };
            let corrupt = if i == 0 { f32::MAX } else if i == 1 { f32::MIN } else { signal };
            assert_eq!(actual_v.process_sample(corrupt, corrupt), expected_v.process_sample(signal, signal), "velvet frame {i}");
            assert_eq!(actual_p.process_sample(corrupt, corrupt), expected_p.process_sample(signal, signal), "plateau frame {i}");
        }
    }

    #[test]
    fn velvet_wraps_out_of_range_tap_delays() {
        let mut engine = Box::new(velvet::VelvetDecorrelator::new());
        engine.init(velvet::Parameters::default());
        engine.current.dynamic = false;
        for delay in [1, velvet::MAX_DELAY_SAMPLES as u32 + 1,
            2 * velvet::MAX_DELAY_SAMPLES as u32 + 1, u32::MAX] {
            engine.reset();
            let d = delay as usize % velvet::MAX_DELAY_SAMPLES;
            let bank = &mut engine.channels[0].banks[engine.active_bank];
            bank.count = 1;
            bank.storage[0] = velvet::Tap { delay, gain: 0.5 };
            engine.process_sample(0.25, 0.0);
            for _ in 1..d { engine.process_sample(0.0, 0.0); }
            let (left, _) = engine.process_sample(0.0, 0.0);
            assert_eq!(left.to_bits(), math::soft_clip(0.125).to_bits(), "delay={delay}");

            // The aligned two-history path must apply the same wrap to both
            // the direct and cascaded sends, including malformed tap values.
            engine.reset();
            let bank = &mut engine.channels[0].banks[engine.active_bank];
            bank.count = 1;
            bank.storage[0] = velvet::Tap { delay, gain: 0.5 };
            let mut cascade = velvet::MonoHistory::new();
            engine.process_shared_mono(&mut cascade, 0.25, -0.5);
            for _ in 1..d { engine.process_shared_mono(&mut cascade, 0.0, 0.0); }
            let ((direct, _), (cascaded, _)) = engine.process_shared_mono(&mut cascade, 0.0, 0.0);
            assert_eq!(direct.to_bits(), math::soft_clip(0.125).to_bits(), "direct delay={delay}");
            assert_eq!(cascaded.to_bits(), math::soft_clip(-0.25).to_bits(), "cascade delay={delay}");
        }
    }

    #[test]
    fn invalid_velvet_bank_count_silences_only_its_channel() {
        let mut damaged = Box::new(velvet::VelvetDecorrelator::new());
        let mut reference = Box::new(velvet::VelvetDecorrelator::new());
        let params = velvet::Parameters { dynamic: false, ..velvet::Parameters::default() };
        damaged.init(params);
        reference.init(params);
        damaged.channels[0].banks[damaged.active_bank].count = usize::MAX;
        for frame in 0..5000 {
            let input = (frame as f32 * 0.03).sin() * 0.1 + 0.1;
            let (left, right) = damaged.process_sample(input, input);
            let (_, expected_right) = reference.process_sample(input, input);
            assert_eq!(left, 0.0, "corrupt left frame {frame}");
            assert_eq!(right, expected_right, "valid right frame {frame}");
        }
        damaged.init(params);
        reference.init(params);
        damaged.channels[0].banks[damaged.active_bank].count = usize::MAX;
        let mut damaged_history = velvet::MonoHistory::new();
        let mut reference_history = velvet::MonoHistory::new();
        for frame in 0..5000 {
            let input = (frame as f32 * 0.03).sin() * 0.1 + 0.1;
            let ((a_l, a_r), (b_l, b_r)) = damaged.process_shared_mono(&mut damaged_history, input, -input);
            let ((_, expected_a), (_, expected_b)) = reference.process_shared_mono(&mut reference_history, input, -input);
            assert_eq!((a_l, b_l), (0.0, 0.0), "corrupt left frame {frame}");
            assert_eq!((a_r, b_r), (expected_a, expected_b), "valid right frame {frame}");
        }
        let dynamic = velvet::Parameters { dynamic: true, refresh_seconds: 0.1,
            transition_ms: 1.0, ..velvet::Parameters::default() };
        damaged.init(dynamic);
        damaged.channels[0].banks[damaged.active_bank].count = usize::MAX;
        for _ in 0..10_000 { damaged.process_sample(0.1, 0.1); }
        assert!(damaged.channels[0].banks[damaged.active_bank].count <= velvet::MAX_TAPS);
        assert_ne!(damaged.process_sample(0.1, 0.1).0, 0.0, "valid regenerated bank stayed silent");
    }

    #[test]
    fn velvet_keeps_the_last_delayed_sample_through_silence() {
        let mut engine = Box::new(velvet::VelvetDecorrelator::new());
        engine.init(velvet::Parameters::default());
        engine.current.dynamic = false;
        for channel in &mut engine.channels {
            let bank = &mut channel.banks[engine.active_bank];
            bank.count = 1;
            bank.storage[0] = velvet::Tap { delay: (velvet::MAX_DELAY_SAMPLES - 1) as u32, gain: 1.0 };
        }
        assert_eq!(engine.process_sample(0.5, -0.5), (0.0, 0.0));
        for _ in 1..velvet::MAX_DELAY_SAMPLES - 1 {
            assert_eq!(engine.process_sample(0.0, 0.0), (0.0, 0.0));
        }
        let (left, right) = engine.process_sample(0.0, 0.0);
        assert_eq!(left.to_bits(), math::soft_clip(0.5).to_bits());
        assert_eq!(right.to_bits(), math::soft_clip(-0.5).to_bits());
        assert_eq!(engine.process_sample(0.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn velvet_advances_dynamic_filters_during_silence_and_resumes() {
        let mut engine = Box::new(velvet::VelvetDecorrelator::new());
        let mut params = velvet::Parameters::default();
        params.refresh_seconds = 0.1;
        params.transition_ms = 1.0;
        engine.init(params);
        for _ in 0..6_000 { assert_eq!(engine.process_sample(0.0, 0.0), (0.0, 0.0)); }
        assert!(engine.generation > 2);
        let mut peak = 0.0f32;
        for i in 0..velvet::MAX_DELAY_SAMPLES {
            let (left, right) = engine.process_sample(if i == 0 { 0.5 } else { 0.0 }, 0.0);
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak > 0.0);
    }

    #[test]
    fn velvet_starts_with_the_reference_zero_delay_impulse() {
        let mut engine = Box::new(velvet::VelvetDecorrelator::new());
        let mut params = velvet::Parameters::default();
        params.dynamic = false;
        params.length_ms = 95.0;
        params.density = 100.0;
        engine.init(params);
        // Reference 115dkk/Velvet-Noise-Decorrelator-VST3 at commit
        // a391a12a0b96b190a05cd7cefa25becf4f31660c, 48 kHz, default seed.
        let expected = [[0, 547, 1102, 1391], [0, 426, 1068, 1706]];
        for (channel, delays) in expected.iter().enumerate() {
            let bank = &engine.channels[channel].banks[engine.active_bank];
            assert_eq!(bank.count, 10);
            for (tap, delay) in bank.storage.iter().zip(delays) {
                assert_eq!(tap.delay, *delay, "channel {channel}");
            }
            assert!(bank.storage[0].gain > 0.0);
        }
        let (left, right) = engine.process_sample(0.5, 0.5);
        assert!(left > 0.0 && right > 0.0, "impulse was delayed or inverted");
        params.dynamic = true;
        params.refresh_seconds = 0.1;
        params.transition_ms = 1.0;
        engine.set_parameters(params);
        for _ in 0..15_000 { engine.process_sample(0.0, 0.0); }
        assert!(engine.generation > 2, "dynamic banks were not regenerated");
        for channel in &engine.channels {
            for bank in &channel.banks {
                assert_eq!(bank.storage[0].delay, 0);
                assert!(bank.storage[0].gain > 0.0);
            }
        }
    }

    #[test]
    fn test_velvet_noise_decorrelator() {
        let mut vnd = velvet::VelvetDecorrelator::new();
        vnd.init(velvet::Parameters::default());

        // Feed an impulse into Left and Right
        let mut out_l = Vec::new();
        let mut out_r = Vec::new();

        // 1 sample impulse, then silence
        let (y_l, y_r) = vnd.process_sample(1.0, 1.0);
        out_l.push(y_l);
        out_r.push(y_r);

        for _ in 1..2000 {
            let (l, r) = vnd.process_sample(0.0, 0.0);
            out_l.push(l);
            out_r.push(r);
        }

        // Calculate energy of wet impulse response (sum of squares)
        let energy_l: f32 = out_l.iter().map(|&x| x * x).sum();
        let energy_r: f32 = out_r.iter().map(|&x| x * x).sum();

        // Unit energy should be close to 1.0 (with dry = 0, amount = 1.0)
        assert!(energy_l > 0.8 && energy_l < 1.2, "Energy L was {energy_l}");
        assert!(energy_r > 0.8 && energy_r < 1.2, "Energy R was {energy_r}");

        // Cross-correlation between Left and Right should be very low (decorrelated)
        let dot_product: f32 = out_l.iter().zip(out_r.iter()).map(|(&l, &r)| l * r).sum();
        let normalized_corr = dot_product / (energy_l * energy_r).sqrt();
        // The reference fixes the first tap of both channels at zero delay;
        // its default normalized zero-lag correlation is about 0.373.
        assert!(
            normalized_corr.abs() < 0.5,
            "Cross-correlation too high: {normalized_corr}"
        );
    }

    #[test]
    fn velvet_density_edits_rebuild_only_when_the_tap_count_changes() {
        let mut edited = Box::new(velvet::VelvetDecorrelator::new());
        let mut reference = Box::new(velvet::VelvetDecorrelator::new());
        let mut params = velvet::Parameters::default();
        params.dynamic = false;
        params.length_ms = 95.0;
        params.density = 2996.0;
        edited.init(params);
        reference.init(params);
        assert_eq!(edited.channels[0].banks[edited.active_bank].count, 285);
        params.density = 3000.0;
        edited.set_parameters(params);
        assert_eq!(edited.current.density, 3000.0);
        assert_eq!(edited.generation, reference.generation, "unchanged count rebuilt a bank");
        for frame in 0..16_000 {
            let sample = (frame as f32 * 0.031).sin() * 0.2;
            assert_eq!(edited.process_sample(sample, sample), reference.process_sample(sample, sample),
                "equivalent density edit changed audio at frame {frame}");
        }
        // A spread change still rebuilds the filter.
        params.length_ms = 90.0;
        let before = edited.generation;
        edited.set_parameters(params);
        assert_ne!(edited.generation, before);
        for _ in 0..13_000 { edited.process_sample(0.0, 0.0); }
        params.density = 100.0;
        let before = edited.generation;
        edited.set_parameters(params);
        assert_ne!(edited.generation, before, "changed count did not rebuild");
        for _ in 0..13_000 { edited.process_sample(0.0, 0.0); }
        assert_eq!(edited.channels[0].banks[edited.active_bank].count, 9);

        // Rounding can also make two uncapped density requests equivalent.
        params.length_ms = 25.0;
        params.density = 400.0;
        edited.init(params);
        let before = edited.generation;
        params.density = 401.0;
        edited.set_parameters(params);
        assert_eq!(edited.generation, before);
        params.density = 440.0;
        edited.set_parameters(params);
        assert_ne!(edited.generation, before);
        for _ in 0..10 { edited.process_sample(0.0, 0.0); }
        let building = edited.generation;
        params.density = 441.0;
        edited.set_parameters(params);
        assert_eq!(edited.generation, building, "equivalent edit restarted a pending build");
    }

    #[test]
    fn test_velvet_dynamic_crossfade() {
        let mut vnd = velvet::VelvetDecorrelator::new();
        let mut params = velvet::Parameters::default();
        params.refresh_seconds = 0.05; // 50 ms fast refresh for test
        params.transition_ms = 10.0;    // 10 ms transition
        vnd.init(params);

        // Run audio through multiple renewals and ensure no NaN or blowups
        for i in 0..10000 {
            let sig = ((i as f32) * 0.05).sin();
            let (l, r) = vnd.process_sample(sig, sig);
            assert!(l.is_finite(), "Sample L was not finite at {i}");
            assert!(r.is_finite(), "Sample R was not finite at {i}");
            assert!(l.abs() < 5.0, "Sample L blew up: {l}");
            assert!(r.abs() < 5.0, "Sample R blew up: {r}");
        }
    }

    #[test]
    fn velvet_gain_and_timer_edits_preserve_the_active_filter() {
        let mut reference = velvet::VelvetDecorrelator::new();
        let mut edited = velvet::VelvetDecorrelator::new();
        let mut params = velvet::Parameters::default();
        params.refresh_seconds = 0.1;
        params.transition_ms = 10.0;
        reference.init(params);
        edited.init(params);
        for i in 0..15000 {
            let signal = (i as f32 * 0.1).sin() * 0.1;
            reference.process_sample(signal, signal);
            edited.process_sample(signal, signal);
        }
        params.amount -= 0.000001;
        params.refresh_seconds = 0.2;
        edited.set_parameters(params);
        for i in 15000..16000 {
            let signal = (i as f32 * 0.1).sin() * 0.1;
            let (a_l, a_r) = reference.process_sample(signal, signal);
            let (b_l, b_r) = edited.process_sample(signal, signal);
            assert!((a_l - b_l).abs() < 0.00001);
            assert!((a_r - b_r).abs() < 0.00001);
        }
    }

    #[test]
    fn velvet_filter_edits_crossfade_even_when_dynamic_is_disabled() {
        let mut edited = velvet::VelvetDecorrelator::new();
        let mut reference = velvet::VelvetDecorrelator::new();
        let mut params = velvet::Parameters::default();
        params.dynamic = false;
        params.transition_ms = 10.0;
        edited.init(params);
        reference.init(params);
        for i in 0..2000 {
            let signal = (i as f32 * 0.1).sin() * 0.1;
            edited.process_sample(signal, signal);
            reference.process_sample(signal, signal);
        }
        params.density = 2000.0;
        edited.set_parameters(params);
        let signal = (2000.0f32 * 0.1).sin() * 0.1;
        let first = edited.process_sample(signal, signal);
        let unchanged = reference.process_sample(signal, signal);
        assert!((first.0 - unchanged.0).abs() < 0.00001);
        assert!((first.1 - unchanged.1).abs() < 0.00001);

        // A second edit during the fade must also finish, even in static mode.
        params.density = 500.0;
        edited.set_parameters(params);
        for i in 2001..4000 {
            let signal = (i as f32 * 0.1).sin() * 0.1;
            let (left, right) = edited.process_sample(signal, signal);
            assert!(left.is_finite() && right.is_finite());
        }
        assert!(!edited.transitioning);
        assert!(!edited.filter_update_pending);
        assert_eq!(edited.channels[0].banks[edited.active_bank].count, 14);
    }

    #[test]
    fn velvet_reset_applies_latest_filter_after_edits() {
        for settle in [false, true] {
            let mut edited = velvet::VelvetDecorrelator::new();
            let mut reference = velvet::VelvetDecorrelator::new();
            let mut params = velvet::Parameters::default();
            params.dynamic = false;
            params.transition_ms = 1.0;
            edited.init(params);
            params.density = 500.0;
            edited.set_parameters(params);
            if settle {
                for _ in 0..100 { edited.process_sample(0.0, 0.0); }
            }
            edited.reset();
            reference.init(params);
            let mut peak = 0.0f32;
            for i in 0..2000 {
                let input = if i == 0 { 0.5 } else { 0.0 };
                let actual = edited.process_sample(input, input);
                let expected = reference.process_sample(input, input);
                peak = peak.max(expected.0.abs()).max(expected.1.abs());
                assert_eq!(actual, expected, "settled={settle}, frame={i}");
            }
            assert!(peak > 0.001);
        }
    }

    #[test]
    fn velvet_builds_pending_filter_without_exposing_partial_taps() {
        let mut edited = velvet::VelvetDecorrelator::new();
        let mut reference = velvet::VelvetDecorrelator::new();
        let mut params = velvet::Parameters::default();
        params.dynamic = false;
        params.transition_ms = 1.0;
        edited.init(params);
        reference.init(params);
        params.length_ms = 95.0;
        params.density = 3000.0;
        edited.set_parameters(params);

        // A maximum-size bank needs time to prepare. Until it is ready the
        // old filter must produce the same output as an unedited instance.
        for i in 0..100 {
            let before = [edited.channels[0].banks[edited.pending_bank],
                          edited.channels[1].banks[edited.pending_bank]];
            let input = (i as f32 * 0.13).sin() * 0.1;
            assert_eq!(edited.process_sample(input, input), reference.process_sample(input, input));
            assert!(!edited.transitioning);
            for c in 0..2 {
                let after = &edited.channels[c].banks[edited.pending_bank];
                let changed = before[c].storage.iter().zip(after.storage.iter())
                    .filter(|(a, b)| a.delay != b.delay || a.gain.to_bits() != b.gain.to_bits())
                    .count();
                assert!(changed <= 1, "frame {i}, channel {c}: {changed} taps changed");
            }
        }

        // A newer edit replaces a bank still being built.
        params.density = 200.0;
        edited.set_parameters(params);
        for _ in 0..500 { edited.process_sample(0.0, 0.0); }
        assert!(!edited.transitioning);
        assert!(!edited.filter_update_pending);
        assert_eq!(edited.channels[0].banks[edited.active_bank].count, 19);
        assert_eq!(edited.channels[1].banks[edited.active_bank].count, 19);
    }

    #[test]
    fn velvet_maximum_density_builds_normalized_banks_and_renews() {
        let mut effect = Box::new(velvet::VelvetDecorrelator::new());
        let params = velvet::Parameters {
            length_ms: 100.0, density: 4000.0, refresh_seconds: 0.1,
            transition_ms: 1.0, ..velvet::Parameters::default()
        };
        effect.init(params);
        let check_banks = |effect: &velvet::VelvetDecorrelator| {
            for channel in &effect.channels {
                for bank in &channel.banks {
                    assert_eq!(bank.count, 400);
                    let taps = &bank.storage[..bank.count];
                    assert_eq!(taps[0].delay, 0);
                    assert!(taps[0].gain > 0.0);
                    assert!(taps.windows(2).all(|pair| pair[0].delay < pair[1].delay));
                    assert!(taps.last().unwrap().delay <= 4800);
                    let energy: f32 = taps.iter().map(|tap| tap.gain * tap.gain).sum();
                    assert!((energy - 1.0).abs() < 0.0001, "energy {energy}");
                }
            }
        };
        check_banks(&effect);
        for i in 0..12_000 {
            let input = (i as f32 * 0.037).sin() * 0.1;
            let (left, right) = effect.process_sample(input, input);
            assert!(left.is_finite() && right.is_finite());
            assert!(left.abs() < 5.0 && right.abs() < 5.0);
        }
        assert!(effect.generation > 2, "maximum-density banks did not renew");
        // Wait until neither bank is being rebuilt before checking normalization.
        effect.set_parameters(velvet::Parameters { dynamic: false, ..params });
        for _ in 0..1000 { effect.process_sample(0.0, 0.0); }
        check_banks(&effect);
    }

    #[test]
    fn weighted_routing_matches_the_synthetic_file_mix_observation() {
        use routing::{DualEfxProcessor, RoutingMode, WeightedChannel};
        let mut processor = Box::new(DualEfxProcessor::new());
        processor.init();
        let mut tracks = [[0.0f32; 128]; 12];
        for (slot, track) in tracks.iter_mut().enumerate() {
            processor.channel_routing[slot] = [RoutingMode::SendA, RoutingMode::SendB,
                RoutingMode::SendAtoB][slot % 3];
            for (frame, sample) in track.iter_mut().enumerate() {
                *sample = ((slot as f32 + 1.0) * 1_000_000.0 + frame as f32 * 1000.0)
                    * (1.0 / 2147483648.0);
            }
        }
        let channels: Vec<_> = tracks.iter().enumerate().map(|(slot, samples)| WeightedChannel {
            samples, gain: (slot as f32 + 1.0) / 16.0, ramp: None,
        }).collect();
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        processor.process_weighted_block(&channels, &mut left, &mut right);
        // Independent ARM/QEMU capture from the synthetic file-mix caller.
        for (frame, expected_l, expected_r) in [
            (0, 0.007768246, 0.007729655), (63, 0.013850859, 0.001429997),
            (64, 0.013852504, 0.001430163), (127, 0.013353780, -0.003259216),
        ] {
            assert!((left[frame] - expected_l).abs() < 1e-6, "left frame {frame}");
            assert!((right[frame] - expected_r).abs() < 1e-6, "right frame {frame}");
        }
    }

    #[test]
    fn weighted_workspace_reuse_clears_old_buses_and_preserves_tails() {
        use routing::{DualEfxProcessor, RoutingMode, RoutingWorkspace, WeightedChannel};
        let mut routed = Box::new(DualEfxProcessor::new());
        let mut reference = Box::new(DualEfxProcessor::new());
        routed.init();
        reference.init();
        routed.channel_routing[..3].copy_from_slice(&[RoutingMode::SendA, RoutingMode::SendB,
            RoutingMode::SendAtoB]);
        assert_eq!(core::mem::size_of::<RoutingWorkspace>(), 3 * 64 * 4);
        let mut storage = Box::new(core::mem::MaybeUninit::<RoutingWorkspace>::uninit());
        unsafe { core::ptr::write_bytes(storage.as_mut_ptr(), 0xa5, 1); }
        let workspace = RoutingWorkspace::initialize_in_place(&mut storage);
        let a: Vec<_> = (0..137).map(|i| (i as f32 * 0.04).sin() * 0.1).collect();
        let b: Vec<_> = (0..137).map(|i| (i as f32 * 0.03).cos() * 0.2).collect();
        let c: Vec<_> = (0..137).map(|i| (i as f32 * 0.02).sin() * 0.15).collect();
        let channels = [WeightedChannel { samples: &a, gain: 2.0, ramp: None },
            WeightedChannel { samples: &b, gain: 0.5, ramp: None },
            WeightedChannel { samples: &c, gain: -1.0, ramp: None }];
        let mut actual = [vec![0.0; 137], vec![0.0; 137]];
        let mut expected = [vec![0.0; 137], vec![0.0; 137]];
        let [ref mut left, ref mut right] = actual;
        routed.process_weighted_with_workspace(&channels, left, right, workspace);
        let [ref mut left, ref mut right] = expected;
        reference.process_buses(&a.iter().map(|v| v * 2.0).collect::<Vec<_>>(),
            &b.iter().map(|v| v * 0.5).collect::<Vec<_>>(),
            &c.iter().map(|v| -v).collect::<Vec<_>>(), left, right);
        assert_eq!(actual, expected);

        // A short next call must not replay buses left by the previous block.
        let short = [0.25, -0.25];
        let channel = [WeightedChannel { samples: &short, gain: 1.0, ramp: None }];
        let mut actual = [[0.0; 9]; 2];
        let mut expected = [[0.0; 9]; 2];
        let [ref mut left, ref mut right] = actual;
        routed.process_weighted_with_workspace(&channel, left, right, workspace);
        let [ref mut left, ref mut right] = expected;
        reference.process_buses(&[0.25, -0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            &[0.0; 9], &[0.0; 9], left, right);
        assert_eq!(actual, expected);
    }

    #[test]
    fn plateau_predelay_delays_impulse_by_requested_samples() {
        let mut immediate = plateau::PlateauReverb::new();
        let mut delayed = plateau::PlateauReverb::new();
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 0.0;
        params.mod_depth = 0.0;
        immediate.init(params);
        params.predelay_ms = 1.0;
        delayed.init(params);

        let mut reference = Vec::new();
        let mut peak = 0.0f32;
        for i in 0..12000 {
            let input = if i == 0 { 1.0 } else { 0.0 };
            let (left, right) = immediate.process_sample(input, input);
            reference.push((left, right));
            let (actual_l, actual_r) = delayed.process_sample(input, input);
            let (expected_l, expected_r) = if i < 48 { (0.0, 0.0) } else { reference[i - 48] };
            assert!((actual_l - expected_l).abs() < 1e-5, "left frame {i}");
            assert!((actual_r - expected_r).abs() < 1e-5, "right frame {i}");
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak > 0.001, "test must observe a reverb response");
    }

    #[test]
    fn plateau_predelay_edits_crossfade_and_retarget_without_a_step() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 0.0;
        reverb.init(params);
        // The first allpass cannot feed back within this short test. Its last
        // written input is therefore the output of the predelay.
        let diffuser_input = |r: &plateau::PlateauReverb| {
            let d = &r.in_apf1.delay;
            d.buffer[(d.write_pos + d.buffer.len() - 1) % d.buffer.len()]
        };
        reverb.in_lp.a = 1.0;
        reverb.in_lp.b = 0.0;
        reverb.predelay.buffer.fill(-1.0);
        let mut dc = plateau::OnePoleHP::new();
        dc.set_cutoff(20.0);
        let mut hp = plateau::OnePoleHP::new();
        hp.set_cutoff(13.75);
        reverb.process_sample(1.0, 1.0);
        assert_eq!(diffuser_input(&reverb), hp.process(dc.process(1.0)));

        reverb.set_predelay_live(10.0);
        reverb.process_sample(1.0, 1.0);
        let filtered = hp.process(dc.process(1.0));
        assert!((diffuser_input(&reverb) - (filtered + (-1.0 - filtered) / 64.0)).abs() < 1e-6);
        for _ in 0..15 { reverb.process_sample(1.0, 1.0); hp.process(dc.process(1.0)); }
        let before_reverse = diffuser_input(&reverb);
        reverb.set_predelay_live(0.0);
        reverb.process_sample(1.0, 1.0);
        let filtered = hp.process(dc.process(1.0));
        let after_reverse = diffuser_input(&reverb);
        assert!((after_reverse - (before_reverse + (filtered - before_reverse) / 64.0)).abs() < 1e-6);
        assert!(after_reverse >= before_reverse);
        assert!((after_reverse - before_reverse).abs() <= 2.0 / 64.0);
        let mut filtered = 0.0;
        for _ in 0..63 { reverb.process_sample(1.0, 1.0); filtered = hp.process(dc.process(1.0)); }
        assert_eq!(diffuser_input(&reverb), filtered);
    }

    #[test]
    fn plateau_predelay_transition_advances_while_pristine() {
        let (mut idle, mut reference) = idle_reverb_pair();
        let mut params = reference.params;
        params.predelay_ms = 0.0;
        idle.set_parameters(params);
        reference.set_parameters(params);
        for frame in 0..10_000 {
            let input = if frame == 7 { 0.5 } else { 0.0 };
            assert_idle_reverb_frame(&mut idle, &mut reference, input, frame);
        }
    }

    #[test]
    fn plateau_predelay_edit_during_freeze_does_not_replay_frozen_input() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 0.0;
        reverb.init(params);
        reverb.in_lp.a = 1.0;
        reverb.in_lp.b = 0.0;
        reverb.set_predelay_live(190.0);
        for _ in 0..8 { reverb.process_sample(1.0, 1.0); }
        params = reverb.params;
        params.freeze = true;
        reverb.set_live_parameters(params);
        reverb.set_predelay_live(0.0);
        for _ in 0..8 { reverb.process_sample(1.0, 1.0); }
        params = reverb.params;
        params.freeze = false;
        reverb.set_live_parameters(params);
        reverb.process_sample(0.0, 0.0);
        assert_eq!(reverb.in_lp.z, 0.0);
    }

    #[test]
    fn shrinking_plateau_size_keeps_new_samples_in_the_delay_ring() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        reverb.init(plateau::PlateauParameters::default());
        // These are valid positions just before each old ring wraps.
        reverb.l_del1.write_pos = reverb.l_del1.length - 1;
        reverb.l_del2.write_pos = reverb.l_del2.length - 1;
        reverb.r_del1.write_pos = reverb.r_del1.length - 1;
        reverb.r_del2.write_pos = reverb.r_del2.length - 1;
        let mut params = reverb.params;
        params.size = 0.2;
        reverb.set_parameters(params);
        reverb.l_del1.write(0.75);
        reverb.l_del2.write(0.75);
        reverb.r_del1.write(0.75);
        reverb.r_del2.write(0.75);
        assert_eq!(reverb.l_del1.read(1), 0.75);
        assert_eq!(reverb.l_del2.read(1), 0.75);
        assert_eq!(reverb.r_del1.read(1), 0.75);
        assert_eq!(reverb.r_del2.read(1), 0.75);
    }

    #[test]
    fn plateau_size_expansion_does_not_replay_abandoned_history() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.size = 1.2;
        params.decay = 0.1;
        reverb.init(params);

        // Fill the long tank delays, then let the short reverb decay well
        // beyond the largest backing buffer's duration.
        for frame in 0..96_000 {
            let input = if frame % 100 < 50 { 0.7 } else { -0.7 };
            reverb.process_sample(input, input);
        }
        params.size = 0.2;
        reverb.set_parameters(params);
        for _ in 0..144_000 {
            reverb.process_sample(0.0, 0.0);
        }
        params.size = 1.2;
        reverb.set_parameters(params);
        let mut peak = 0.0f32;
        for _ in 0..16_000 {
            let (left, right) = reverb.process_sample(0.0, 0.0);
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak < 1e-5, "size expansion replayed an old tail: {peak}");

        // Discarding stale history must not prevent subsequent input from
        // reaching the reverb.
        let mut response = 0.0f32;
        for frame in 0..16_000 {
            let input = if frame == 0 { 1.0 } else { 0.0 };
            let (left, right) = reverb.process_sample(input, input);
            response = response.max(left.abs()).max(right.abs());
        }
        assert!(response > 0.001, "reverb did not respond to new input");
    }

    #[test]
    fn plateau_maximum_settings_fit_delay_histories_and_output_taps() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.size = 1.2;
        params.predelay_ms = 190.0;
        params.mod_depth = 16.0;
        reverb.init(params);
        let scale = (48_000.0f32 / 29_761.0) * params.size;
        let samples = |value: f32| (value * scale) as usize;
        let input_samples = |value: f32| (value * (48_000.0f32 / 29_761.0)) as usize;
        assert_eq!(reverb.predelay_samples, 9_120);
        assert!(reverb.predelay_samples < reverb.predelay.buffer.len());
        for (actual, expected) in [
            (reverb.in_apf1.delay_time, input_samples(141.0)),
            (reverb.in_apf2.delay_time, input_samples(107.0)),
            (reverb.in_apf3.delay_time, input_samples(379.0)),
            (reverb.in_apf4.delay_time, input_samples(277.0)),
            (reverb.l_apf1.delay_time, samples(672.0)),
            (reverb.l_apf2.delay_time, samples(1800.0)),
            (reverb.r_apf1.delay_time, samples(908.0)),
            (reverb.r_apf2.delay_time, samples(2656.0)),
            (reverb.l_del1.length, samples(4453.0)),
            (reverb.l_del2.length, samples(3720.0)),
            (reverb.r_del1.length, samples(4217.0)),
            (reverb.r_del2.length, samples(3163.0)),
        ] {
            assert_eq!(actual, expected, "maximum delay was clamped");
        }
        for (delay, capacity) in [
            (reverb.l_apf1.delay_time, reverb.l_apf1.delay.buffer.len()),
            (reverb.l_apf2.delay_time, reverb.l_apf2.delay.buffer.len()),
            (reverb.r_apf1.delay_time, reverb.r_apf1.delay.buffer.len()),
            (reverb.r_apf2.delay_time, reverb.r_apf2.delay.buffer.len()),
        ] {
            assert!(delay + params.mod_depth as usize <= capacity - 2);
        }
        for (tap, capacity) in [
            (2974.0, reverb.l_del1.buffer.len()),
            (1913.0, reverb.l_apf2.delay.buffer.len()),
            (1996.0, reverb.l_del2.buffer.len()),
            (2974.0, reverb.r_del1.buffer.len()),
            (1913.0, reverb.r_apf2.delay.buffer.len()),
            (1996.0, reverb.r_del2.buffer.len()),
        ] {
            let tap_samples = (tap * (48_000.0f32 / 29_761.0)) as usize;
            assert!(tap_samples < capacity, "output tap exceeded retained history");
        }
        params.size = 0.2;
        reverb.set_parameters(params);
        for delay in [reverb.l_apf1.delay_time, reverb.l_apf2.delay_time,
                      reverb.r_apf1.delay_time, reverb.r_apf2.delay_time] {
            assert!(delay >= params.mod_depth as usize, "size 0.2 would clamp modulation");
        }
    }

    #[test]
    fn plateau_invalid_delay_metadata_fails_silent_before_updating_state() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        for which in 0..13 {
            for damage in 0..4 {
                // Predelay and tank delays can represent a one-sample length;
                // interpolated allpasses require at least two.
                if damage == 3 && matches!(which, 0 | 6 | 8 | 10 | 12) { continue; }
                reverb.init(plateau::PlateauParameters::default());
                reverb.predelay.buffer[0] = 0.25;
                macro_rules! invalidate {
                    ($d:expr) => {{
                        let delay = &mut $d;
                        match damage {
                            0 => delay.write_pos = delay.buffer.len(),
                            1 => delay.length = 0,
                            2 => delay.length = delay.buffer.len() + 1,
                            _ => delay.length = 1,
                        }
                    }};
                }
                match which {
                    0 => invalidate!(reverb.predelay),
                    1 => invalidate!(reverb.in_apf1.delay),
                    2 => invalidate!(reverb.in_apf2.delay),
                    3 => invalidate!(reverb.in_apf3.delay),
                    4 => invalidate!(reverb.in_apf4.delay),
                    5 => invalidate!(reverb.l_apf1.delay),
                    6 => invalidate!(reverb.l_del1),
                    7 => invalidate!(reverb.l_apf2.delay),
                    8 => invalidate!(reverb.l_del2),
                    9 => invalidate!(reverb.r_apf1.delay),
                    10 => invalidate!(reverb.r_del1),
                    11 => invalidate!(reverb.r_apf2.delay),
                    _ => invalidate!(reverb.r_del2),
                }
                let position = reverb.predelay.write_pos;
                let phase = reverb.lfo1.phase;
                assert_eq!(reverb.process_sample(0.5, 0.5), (0.0, 0.0), "delay {which}, damage {damage}");
                assert_eq!(reverb.predelay.write_pos, position);
                assert_eq!(reverb.predelay.buffer[0], 0.25);
                assert_eq!(reverb.lfo1.phase, phase);
                reverb.init(plateau::PlateauParameters::default());
                reverb.process_sample(0.5, 0.5);
                assert_ne!(reverb.predelay.buffer[0], 0.0, "reinitialization failed for delay {which}");
            }
        }

        // Mutating through IdlePlateau revokes its pristine fast path.
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        processor.plateau.l_del1.write_pos = processor.plateau.l_del1.buffer.len();
        assert_eq!(processor.plateau.process_sample(0.5, 0.5), (0.0, 0.0));
    }

    #[test]
    fn plateau_size_changes_tank_without_retiming_input_diffusers() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        reverb.init(params);
        let initial = [reverb.in_apf1.delay_time, reverb.in_apf2.delay_time,
                       reverb.in_apf3.delay_time, reverb.in_apf4.delay_time];
        let expected = [141.0f32, 107.0, 379.0, 277.0]
            .map(|time| (time * (48_000.0f32 / 29_761.0)) as usize);
        assert_eq!(initial, expected);
        reverb.in_apf1.delay.write(0.75);
        for size in [0.01, 0.2, 0.55, 1.0, 1.2] {
            params.size = size;
            reverb.set_parameters(params);
            assert_eq!([reverb.in_apf1.delay_time, reverb.in_apf2.delay_time,
                        reverb.in_apf3.delay_time, reverb.in_apf4.delay_time], initial);
            assert_eq!(reverb.in_apf1.delay.read(1), 0.75);
            let expected_tank = (4453.0f32 * ((48_000.0f32 / 29_761.0) * size)) as usize;
            assert_eq!(reverb.l_del1.length, expected_tank, "size {size}");
        }
    }

    #[test]
    fn plateau_live_size_edit_transitions_without_an_output_step() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        reverb.init(params);
        let mut previous = (0.0f32, 0.0f32);
        let mut max_step = 0.0f32;
        for frame in 0..96_064 {
            if frame == 96_000 {
                params.size += 0.02;
                reverb.set_parameters(params);
            }
            let input = (frame as f32 * std::f32::consts::TAU * 250.0 / 48_000.0).sin() * 0.1;
            let output = reverb.process_sample(input, input);
            if frame >= 96_000 {
                max_step = max_step.max((output.0 - previous.0).abs())
                    .max((output.1 - previous.1).abs());
            }
            previous = output;
        }
        assert!(
            max_step < 0.04,
            "size edit produced a large output step: {max_step}"
        );

        for frame in 0..64 {
            let editing = frame % 8 == 0;
            if editing {
                params.size = if params.size < 1.01 { 1.02 } else { 1.0 };
                reverb.set_parameters(params);
            }
            let output = reverb.process_sample(0.1, 0.1);
            if editing {
                assert!((output.0 - previous.0).abs() <= 0.032);
                assert!((output.1 - previous.1).abs() <= 0.032);
            }
            previous = output;
        }

        // The wet control must take effect immediately, including when a
        // further size edit restarts an in-progress transition.
        params.size -= 0.02;
        params.wet = 0.0;
        reverb.set_parameters(params);
        for frame in 0..16 {
            if frame == 8 {
                params.size += 0.02;
                reverb.set_parameters(params);
            }
            assert_eq!(reverb.process_sample(0.1, 0.1), (0.0, 0.0));
        }
    }

    #[test]
    fn plateau_freeze_edges_transition_on_both_control_paths() {
        use parameters::{edit_encoder, ParameterBank};
        let (mut live, mut full) = paired_reverb_return();
        let mut previous = (0.0f32, 0.0f32);
        for frame in 0..48_064 {
            if frame >= 48_000 && frame % 8 == 0 {
                let freeze = (frame / 8) % 2 == 0;
                let mut params = full.plateau.params;
                params.freeze = freeze;
                full.plateau.set_parameters(params);
                let delta = if freeze { 1 } else { -1 };
                assert!(edit_encoder(&mut live, ParameterBank::Plateau, 7, delta).is_some());
            }
            let input = (frame as f32 * std::f32::consts::TAU * 60.0 / 48_000.0).sin() * 0.5;
            let actual = live.plateau.process_sample(input, input);
            let expected = full.plateau.process_sample(input, input);
            assert_eq!(actual, expected, "control paths differ at frame {frame}");
            if frame >= 48_000 && frame % 8 == 0 {
                assert!((actual.0 - previous.0).abs() <= 0.032);
                assert!((actual.1 - previous.1).abs() <= 0.032);
            }
            previous = actual;
        }
    }

    #[test]
    fn test_plateau_reverb_decay() {
        let mut plat = plateau::PlateauReverb::new();
        plat.init(plateau::PlateauParameters::default());

        // Feed impulse
        let (y_l, y_r) = plat.process_sample(1.0, 1.0);
        let mut max_tail = y_l.abs().max(y_r.abs());

        // Process 1 second of tail
        for _ in 1..48000 {
            let (l, r) = plat.process_sample(0.0, 0.0);
            assert!(l.is_finite(), "Reverb L not finite");
            assert!(r.is_finite(), "Reverb R not finite");
            max_tail = max_tail.max(l.abs()).max(r.abs());
        }

        assert!(max_tail > 0.01, "Reverb did not produce output");

        // After 2 seconds, tail should have decayed
        let (final_l, final_r) = plat.process_sample(0.0, 0.0);
        assert!(final_l.abs() < 0.01, "Reverb did not decay: {final_l}");
        assert!(final_r.abs() < 0.01, "Reverb did not decay: {final_r}");
    }

    #[test]
    fn test_plateau_freeze() {
        let mut plat = plateau::PlateauReverb::new();
        let mut params = plateau::PlateauParameters::default();
        plat.init(params);

        // Fill the tank before freezing; a short input mostly exercises its
        // initial transient rather than sustained feedback.
        for i in 0..96_000 {
            let s = ((i as f32) * 0.057595866).sin() * 0.25;
            plat.process_sample(s, s);
        }

        // Turn on Freeze
        params.freeze = true;
        plat.set_parameters(params);

        // Compare successive five-second windows rather than relying on one
        // short energy measurement, and check the output remains bounded.
        let mut energy = [0.0f64; 2];
        for window in &mut energy {
            for _ in 0..240_000 {
                let (l, r) = plat.process_sample(0.0, 0.0);
                assert!(l.is_finite() && r.is_finite());
                assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
                *window += (l as f64) * (l as f64) + (r as f64) * (r as f64);
            }
        }
        assert!(energy[0] > 1.0, "frozen tank was not energized");
        assert!(
            energy[1] > energy[0] * 0.5 && energy[1] < energy[0] * 2.0,
            "frozen energy changed excessively: {energy:?}"
        );
    }

    #[test]
    fn plateau_modulates_both_second_tank_allpasses() {
        let mut first = Box::new(plateau::PlateauReverb::new());
        let mut second = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.mod_depth = 16.0;
        first.init(params);
        second.init(params);
        for engine in [&mut first, &mut second] {
            for (index, sample) in engine.l_apf2.delay.buffer.iter_mut().enumerate() {
                *sample = index as f32 / 5000.0;
            }
            for (index, sample) in engine.r_apf2.delay.buffer.iter_mut().enumerate() {
                *sample = index as f32 / 6000.0;
            }
        }
        first.lfo2.phase = 0.1;
        second.lfo2.phase = 0.6;
        first.lfo4.phase = 0.2;
        second.lfo4.phase = 0.7;
        first.process_sample(0.0, 0.0);
        second.process_sample(0.0, 0.0);
        // Each allpass consumes the preceding frame's interpolated delay read.
        first.process_sample(0.0, 0.0);
        second.process_sample(0.0, 0.0);
        assert_ne!(first.l_del2.buffer[1], second.l_del2.buffer[1], "left second allpass");
        assert_ne!(first.r_del2.buffer[1], second.r_del2.buffer[1], "right second allpass");
    }

    #[test]
    fn plateau_freeze_preserves_tank_energy_with_maximum_modulation() {
        for size in [0.01, 0.2, 0.6, 1.0, 1.2] {
            let mut reverb = Box::new(plateau::PlateauReverb::new());
            let mut params = plateau::PlateauParameters::default();
            params.size = size;
            params.decay = 0.999;
            params.mod_depth = 16.0;
            reverb.init(params);
            let mut random = 7u32;
            for _ in 0..240_000 {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let input = random as i32 as f32 * (0.25 / 2_147_483_648.0);
                reverb.process_sample(input, input);
            }
            params.freeze = true;
            reverb.set_parameters(params);
            let mut energy = [0.0f64; 2];
            for window in &mut energy {
                for _ in 0..960_000 {
                    reverb.process_sample(0.0, 0.0);
                    let (l, r) = (reverb.l_sum as f64, reverb.r_sum as f64);
                    *window += l * l + r * r;
                }
            }
            assert!(energy[0] > 1.0, "tank must be energized for size {size}");
            assert!(energy[1] > energy[0] * 0.85 && energy[1] < energy[0] * 1.15,
                "frozen tank energy changed excessively for size {size}: {energy:?}");
        }
    }

    #[test]
    fn plateau_freeze_holds_modulation_then_resumes_it() {
        let mut modulated = Box::new(plateau::PlateauReverb::new());
        let mut fixed = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.mod_depth = 16.0;
        modulated.init(params);
        fixed.init(params);
        for frame in 0..48_000 {
            let input = (frame as f32 * 0.031).sin() * 0.2;
            assert_eq!(modulated.process_sample(input, input), fixed.process_sample(input, input));
        }
        params.freeze = true;
        modulated.set_parameters(params);
        params.mod_depth = 0.0;
        fixed.set_parameters(params);
        let phases_before = [modulated.lfo1.phase, modulated.lfo2.phase,
                             modulated.lfo3.phase, modulated.lfo4.phase];
        for frame in 0..20_000 {
            assert_eq!(modulated.process_sample(0.0, 0.0), fixed.process_sample(0.0, 0.0),
                "modulation affected frozen output at frame {frame}");
        }
        for (before, after) in phases_before.into_iter().zip([
            modulated.lfo1.phase, modulated.lfo2.phase, modulated.lfo3.phase, modulated.lfo4.phase,
        ]) {
            assert_ne!(after, before, "each LFO must continue advancing");
        }
        params.freeze = false;
        fixed.set_parameters(params);
        params.mod_depth = 16.0;
        modulated.set_parameters(params);
        let mut difference = 0.0f32;
        for frame in 0..20_000 {
            let input = (frame as f32 * 0.043).sin() * 0.1;
            let a = modulated.process_sample(input, input);
            let b = fixed.process_sample(input, input);
            difference = difference.max((a.0 - b.0).abs()).max((a.1 - b.1).abs());
        }
        assert!(difference > 1e-4, "modulation did not resume after freeze");
    }

    #[test]
    fn plateau_freeze_blocks_buffered_diffuser_signal_from_tank() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        reverb.init(params);
        // Leave an impulse in the final diffuser while the tank is empty.
        reverb.in_apf4.process(1.0);
        params.freeze = true;
        reverb.set_parameters(params);
        for frame in 0..12_000 {
            let (left, right) = reverb.process_sample(0.0, 0.0);
            assert_eq!((left, right), (0.0, 0.0), "frame {frame}");
        }
        params.freeze = false;
        reverb.set_parameters(params);
        let mut peak = 0.0f32;
        for frame in 0..12_000 {
            let input = if frame == 0 { 1.0 } else { 0.0 };
            let (left, right) = reverb.process_sample(input, input);
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak > 0.001, "input must reach the tank after unfreezing");
    }

    #[test]
    fn plateau_freeze_does_not_replay_old_damping_state() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        reverb.init(params);
        // Model filter history from before freeze with an otherwise empty tank.
        reverb.l_hp.process(1.0);
        reverb.r_hp.process(-1.0);
        params.freeze = true;
        reverb.set_parameters(params);
        for _ in 0..20_000 {
            assert_eq!(reverb.process_sample(0.0, 0.0), (0.0, 0.0));
        }
        params.freeze = false;
        reverb.set_parameters(params);
        let mut peak = 0.0f32;
        for _ in 0..12_000 {
            let (left, right) = reverb.process_sample(0.0, 0.0);
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak < 1e-7, "stale damping state reappeared after freeze: {peak}");
    }

    #[test]
    fn plateau_input_dc_filter_precedes_predelay_and_rejects_frozen_input() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        reverb.init(params);
        let mut left = plateau::OnePoleHP::new();
        let mut right = plateau::OnePoleHP::new();
        left.set_cutoff(20.0);
        right.set_cutoff(20.0);
        let mut lp = plateau::OnePoleLP::new();
        lp.set_cutoff(14_080.0);
        let mut hp = plateau::OnePoleHP::new();
        hp.set_cutoff(13.75);

        for frame in 0..512 {
            if frame == 160 {
                params.freeze = true; reverb.set_parameters(params);
                left.clear(); right.clear(); lp.clear(); hp.clear();
            }
            if frame == 320 { params.freeze = false; reverb.set_parameters(params); }
            let l = if frame % 17 == 0 { 0.75 } else { -0.1 };
            let r = if frame % 23 == 0 { -0.5 } else { 0.2 };
            let (input_l, input_r) = if params.freeze { (0.0, 0.0) } else { (l, r) };
            let expected = hp.process(lp.process(0.5 * (left.process(input_l) + right.process(input_r))));
            let pos = reverb.predelay.write_pos;
            reverb.process_sample(l, r);
            let expected = if params.freeze { 0.0 } else { expected };
            assert!((reverb.predelay.buffer[pos] - expected).abs() < 1e-6, "frame {frame}");
        }
        reverb.clear();
        let pos = reverb.predelay.write_pos;
        reverb.process_sample(1.0, 0.0);
        let mut fresh = plateau::OnePoleHP::new();
        fresh.set_cutoff(20.0);
        lp.clear(); hp.clear();
        assert_eq!(reverb.predelay.buffer[pos], hp.process(lp.process(fresh.process(0.5))));
    }

    #[test]
    fn plateau_freeze_does_not_delay_frozen_input_until_release() {
        let mut reverb = Box::new(plateau::PlateauReverb::new());
        let mut params = plateau::PlateauParameters::default();
        params.predelay_ms = 100.0;
        params.freeze = true;
        reverb.init(params);
        for frame in 0..64 {
            let input = if frame == 63 { 1.0 } else { 0.0 };
            assert_eq!(reverb.process_sample(input, input), (0.0, 0.0));
        }
        params.freeze = false;
        reverb.set_parameters(params);
        for frame in 0..12_000 {
            let (left, right) = reverb.process_sample(0.0, 0.0);
            assert_eq!((left, right), (0.0, 0.0), "frozen input leaked at frame {frame}");
        }
        let mut peak = 0.0f32;
        for frame in 0..16_000 {
            let input = if frame == 0 { 1.0 } else { 0.0 };
            let (left, right) = reverb.process_sample(input, input);
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak > 0.001, "new input must pass after freeze is released");
    }

    #[test]
    fn processor_initialization_restores_zeroed_and_dirty_state() {
        let mut reference = Box::new(routing::DualEfxProcessor::new());
        let mut storage = Box::<routing::DualEfxProcessor>::new_uninit();
        // Model dirty raw storage without first creating a reference to the
        // processor or relying on its normal constructor.
        unsafe { std::ptr::write_bytes(storage.as_mut_ptr().cast::<u8>(), 0xa5,
            std::mem::size_of::<routing::DualEfxProcessor>()) };
        let zeroed = routing::DualEfxProcessor::initialize_in_place(&mut storage);
        reference.init();
        assert_eq!(zeroed.return_gain_a, 1.0);
        assert_eq!(zeroed.return_gain_b, 1.0);
        assert!(zeroed.plateau.lfo1.phase_inc > 0.0);
        assert!(zeroed.plateau.lfo2.phase_inc > 0.0);
        assert!(zeroed.plateau.lfo3.phase_inc > 0.0);
        assert!(zeroed.plateau.lfo4.phase_inc > 0.0);

        // Compare both effects after initialization, including more than one
        // full delay-buffer rotation, and verify an audible response exists.
        let mut plateau_peak = 0.0f32;
        let mut velvet_peak = 0.0f32;
        for i in 0..12000 {
            let input = if i == 0 { 1.0 } else { 0.0 };
            let actual = zeroed.plateau.process_sample(input, input);
            let expected = reference.plateau.process_sample(input, input);
            assert_eq!(actual, expected, "plateau frame {i}");
            plateau_peak = plateau_peak.max(actual.0.abs()).max(actual.1.abs());
            let actual = zeroed.velvet.process_sample(input, input);
            let expected = reference.velvet.process_sample(input, input);
            assert_eq!(actual, expected, "velvet frame {i}");
            velvet_peak = velvet_peak.max(actual.0.abs()).max(actual.1.abs());
        }
        assert!(plateau_peak > 0.001 && velvet_peak > 0.001);

        zeroed.channel_routing[0] = routing::RoutingMode::SendA;
        zeroed.channel_send[0] = 0.7;
        zeroed.cascade_gain = 0.0;
        zeroed.return_gain_a = 0.0;
        zeroed.pre_ainr_enabled = true;
        zeroed.init();
        assert_eq!(zeroed.channel_routing[0], routing::RoutingMode::Neither);
        assert_eq!(zeroed.channel_send[0], 0.0);
        assert_eq!(zeroed.cascade_gain, 1.0);
        assert_eq!(zeroed.return_gain_a, 1.0);
        assert!(!zeroed.pre_ainr_enabled);
    }

    #[test]
    fn cascade_receives_only_channels_routed_through_it() {
        let mut isolated = Box::new(routing::DualEfxProcessor::new());
        let mut mixed = Box::new(routing::DualEfxProcessor::new());
        isolated.init();
        mixed.init();
        for processor in [&mut isolated, &mut mixed] {
            processor.cascade_gain = 1.0;
            processor.return_gain_a = 0.0;
            processor.set_channel_routing(0, routing::RoutingMode::SendAtoB);
            processor.set_channel_send(0, 1.0);
        }
        mixed.set_channel_routing(1, routing::RoutingMode::SendA);
        mixed.set_channel_send(1, 1.0);

        let mut peak = 0.0f32;
        for block in 0..300 {
            let mut cascade = [0.0f32; 64];
            let mut a_only = [0.0f32; 64];
            if block == 0 {
                cascade[0] = 1.0;
                a_only[0] = -1.0;
            }
            let mut expected_l = [0.0; 64];
            let mut expected_r = [0.0; 64];
            let mut actual_l = [0.0; 64];
            let mut actual_r = [0.0; 64];
            isolated.process_block(&[&cascade], &mut expected_l, &mut expected_r);
            mixed.process_block(&[&cascade, &a_only], &mut actual_l, &mut actual_r);
            for i in 0..64 {
                assert!((actual_l[i] - expected_l[i]).abs() < 1e-6);
                assert!((actual_r[i] - expected_r[i]).abs() < 1e-6);
                peak = peak.max(expected_l[i].abs()).max(expected_r[i].abs());
            }
        }
        assert!(peak > 0.001, "cascade must produce a reverb response");
    }

    #[test]
    fn corrupt_direct_bus_does_not_silence_valid_cascade() {
        for corrupt in [f32::NAN, f32::INFINITY, f32::MAX] {
            let (mut actual, mut expected) = paired_reverb_return();
            let silence = [0.0; 64];
            let bad_bus = [corrupt; 64];
            let mut impulse = silence;
            impulse[0] = 1.0;
            let mut peak = 0.0f32;
            for block in 0..200 {
                let mut actual_l = [0.0; 64];
                let mut actual_r = [0.0; 64];
                let mut expected_l = [0.0; 64];
                let mut expected_r = [0.0; 64];
                actual.process_buses(&silence, &bad_bus, &impulse, &mut actual_l, &mut actual_r);
                expected.process_buses(&silence, &silence, &impulse, &mut expected_l, &mut expected_r);
                assert_eq!(actual_l, expected_l, "left block {block}, corrupt {corrupt}");
                assert_eq!(actual_r, expected_r, "right block {block}, corrupt {corrupt}");
                for (&l, &r) in expected_l.iter().zip(&expected_r) { peak = peak.max(l.abs()).max(r.abs()); }
                impulse[0] = 0.0;
            }
            assert!(peak > 0.001, "test must observe the cascade");
        }
    }

    #[test]
    fn routing_processes_long_buffers_like_successive_blocks() {
        let mut long = Box::new(routing::DualEfxProcessor::new());
        let mut split = Box::new(routing::DualEfxProcessor::new());
        for processor in [&mut long, &mut split] {
            processor.init();
            processor.set_channel_routing(0, routing::RoutingMode::SendA);
            processor.set_channel_routing(1, routing::RoutingMode::SendB);
            processor.set_channel_send(0, 1.0);
            processor.set_channel_send(1, 0.5);
        }
        let input: Vec<_> = (0..145).map(|i| (i as f32 * 0.1).sin() * 0.2).collect();
        // The second channel ends partway through the second block.
        let short = &input[..80];
        let mut actual_l = vec![42.0; 145];
        let mut actual_r = vec![42.0; 145];
        let mut expected_l = vec![42.0; 145];
        let mut expected_r = vec![42.0; 145];
        long.process_block(&[&input, short], &mut actual_l, &mut actual_r);
        for start in (0..145).step_by(64) {
            let end = (start + 64).min(145);
            let short_chunk = short.get(start..end.min(short.len())).unwrap_or(&[]);
            split.process_block(&[&input[start..end], short_chunk],
                &mut expected_l[start..end], &mut expected_r[start..end]);
        }
        assert_eq!(actual_l, expected_l);
        assert_eq!(actual_r, expected_r);
        assert!(actual_l.iter().all(|&x| x != 42.0));
    }

    #[test]
    fn preweighted_buses_match_channel_routing_and_preserve_tails() {
        let mut channels = Box::new(routing::DualEfxProcessor::new());
        let mut buses = Box::new(routing::DualEfxProcessor::new());
        channels.init();
        buses.init();
        channels.set_channel_routing(0, routing::RoutingMode::SendA);
        channels.set_channel_routing(1, routing::RoutingMode::SendB);
        channels.set_channel_routing(2, routing::RoutingMode::SendAtoB);
        for ch in 0..3 {
            channels.set_channel_send(ch, 0.5);
        }
        let mut inputs = [[0.0; 64]; 3];
        inputs[0][0] = 0.25;
        inputs[1][1] = 0.5;
        inputs[2][2] = -0.75;
        let mut peak = 0.0f32;
        for _ in 0..200 {
            let weighted = inputs.map(|channel| channel.map(|sample| sample * 0.5));
            let mut expected = [[0.0; 64]; 2];
            let mut actual = [[0.0; 64]; 2];
            let [ref mut expected_l, ref mut expected_r] = expected;
            channels.process_block(&[&inputs[0], &inputs[1], &inputs[2]], expected_l, expected_r);
            let [ref mut actual_l, ref mut actual_r] = actual;
            buses.process_buses(&weighted[0], &weighted[1], &weighted[2], actual_l, actual_r);
            assert_eq!(actual, expected);
            for (&l, &r) in actual[0].iter().zip(&actual[1]) {
                peak = peak.max(l.abs()).max(r.abs());
            }
            inputs.fill([0.0; 64]);
        }
        assert!(peak > 0.001, "the test must observe an effect response");
    }

    #[test]
    fn routed_frame_processing_matches_block_processing() {
        let mut block = Box::new(routing::DualEfxProcessor::new());
        let mut frames = Box::new(routing::DualEfxProcessor::new());
        block.init();
        frames.init();
        let mut peak = 0.0f32;
        for n in 0..150 {
            if n == 25 {
                let mut p = block.velvet.current;
                p.density = 2000.0;
                block.velvet.set_parameters(p);
                frames.velvet.set_parameters(p);
            }
            let a = [if n == 0 { 0.25 } else { 0.0 }; 64];
            let b = [if n == 1 { 0.125 } else { 0.0 }; 64];
            let c = [if n == 2 { -0.25 } else { 0.0 }; 64];
            let mut left = [0.0; 64];
            let mut right = [0.0; 64];
            block.process_buses(&a, &b, &c, &mut left, &mut right);
            for i in 0..64 {
                let actual = frames.process_bus_sample(a[i], b[i], c[i]);
                assert_eq!(actual, (left[i], right[i]), "block {n}, frame {i}");
                peak = peak.max(actual.0.abs()).max(actual.1.abs());
            }
        }
        assert!(peak > 0.001);
    }

    #[test]
    fn weighted_channels_route_signed_gains_and_per_frame_ramps() {
        let mut routed = Box::new(routing::DualEfxProcessor::new());
        let mut reference = Box::new(routing::DualEfxProcessor::new());
        routed.init();
        reference.init();
        routed.set_channel_routing(0, routing::RoutingMode::SendA);
        routed.set_channel_routing(1, routing::RoutingMode::SendB);
        routed.set_channel_routing(2, routing::RoutingMode::SendAtoB);
        let a: Vec<_> = (0..145).map(|i| if i == 0 || i == 70 { 0.25 } else { 0.0 }).collect();
        let b: Vec<_> = (0..145).map(|i| if i == 1 || i == 71 || i == 110 { 0.5 } else { 0.0 }).collect();
        let c: Vec<_> = (0..145).map(|i| if i == 2 || i == 72 { -0.25 } else { 0.0 }).collect();
        let ramp: Vec<_> = (0..100).map(|i| i as f32 / 100.0).collect();
        let channels = [
            routing::WeightedChannel { samples: &a, gain: 2.0, ramp: None },
            routing::WeightedChannel { samples: &b, gain: 0.75, ramp: Some(&ramp) },
            routing::WeightedChannel { samples: &c, gain: -1.0, ramp: None },
        ];
        let expected_a: Vec<_> = a.iter().map(|x| x * 2.0).collect();
        let expected_b: Vec<_> = b.iter().enumerate().map(|(i, x)| x * ramp.get(i).copied().unwrap_or(0.0) * 0.75).collect();
        let expected_c: Vec<_> = c.iter().map(|x| -x).collect();
        let mut actual = [vec![0.0; 145], vec![0.0; 145]];
        let mut expected = [vec![0.0; 145], vec![0.0; 145]];
        let [ref mut left, ref mut right] = actual;
        routed.process_weighted_block(&channels, left, right);
        let [ref mut left, ref mut right] = expected;
        reference.process_buses(&expected_a, &expected_b, &expected_c, left, right);
        assert_eq!(actual, expected);
        assert!(actual.iter().flatten().any(|x| x.abs() > 0.001));
    }

    #[test]
    fn corrupt_weighted_channel_does_not_silence_other_channels() {
        let mut actual_processor = Box::new(routing::DualEfxProcessor::new());
        let mut reference_processor = Box::new(routing::DualEfxProcessor::new());
        actual_processor.init();
        reference_processor.init();
        for processor in [&mut actual_processor, &mut reference_processor] {
            processor.set_channel_routing(0, routing::RoutingMode::SendA);
            processor.set_channel_routing(1, routing::RoutingMode::SendA);
            processor.set_channel_routing(2, routing::RoutingMode::SendA);
        }
        let mut valid = [0.0; 512];
        let mut corrupt = [0.0; 512];
        let mut ramp_input = [0.0; 512];
        let mut ramp = [1.0; 512];
        for i in [0, 64, 128] { valid[i] = 0.5; }
        corrupt[0] = f32::NAN;
        corrupt[64] = f32::INFINITY;
        ramp_input[128] = 0.25;
        ramp[128] = f32::NAN;
        let channels = [
            routing::WeightedChannel { samples: &valid, gain: 1.0, ramp: None },
            routing::WeightedChannel { samples: &corrupt, gain: 1.0, ramp: None },
            routing::WeightedChannel { samples: &ramp_input, gain: 1.0, ramp: Some(&ramp) },
        ];
        let mut actual = [[0.0; 512]; 2];
        let mut reference = [[0.0; 512]; 2];
        let [left, right] = &mut actual;
        actual_processor.process_weighted_block(&channels, left, right);
        let [left, right] = &mut reference;
        reference_processor.process_weighted_block(&channels[..1], left, right);
        assert_eq!(actual, reference);
        assert!(reference.iter().flatten().any(|sample| sample.abs() > 0.001));
    }

    #[test]
    fn strip_routing_updates_both_sides_of_paired_strips() {
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        let expected: [&[usize]; 8] = [&[0], &[1], &[2], &[3], &[4, 5], &[6, 7], &[8, 9], &[10, 11]];
        for (strip, slots) in expected.iter().enumerate() {
            processor.channel_routing.fill(routing::RoutingMode::Neither);
            processor.set_strip_routing(strip, routing::RoutingMode::SendAtoB);
            for (slot, &mode) in processor.channel_routing.iter().enumerate() {
                assert_eq!(mode == routing::RoutingMode::SendAtoB, slots.contains(&slot));
            }
        }
        let before = processor.channel_routing;
        processor.set_strip_routing(8, routing::RoutingMode::SendA);
        assert_eq!(processor.channel_routing, before);
        assert_eq!(processor.cycle_strip_routing(8, 1), None);
        for (strip, slots) in expected.iter().enumerate() {
            processor.set_strip_routing(strip, routing::RoutingMode::Neither);
            for mode in [routing::RoutingMode::SendA, routing::RoutingMode::SendB,
                         routing::RoutingMode::SendAtoB, routing::RoutingMode::Neither] {
                assert_eq!(processor.cycle_strip_routing(strip, 1), Some(mode));
                for &slot in *slots { assert_eq!(processor.channel_routing[slot], mode); }
            }
            assert_eq!(processor.cycle_strip_routing(strip, -1), Some(routing::RoutingMode::SendAtoB));
            assert_eq!(processor.cycle_strip_routing(strip, i32::MIN), Some(routing::RoutingMode::SendAtoB));
            assert_eq!(processor.cycle_strip_routing(strip, i32::MAX), Some(routing::RoutingMode::SendB));
        }
    }

    #[test]
    fn stereo_send_processes_long_buffers_like_successive_blocks() {
        let mut long = Box::new(routing::DualEfxProcessor::new());
        let mut split = Box::new(routing::DualEfxProcessor::new());
        long.init();
        split.init();
        long.cascade_gain = 0.5;
        split.cascade_gain = 0.5;
        let input: Vec<_> = (0..145).map(|i| (i as f32 * 0.1).sin() * 0.2).collect();
        let mut actual_l = vec![42.0; 145];
        let mut actual_r = vec![42.0; 145];
        let mut expected_l = vec![42.0; 145];
        let mut expected_r = vec![42.0; 145];
        long.process_stereo_send(&input, &input, &mut actual_l, &mut actual_r, true, true);
        for start in (0..145).step_by(64) {
            let end = (start + 64).min(145);
            split.process_stereo_send(&input[start..end], &input[start..end],
                &mut expected_l[start..end], &mut expected_r[start..end], true, true);
        }
        assert_eq!(actual_l, expected_l);
        assert_eq!(actual_r, expected_r);
        assert!(actual_l.iter().all(|&x| x != 42.0));
    }

    #[test]
    fn stereo_send_preserves_quiet_cascade() {
        let mut off = Box::new(routing::DualEfxProcessor::new());
        let mut quiet = Box::new(routing::DualEfxProcessor::new());
        off.init();
        quiet.init();
        off.cascade_gain = 0.0;
        quiet.cascade_gain = 0.0001;
        off.return_gain_a = 0.0;
        quiet.return_gain_a = 0.0;
        let mut input = [0.0; 64];
        input[0] = 1.0;
        let mut peak_difference = 0.0f32;
        for _ in 0..400 {
            let mut off_l = [0.0; 64];
            let mut off_r = [0.0; 64];
            let mut quiet_l = [0.0; 64];
            let mut quiet_r = [0.0; 64];
            off.process_stereo_send(&input, &input, &mut off_l, &mut off_r, true, true);
            quiet.process_stereo_send(&input, &input, &mut quiet_l, &mut quiet_r, true, true);
            for i in 0..64 {
                peak_difference = peak_difference.max((quiet_l[i] - off_l[i]).abs())
                    .max((quiet_r[i] - off_r[i]).abs());
            }
            input[0] = 0.0;
        }
        assert!(peak_difference > 1e-9, "quiet cascade was lost: {peak_difference}");
    }

    #[test]
    fn corrupt_stereo_input_does_not_silence_cascade_tail() {
        for corrupt in [f32::NAN, f32::INFINITY, f32::MAX] {
            let (mut actual, mut expected) = paired_reverb_return();
            let silence = [0.0; 64];
            let mut impulse = silence;
            impulse[0] = 1.0;
            let mut actual_l = [0.0; 64];
            let mut actual_r = [0.0; 64];
            let mut expected_l = [0.0; 64];
            let mut expected_r = [0.0; 64];
            actual.process_stereo_send(&impulse, &silence, &mut actual_l, &mut actual_r, true, true);
            expected.process_stereo_send(&impulse, &silence, &mut expected_l, &mut expected_r, true, true);
            let bad = [corrupt; 64];
            let mut peak = 0.0f32;
            for block in 0..200 {
                actual.process_stereo_send(&bad, &silence, &mut actual_l, &mut actual_r, true, true);
                expected.process_stereo_send(&silence, &silence, &mut expected_l, &mut expected_r, true, true);
                assert_eq!(actual_l, expected_l, "left block {block}, corrupt {corrupt}");
                assert_eq!(actual_r, expected_r, "right block {block}, corrupt {corrupt}");
                for (&l, &r) in expected_l.iter().zip(&expected_r) { peak = peak.max(l.abs()).max(r.abs()); }
            }
            assert!(peak > 0.001, "test must observe the cascade tail");
        }
    }

    #[test]
    fn stereo_send_advances_inactive_cascade_history() {
        let mut switched = Box::new(routing::DualEfxProcessor::new());
        let mut reference = Box::new(routing::DualEfxProcessor::new());
        switched.init();
        reference.init();
        let silence = [0.0; 64];
        let mut impulse = silence;
        impulse[0] = 1.0;
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        for p in [&mut switched, &mut reference] {
            p.return_gain_a = 0.0;
            p.process_buses(&silence, &silence, &impulse, &mut left, &mut right);
            p.cascade_gain = 0.0;
        }
        // Silence the cascade while advancing the same elapsed audio time.
        for _ in 0..80 {
            switched.process_stereo_send(&silence, &silence, &mut left, &mut right, false, false);
            reference.process_buses(&silence, &silence, &silence, &mut left, &mut right);
        }
        switched.cascade_gain = 1.0;
        reference.cascade_gain = 1.0;
        for block in 0..100 {
            let mut expected_l = [0.0; 64];
            let mut expected_r = [0.0; 64];
            switched.process_buses(&silence, &silence, &silence, &mut left, &mut right);
            reference.process_buses(&silence, &silence, &silence, &mut expected_l, &mut expected_r);
            assert_eq!(left, expected_l, "left block {block}");
            assert_eq!(right, expected_r, "right block {block}");
        }
    }

    #[test]
    fn reverb_tail_continues_after_send_is_closed() {
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        processor.set_channel_routing(0, routing::RoutingMode::SendB);
        processor.set_channel_send(0, 1.0);
        let mut impulse = [0.0f32; 64];
        impulse[0] = 1.0;
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        processor.process_block(&[&impulse], &mut left, &mut right);
        processor.set_channel_send(0, 0.0);
        let silence = [0.0; 64];
        let mut tail_peak = 0.0f32;
        for _ in 0..300 {
            processor.process_block(&[&silence], &mut left, &mut right);
            for (&l, &r) in left.iter().zip(&right) {
                assert!(l.is_finite() && r.is_finite());
                tail_peak = tail_peak.max(l.abs()).max(r.abs());
            }
        }
        assert!(tail_peak > 0.001, "closing a send must not cut the reverb tail");
    }

    #[test]
    fn quiet_positive_send_is_not_silenced() {
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        processor.set_channel_routing(0, routing::RoutingMode::SendB);
        processor.set_channel_send(0, 0.0001);
        let mut input = [0.0f32; 64];
        input[0] = 1.0;
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        let mut peak = 0.0f32;
        for _ in 0..200 {
            processor.process_block(&[&input], &mut left, &mut right);
            for (&l, &r) in left.iter().zip(&right) {
                peak = peak.max(l.abs()).max(r.abs());
            }
            input[0] = 0.0;
        }
        assert!(peak > 0.0000001 && peak < 0.001, "quiet send peak was {peak}");
    }

    #[test]
    fn cascade_route_feeds_reverb_with_default_settings() {
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        processor.set_channel_routing(0, routing::RoutingMode::SendAtoB);
        processor.set_channel_send(0, 1.0);
        // Isolate the reverb return from the direct decorrelator return.
        processor.return_gain_a = 0.0;
        let mut input = [0.0f32; 64];
        input[0] = 1.0;
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        let mut peak = 0.0f32;
        for _ in 0..200 {
            processor.process_block(&[&input], &mut left, &mut right);
            for (&l, &r) in left.iter().zip(&right) {
                peak = peak.max(l.abs()).max(r.abs());
            }
            input[0] = 0.0;
        }
        assert!(peak > 0.001, "cascade produced no reverb response: {peak}");
    }

    #[test]
    fn test_dual_routing_matrix() {
        let mut dual = routing::DualEfxProcessor::new();
        dual.init();

        // Configure channel 0: SendA (VND)
        dual.set_channel_routing(0, routing::RoutingMode::SendA);
        dual.set_channel_send(0, 1.0);

        // Configure channel 1: SendB (Plateau)
        dual.set_channel_routing(1, routing::RoutingMode::SendB);
        dual.set_channel_send(1, 1.0);

        // Configure channel 2: SendAtoB (Cascade)
        dual.set_channel_routing(2, routing::RoutingMode::SendAtoB);
        dual.set_channel_send(2, 1.0);

        let ch0_data = [1.0f32; 32];
        let ch1_data = [0.5f32; 32];
        let ch2_data = [0.25f32; 32];
        let channels: [&[f32]; 3] = [&ch0_data, &ch1_data, &ch2_data];

        let mut out_l = [0.0f32; 32];
        let mut out_r = [0.0f32; 32];

        dual.process_block(&channels, &mut out_l, &mut out_r);

        for i in 0..32 {
            assert!(out_l[i].is_finite());
            assert!(out_r[i].is_finite());
        }
    }

    #[test]
    fn stereo_disabled_effects_advance_without_accepting_input() {
        for (enable_a, enable_b) in [(true, false), (false, true), (true, true)] {
            let mut disabled = Box::new(routing::DualEfxProcessor::new());
            let mut reference = Box::new(routing::DualEfxProcessor::new());
            disabled.init();
            reference.init();
            disabled.cascade_gain = 0.0;
            reference.cascade_gain = 0.0;
            let silence = [0.0; 64];
            let mut impulse = silence;
            impulse[0] = 1.0;
            let mut left = silence;
            let mut right = silence;
            let mut expected_left = silence;
            let mut expected_right = silence;
            disabled.process_stereo_send(&impulse, &impulse, &mut left, &mut right, enable_a, enable_b);
            reference.process_stereo_send(&impulse, &impulse, &mut expected_left, &mut expected_right, enable_a, enable_b);
            // The disabled effects must receive silence even when the bus carries
            // signal, and their output must remain muted while their history ages.
            for _ in 0..4 {
                disabled.process_stereo_send(&impulse, &impulse, &mut left, &mut right, false, false);
                reference.process_stereo_send(&silence, &silence, &mut expected_left, &mut expected_right, enable_a, enable_b);
                assert!(left.iter().chain(&right).all(|&sample| sample == 0.0));
            }
            let mut peak = 0.0f32;
            for _ in 0..200 {
                disabled.process_stereo_send(&silence, &silence, &mut left, &mut right, enable_a, enable_b);
                reference.process_stereo_send(&silence, &silence, &mut expected_left, &mut expected_right, enable_a, enable_b);
                assert_eq!(left, expected_left);
                assert_eq!(right, expected_right);
                for &sample in left.iter().chain(&right) { peak = peak.max(sample.abs()); }
            }
            assert!(peak > 0.0001, "comparison must include a reverb tail");
        }
    }

    #[test]
    fn test_parameter_formatting() {
        use parameters::EfxParamId as Id;
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        let mut velvet = processor.velvet.current;
        velvet.length_ms = 999.0;
        velvet.decay_db = -0.5;
        velvet.transition_ms = 17.5;
        velvet.dynamic = false;
        processor.velvet.set_parameters(velvet);
        let mut plateau = processor.plateau.params;
        plateau.predelay_ms = 999.0;
        plateau.size = 1.2;
        plateau.decay = 0.91;
        plateau.low_cut_hz = 920.0;
        plateau.high_cut_hz = 12450.0;
        plateau.mod_depth = 11.5;
        plateau.freeze = true;
        processor.plateau.set_parameters(plateau);
        for (id, expected) in [
            (Id::VndAmount, "100.0 %"),
            (Id::VndTimeSpread, "100.0 ms"),
            (Id::VndDensity, "1090.0 pps"),
            (Id::VndDecay, "-0.5 dB"),
            (Id::VndRefresh, "5.0 s"),
            (Id::VndTransition, "17.5 ms"),
            (Id::VndMode, "STATIC"),
            (Id::PlatPreDelay, "190.0 ms"),
            (Id::PlatSize, "120.0 %"),
            (Id::PlatDecay, "91.0 %"),
            (Id::PlatLowCut, "920.0 Hz"),
            (Id::PlatHighCut, "12450.0 Hz"),
            (Id::PlatDiffusion, "75.0 %"),
            (Id::PlatModDepth, "11.5 samples"),
            (Id::PlatFreeze, "FROZEN"),
            (Id::GlobalCascade, "100.0 %"),
        ] {
            let formatted = parameters::format_parameter(id, &processor);
            let value = std::str::from_utf8(&formatted.value_str[..formatted.value_len]).unwrap();
            assert_eq!(value, expected, "{id:?}");
        }
    }

    #[test]
    fn plateau_reports_the_cutoffs_its_filters_apply() {
        let mut processor = Box::new(routing::DualEfxProcessor::new());
        processor.init();
        let mut params = processor.plateau.params;
        params.low_cut_hz = f32::NAN;
        params.high_cut_hz = f32::INFINITY;
        processor.plateau.set_parameters(params);
        assert_eq!(processor.plateau.params.low_cut_hz, 20.0);
        assert_eq!(processor.plateau.params.high_cut_hz, plateau::SAMPLE_RATE * 0.49);
        let formatted = parameters::format_parameter(parameters::EfxParamId::PlatLowCut, &processor);
        assert_eq!(std::str::from_utf8(&formatted.value_str[..formatted.value_len]).unwrap(), "20.0 Hz");
        let formatted = parameters::format_parameter(parameters::EfxParamId::PlatHighCut, &processor);
        assert_eq!(std::str::from_utf8(&formatted.value_str[..formatted.value_len]).unwrap(), "23520.0 Hz");
    }
    #[test]
    fn routed_blocks_match_frames_across_edits_and_path_switches() {
        let mut block = Box::new(routing::DualEfxProcessor::new());
        let mut frames = Box::new(routing::DualEfxProcessor::new());
        block.init(); frames.init();
        let mut random = 193u32;
        for step in 0..1300 {
            if [2, 80, 300, 700, 1000].contains(&step) {
                let mut p = block.velvet.current;
                p.length_ms = if step % 3 == 0 { 95.0 } else { 25.0 };
                p.density = if step % 2 == 0 { 3000.0 } else { 800.0 };
                p.transition_ms = 7.0;
                p.refresh_seconds = 0.1;
                p.amount = if step == 700 { 0.3 } else { 0.8 };
                block.velvet.set_parameters(p); frames.velvet.set_parameters(p);
            }
            let n = [1, 7, 16, 31, 64][step % 5];
            let mut a = [0.0; 64]; let mut b = [0.0; 64]; let mut c = [0.0; 64];
            for i in 0..n {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                a[i] = if step % 200 < 140 { (random as i32 as f32) / 4294967296.0 } else { 0.0 };
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                b[i] = (random as i32 as f32) / 8589934592.0;
                c[i] = if step % 150 < 100 { -a[i] * 0.37 } else { 0.0 };
            }
            let mut l = [0.0; 64]; let mut r = [0.0; 64];
            if step % 250 >= 200 {
                let mut el = [0.0; 64]; let mut er = [0.0; 64];
                block.process_stereo_send(&a[..n], &b[..n], &mut l[..n], &mut r[..n], true, true);
                frames.process_stereo_send(&a[..n], &b[..n], &mut el[..n], &mut er[..n], true, true);
                assert_eq!((&l[..n], &r[..n]), (&el[..n], &er[..n]), "stereo step {step}");
            } else {
                block.process_buses(&a[..n], &b[..n], &c[..n], &mut l[..n], &mut r[..n]);
                for i in 0..n {
                    let (el, er) = frames.process_bus_sample(a[i], b[i], c[i]);
                    assert_eq!((l[i], r[i]), (el, er), "step {step}, sample {i}");
                }
            }
        }
    }

    #[test]
    fn routed_blocks_preserve_near_ring_end_taps() {
        let mut block = Box::new(routing::DualEfxProcessor::new());
        let mut frames = Box::new(routing::DualEfxProcessor::new());
        block.init(); frames.init();
        for engine in [&mut block, &mut frames] {
            let mut p = engine.velvet.current; p.dynamic = false; engine.velvet.set_parameters(p);
            for channel in &mut engine.velvet.channels {
                channel.banks[0].count = 2;
                channel.banks[0].storage[0] = velvet::Tap { delay: (velvet::MAX_DELAY_SAMPLES - 1) as u32, gain: 0.5 };
                channel.banks[0].storage[1] = velvet::Tap { delay: u32::MAX, gain: 0.5 };
            }
        }
        for step in 0..180 {
            let a = [if step % 13 == 0 { 0.2 } else { 0.0 }; 64];
            let b = [0.0; 64]; let c = [if step % 23 == 0 { -0.3 } else { 0.0 }; 64];
            let mut l = [0.0; 64]; let mut r = [0.0; 64];
            block.process_buses(&a, &b, &c, &mut l, &mut r);
            for i in 0..64 { assert_eq!((l[i], r[i]), frames.process_bus_sample(a[i], b[i], c[i]), "step {step}, frame {i}"); }
        }
    }
}
