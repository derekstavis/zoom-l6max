// L6MAX MAIN 1.10 gui bindings.

firmware_functions! {
    pub unsafe fn gui_notification_is_active() -> u32, at 0x8000_6060;
    pub unsafe fn gui_notification_pop(reason: u32) -> u32, at 0x8000_6110;
    pub unsafe fn gui_notification_tick() at 0x8000_64c0;
    pub unsafe fn gui_notification_show(message: u32) -> u32, at 0x8000_6660;
    pub unsafe fn gui_notification_draw() at 0x8000_68f8;
    pub unsafe fn gui_notification_bind_text(message: u32) -> u32, at 0x8000_69b0;
    pub unsafe fn gui_notification_request_redraw() -> u32, at 0x8000_65f8;
    pub unsafe fn gui_draw_background(layer: u32) at 0x8000_5e70;
    pub unsafe fn gui_clear_background(background: u32) at 0x8000_5db0;
    pub unsafe fn gui_clear_layer_rect(layer: u32, x0: u32, y0: u32, x1: u32, y1: u32) at 0x8000_5f18;
    pub unsafe fn gui_publish(layer: u32) at 0x8000_5f50;
    pub unsafe fn gui_draw_text_widget(widget: u32) at 0x8000_6ad8;
    pub unsafe fn gui_current_language() -> u32, at 0x8000_afd0;
    pub unsafe fn gui_measure_text_widget(language: u32, widget: u32, out: u32) -> u32, at 0x8000_6d50;
    pub unsafe fn gui_set_localized_widget_text(language: u32, widget: u32, text: u32) -> u32, at 0x8000_6fa8;
    pub unsafe fn gui_set_widget_polarity(widget: u32, polarity: u32) -> u32, at 0x8000_7080;
    pub unsafe fn gui_set_widget_resource(widget: u32, resource: u32) -> u32, at 0x8000_7090;
    pub unsafe fn gui_set_widget_x(widget: u32, x: u32) -> u32, at 0x8000_70a0;
    pub unsafe fn gui_set_widget_y(widget: u32, y: u32) -> u32, at 0x8000_70b0;
    pub unsafe fn gui_set_widget_width(widget: u32, width: u32) -> u32, at 0x8000_70c0;
    pub unsafe fn gui_set_widget_text(widget: u32, text: u32) -> u32, at 0x8000_70d8;
    pub unsafe fn gui_draw_text_layout(language: u32, widget: u32, offset: u32, scroll: u32) -> u32, at 0x8006_1ab0;
    /// Valid slots are `0..GUI_DISPLAY_CALLBACK_COUNT`; firmware does not check the bound.
    pub unsafe fn gui_display_callback_register(slot: u32, callback: u32) at 0x8002_4ed0;
    pub unsafe fn gui_display_callback_unregister(slot: u32) at 0x8002_4eb8;
    pub unsafe fn gui_push_window(window: u32) -> u32, at 0x8008_bd40;
}

firmware_variables! {
    pub mut notification_slot: u32, at 0x8020_a2a0;
    pub current_window: u32, at 0x8020_2610;
    pub notification_active: u8, at 0x8020_a2b0;
    pub notification_busy: u8, at 0x8020_a2c0;
    pub notification_pause: u8, at 0x8020_a2c8;
    pub mut notification_timer: u16, at 0x8020_a2c4;
    pub current_notification_message: u8, at 0x8020_a2c2;
    pub mut gui_notification_header_y: u16, at 0x8020_098c;
    pub mut gui_notification_header_polarity: u16, at 0x8020_0986;
    pub mut gui_notification_header_font: u16, at 0x8020_0990;
    pub mut gui_notification_value_y: u16, at 0x8020_099c;
    /// Root pointer to the per-language UTF-16 resource-capacity tables.
    pub gui_text_lengths_by_language: u32, at 0x8020_50fc;
    /// Root pointer to the per-language UTF-16 text-resource pointer tables.
    pub gui_text_pointers_by_language: u32, at 0x8020_5118;
    /// GUI descriptor pointer arrays are stored directly at these addresses;
    /// use `.address()` to index their entries.
    pub gui_background_table: u32, at 0x8020_0670;
    pub gui_widget_table: u32, at 0x8020_0af4;
    /// Six-slot DisplayCycle2 callback pointer table; `.address()` is its base.
    pub gui_display_callback_table: u32, at 0x8020_a59c;
    /// Null bitmap-table slot 0 in L6max MAIN 1.10; background drawing skips
    /// bitmap ID 0 before dereferencing the table. knob-dialog uses it as a
    /// persistent state handle while its callback is registered.
    pub mut gui_bitmap_table_entry_0: u32, at 0x8020_73c8;
}

// Fixed table sizes in L6max MAIN 1.10.
pub const GUI_WIDGET_COUNT: usize = 61;
pub const GUI_BACKGROUND_COUNT: usize = 49;
pub const GUI_DISPLAY_CALLBACK_COUNT: usize = 6;
pub const GUI_DISPLAY_WIDTH: u32 = 128;
pub const GUI_DISPLAY_HEIGHT: u32 = 64;
/// Address of the recorder home window descriptor in L6max MAIN 1.10.
pub const GUI_RECORDER_WINDOW: u32 = 0x8020_0408;

/// Callback entry passed to the firmware's display callback table.
pub const GUI_NOTIFICATION_TICK: usize = 0x8000_64c0;
