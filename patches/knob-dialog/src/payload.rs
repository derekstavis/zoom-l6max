#![cfg_attr(not(sd_composed), no_std)]
#![cfg_attr(not(sd_composed), no_main)]
#[cfg(not(sd_composed))]
#[path = "../../api/patchrt.rs"]
#[macro_use]
#[cfg(not(sd_composed))]
pub mod patch_api;
#[cfg(sd_composed)]
use crate::patch_api;
use core::ptr::{read_volatile as read, write_volatile as write};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use patch_api::function_call_post_hook;

#[cfg(not(sd_composed))]
mod firmware;
#[cfg(not(sd_composed))]
use firmware as firmware_bindings;
#[path = "../../api/l6max/hooks.rs"]
#[cfg(not(sd_composed))]
mod hook_catalog;
mod values;
#[cfg(not(sd_composed))]
use firmware::*;
#[cfg(sd_composed)]
use crate::firmware_bindings::*;

// Original firmware EQ coefficient tables, not reconstructed curves.
const EQ_GAIN_DB_TABLE: u32 = 0x800a_5138;
const EQ_MID_FREQUENCY_HZ_TABLE: u32 = 0x800a_5738;

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

// External patches publish copied ASCII strings through this one-slot
// mailbox. Atomic words plus a nonblocking writer claim let an audio task
// update it without calling GUI code or waiting for the display task.
const EXTERNAL_TEXT_BYTES: usize = 24;
#[cfg(sd_dynamic_module)]
struct ExternalTextRequest {
    sequence: AtomicU32,
    title_len: AtomicU32,
    value_len: AtomicU32,
    words: [AtomicU32; 12],
}
#[cfg(sd_dynamic_module)]
impl ExternalTextRequest {
    const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            title_len: AtomicU32::new(0),
            value_len: AtomicU32::new(0),
            words: [const { AtomicU32::new(0) }; 12],
        }
    }
}
#[cfg(sd_dynamic_module)]
static EXTERNAL_TEXT: ExternalTextRequest = ExternalTextRequest::new();
#[cfg(sd_dynamic_module)]
static EXTERNAL_TEXT_WRITER: AtomicBool = AtomicBool::new(false);
#[cfg(sd_dynamic_module)]
static EXTERNAL_TEXT_PENDING: AtomicU8 = AtomicU8::new(0);
#[cfg(sd_dynamic_module)]
#[unsafe(no_mangle)]
pub(crate) static KNOB_DIALOG_EXTERNAL_TEXT_ACTIVE: AtomicU8 = AtomicU8::new(0);

#[cfg(sd_dynamic_module)]
unsafe fn publish_external_text(
    title: *const u8,
    title_len: u32,
    value: *const u8,
    value_len: u32,
) -> bool {
    let title_len = title_len as usize;
    let value_len = value_len as usize;
    if title.is_null()
        || value.is_null()
        || title_len == 0
        || title_len >= EXTERNAL_TEXT_BYTES
        || value_len == 0
        || value_len >= EXTERNAL_TEXT_BYTES
        || EXTERNAL_TEXT_WRITER
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
    {
        return false;
    }

    let previous = EXTERNAL_TEXT.sequence.load(Ordering::Relaxed) & !1;
    EXTERNAL_TEXT
        .sequence
        .store(previous.wrapping_add(1), Ordering::Release);
    EXTERNAL_TEXT.title_len.store(title_len as u32, Ordering::Relaxed);
    EXTERNAL_TEXT.value_len.store(value_len as u32, Ordering::Relaxed);
    for word_index in 0..12 {
        let start = if word_index < 6 {
            word_index * 4
        } else {
            (word_index - 6) * 4
        };
        let (source, length) = if word_index < 6 {
            (title, title_len)
        } else {
            (value, value_len)
        };
        let mut packed = 0u32;
        for byte_index in 0..4 {
            let index = start + byte_index;
            if index < length {
                packed |= u32::from(unsafe { core::ptr::read_volatile(source.add(index)) })
                    << (byte_index * 8);
            }
        }
        EXTERNAL_TEXT.words[word_index].store(packed, Ordering::Relaxed);
    }
    EXTERNAL_TEXT
        .sequence
        .store(previous.wrapping_add(2), Ordering::Release);
    EXTERNAL_TEXT_PENDING.store(1, Ordering::Release);
    KNOB_DIALOG_EXTERNAL_TEXT_ACTIVE.store(1, Ordering::Release);
    // A caller-provided value supersedes any generic knob label queued by
    // the post-call hook for the same physical turn.
    let state = unsafe { state() };
    if !state.is_null() {
        unsafe { (*state).pending.store(NO_PENDING_EVENT, Ordering::Release) };
    }
    EXTERNAL_TEXT_WRITER.store(false, Ordering::Release);
    true
}

/// Prepare the shared dialog before a composed dependent patch publishes text.
#[cfg(sd_dynamic_module)]
pub(crate) fn prepare_external_text() -> bool {
    unsafe { !ensure_state().is_null() }
}

/// Publish bounded ASCII text from another patch in the same composed module.
#[cfg(sd_dynamic_module)]
pub(crate) fn show_external_text(title: &[u8], value: &[u8]) -> bool {
    unsafe {
        if state().is_null() {
            return false;
        }
        publish_external_text(
            title.as_ptr(),
            title.len() as u32,
            value.as_ptr(),
            value.len() as u32,
        )
    }
}

#[cfg(sd_dynamic_module)]
unsafe fn take_external_text() -> Option<([u16; 24], [u16; 24])> {
    if EXTERNAL_TEXT_PENDING.swap(0, Ordering::AcqRel) == 0 {
        return None;
    }
    let before = EXTERNAL_TEXT.sequence.load(Ordering::Acquire);
    if before & 1 != 0 {
        EXTERNAL_TEXT_PENDING.store(1, Ordering::Release);
        return None;
    }
    let title_len = EXTERNAL_TEXT.title_len.load(Ordering::Relaxed) as usize;
    let value_len = EXTERNAL_TEXT.value_len.load(Ordering::Relaxed) as usize;
    if title_len == 0
        || title_len >= EXTERNAL_TEXT_BYTES
        || value_len == 0
        || value_len >= EXTERNAL_TEXT_BYTES
    {
        return None;
    }
    let mut title = [0u16; 24];
    let mut value = [0u16; 24];
    for index in 0..title_len {
        let word = EXTERNAL_TEXT.words[index / 4].load(Ordering::Relaxed);
        title[index] = ((word >> ((index % 4) * 8)) & 0xff) as u16;
    }
    for index in 0..value_len {
        let word = EXTERNAL_TEXT.words[6 + index / 4].load(Ordering::Relaxed);
        value[index] = ((word >> ((index % 4) * 8)) & 0xff) as u16;
    }
    if EXTERNAL_TEXT.sequence.load(Ordering::Acquire) != before {
        EXTERNAL_TEXT_PENDING.store(1, Ordering::Release);
        return None;
    }
    Some((title, value))
}

#[cfg(not(sd_dynamic_module))]
unsafe fn take_external_text() -> Option<([u16; 24], [u16; 24])> {
    None
}

unsafe fn language_tables() -> (*mut u32, *mut u16) {
    unsafe {
        (
            gui_text_pointers_by_language.read() as *mut u32,
            gui_text_lengths_by_language.read() as *mut u16,
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
        #[cfg(sd_dynamic_module)]
        KNOB_DIALOG_EXTERNAL_TEXT_ACTIVE.store(0, Ordering::Release);
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
        if current_window.read() != GUI_RECORDER_WINDOW {
            if (*pointer).ticks != 0 {
                restore_scene(true);
            } else {
                (*pointer).pending.store(NO_PENDING_EVENT, Ordering::Relaxed);
            }
            #[cfg(sd_dynamic_module)]
            {
                EXTERNAL_TEXT_PENDING.store(0, Ordering::Release);
                KNOB_DIALOG_EXTERNAL_TEXT_ACTIVE.store(0, Ordering::Release);
            }
            return;
        }
        if notification_active.read() == 0 && notification_busy.read() == 0 {
            #[cfg(sd_dynamic_module)]
            if let Some((title, value)) = take_external_text() {
                if (*pointer).ticks == 0 && !scene_is_available_for_first_draw() {
                    KNOB_DIALOG_EXTERNAL_TEXT_ACTIVE.store(0, Ordering::Release);
                } else {
                    write(
                        core::ptr::addr_of_mut!((*pointer).ticks),
                        SCENE_TIMEOUT_TICKS,
                    );
                    render_scene(&title, &value);
                }
            }
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
        if current_window.read() != GUI_RECORDER_WINDOW {
            return;
        }
        #[cfg(sd_dynamic_module)]
        if KNOB_DIALOG_EXTERNAL_TEXT_ACTIVE.load(Ordering::Acquire) != 0 {
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
        let callback = knob_dialog_display_tick as usize as u32 | 1;
        let registered = read(callback_slot);
        if gui_bitmap_table_entry_0.read() != 0
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
        let current = current_window.read();
        let pointer = state();
        if pointer.is_null()
            || current != GUI_RECORDER_WINDOW
            || notification_active.read() != 0
            || notification_busy.read() != 0
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
