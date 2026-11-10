// L6max MAIN 1.10 panel and UI-event bindings.

firmware_functions! {
    /// Route-0 main-button event handler. `event` points to the five-word UI
    /// event and `context` is the current window/controller context.
    pub unsafe fn panel_handle_main_button_event(event: *const u32, context: *mut u32) at 0x8003_6a40;

    /// Enqueue a five-word event through the stock UI queue. The fifth word is
    /// passed on the caller's stack per AAPCS.
    pub unsafe fn panel_ui_event_queue_send(source: u32, kind: u32, event: u32, reserved: u32, reserved2: u32) -> u32, at 0x8002_8088;
}
