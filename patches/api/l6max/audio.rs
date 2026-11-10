// L6max MAIN 1.10 audio processing bindings.

firmware_functions! {
    /// DSP stream callback reached from the relocated audio wrapper; emulator-observed ABI registers.
    pub unsafe fn dsp_stream_callback(
        argument_0: u32,
        argument_1: u32,
        argument_2: u32,
        argument_3: u32,
    ) at 0x8006_6180;
}
