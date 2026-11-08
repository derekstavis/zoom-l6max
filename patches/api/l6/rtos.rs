// L6 MAIN 1.10 rtos bindings.

firmware_functions! {
    pub unsafe fn l6_initialize_and_start_scheduler() at 0x8006_7a61;
    pub unsafe fn vTaskStartScheduler() at 0x8007_45b9;
}
