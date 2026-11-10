#![no_std]
#![no_main]

#[path = "../../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
use patch_api::function_entry_hook;

mod firmware;
use firmware as firmware_bindings;
use firmware_bindings::{
    current_window, dsp_stream_callback, effect_workspace_clear, main_startup_initialize,
    mixer_apply_channel_parameter, mixer_get_efx_type, mixer_select_efx_callback,
    mixer_set_efx_type,
};
#[path = "../../../api/l6max/hooks.rs"]
mod hook_catalog;

#[path = "math.rs"]
mod math;
#[path = "parameter_view.rs"]
mod parameter_view;
#[path = "parameters.rs"]
mod parameters;
#[path = "plateau.rs"]
mod plateau;
#[path = "routing.rs"]
mod routing;
#[path = "velvet.rs"]
mod velvet;

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};
use routing::{DualEfxProcessor, RoutingMode, BLOCK_SIZE, MAX_CHANNELS};

const EFFECT_WORKSPACE: usize = 0x80d0_0000;
const EFFECT_WORKSPACE_BYTES: usize = 0x000b_b800;
// Static patches execute from firmware storage, so mutable Rust statics cannot
// live beside the code. The static linker places these state words at the
// beginning of stock's shared effect workspace; keep the processor after them.
const EFFECT_STATE_BYTES: usize = 0x100;
const EFFECT_PROCESSOR: usize = EFFECT_WORKSPACE + EFFECT_STATE_BYTES;
const CALLBACK_CLOSED: u32 = 1 << 31;
const ROOM_CALLBACK: u32 = 0x2028_0ab9;
const SPRING_CALLBACK: u32 = 0x2028_2221;
const ACTIVE_CALLBACK: *mut u32 = 0x202a_6f20 as *mut u32;

// SD-module memory survives stock effect-workspace clears. Keep the extended
// settings there so switching Room/Spring can rebuild the large DSP state
// without discarding the user's parameter edits.
#[cfg(sd_dynamic_module)]
#[unsafe(no_mangle)]
pub static PARAMETER_HANDOFF: parameters::ParameterHandoff = parameters::ParameterHandoff::new();

/// DSP state occupies stock's cleared shared effect workspace, not the heap.
/// Its bounds are checked against the observed L6max effect-buffer span.
#[repr(C, align(8))]
struct EffectWorkspace {
    processor: MaybeUninit<DualEfxProcessor>,
}

const _: () = assert!(
    EFFECT_STATE_BYTES + core::mem::size_of::<EffectWorkspace>() <= EFFECT_WORKSPACE_BYTES
);

#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DSP_STREAM_ENTRY_HITS: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DSP_STREAM_LAST_ARG0: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DSP_STREAM_LAST_ARG1: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DSP_STREAM_LAST_ARG2: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DSP_STREAM_LAST_ARG3: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static EFFECT_WORKSPACE_CLEAR_HITS: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_TYPE_SET_HITS: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_TYPE_SET_SEEN: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_CALLBACK_HITS: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static ROOM_CALLBACK_HITS: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static ROOM_CALLBACK_ADMITTED: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static SPRING_CALLBACK_HITS: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static SPRING_CALLBACK_ADMITTED: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_CALLBACK_ORIGINAL: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_CALLBACK_CONTEXT: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_CALLBACK_DESCRIPTOR: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_OUTPUT_NONZERO: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_ROOM_OUTPUT_NONZERO: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_SPRING_OUTPUT_NONZERO: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_OUTPUT_INVALID: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_ROUTED_CALLBACK: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_INPUT_NONZERO: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_SEND_NONZERO: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_EDIT_EVENT: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_EDIT_APPLIED: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_EDIT_DROPPED: AtomicU32 = AtomicU32::new(0);
/// A closed bit and exclusive callback lease share one atomic word. The
/// selector closes admission before waiting, so the processor cannot be
/// cleared or reinitialized while a callback owns it.
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_CALLBACK_GATE: AtomicU32 = AtomicU32::new(CALLBACK_CLOSED);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_SELECTION_PENDING: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(sd_dynamic_module), unsafe(link_section = ".effect_workspace_state"))]
#[unsafe(no_mangle)]
pub static DUAL_EFFECT_PROCESSOR_TYPE: AtomicU32 = AtomicU32::new(0);

/// Static payloads put their small control state in the beginning of stock's
/// shared effect workspace. The stock selector clears it before each change,
/// so rebuild all control words before publishing a callback. SD modules keep
/// this state in the loader-zeroed module allocation instead.
#[cfg(not(sd_dynamic_module))]
fn reset_static_state() {
    DSP_STREAM_ENTRY_HITS.store(0, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG0.store(0, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG1.store(0, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG2.store(0, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG3.store(0, Ordering::Relaxed);
    EFFECT_WORKSPACE_CLEAR_HITS.store(0, Ordering::Relaxed);
    DUAL_EFFECT_TYPE_SET_HITS.store(0, Ordering::Relaxed);
    DUAL_EFFECT_TYPE_SET_SEEN.store(0, Ordering::Relaxed);
    DUAL_EFFECT_CALLBACK_HITS.store(0, Ordering::Relaxed);
    ROOM_CALLBACK_HITS.store(0, Ordering::Relaxed);
    ROOM_CALLBACK_ADMITTED.store(0, Ordering::Relaxed);
    SPRING_CALLBACK_HITS.store(0, Ordering::Relaxed);
    SPRING_CALLBACK_ADMITTED.store(0, Ordering::Relaxed);
    DUAL_EFFECT_CALLBACK_ORIGINAL.store(0, Ordering::Relaxed);
    DUAL_EFFECT_CALLBACK_CONTEXT.store(0, Ordering::Relaxed);
    DUAL_EFFECT_CALLBACK_DESCRIPTOR.store(0, Ordering::Relaxed);
    DUAL_EFFECT_OUTPUT_NONZERO.store(0, Ordering::Relaxed);
    DUAL_EFFECT_ROOM_OUTPUT_NONZERO.store(0, Ordering::Relaxed);
    DUAL_EFFECT_SPRING_OUTPUT_NONZERO.store(0, Ordering::Relaxed);
    DUAL_EFFECT_OUTPUT_INVALID.store(0, Ordering::Relaxed);
    DUAL_EFFECT_ROUTED_CALLBACK.store(0, Ordering::Relaxed);
    DUAL_EFFECT_INPUT_NONZERO.store(0, Ordering::Relaxed);
    DUAL_EFFECT_SEND_NONZERO.store(0, Ordering::Relaxed);
    DUAL_EFFECT_EDIT_EVENT.store(0, Ordering::Relaxed);
    DUAL_EFFECT_EDIT_APPLIED.store(0, Ordering::Relaxed);
    DUAL_EFFECT_EDIT_DROPPED.store(0, Ordering::Relaxed);
    DUAL_EFFECT_CALLBACK_GATE.store(CALLBACK_CLOSED, Ordering::Release);
    DUAL_EFFECT_SELECTION_PENDING.store(0, Ordering::Relaxed);
    DUAL_EFFECT_PROCESSOR_TYPE.store(0, Ordering::Relaxed);
}

#[cfg(not(sd_dynamic_module))]
fn stock_callback(effect_type: u32) -> u32 {
    match effect_type {
        1 => ROOM_CALLBACK,
        2 => SPRING_CALLBACK,
        _ => 0,
    }
}

#[cfg(not(sd_dynamic_module))]
fn install_static_callback(effect_type: u32) {
    let callback = stock_callback(effect_type);
    if callback != 0 {
        DUAL_EFFECT_CALLBACK_ORIGINAL.store(callback, Ordering::Release);
        unsafe {
            core::ptr::write_volatile(
                ACTIVE_CALLBACK,
                dual_effect_callback as *const () as usize as u32 | 1,
            );
        }
    }
}

#[cfg(not(sd_dynamic_module))]
fn prepare_static_callback(effect_type: u32) {
    let rebuilding = DUAL_EFFECT_SELECTION_PENDING.load(Ordering::Acquire) != 0
        || DUAL_EFFECT_PROCESSOR_TYPE.load(Ordering::Acquire) != effect_type;
    if rebuilding {
        close_and_drain_callbacks();
        reset_static_state();
        initialize_processor(effect_type);
    }
    install_static_callback(effect_type);
    DUAL_EFFECT_SELECTION_PENDING.store(0, Ordering::Release);
    if DUAL_EFFECT_CALLBACK_GATE.load(Ordering::Acquire) == CALLBACK_CLOSED {
        DUAL_EFFECT_CALLBACK_GATE.store(0, Ordering::Release);
    }
}

/// Static firmware payloads initialize the workspace control state after the
/// stock startup routine and before audio tasks can reach the hook sites.
#[cfg(not(sd_dynamic_module))]
#[function_entry_hook(main_startup_initialize)]
pub unsafe fn dual_efx_initialize_static_state() {
    let continuation = hook_continue();
    reset_static_state();
    let current_type = unsafe { mixer_get_efx_type(0) };
    if current_type == 1 || current_type == 2 {
        prepare_static_callback(current_type);
    }
    continuation
}

fn initialize_processor(effect_type: u32) {
    let workspace = unsafe { &mut *(EFFECT_PROCESSOR as *mut EffectWorkspace) };
    let processor = DualEfxProcessor::initialize_in_place(&mut workspace.processor);
    for channel in 0..MAX_CHANNELS {
        processor.set_channel_routing(channel, RoutingMode::Neither);
    }
    #[cfg(sd_dynamic_module)]
    PARAMETER_HANDOFF.restore(processor);
    DUAL_EFFECT_EDIT_EVENT.store(0, Ordering::Relaxed);
    DUAL_EFFECT_ROUTED_CALLBACK.store(0, Ordering::Relaxed);
    DUAL_EFFECT_PROCESSOR_TYPE.store(effect_type, Ordering::Release);
}

/// Claim an idle callback gate to initialize an already-selected stock effect
/// when the SD module is loaded after mixer startup. If a callback is active,
/// leave this block to stock; the setter hook will handle a selector change.
fn ensure_processor_ready(effect_type: u32) -> bool {
    let gate = DUAL_EFFECT_CALLBACK_GATE.load(Ordering::Acquire);
    let claimed = if gate == CALLBACK_CLOSED {
        true
    } else if gate == 0 {
        DUAL_EFFECT_CALLBACK_GATE
            .compare_exchange(0, CALLBACK_CLOSED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    } else {
        false
    };
    if !claimed {
        return DUAL_EFFECT_PROCESSOR_TYPE.load(Ordering::Acquire) == effect_type
            && gate & CALLBACK_CLOSED == 0;
    }

    if DUAL_EFFECT_PROCESSOR_TYPE.load(Ordering::Acquire) != effect_type {
        initialize_processor(effect_type);
    }
    DUAL_EFFECT_CALLBACK_GATE.store(0, Ordering::Release);
    true
}

fn callback_enter() -> bool {
    DUAL_EFFECT_CALLBACK_GATE
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn callback_leave() {
    DUAL_EFFECT_CALLBACK_GATE.fetch_sub(1, Ordering::Release);
}

fn close_and_drain_callbacks() {
    DUAL_EFFECT_CALLBACK_GATE.fetch_or(CALLBACK_CLOSED, Ordering::AcqRel);
    while DUAL_EFFECT_CALLBACK_GATE.load(Ordering::Acquire) != CALLBACK_CLOSED {
        core::hint::spin_loop();
    }
}

fn finish_effect_selection(effect_type: u32) {
    if effect_type == 1 || effect_type == 2 {
        if DUAL_EFFECT_PROCESSOR_TYPE.load(Ordering::Acquire) != effect_type {
            initialize_processor(effect_type);
        }
        DUAL_EFFECT_SELECTION_PENDING.store(0, Ordering::Release);
        DUAL_EFFECT_CALLBACK_GATE.store(0, Ordering::Release);
    } else {
        DUAL_EFFECT_PROCESSOR_TYPE.store(0, Ordering::Release);
        DUAL_EFFECT_ROUTED_CALLBACK.store(0, Ordering::Relaxed);
        DUAL_EFFECT_EDIT_EVENT.store(0, Ordering::Relaxed);
        DUAL_EFFECT_SELECTION_PENDING.store(0, Ordering::Release);
        DUAL_EFFECT_CALLBACK_GATE.store(CALLBACK_CLOSED, Ordering::Release);
    }
}

unsafe fn clear_callback_output(descriptor: *mut u8) {
    if descriptor.is_null() {
        return;
    }
    let buffer = unsafe { core::ptr::read_volatile(descriptor.add(4).cast::<u32>()) } as *mut f32;
    let control_link = unsafe { core::ptr::read_volatile(descriptor.add(28).cast::<u32>()) };
    let control_target = if control_link == 0 {
        core::ptr::null_mut()
    } else {
        unsafe { core::ptr::read_volatile(control_link as *const u32) as *mut u32 }
    };
    let control_source =
        unsafe { core::ptr::read_volatile(descriptor.add(32).cast::<u32>()) as *const u32 };
    if !buffer.is_null() {
        for frame in 0..BLOCK_SIZE {
            unsafe {
                core::ptr::write_volatile(buffer.add(frame), 0.0);
                core::ptr::write_volatile(buffer.add(frame + BLOCK_SIZE), 0.0);
            }
        }
    }
    if !control_target.is_null() && !control_source.is_null() {
        for frame in 0..BLOCK_SIZE {
            unsafe {
                core::ptr::write_volatile(
                    control_target,
                    core::ptr::read_volatile(control_source.add(frame)),
                );
            }
        }
    }
}

/// Firmware knob entry point. Ordinary recorder-screen controls still reach
/// stock code; in another window, EFX encoder turns are handed to the audio
/// callback so it alone mutates the processor state.
#[function_entry_hook(mixer_apply_channel_parameter)]
pub unsafe fn dual_efx_parameter_edit(
    channel: u32,
    mode: u32,
    absolute: u32,
    delta: i32,
    value: u32,
) {
    let window = unsafe { current_window.read() };
    if window == 0x8020_0408 || mode != 6 || channel >= 8 || delta == 0 {
        return hook_continue(channel, mode, absolute, delta, value);
    }

    let bank = match unsafe { mixer_get_efx_type(0) } {
        1 => parameters::ParameterBank::Plateau,
        2 => parameters::ParameterBank::Velvet,
        _ => return hook_continue(channel, mode, absolute, delta, value),
    };
    let step = delta.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    let encoded = ((bank == parameters::ParameterBank::Plateau) as u32) << 31
        | (channel << 24)
        | ((step as u16 as u32) << 8)
        | 1;
    if DUAL_EFFECT_EDIT_EVENT
        .compare_exchange(0, encoded, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        DUAL_EFFECT_EDIT_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
    hook_done()
}

#[function_entry_hook(effect_workspace_clear)]
pub unsafe fn dual_efx_effect_workspace_clear() {
    #[cfg(sd_dynamic_module)]
    {
        EFFECT_WORKSPACE_CLEAR_HITS.fetch_add(1, Ordering::Relaxed);
        close_and_drain_callbacks();
        DUAL_EFFECT_SELECTION_PENDING.store(1, Ordering::Release);
        DUAL_EFFECT_PROCESSOR_TYPE.store(0, Ordering::Release);
        DUAL_EFFECT_ROUTED_CALLBACK.store(0, Ordering::Relaxed);
        DUAL_EFFECT_EDIT_EVENT.store(0, Ordering::Relaxed);
    }
    #[cfg(not(sd_dynamic_module))]
    {
        // Stock clears the active callback before clearing this workspace.
        // Drain our callback here, after the selector's fade, rather than at
        // selector entry where stock still needs the callback to complete it.
        // Stock effects may overwrite the workspace control words while they
        // are selected, so only inspect the gate when Dual EFX owned the old
        // callback. A stock-to-custom switch has no custom callback in flight.
        let current_type = unsafe { mixer_get_efx_type(0) };
        if current_type == 1 || current_type == 2 {
            close_and_drain_callbacks();
        }
    }
    let continuation = hook_continue();
    #[cfg(not(sd_dynamic_module))]
    {
        // Stock may publish the next callback immediately after this clear.
        // Keep the callback gate closed and mark the state as needing rebuild;
        // the DSP entry hook rebuilds it before the published callback runs.
        reset_static_state();
        DUAL_EFFECT_SELECTION_PENDING.store(1, Ordering::Release);
        DUAL_EFFECT_CALLBACK_GATE.store(CALLBACK_CLOSED, Ordering::Release);
    }
    #[cfg(sd_dynamic_module)]
    {
        // Rebuild the currently selected effect after the clear. This also
        // handles startup clears that have no following selector call; the
        // selector hook will replace it with the newly selected effect after
        // the stock fade completes.
        let current_type = unsafe { mixer_get_efx_type(0) };
        finish_effect_selection(current_type);
    }
    continuation
}

/// The stock selector clears the shared workspace and publishes its callback
/// before setting the selected effect type. This setter is therefore the
/// point where we can safely initialize our state after a clear, or close the
/// gate when stock takes ownership of the workspace.
#[function_entry_hook(mixer_set_efx_type)]
pub unsafe fn dual_efx_set_effect_type(bus: u32, effect_type: u32) {
    #[cfg(sd_dynamic_module)]
    DUAL_EFFECT_TYPE_SET_HITS.fetch_add(1, Ordering::Relaxed);
    #[cfg(sd_dynamic_module)]
    if effect_type < 32 {
        DUAL_EFFECT_TYPE_SET_SEEN.fetch_or(1 << effect_type, Ordering::Relaxed);
    }
    if bus != 0 {
        return hook_continue(bus, effect_type);
    }

    #[cfg(sd_dynamic_module)]
    close_and_drain_callbacks();
    let continuation = hook_continue(bus, effect_type);
    let selected = unsafe { mixer_get_efx_type(0) };
    #[cfg(sd_dynamic_module)]
    finish_effect_selection(selected);
    #[cfg(not(sd_dynamic_module))]
    {
        if selected == 1 || selected == 2 {
            prepare_static_callback(selected);
        }
    }
    continuation
}

/// Leave the active callback running through the stock fade. The workspace
/// clear hook closes and drains it after that fade; this hook then initializes
/// Room or Spring before the audio task uses the newly selected callback. The
/// following type setter is retained as a fallback for other selection paths.
#[function_entry_hook(mixer_select_efx_callback)]
pub unsafe fn dual_efx_select_callback(effect_type: u32, bus: u32) {
    if bus != 0 {
        return hook_continue(effect_type, bus);
    }

    let continuation = hook_continue(effect_type, bus);
    #[cfg(not(sd_dynamic_module))]
    {
        if effect_type == 1 || effect_type == 2 {
            prepare_static_callback(effect_type);
        }
    }
    #[cfg(sd_dynamic_module)]
    {
        finish_effect_selection(effect_type);
    }
    continuation
}

#[function_entry_hook(dsp_stream_callback)]
pub unsafe fn dsp_stream_entry_probe(
    argument_0: u32,
    argument_1: u32,
    argument_2: u32,
    argument_3: u32,
) {
    DSP_STREAM_ENTRY_HITS.fetch_add(1, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG0.store(argument_0, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG1.store(argument_1, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG2.store(argument_2, Ordering::Relaxed);
    DSP_STREAM_LAST_ARG3.store(argument_3, Ordering::Relaxed);

    #[cfg(not(sd_dynamic_module))]
    unsafe {
        // Static control words share the effect workspace, so intercept an
        // already-selected Room/Spring callback before stock code can use and
        // overwrite that workspace on its first audio block.
        let selected = core::ptr::read_volatile(ACTIVE_CALLBACK);
        let selected_type = match selected {
            ROOM_CALLBACK => 1,
            SPRING_CALLBACK => 2,
            _ => 0,
        };
        if selected_type != 0 {
            if DUAL_EFFECT_SELECTION_PENDING.load(Ordering::Acquire) != 0 {
                prepare_static_callback(selected_type);
            } else if ensure_processor_ready(selected_type) {
                DUAL_EFFECT_CALLBACK_ORIGINAL.store(selected, Ordering::Release);
                install_static_callback(selected_type);
            }
        }
    }

    // This fixed ROM routine runs immediately before the relocated wrapper
    // loads its active callback pointer. Install only after it returns.
    let continuation = hook_continue(argument_0, argument_1, argument_2, argument_3);
    #[cfg(sd_dynamic_module)]
    unsafe {
        let selected = core::ptr::read_volatile(ACTIVE_CALLBACK);
        let selected_type = match selected {
            ROOM_CALLBACK => 1,
            SPRING_CALLBACK => 2,
            _ => 0,
        };
        if selected_type != 0 {
            DUAL_EFFECT_CALLBACK_ORIGINAL.store(selected, Ordering::Release);
            if DUAL_EFFECT_SELECTION_PENDING.load(Ordering::Acquire) == 0 {
                if ensure_processor_ready(selected_type) {
                    core::ptr::write_volatile(
                        ACTIVE_CALLBACK,
                        dual_effect_callback as *const () as usize as u32 | 1,
                    );
                }
            } else {
                // Keep the cleared workspace inaccessible while the stock
                // selector moves from its clear to its type setter. The
                // closed-gate callback only silences output and copies the
                // stock control stream.
                core::ptr::write_volatile(
                    ACTIVE_CALLBACK,
                    dual_effect_callback as *const () as usize as u32 | 1,
                );
            }
        }
    }
    continuation
}

/// Dynamic replacement for stock Room/Spring DSP. The callback ABI and its
/// two 64-sample output channels are emulator-observed for L6max MAIN 1.10.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dual_effect_callback(context: *mut u8, descriptor: *mut u8) {
    DUAL_EFFECT_CALLBACK_HITS.fetch_add(1, Ordering::Relaxed);
    DUAL_EFFECT_CALLBACK_CONTEXT.store(context as u32, Ordering::Relaxed);
    DUAL_EFFECT_CALLBACK_DESCRIPTOR.store(descriptor as u32, Ordering::Relaxed);

    let original = DUAL_EFFECT_CALLBACK_ORIGINAL.load(Ordering::Acquire);
    if original == ROOM_CALLBACK {
        ROOM_CALLBACK_HITS.fetch_add(1, Ordering::Relaxed);
    } else if original == SPRING_CALLBACK {
        SPRING_CALLBACK_HITS.fetch_add(1, Ordering::Relaxed);
    }

    if !callback_enter() {
        unsafe { clear_callback_output(descriptor) };
        return;
    }
    if original == ROOM_CALLBACK {
        ROOM_CALLBACK_ADMITTED.fetch_add(1, Ordering::Relaxed);
    } else if original == SPRING_CALLBACK {
        SPRING_CALLBACK_ADMITTED.fetch_add(1, Ordering::Relaxed);
    }

    let result = unsafe { process_effect_block(descriptor) };
    callback_leave();
    if !result {
        unsafe { clear_callback_output(descriptor) };
    }
}

unsafe fn process_effect_block(descriptor: *mut u8) -> bool {
    if descriptor.is_null() {
        return false;
    }
    let buffer = unsafe { core::ptr::read_volatile(descriptor.add(4).cast::<u32>()) } as *mut f32;
    let control_link = unsafe { core::ptr::read_volatile(descriptor.add(28).cast::<u32>()) };
    let control_target = if control_link == 0 {
        core::ptr::null_mut()
    } else {
        unsafe { core::ptr::read_volatile(control_link as *const u32) as *mut u32 }
    };
    let control_source =
        unsafe { core::ptr::read_volatile(descriptor.add(32).cast::<u32>()) as *const u32 };
    if buffer.is_null() || control_target.is_null() || control_source.is_null() {
        return false;
    }

    let workspace = unsafe { &mut *(EFFECT_PROCESSOR as *mut EffectWorkspace) };
    // SAFETY: callback_enter owns an exclusive processing lease; selector clear
    // closes the gate and drains leases before changing this storage.
    let processor = unsafe { workspace.processor.assume_init_mut() };
    let original = DUAL_EFFECT_CALLBACK_ORIGINAL.load(Ordering::Acquire);
    if DUAL_EFFECT_ROUTED_CALLBACK.load(Ordering::Relaxed) != original {
        let default_route = match original {
            ROOM_CALLBACK => RoutingMode::SendB,
            SPRING_CALLBACK => RoutingMode::SendA,
            _ => RoutingMode::Neither,
        };
        for channel in 0..MAX_CHANNELS {
            processor.set_channel_routing(channel, default_route);
        }
        DUAL_EFFECT_ROUTED_CALLBACK.store(original, Ordering::Relaxed);
    }
    let edit = DUAL_EFFECT_EDIT_EVENT.swap(0, Ordering::AcqRel);
    if edit & 1 != 0 {
        let bank = if edit >> 31 != 0 {
            parameters::ParameterBank::Plateau
        } else {
            parameters::ParameterBank::Velvet
        };
        let encoder = ((edit >> 24) & 0x7) as usize;
        let delta = ((edit >> 8) as u16 as i16) as i32;
        if let Some(id) = parameters::edit_encoder(processor, bank, encoder, delta) {
            #[cfg(sd_dynamic_module)]
            PARAMETER_HANDOFF.record(id, processor);
            DUAL_EFFECT_EDIT_APPLIED.fetch_add(1, Ordering::Relaxed);
        }
    }
    for frame in 0..BLOCK_SIZE {
        let mut send_a = 0.0f32;
        let mut send_b = 0.0f32;
        let mut send_cascade = 0.0f32;
        for slot in 0..MAX_CHANNELS {
            let mode = processor.channel_routing[slot];
            if mode == RoutingMode::Neither {
                continue;
            }
            let ramp_descriptor = (0x2001_61d0usize + 16 * slot) as *const u32;
            let ramped = unsafe { core::ptr::read_volatile(ramp_descriptor) } != 0;
            let gain = if ramped {
                let ramp_base = unsafe { core::ptr::read_volatile(ramp_descriptor.add(1)) };
                if ramp_base < 256 {
                    0.0
                } else {
                    let ramp = (ramp_base - 256) as *const f32;
                    unsafe { core::ptr::read_volatile(ramp.add(frame)) }
                }
            } else {
                unsafe {
                    core::ptr::read_volatile((0x2001_6170usize + 8 * slot) as *const f32)
                }
            };
            let smoothed = unsafe {
                core::ptr::read_volatile((0x2001_6140usize + 4 * slot) as *const f32)
            };
            let input = unsafe {
                core::ptr::read_volatile((0x2000_e410usize + 256 * slot) as *const f32)
            } * (1.0 / 2_147_483_648.0);
            let contribution = math::audio_input((input * gain) * smoothed);
            if contribution != 0.0 {
                DUAL_EFFECT_INPUT_NONZERO.fetch_add(1, Ordering::Relaxed);
            }
            match mode {
                RoutingMode::SendA => send_a += contribution,
                RoutingMode::SendB => send_b += contribution,
                RoutingMode::SendAtoB => send_cascade += contribution,
                RoutingMode::Neither => {}
            }
        }

        if send_a != 0.0 || send_b != 0.0 || send_cascade != 0.0 {
            DUAL_EFFECT_SEND_NONZERO.fetch_add(1, Ordering::Relaxed);
        }

        unsafe {
            core::ptr::write_volatile(
                control_target,
                core::ptr::read_volatile(control_source.add(frame)),
            );
        }
        let (left, right) = processor.process_bus_sample(send_a, send_b, send_cascade);
        if left.is_finite() && right.is_finite() {
            if left != 0.0 || right != 0.0 {
                DUAL_EFFECT_OUTPUT_NONZERO.fetch_add(1, Ordering::Relaxed);
                match original {
                    ROOM_CALLBACK => {
                        DUAL_EFFECT_ROOM_OUTPUT_NONZERO.fetch_add(1, Ordering::Relaxed);
                    }
                    SPRING_CALLBACK => {
                        DUAL_EFFECT_SPRING_OUTPUT_NONZERO.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
            unsafe {
                core::ptr::write_volatile(buffer.add(frame), left);
                core::ptr::write_volatile(buffer.add(frame + BLOCK_SIZE), right);
            }
        } else {
            DUAL_EFFECT_OUTPUT_INVALID.fetch_add(1, Ordering::Relaxed);
            unsafe {
                core::ptr::write_volatile(buffer.add(frame), 0.0);
                core::ptr::write_volatile(buffer.add(frame + BLOCK_SIZE), 0.0);
            }
        }
    }
    true
}
