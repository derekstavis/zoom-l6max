//! Locked veneers preserve original routines and synchronous stock API results.
use crate::{firmware::*, synchronization as sync, trampolines as original};
use core::ptr::read_volatile as read;
unsafe fn detach() {
    unsafe {
        sync::cancel();
        let custom = !crate::state().is_null();
        crate::restore(); // Restore BEFORE pop can draw its stock successor.
        if custom && crate::current_message() == 15 {
            call1(original::notification_pop(), 2);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_initialize_hook(slot: u32, initial: u32) {
    unsafe {
        sync::initialize(slot, initial);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn semaphore_take_hook(handle: u32, timeout: u32) -> u32 {
    unsafe { sync::take(handle, timeout) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn semaphore_give_hook(
    handle: u32,
    item: u32,
    timeout: u32,
    position: u32,
) -> u32 {
    unsafe { sync::give(handle, item, timeout, position) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_pop_hook(reason: u32) -> u32 {
    unsafe {
        let _guard = sync::guard();
        sync::cancel();
        crate::restore();
        call1(original::notification_pop(), reason)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_show_hook(message: u32) -> u32 {
    unsafe {
        let _guard = sync::guard();
        if message != 0 {
            detach();
        }
        call1(original::notification_show(), message)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_tick_hook() {
    unsafe {
        let _guard = sync::guard();
        if let Some((mode, raw)) = sync::consume() {
            crate::present(mode, raw);
        }
        // The retained callback must preserve the stock busy-overlay pause:
        // unregistering slot 4 originally stopped expiry while c8 was set.
        if read(0x8020a2c0 as *const u8) != 0 && read(0x8020a2c8 as *const u8) == 0 {
            call1(original::notification_tick(), 0);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_draw_hook() {
    unsafe {
        let _guard = sync::guard();
        if !crate::state().is_null() && crate::current_message() == 15 {
            crate::redraw();
        } else {
            call1(original::notification_draw(), 0);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn callback_unregister_hook(slot: u32) {
    unsafe {
        // Slot 4 remains the original notification callback, now also servicing
        // coalesced knob values. Other callback slots preserve stock behavior.
        if slot != 4 || sync::context().is_null() {
            call1(original::callback_unregister(), slot);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn window_push_hook(window: u32) -> u32 {
    unsafe {
        if window == 0 || window == read(0x80202610 as *const u32) {
            return call1(original::window_push(), window);
        }
        sync::block(1);
        {
            let _guard = sync::guard();
            detach();
        }
        // Window callbacks retain their original execution/lock ordering.
        let result = call1(original::window_push(), window);
        sync::block(0);
        result
    }
}
macro_rules! locked0 {
    ($hook:ident,$original:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $hook() -> u32 {
            unsafe {
                let _guard = sync::guard();
                call1(original::$original(), 0)
            }
        }
    };
}
macro_rules! detached0 {
    ($hook:ident,$original:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $hook() -> u32 {
            unsafe {
                let _guard = sync::guard();
                detach();
                call1(original::$original(), 0)
            }
        }
    };
}
macro_rules! detached1 {
    ($hook:ident,$original:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $hook(a: u32) -> u32 {
            unsafe {
                let _guard = sync::guard();
                detach();
                call1(original::$original(), a)
            }
        }
    };
}
// Preserve nonblocking input-path queries. These read bounded queue IDs / an
// atomic flag, not a pointer into the temporary popup allocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_current_hook() -> u32 {
    unsafe { call1(original::notification_current(), 0) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_active_hook() -> u32 {
    unsafe { call1(original::notification_active(), 0) }
}
locked0!(
    notification_redraw_request_hook,
    notification_redraw_request
);
locked0!(busy_tick_hook, busy_tick);
locked0!(text_scroll_hook, text_scroll);
locked0!(display_scene_refresh_hook, display_scene_refresh);
detached0!(overlays_clear_hook, overlays_clear);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn busy_hide_hook() -> u32 {
    unsafe {
        let _guard = sync::guard();
        // Recorder polling calls hide while already idle. Preserve the stock
        // no-op instead of treating every call as a popup dismissal request.
        if read(0x8020a2b0 as *const u8) == 1 {
            detach();
        }
        call1(original::busy_hide(), 0)
    }
}
detached1!(busy_show_hook, busy_show);
detached1!(notification_pause_hook, notification_pause);
detached1!(notification_remove_hook, notification_remove);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notification_bind_hook(message: u32) -> u32 {
    unsafe {
        let _guard = sync::guard();
        crate::restore();
        call1(original::notification_bind(), message)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scene_clear_hook(mode: u32) -> u32 {
    unsafe {
        let _guard = sync::guard();
        call1(original::scene_clear(), mode)
    }
}
macro_rules! widget2 {
    ($hook:ident,$original:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $hook(widget: u32, value: u32) -> u32 {
            unsafe {
                let _guard = if (58..=60).contains(&widget) {
                    sync::guard()
                } else {
                    None
                };
                call2(original::$original(), widget, value)
            }
        }
    };
}
widget2!(widget_polarity_hook, widget_polarity);
widget2!(widget_y_hook, widget_y);
widget2!(widget_resource_hook, widget_resource);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn localized_text_hook(language: u32, widget: u32, text: u32) -> u32 {
    unsafe {
        let _guard = if (58..=60).contains(&widget) {
            sync::guard()
        } else {
            None
        };
        call3(original::localized_text(), language, widget, text)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn text_measure_hook(language: u32, widget: u32, out: u32) -> u32 {
    unsafe {
        let _guard = if (58..=60).contains(&widget) {
            sync::guard()
        } else {
            None
        };
        call3(original::text_measure(), language, widget, out)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn text_layout_hook(
    language: u32,
    widget: u32,
    offset: u32,
    scroll: u32,
) -> u32 {
    unsafe {
        let _guard = if (58..=60).contains(&widget) {
            sync::guard()
        } else {
            None
        };
        call4(original::text_layout(), language, widget, offset, scroll)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn text_copy_hook(resource: u32, out: u32, size: u32, language: u32) -> u32 {
    unsafe {
        let _guard = if (229..=231).contains(&resource) {
            sync::guard()
        } else {
            None
        };
        call4(original::text_copy(), resource, out, size, language)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn background_draw_hook(background: u32) -> u32 {
    unsafe {
        let _guard = if (44..=45).contains(&background) {
            sync::guard()
        } else {
            None
        };
        call1(original::background_draw(), background)
    }
}
