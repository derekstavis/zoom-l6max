//! Stable sidecar for the existing counting semaphore; no fabricated RTOS objects.
//! Only the patched notification slot contains a tagged pointer. Kernel calls
//! always receive the original, untagged semaphore handle.
use crate::firmware::*;
use core::ptr::{read_volatile as read, write_volatile as write};
const SLOT: *mut u32 = 0x8020a2a0 as *mut u32;
const CURRENT_TCB: *const u32 = 0x808a3af0 as *const u32;
#[repr(C)]
pub struct Context {
    pub semaphore: u32,
    owner: u32,
    depth: u32,
    pub pending: u32,
    pub mode: u32,
    pub raw: u32,
    pub generation: u32,
    pub event_generation: u32,
    pub blocked: u32,
}
pub unsafe fn context() -> *mut Context {
    let p = unsafe { read(SLOT) };
    if p & 1 == 0 {
        core::ptr::null_mut()
    } else {
        (p & !1) as *mut Context
    }
}
unsafe fn enter() {
    unsafe {
        call1(V_PORT_ENTER_CRITICAL, 0);
    }
}
unsafe fn exit() {
    unsafe {
        call1(V_PORT_EXIT_CRITICAL, 0);
    }
}
pub unsafe fn initialize(slot: u32, initial: u32) {
    unsafe {
        call2(FIRMWARE_SEMAPHORE_CREATE, slot, initial);
        let semaphore = read(slot as *const u32);
        if semaphore == 0 {
            return;
        }
        let c = call1(PV_PORT_MALLOC, core::mem::size_of::<Context>() as u32) as *mut Context;
        if c.is_null() {
            return;
        } // Stock notification behavior remains available.
        core::ptr::write_bytes(c, 0, 1);
        write(core::ptr::addr_of_mut!((*c).semaphore), semaphore);
        write(slot as *mut u32, c as u32 | 1);
        // Keep the existing notification slot alive so first-knob presentation
        // is serviced even while the stock notification queue is empty.
        call2(
            GUI_DISPLAY_CALLBACK_REGISTER,
            4,
            GUI_NOTIFICATION_TICK as u32 | 1,
        );
    }
}
pub unsafe fn take(tagged: u32, timeout: u32) -> u32 {
    unsafe {
        if tagged & 1 == 0 {
            return call2(X_QUEUE_SEMAPHORE_TAKE, tagged, timeout);
        }
        let c = (tagged & !1) as *mut Context;
        enter();
        let task = read(CURRENT_TCB);
        if read(core::ptr::addr_of!((*c).depth)) != 0
            && read(core::ptr::addr_of!((*c).owner)) == task
        {
            let depth = read(core::ptr::addr_of!((*c).depth));
            write(core::ptr::addr_of_mut!((*c).depth), depth + 1);
            exit();
            return 1;
        }
        exit();
        if call2(
            X_QUEUE_SEMAPHORE_TAKE,
            read(core::ptr::addr_of!((*c).semaphore)),
            timeout,
        ) == 0
        {
            return 0;
        }
        enter();
        write(core::ptr::addr_of_mut!((*c).owner), read(CURRENT_TCB));
        write(core::ptr::addr_of_mut!((*c).depth), 1);
        exit();
        1
    }
}
pub unsafe fn give(tagged: u32, item: u32, timeout: u32, position: u32) -> u32 {
    unsafe {
        if tagged & 1 == 0 {
            return call4(X_QUEUE_GENERIC_SEND, tagged, item, timeout, position);
        }
        let c = (tagged & !1) as *mut Context;
        enter();
        let depth = read(core::ptr::addr_of!((*c).depth));
        if depth == 0 || read(core::ptr::addr_of!((*c).owner)) != read(CURRENT_TCB) {
            exit();
            return 0;
        }
        write(core::ptr::addr_of_mut!((*c).depth), depth - 1);
        if depth > 1 {
            exit();
            return 1;
        }
        write(core::ptr::addr_of_mut!((*c).owner), 0);
        exit();
        call4(
            X_QUEUE_GENERIC_SEND,
            read(core::ptr::addr_of!((*c).semaphore)),
            0,
            0,
            0,
        )
    }
}
pub struct Guard(u32);
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            give(self.0, 0, 0, 0);
        }
    }
}
pub unsafe fn guard() -> Option<Guard> {
    unsafe {
        let tagged = read(SLOT);
        if tagged & 1 == 0 {
            None
        } else if take(tagged, u32::MAX) != 0 {
            Some(Guard(tagged))
        } else {
            None
        }
    }
}
pub unsafe fn publish(mode: u32, raw: u32) {
    unsafe {
        let c = context();
        if c.is_null() {
            return;
        }
        enter();
        if read(core::ptr::addr_of!((*c).blocked)) == 0 {
            write(core::ptr::addr_of_mut!((*c).mode), mode);
            write(core::ptr::addr_of_mut!((*c).raw), raw);
            write(
                core::ptr::addr_of_mut!((*c).event_generation),
                read(core::ptr::addr_of!((*c).generation)),
            );
            write(core::ptr::addr_of_mut!((*c).pending), 1);
        }
        exit();
    }
}
pub unsafe fn consume() -> Option<(u32, u32)> {
    unsafe {
        let c = context();
        if c.is_null() {
            return None;
        }
        enter();
        let event = if read(core::ptr::addr_of!((*c).pending)) != 0
            && read(core::ptr::addr_of!((*c).blocked)) == 0
            && read(core::ptr::addr_of!((*c).generation))
                == read(core::ptr::addr_of!((*c).event_generation))
        {
            Some((
                read(core::ptr::addr_of!((*c).mode)),
                read(core::ptr::addr_of!((*c).raw)),
            ))
        } else {
            None
        };
        write(core::ptr::addr_of_mut!((*c).pending), 0);
        exit();
        event
    }
}
pub unsafe fn cancel() {
    unsafe {
        let c = context();
        if c.is_null() {
            return;
        }
        enter();
        write(core::ptr::addr_of_mut!((*c).pending), 0);
        let generation = read(core::ptr::addr_of!((*c).generation));
        write(
            core::ptr::addr_of_mut!((*c).generation),
            generation.wrapping_add(1),
        );
        exit();
    }
}
pub unsafe fn block(value: u32) {
    unsafe {
        let c = context();
        if c.is_null() {
            return;
        }
        enter();
        write(core::ptr::addr_of_mut!((*c).blocked), value);
        exit();
    }
}
