// L6max MAIN 1.10 effect callback declarations.

firmware_functions! {
    /// Clears the stock shared effect workspace before publishing an effect callback.
    pub unsafe fn effect_workspace_clear() at 0x8002_4678;

    /// Room effect callback selected by L6max MAIN 1.10.
    pub unsafe fn room_effect_callback(
        effect_context: *mut u8,
        effect_descriptor: *mut u8,
    ) at 0x8010_5928;

    /// Spring effect callback selected by L6max MAIN 1.10.
    pub unsafe fn spring_effect_callback(
        effect_context: *mut u8,
        effect_descriptor: *mut u8,
    ) at 0x8010_7090;
}
