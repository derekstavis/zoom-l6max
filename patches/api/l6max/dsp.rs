// L6max MAIN 1.10 DSP and effect callback bindings.

/// Size of the shared stock effect workspace in bytes.
pub const DSP_EFFECT_WORKSPACE_BYTES: usize = 0x000b_b800;

firmware_functions! {
    /// Dispatches effect processing through the relocated mixer effect selector.
    /// The last argument requests a refresh of the effect setup from the
    /// selected type's stored parameters.
    pub unsafe fn dsp_effect_processing_dispatch(
        bus: u32,
        effect_type: u32,
        parameter_0: u32,
        parameter_1: u32,
        refresh: u32,
    ) at 0x8000_3c58;

    /// Audio-path helper invoked before the active effect callback; it reads its stream state from globals.
    pub unsafe fn dsp_stream_callback() at 0x8006_6180;

    /// Clears the shared stock effect workspace before publishing an effect callback.
    pub unsafe fn dsp_effect_workspace_clear() at 0x8002_4678;

    /// Room effect callback selected by L6max MAIN 1.10.
    pub unsafe fn dsp_room_effect_callback(
        effect_context: *mut u8,
        effect_descriptor: *mut u8,
    ) at 0x8010_5928;

    /// Spring effect callback selected by L6max MAIN 1.10.
    pub unsafe fn dsp_spring_effect_callback(
        effect_context: *mut u8,
        effect_descriptor: *mut u8,
    ) at 0x8010_7090;
}

firmware_variables! {
    /// Fixed shared DSP state region cleared and reused by the stock effect selector.
    /// Use `.address()` to access it in place; reading this variable would copy 768 KB.
    pub mut dsp_effect_workspace: [u8; DSP_EFFECT_WORKSPACE_BYTES], at 0x80d0_0000;
}
