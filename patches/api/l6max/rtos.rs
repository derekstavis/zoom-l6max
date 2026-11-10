// FreeRTOS and firmware semaphore bindings for L6max MAIN 1.10.

firmware_functions! {
    pub unsafe fn vTaskStartScheduler() at 0x8008_ec88;
    pub unsafe fn pvPortMalloc(size: usize) -> *mut u8, at 0x8008_5478;
    pub unsafe fn vPortFree(pointer: *mut u8) at 0x8008_e588;
    pub unsafe fn xQueueSemaphoreTake(queue: *mut u8, ticks_to_wait: u32) -> i32, at 0x8009_1048;
    pub unsafe fn xQueueGenericSend(
        queue: *mut u8,
        item: *const u8,
        ticks_to_wait: u32,
        copy_position: i32,
    ) -> i32, at 0x8009_0ad0;
    pub unsafe fn vPortEnterCritical() at 0x8008_e508;
    pub unsafe fn vPortExitCritical() at 0x8008_e558;

    /// Firmware wrapper around xQueueCreateCountingSemaphore(max_count = 1).
    pub unsafe fn firmware_semaphore_create(slot: u32, initial: u32) at 0x8005_f600;
}

firmware_variables! {
    pub mut pxCurrentTCB: *mut u8, at 0x808a_3af0;
    pub mut xSchedulerRunning: u32, at 0x8020_ab44;
    pub mut uxSchedulerSuspended: u32, at 0x8020_ab48;
    pub mut xTickCount: u32, at 0x8020_ab4c;
    pub mut xYieldPending: u32, at 0x8020_ab58;
    pub mut xPendedTicks: u32, at 0x8020_ab64;
    pub mut xNumOfOverflows: u32, at 0x8020_ab68;
    pub mut uxCriticalNesting: u32, at 0x8020_7634;
    pub mut xFreeBytesRemaining: u32, at 0x8020_ab1c;
    pub mut xMinimumEverFreeBytesRemaining: u32, at 0x8020_ab20;
    pub mut xNumberOfSuccessfulAllocations: u32, at 0x8020_ab24;
    pub mut xNumberOfSuccessfulFrees: u32, at 0x8020_ab28;
}
