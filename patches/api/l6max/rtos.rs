// L6MAX MAIN 1.10 rtos bindings.

firmware_functions! {
    pub unsafe fn main_startup_initialize() at 0x8000_fe09;
    pub unsafe fn vTaskStartScheduler() at 0x8008_ec89;
    pub unsafe fn firmware_semaphore_create(slot: u32, initial: u32) at 0x8005_f600;
    pub unsafe fn x_queue_semaphore_take(semaphore: u32, timeout: u32) -> u32, at 0x8009_1048;
    pub unsafe fn x_queue_generic_send(semaphore: u32, item: u32, timeout: u32, position: u32) -> u32, at 0x8009_0ad0;
    pub unsafe fn v_port_enter_critical() at 0x8008_e508;
    pub unsafe fn v_port_exit_critical() at 0x8008_e558;
}

firmware_variables! {
    pub current_task_control_block: u32, at 0x808a_3af0;
}
