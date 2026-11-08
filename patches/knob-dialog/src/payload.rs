#![no_std]
#![no_main]
#[path = "../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
use core::ptr::{read_volatile as read, write_volatile as write};
use core::sync::atomic::{AtomicU8, Ordering};
use patch_api::function_call_post_hook;

mod firmware;
use firmware as firmware_bindings;
#[path = "../../api/l6max/hooks.rs"]
mod hook_catalog;
mod values;
use firmware::*;

#[used]
#[unsafe(link_section = ".rodata.scene_prototype")]
static SCENE_PROTOTYPE_TAG: [u8; 9] = *b"KNOB_WIN1";

#[repr(C)]
struct State {
    magic: u16,
    ticks: u8,
    pending: AtomicU8,
}
const _: [(); 4] = [(); core::mem::size_of::<State>()];
const _: [(); 3] = [(); core::mem::offset_of!(State, pending)];
const MAGIC: u16 = 0x4b4e;
const STATE_SLOT: *mut State = gui_bitmap_table_entry_0.address() as *mut State;
// Stock notification frame and body text use layers 5 and 6. Keeping the
// dialog on layer 4 lets notifications appear above it temporarily.
const SCENE_LAYER: u8 = 4;
const DISPLAY_CALLBACK_SLOT: usize = 5;
const _: () = assert!(DISPLAY_CALLBACK_SLOT < GUI_DISPLAY_CALLBACK_COUNT);
const TITLE_WIDGET: usize = 7;
const VALUE_WIDGET: usize = 33;
const TITLE_BACKGROUND: usize = 10;
const VALUE_BACKGROUND: usize = 37;
const BLACK_BACKGROUND: usize = 48;
const SCENE_BACKGROUNDS: [usize; 3] = [
    TITLE_BACKGROUND,
    VALUE_BACKGROUND,
    BLACK_BACKGROUND,
];
const STOCK_BACKGROUND_LAYERS: [u8; 3] = [0, 0, 8];
const DIALOG_TEXT_RESOURCE: usize = 179;
const DIALOG_TEXT_CAPACITY: usize = 255;
const SCENE_TIMEOUT_TICKS: u8 = 20;
const CHANNEL_COUNT: u32 = 8;
const MODE_COUNT: u32 = 10;
const NO_PENDING_EVENT: u8 = u8::MAX;
const MODE_TITLES: [&[u8]; MODE_COUNT as usize] = [
    b"High", b"Freq", b"Mid", b"Low", b"Aux1", b"Aux2", b"Efx", b"Sub-Mix", b"Pan",
    b"Level",
];

unsafe fn language_tables() -> (*mut u32, *mut u16) {
    unsafe {
        (
            variable_read!(gui_text_pointers_by_language) as *mut u32,
            variable_read!(gui_text_lengths_by_language) as *mut u16,
        )
    }
}

unsafe fn widget_descriptor(id: usize) -> *mut u8 {
    unsafe {
        let table = gui_widget_table.address() as *const u32;
        read(table.add(id)) as *mut u8
    }
}

unsafe fn background_descriptor(id: usize) -> *mut u8 {
    unsafe {
        let table = gui_background_table.address() as *const u32;
        read(table.add(id)) as *mut u8
    }
}

#[derive(Clone, Copy)]
struct WidgetStyle {
    layer: u8,
    polarity: u16,
    resource: u16,
    x: u16,
    y: u16,
    width: u16,
    font: u16,
    alignment: u8,
    scroll: u8,
}

const STOCK_TITLE_STYLE: WidgetStyle = WidgetStyle {
    layer: 1,
    polarity: 0,
    resource: 10,
    x: 1,
    y: 1,
    width: 126,
    font: 1,
    alignment: 1,
    scroll: 2,
};
const STOCK_VALUE_STYLE: WidgetStyle = WidgetStyle {
    layer: 1,
    polarity: 1,
    resource: 40,
    x: 30,
    y: 33,
    width: 67,
    font: 1,
    alignment: 1,
    scroll: 0,
};
const DIALOG_TITLE_STYLE: WidgetStyle = WidgetStyle {
    layer: SCENE_LAYER,
    polarity: 0,
    resource: DIALOG_TEXT_RESOURCE as u16,
    x: 0,
    y: 2,
    width: GUI_DISPLAY_WIDTH as u16,
    font: 1,
    alignment: 1,
    scroll: 0,
};
const DIALOG_VALUE_STYLE: WidgetStyle = WidgetStyle {
    y: 33,
    ..DIALOG_TITLE_STYLE
};

fn stock_widget_style(id: usize) -> Option<WidgetStyle> {
    match id {
        TITLE_WIDGET => Some(STOCK_TITLE_STYLE),
        VALUE_WIDGET => Some(STOCK_VALUE_STYLE),
        _ => None,
    }
}

unsafe fn set_widget_style(id: usize, widget: *mut u8, style: WidgetStyle) {
    unsafe {
        write(widget, style.layer);
        write(widget.add(2) as *mut u16, style.polarity);
        gui_set_widget_resource(id as u32, style.resource as u32);
        write(widget.add(6) as *mut u16, style.x);
        write(widget.add(8) as *mut u16, style.y);
        write(widget.add(10) as *mut u16, style.width);
        write(widget.add(12) as *mut u16, style.font);
        write(widget.add(14), style.alignment);
        write(widget.add(15), style.scroll);
    }
}
unsafe fn state() -> *mut State {
    unsafe {
        if read(core::ptr::addr_of!((*STATE_SLOT).magic)) == MAGIC {
            STATE_SLOT
        } else {
            core::ptr::null_mut()
        }
    }
}

unsafe fn restore_shared_text(buffer: *mut u16) {
    unsafe {
        for index in 0..DIALOG_TEXT_CAPACITY {
            write(buffer.add(index), b'*' as u16);
        }
        write(buffer.add(DIALOG_TEXT_CAPACITY), 0);
    }
}

unsafe fn shared_text_is_available(buffer: *const u16) -> bool {
    unsafe {
        if buffer.is_null() {
            return false;
        }
        for index in 0..DIALOG_TEXT_CAPACITY {
            if read(buffer.add(index)) != b'*' as u16 {
                return false;
            }
        }
        read(buffer.add(DIALOG_TEXT_CAPACITY)) == 0
    }
}

unsafe fn widget_matches_firmware_default(widget: *const u8, id: usize) -> bool {
    unsafe {
        let Some(style) = stock_widget_style(id) else {
            return false;
        };
        read(widget) == style.layer
            && read(widget.add(2) as *const u16) == style.polarity
            && read(widget.add(4) as *const u16) == style.resource
            && read(widget.add(6) as *const u16) == style.x
            && read(widget.add(8) as *const u16) == style.y
            && read(widget.add(10) as *const u16) == style.width
            && read(widget.add(12) as *const u16) == style.font
            && read(widget.add(14)) == style.alignment
            && read(widget.add(15)) == style.scroll
    }
}

unsafe fn scene_resources_are_available() -> bool {
    unsafe {
        for widget_id in 0..GUI_WIDGET_COUNT {
            let widget = widget_descriptor(widget_id) as *const u8;
            if widget.is_null() {
                return false;
            }
            if widget_id != TITLE_WIDGET
                && widget_id != VALUE_WIDGET
                && (read(widget) == SCENE_LAYER
                    || read(widget.add(4) as *const u16) as usize == DIALOG_TEXT_RESOURCE)
            {
                return false;
            }
        }
        for background_id in 0..GUI_BACKGROUND_COUNT {
            if SCENE_BACKGROUNDS.contains(&background_id) {
                continue;
            }
            let background = background_descriptor(background_id) as *const u8;
            if background.is_null() || read(background) as u16 == SCENE_LAYER as u16 {
                return false;
            }
        }
        true
    }
}

unsafe fn scene_is_available_for_first_draw() -> bool {
    unsafe {
        let (text_table, lengths) = language_tables();
        let shared_text_buffer = read(text_table.add(DIALOG_TEXT_RESOURCE)) as *const u16;
        if read(lengths.add(DIALOG_TEXT_RESOURCE)) as usize != DIALOG_TEXT_CAPACITY
            || !shared_text_is_available(shared_text_buffer)
            || !scene_resources_are_available()
        {
            return false;
        }
        for widget_id in [TITLE_WIDGET, VALUE_WIDGET] {
            let widget = widget_descriptor(widget_id) as *const u8;
            if widget.is_null() || !widget_matches_firmware_default(widget, widget_id) {
                return false;
            }
        }
        for (background_id, stock_layer) in SCENE_BACKGROUNDS
            .into_iter()
            .zip(STOCK_BACKGROUND_LAYERS)
        {
            let background = background_descriptor(background_id) as *const u8;
            if background.is_null() || read(background) != stock_layer {
                return false;
            }
        }
        true
    }
}

unsafe fn restore_widget(widget: *mut u8, id: usize) {
    unsafe {
        let Some(style) = stock_widget_style(id) else {
            return;
        };
        set_widget_style(id, widget, style);
    }
}

unsafe fn restore_scene(discard_pending: bool) {
    unsafe {
        let pointer = state();
        if pointer.is_null() {
            return;
        }
        gui_clear_layer_rect(
            SCENE_LAYER as u32,
            0,
            0,
            GUI_DISPLAY_WIDTH,
            GUI_DISPLAY_HEIGHT,
        );
        gui_publish(0);
        let (text_table, _) = language_tables();
        let shared_text_buffer = read(text_table.add(DIALOG_TEXT_RESOURCE)) as *mut u16;
        if !shared_text_buffer.is_null() {
            restore_shared_text(shared_text_buffer);
        }
        for widget_id in [TITLE_WIDGET, VALUE_WIDGET] {
            let widget = widget_descriptor(widget_id);
            restore_widget(widget, widget_id);
        }
        for (background_id, stock_layer) in SCENE_BACKGROUNDS
            .into_iter()
            .zip(STOCK_BACKGROUND_LAYERS)
        {
            let background = background_descriptor(background_id);
            write(background, stock_layer);
        }
        write(core::ptr::addr_of_mut!((*pointer).ticks), 0);
        if discard_pending {
            (*pointer)
                .pending
                .store(NO_PENDING_EVENT, Ordering::Relaxed);
        }
        // Keep the inline state record for the rest of this boot so bitmap-
        // table slot 0 remains a valid state handle while slot 5 is registered.
    }
}

unsafe fn render_scene(title_text: &[u16; 24], value_text: &[u16; 24]) {
    unsafe {
        let pointer = state();
        if pointer.is_null() {
            return;
        }
        let (text_table, lengths) = language_tables();
        let current_text_buffer = read(text_table.add(DIALOG_TEXT_RESOURCE));
        let text_capacity = read(lengths.add(DIALOG_TEXT_RESOURCE)) as usize;
        if current_text_buffer == 0 || text_capacity != DIALOG_TEXT_CAPACITY {
            restore_scene(true);
            return;
        }
        gui_clear_layer_rect(SCENE_LAYER as u32, 0, 0, 128, 64);
        for background_id in SCENE_BACKGROUNDS {
            let background = background_descriptor(background_id);
            write(background, SCENE_LAYER);
        }
        // Paint an opaque black field and title band on the overlay layer.
        // Both text widgets draw on this same layer.
        gui_draw_background(BLACK_BACKGROUND as u32);
        gui_draw_background(TITLE_BACKGROUND as u32);
        let title = widget_descriptor(TITLE_WIDGET);
        set_widget_style(TITLE_WIDGET, title, DIALOG_TITLE_STYLE);
        // This firmware revision's language getter is constant zero, so the
        // generic setter resolves the same table validated during setup.
        gui_set_widget_text(TITLE_WIDGET as u32, title_text.as_ptr() as u32);
        gui_draw_text_widget(TITLE_WIDGET as u32);
        let value = widget_descriptor(VALUE_WIDGET);
        set_widget_style(VALUE_WIDGET, value, DIALOG_VALUE_STYLE);
        gui_set_widget_text(VALUE_WIDGET as u32, value_text.as_ptr() as u32);
        gui_draw_text_widget(VALUE_WIDGET as u32);
        gui_publish(0);
    }
}

unsafe fn scene_tick() {
    unsafe {
        let pointer = state();
        if pointer.is_null() {
            return;
        }
        let ticks = read(core::ptr::addr_of!((*pointer).ticks));
        if ticks == 0 {
            return;
        }
        write(core::ptr::addr_of_mut!((*pointer).ticks), ticks - 1);
        if ticks == 1 {
            // Preserve an event that arrived on the expiry boundary. It will
            // open a fresh scene on the next callback pass.
            restore_scene(false);
        }
    }
}

unsafe fn take_pending() -> Option<(u32, u32)> {
    unsafe {
        let pointer = state();
        if pointer.is_null() {
            return None;
        }
        // The byte contains the entire message; no other memory is published
        // through this mailbox, so atomicity is needed without memory fences.
        let event = (*pointer).pending.swap(NO_PENDING_EVENT, Ordering::Relaxed);
        if event == NO_PENDING_EVENT {
            return None;
        }
        let channel = (event >> 4) as u32;
        let mode = (event & 0x0f) as u32;
        (channel < CHANNEL_COUNT && mode < MODE_COUNT).then_some((channel, mode))
    }
}

/// Service knob updates and the overlay from a dedicated DisplayCycle2 slot.
/// Slot 5 runs after the stock scrolling, busy-overlay, and notification slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn knob_dialog_display_tick() {
    unsafe {
        let pointer = state();
        if pointer.is_null() {
            return;
        }
        if variable_read!(current_window) != GUI_RECORDER_WINDOW {
            if (*pointer).ticks != 0 {
                restore_scene(true);
            } else {
                (*pointer).pending.store(NO_PENDING_EVENT, Ordering::Relaxed);
            }
            return;
        }
        if variable_read!(notification_active) == 0 && variable_read!(notification_busy) == 0 {
            if let Some((channel, mode)) = take_pending() {
                present(channel, mode);
            }
            scene_tick();
        }
    }
}

fn text(out: &mut [u16; 24], bytes: &[u8]) {
    out.fill(0);
    for (a, b) in out.iter_mut().zip(bytes.iter().take(23)) {
        *a = *b as u16;
    }
}
#[function_call_post_hook(mixer_apply_channel_parameter, at = 0x8003_6c74)]
pub unsafe fn knob_changed(
    channel: u32,
    mode: u32,
    absolute: u32,
    delta: i32,
    value: u32,
) {
    unsafe {
        if channel >= CHANNEL_COUNT || mode >= MODE_COUNT || delta == 0 {
            return;
        }
        // Only the normal recorder window: leave setup and modal dialogs alone.
        if variable_read!(current_window) != GUI_RECORDER_WINDOW {
            return;
        }
        let active = state();
        if !active.is_null() {
            (*active)
                .pending
                .store(((channel as u8) << 4) | mode as u8, Ordering::Relaxed);
            return;
        }
        // The mixer operation already ran. Slot 0 stores the retained state;
        // stock notifications continue using bitmap-table entry 36.
        let pointer = ensure_state();
        if pointer.is_null() {
            return;
        }
        (*pointer)
            .pending
            .store(((channel as u8) << 4) | mode as u8, Ordering::Relaxed);
    }
}

unsafe fn ensure_state() -> *mut State {
    unsafe {
        let existing = state();
        if !existing.is_null() {
            return existing;
        }

        // Slot 5 is not used by stock firmware. Refuse to replace a callback
        // another component may have installed since boot.
        let callback_slot = gui_display_callback_table.address() as *const u32;
        let callback_slot = callback_slot.add(DISPLAY_CALLBACK_SLOT);
        let callback = crate::knob_dialog_display_tick as usize as u32 | 1;
        let registered = read(callback_slot);
        if variable_read!(gui_bitmap_table_entry_0) != 0
            || (registered != 0 && registered != callback)
        {
            return core::ptr::null_mut();
        }
        if !scene_is_available_for_first_draw() {
            return core::ptr::null_mut();
        }
        // Bitmap ID 0 returns before reading its table entry, so its four-byte
        // null slot can hold the state record directly. Initialize its atomic
        // mailbox before publishing the signature or registering the callback.
        core::ptr::write_bytes(STATE_SLOT, 0, 1);
        (*STATE_SLOT)
            .pending
            .store(NO_PENDING_EVENT, Ordering::Relaxed);
        (*STATE_SLOT).magic = MAGIC;
        // Publish fully initialized state before making the callback visible
        // to DisplayCycle2. Slot 5 has no stock registrants, and this setup is
        // reached only from the single mixer-hook task.
        gui_display_callback_register(DISPLAY_CALLBACK_SLOT as u32, callback);
        STATE_SLOT
    }
}

// Only the slot-5 display callback calls this.
unsafe fn present(channel: u32, mode: u32) {
    unsafe {
        let current = variable_read!(current_window);
        let pointer = state();
        if pointer.is_null()
            || current != GUI_RECORDER_WINDOW
            || variable_read!(notification_active) != 0
            || variable_read!(notification_busy) != 0
        {
            return;
        }
        // The mixer task and display callback are separate contexts. Repeat
        // the first-use checks here before the first descriptor mutation.
        if (*pointer).ticks == 0 && !scene_is_available_for_first_draw() {
            return;
        }
        let mut title = [0u16; 24];
        text(&mut title, MODE_TITLES[mode as usize]);
        let audio = mixer_channel_index(channel);
        let raw = match mode {
            0 => mixer_get_eq_gain(audio, 0),
            1 => mixer_get_eq_frequency(audio, 1),
            2 => mixer_get_eq_gain(audio, 1),
            3 => mixer_get_eq_gain(audio, 2),
            4 | 5 => mixer_get_aux(audio, mode - 4),
            6 => mixer_get_efx(audio, 0),
            7 => mixer_get_submix(audio),
            8 => mixer_get_pan(audio),
            _ => mixer_get_level(audio),
        };
        if raw > 127 {
            return;
        }
        let gain = if matches!(mode, 0 | 2 | 3) {
            read((EQ_GAIN_DB_TABLE + 4 * raw) as *const f32)
        } else {
            0.0
        };
        let frequency = if mode == 1 {
            read((EQ_MID_FREQUENCY_HZ_TABLE + 4 * raw) as *const f32)
        } else {
            0.0
        };
        let value_text = values::format_value(mode, raw, gain, frequency);
        write(
            core::ptr::addr_of_mut!((*pointer).ticks),
            SCENE_TIMEOUT_TICKS,
        );
        render_scene(&title, &value_text);
    }
}
