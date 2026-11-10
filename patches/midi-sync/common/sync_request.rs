//! Shared MIDI sync request recognition for L6 and L6max.

pub const SYNC_REQUEST_CC: u8 = 127;
pub const SYNC_REQUEST_MIN_VALUE: u8 = 64;

pub fn is_sync_request(status: u8, controller: u8, value: u8, configured_channel: u8) -> bool {
    status & 0xf0 == 0xb0
        && status & 0x0f == configured_channel
        && controller == SYNC_REQUEST_CC
        && value >= SYNC_REQUEST_MIN_VALUE
}

/// Stock MIDI handler messages store status, controller and value at +4..+7.
#[allow(dead_code)]
pub unsafe fn is_sync_message(message: *const u8, configured_channel: u8) -> bool {
    unsafe {
        is_sync_request(
            core::ptr::read_volatile(message.add(4)),
            core::ptr::read_volatile(message.add(5)),
            core::ptr::read_volatile(message.add(6)),
            configured_channel,
        )
    }
}
