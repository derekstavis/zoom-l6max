// L6max MAIN 1.10 configuration and settings bindings.

firmware_functions! {
    /// Copies live mixer/global values into the settings image and queues a NOR save.
    pub unsafe fn cfg_request_save() at 0x8000_c168;
}
