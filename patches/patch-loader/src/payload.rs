#![no_std]
#![no_main]
//! Experimental Patch loader for L6 and L6max 1.10.
#[path = "../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
use core::{arch::asm, ptr, slice};
use patch_api::{function_call_post_hook, function_offset_hook};
#[allow(dead_code, unused_attributes)]
#[path = "../../../tools/patch-loader-core/src/lib.rs"]
mod core_loader;

#[cfg(not(sd_l6))]
const IMAGE_BASE: usize = 0x8000_0000;
#[cfg(sd_l6)]
const IMAGE_BASE: usize = 0x8000_1000;
#[cfg(not(sd_l6))]
const IMAGE_LEN: usize = 0x107fe8;
#[cfg(sd_l6)]
const IMAGE_LEN: usize = 0xb5ae4;
#[cfg(not(sd_l6))]
const REGISTRY_POINTER: usize = core_loader::registry::L6MAX_POINTER as usize;
#[cfg(sd_l6)]
const REGISTRY_POINTER: usize = core_loader::registry::L6_POINTER as usize;
// Cap a single code + zero-filled state allocation at 384 KiB. Larger modules
// are rejected before allocating from the shared FreeRTOS heap.
const MODULE_CAPACITY: usize = 0x60000;
#[cfg(sd_qemu_module_arena)]
const MODULE_ARENA_BASE: usize = 0x83e0_0000;
#[cfg(sd_qemu_module_arena)]
const MODULE_ARENA_END: usize = MODULE_ARENA_BASE + 0x80000;
const MAX_FUNCTION_CHAINS: usize = core_loader::dynamic::MAX_FUNCTION_CHAINS;
const MAX_RELOCATIONS: u32 = 256;

/// The heap-allocated registry, or zero before the first card mount.
fn registry_base() -> usize {
    unsafe { ptr::read_volatile(REGISTRY_POINTER as *const u32) as usize }
}
fn registry_word(offset: u32) -> usize {
    registry_base() + offset as usize
}
fn debug_word() -> usize {
    registry_word(core_loader::registry::DEBUG_OFFSET)
}
fn claim_word() -> usize {
    registry_word(core_loader::registry::CLAIM_OFFSET)
}
fn module_ptr_word() -> usize {
    registry_word(core_loader::registry::MODULE_PTR_OFFSET)
}
fn module_len_word() -> usize {
    registry_word(core_loader::registry::MODULE_LEN_OFFSET)
}
fn hook_chain_table() -> usize {
    registry_word(core_loader::registry::HOOK_CHAINS_OFFSET)
}
fn dynamic_handler_table() -> usize {
    registry_word(core_loader::registry::HANDLER_TABLE_OFFSET)
}
#[cfg(sd_qemu_module_arena)]
fn module_arena_next_word() -> usize {
    registry_word(core_loader::registry::ARENA_NEXT_OFFSET)
}
#[cfg(not(sd_l6))]
const SHA: [u8; 32] = [
    0x98, 0x0d, 0x92, 0x27, 0x2c, 0x1f, 0x84, 0xbe, 0xd2, 0x00, 0xb4, 0xd2, 0xf8, 0x35, 0x75, 0x20,
    0x2f, 0x6a, 0x00, 0x08, 0x2c, 0x92, 0x8d, 0x99, 0xb9, 0xe8, 0xb5, 0x3a, 0xf8, 0xd4, 0x44, 0x61,
];
#[cfg(sd_l6)]
const SHA: [u8; 32] = [
    0x82, 0x7c, 0x7c, 0x5c, 0xb2, 0xa4, 0x3c, 0xc3, 0xa0, 0x11, 0xd4, 0x55, 0x64, 0x62, 0xb8, 0xcb,
    0x95, 0x39, 0x24, 0x2f, 0x81, 0x7e, 0xbe, 0xed, 0x77, 0x27, 0xd6, 0x6a, 0x36, 0xc0, 0x37, 0x99,
];
#[cfg(not(sd_l6))]
#[path = "../../api/l6max/mod.rs"]
mod firmware_bindings;
#[cfg(sd_l6)]
#[path = "../../api/l6/mod.rs"]
mod firmware_bindings;
use firmware_bindings::*;
#[cfg(not(sd_l6))]
#[path = "../../api/l6max/hooks.rs"]
mod hook_catalog;
#[cfg(sd_l6)]
#[path = "../../api/l6/hooks.rs"]
mod hook_catalog;
const PATCH_PATTERN: [u16; 17] = [
    65, 58, 92, 80, 65, 84, 67, 72, 69, 83, 92, 42, 46, 66, 73, 78, 0,
];
const PATCH_DIRECTORY: [u16; 11] = [65, 58, 92, 80, 65, 84, 67, 72, 69, 83, 92];
const SEARCH_INFO_LEN: usize = 0x46c;
const SEARCH_NAME_OFFSET: usize = 0x12;
const SEARCH_ALT_NAME_OFFSET: usize = 0x2c;
const SEARCH_NAME_CAPACITY: usize = 256;
const MAX_PATCH_FILES: usize = 64;

#[cfg(not(sd_l6))]
const fn midi_hook_offset() -> usize {
    0x3f7c8
}
#[cfg(sd_l6)]
const fn midi_hook_offset() -> usize {
    0x2e490
}

/// Allocate the registry from the FreeRTOS heap on the first card mount and
/// publish it. Mount callers can race here, so the address is published only if
/// no other caller published one first; the loser frees its allocation.
unsafe fn ensure_registry() -> bool {
    let current = registry_base();
    if current != 0 {
        return unsafe { ptr::read_volatile(current as *const u32) } == core_loader::registry::MAGIC;
    }
    let len = core_loader::registry::LEN as usize;
    let fresh = unsafe { pvPortMalloc(len) };
    if fresh.is_null() {
        return false;
    }
    let base = fresh as usize;
    unsafe {
        ptr::write_bytes(fresh, 0, len);
        #[cfg(sd_qemu_module_arena)]
        ptr::write_volatile(
            (base + core_loader::registry::ARENA_NEXT_OFFSET as usize) as *mut u32,
            MODULE_ARENA_BASE as u32,
        );
        ptr::write_volatile(base as *mut u32, core_loader::registry::MAGIC);
        let primask: u32;
        asm!("dmb", "mrs {0}, PRIMASK", "cpsid i", out(reg) primask, options(nostack, preserves_flags));
        let published = ptr::read_volatile(REGISTRY_POINTER as *const u32) == 0;
        if published {
            ptr::write_volatile(REGISTRY_POINTER as *mut u32, base as u32);
        }
        asm!("dmb", "msr PRIMASK, {0}", in(reg) primask, options(nostack, preserves_flags));
        if !published {
            vPortFree(fresh);
        }
    }
    true
}

/// Called from the early MAIN startup function on every reset. The firmware
/// rebuilds its FreeRTOS heap during reset initialization, and external RAM can
/// retain the old pointer across a warm reset, so forget the registry before
/// any dispatch stub or ordinary feature hook can run.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.sd_reset_registry")]
pub unsafe extern "C" fn sd_reset_registry() {
    unsafe {
        ptr::write_volatile(REGISTRY_POINTER as *mut u32, 0);
    }
}

// Explicit function-pointer conversions avoid loading data from the firmware address.
unsafe fn mount(drive: u32) -> i32 {
    unsafe { fs_mount(drive) }
}
unsafe fn ready() -> u32 {
    unsafe { fs_ready() }
}
unsafe fn open(handle: *mut u32, path: *const u16) -> i32 {
    unsafe { fs_open(handle, path, 0, 0x100) }
}
unsafe fn find_first(path: *const u16, attributes: u32, info: *mut u8) -> i32 {
    unsafe { fs_find_first(path, attributes, info) }
}
unsafe fn find_next(info: *mut u8) -> i32 {
    unsafe { fs_find_next(info) }
}
unsafe fn find_close(info: *mut u8) -> i32 {
    unsafe { fs_find_close(info) }
}
unsafe fn read(handle: u32, dest: *mut u8, count: u32, actual: *mut u32) -> i32 {
    unsafe { fs_read(handle, dest, count, actual) }
}
unsafe fn close(handle: u32) {
    unsafe {
        let _ = fs_close(handle);
    }
}
unsafe fn alloc(size: usize) -> *mut u8 {
    #[cfg(sd_qemu_module_arena)]
    {
        let next = unsafe { ptr::read_volatile(module_arena_next_word() as *const u32) as usize };
        let Some(start) = next.checked_add(7).map(|address| address & !7) else {
            return ptr::null_mut();
        };
        let Some(end) = start.checked_add(size) else {
            return ptr::null_mut();
        };
        if start < MODULE_ARENA_BASE || end > MODULE_ARENA_END {
            return ptr::null_mut();
        }
        unsafe { ptr::write_volatile(module_arena_next_word() as *mut u32, end as u32) };
        start as *mut u8
    }
    #[cfg(not(sd_qemu_module_arena))]
    {
        unsafe { pvPortMalloc(size) }
    }
}
unsafe fn free(p: *mut u8) {
    #[cfg(sd_qemu_module_arena)]
    {
        let address = p as usize;
        let current = unsafe { ptr::read_volatile(module_arena_next_word() as *const u32) as usize };
        if (MODULE_ARENA_BASE..=current).contains(&address) {
            unsafe { ptr::write_volatile(module_arena_next_word() as *mut u32, address as u32) };
        }
    }
    #[cfg(not(sd_qemu_module_arena))]
    unsafe {
        vPortFree(p);
    }
}

#[repr(align(4))]
struct SearchInfo([u8; SEARCH_INFO_LEN]);

#[derive(Clone, Copy)]
enum Filename<'a> {
    Search(&'a SearchInfo, usize, usize),
    Bytes(&'a [u8]),
}

impl Filename<'_> {
    fn len(self) -> usize {
        match self {
            Self::Search(_, _, len) => len,
            Self::Bytes(bytes) => bytes.len(),
        }
    }
    fn byte(self, index: usize) -> u8 {
        match self {
            Self::Search(info, offset, _) => {
                u16::from_le_bytes([info.0[offset + index * 2], info.0[offset + index * 2 + 1]])
                    as u8
            }
            Self::Bytes(bytes) => bytes[index],
        }
    }
}

impl core_loader::dynamic::PatchFilename for Filename<'_> {
    fn name_len(&self) -> usize {
        self.len()
    }
    fn name_byte(&self, index: usize) -> u8 {
        self.byte(index)
    }
}

/// Read the search record's basename as a view. The firmware returns either a
/// basename or a full `A:\\PATCHES\\` path, depending on model/API wrapper.
fn search_filename(info: &SearchInfo) -> Option<Filename<'_>> {
    let read_name = |offset: usize| u16::from_le_bytes([info.0[offset], info.0[offset + 1]]);
    let name_offset = if read_name(SEARCH_ALT_NAME_OFFSET) != 0 {
        SEARCH_ALT_NAME_OFFSET
    } else {
        SEARCH_NAME_OFFSET
    };
    let mut full_len = 0;
    while full_len < SEARCH_NAME_CAPACITY && read_name(name_offset + full_len * 2) != 0 {
        full_len += 1;
    }
    if full_len == 0 || full_len == SEARCH_NAME_CAPACITY {
        return None;
    }
    let prefix_len = PATCH_DIRECTORY.len();
    let has_prefix = full_len >= prefix_len
        && (0..prefix_len).all(|index| {
            let value = read_name(name_offset + index * 2);
            value <= 0x7f
                && (value as u8).to_ascii_lowercase()
                    == (PATCH_DIRECTORY[index] as u8).to_ascii_lowercase()
        });
    let start = if has_prefix { prefix_len } else { 0 };
    let len = full_len - start;
    if len < 4
        || (0..len).any(|index| {
            let value = read_name(name_offset + (start + index) * 2);
            value > 0x7f || value == b'/' as u16 || value == b'\\' as u16
        })
    {
        return None;
    }
    let extension = start + len - 4;
    if read_name(name_offset + extension * 2) != b'.' as u16
        || (read_name(name_offset + (extension + 1) * 2) as u8).to_ascii_lowercase() != b'b'
        || (read_name(name_offset + (extension + 2) * 2) as u8).to_ascii_lowercase() != b'i'
        || (read_name(name_offset + (extension + 3) * 2) as u8).to_ascii_lowercase() != b'n'
    {
        return None;
    }
    Some(Filename::Search(info, name_offset + start * 2, len))
}

fn copy_filename(source: Filename<'_>, destination: &mut [u8; SEARCH_NAME_CAPACITY]) -> usize {
    for index in 0..source.len() {
        destination[index] = source.byte(index);
    }
    source.len()
}

fn make_patch_path(
    filename: &[u8],
    output: &mut [u16; SEARCH_NAME_CAPACITY + 12],
) -> Option<usize> {
    let prefix_len = PATCH_DIRECTORY.len();
    let len = prefix_len.checked_add(filename.len())?;
    if len >= output.len() {
        return None;
    }
    output[..prefix_len].copy_from_slice(&PATCH_DIRECTORY);
    for (index, byte) in filename.iter().enumerate() {
        output[prefix_len + index] = u16::from(*byte);
    }
    output[len] = 0;
    Some(len)
}

/// Find the least filename greater than `after` without retaining a directory
/// worth of long names on the task stack. Repeated scans trade directory I/O
/// for a small fixed scratch footprint and deterministic ordering.
unsafe fn find_next_patch(
    after: &[u8],
    selected: &mut [u8; SEARCH_NAME_CAPACITY],
) -> Result<Option<usize>, ()> {
    let mut info = SearchInfo([0; SEARCH_INFO_LEN]);
    if unsafe { find_first(PATCH_PATTERN.as_ptr(), 0x31, info.0.as_mut_ptr()) } != 0 {
        unsafe { find_close(info.0.as_mut_ptr()) };
        return Ok(None);
    }
    let mut count = 0;
    let mut selected_len = 0;
    loop {
        if count == MAX_PATCH_FILES {
            unsafe { find_close(info.0.as_mut_ptr()) };
            return Err(());
        }
        count += 1;
        if let Some(candidate) = search_filename(&info) {
            let after_order = if after.is_empty() {
                core::cmp::Ordering::Greater
            } else {
                core_loader::dynamic::compare_patch_filenames(&candidate, after)
            };
            let best_order = if selected_len == 0 {
                core::cmp::Ordering::Less
            } else {
                core_loader::dynamic::compare_patch_filenames(&candidate, &selected[..selected_len])
            };
            if after_order == core::cmp::Ordering::Greater
                && best_order == core::cmp::Ordering::Less
            {
                selected_len = copy_filename(candidate, selected);
            }
        }
        if unsafe { find_next(info.0.as_mut_ptr()) } != 0 {
            break;
        }
    }
    unsafe { find_close(info.0.as_mut_ptr()) };
    Ok((selected_len != 0).then_some(selected_len))
}

struct FirmwareReader(u32);
impl core_loader::bundle::Reader for FirmwareReader {
    type Error = ();
    fn read(&mut self, destination: &mut [u8]) -> Result<usize, ()> {
        let mut actual = 0;
        if unsafe {
            read(
                self.0,
                destination.as_mut_ptr(),
                destination.len() as u32,
                &mut actual,
            )
        } != 0
        {
            return Err(());
        }
        Ok(actual as usize)
    }

    fn finish(&mut self) -> Result<(), core_loader::bundle::ReadError> {
        let mut byte = 0;
        let mut actual = 0;
        let _status = unsafe { read(self.0, &mut byte, 1, &mut actual) };
        if actual != 0 {
            Err(core_loader::bundle::ReadError::Invalid)
        } else {
            // FatFs reports EOF as an error status with an unchanged byte
            // count. The declared file body and CRC have already been read.
            Ok(())
        }
    }
}

unsafe fn clean_cache(start: usize, len: usize) {
    let mut line = start & !31;
    let end = (start + len + 31) & !31;
    while line < end {
        unsafe { ptr::write_volatile(0xe000_ef68 as *mut u32, line as u32) };
        line += 32;
    }
}

unsafe fn synchronize_instructions() {
    unsafe {
        asm!("dsb sy", options(nostack, preserves_flags));
        ptr::write_volatile(0xe000_ef50 as *mut u32, 0); // ICIALLU
        asm!("dsb sy", "isb sy", options(nostack, preserves_flags));
    }
}

unsafe fn clear_payload(base: usize, len: usize) {
    unsafe {
        ptr::write_bytes(base as *mut u8, 0xff, len);
        clean_cache(base, len);
        synchronize_instructions();
    }
}

unsafe fn find_hook_chain(
    address: u32,
    reserved: &[*mut core_loader::dynamic::FunctionHookChain],
) -> Option<*mut core_loader::dynamic::FunctionHookChain> {
    let mut free = None;
    for index in 0..MAX_FUNCTION_CHAINS {
        let chain = (hook_chain_table()
            + index * core::mem::size_of::<core_loader::dynamic::FunctionHookChain>())
            as *mut core_loader::dynamic::FunctionHookChain;
        let value = unsafe { ptr::read_volatile(chain) };
        if value.count != 0 && value.address == address {
            return Some(chain);
        }
        if free.is_none() && value.count == 0 && !reserved.contains(&chain) {
            free = Some(chain);
        }
    }
    free
}

/// Load one compact version-1 module directly into its final heap allocation. Hook
/// registrations stay local until the streaming CRC, relocations, and every
/// site/capacity check pass. The complete module's hook set is published under
/// one interrupt-masked registry update.
unsafe fn load_dynamic<R: core_loader::bundle::Reader>(reader: &mut R, order: u32) -> bool {
    const MAX_HOOKS: usize = core_loader::dynamic::MAX_HOOKS_PER_MODULE;
    let pending = match core_loader::dynamic::read_header(
        reader,
        MODULE_CAPACITY,
        MAX_HOOKS,
        MAX_RELOCATIONS as usize,
    ) {
        Ok(header) => header,
        Err(_) => {
            return false;
        }
    };
    // This first format revision supports stateless handlers. A later loader
    // ABI can add an initializer protocol without changing function hooks.
    if pending.header.init_offset != 0 {
        return false;
    }
    let module = unsafe { alloc(pending.header.memory_len) };
    if module.is_null() || module as usize > u32::MAX as usize {
        if !module.is_null() {
            unsafe { free(module) };
        }
        return false;
    }
    let loaded_base = module as usize;
    let mut hooks = [core_loader::dynamic::Hook::default(); MAX_HOOKS];
    let code = unsafe { slice::from_raw_parts_mut(module, pending.header.memory_len) };
    let header = match core_loader::dynamic::read_body(
        reader,
        pending,
        code,
        &mut hooks,
        0,
        loaded_base as u32,
    ) {
        Ok(header) => header,
        Err(error) => {
            let error_code = match error {
                core_loader::dynamic::Error::Read(core_loader::bundle::ReadError::Io) => 1,
                core_loader::dynamic::Error::Read(core_loader::bundle::ReadError::Truncated) => 2,
                core_loader::dynamic::Error::Read(core_loader::bundle::ReadError::Invalid) => 3,
                core_loader::dynamic::Error::Read(core_loader::bundle::ReadError::TooLarge) => 4,
                core_loader::dynamic::Error::Invalid => 5,
                core_loader::dynamic::Error::TooLarge => 6,
                core_loader::dynamic::Error::Relocation => 7,
                core_loader::dynamic::Error::Checksum => 8,
            };
            unsafe {
                clear_payload(loaded_base, code.len());
                free(module);
            }
            return false;
        }
    };
    #[derive(Clone, Copy)]
    struct HookPlan {
        address: u32,
        entry: u32,
        next_slot: u32,
        chain: *mut core_loader::dynamic::FunctionHookChain,
        expected_count: u32,
        dispatch_index: usize,
    }

    let mut plans = [HookPlan {
        address: 0,
        entry: 0,
        next_slot: 0,
        chain: ptr::null_mut(),
        expected_count: 0,
        dispatch_index: 0,
    }; MAX_HOOKS];
    let mut reserved_chains = [ptr::null_mut(); MAX_HOOKS];
    let mut planned = 0usize;
    for hook in hooks.iter().take(header.hook_count).copied() {
        if hook.next_slot_offset == u32::MAX {
            unsafe {
                clear_payload(loaded_base, header.memory_len);
                free(module);
            }
            return false;
        }
        let Some(dispatch_index) = hook_catalog::FUNCTION_HOOK_SITES
            .iter()
            .filter(|site| site.dynamic_dispatch)
            .position(|site| site.address == hook.address)
        else {
            unsafe {
                clear_payload(loaded_base, header.memory_len);
                free(module);
            }
            return false;
        };
        // Several handlers in one module may target the same function. Reuse
        // their reserved chain row instead of reserving an empty row per hook.
        let chain = plans[..planned]
            .iter()
            .find(|plan| plan.address == hook.address)
            .map(|plan| plan.chain)
            .or_else(|| unsafe { find_hook_chain(hook.address, &reserved_chains[..planned]) });
        let Some(chain) = chain else {
            unsafe {
                clear_payload(loaded_base, header.memory_len);
                free(module);
            }
            return false;
        };
        let mut chain_value = unsafe { ptr::read_volatile(chain) };
        let expected_count = chain_value.count;
        let entry = loaded_base as u32 + hook.entry_offset | 1;
        let next_slot = loaded_base as u32 + hook.next_slot_offset;
        let previous_slot =
            chain_value.append(hook.address, entry, next_slot, order, loaded_base as u32);
        if previous_slot.is_err() {
            unsafe {
                clear_payload(loaded_base, header.memory_len);
                free(module);
            }
            return false;
        }
        plans[planned] = HookPlan {
            address: hook.address,
            entry,
            next_slot,
            chain,
            expected_count,
            dispatch_index,
        };
        reserved_chains[planned] = chain;
        planned += 1;
    }
    if planned != header.hook_count {
        unsafe {
            clear_payload(loaded_base, header.memory_len);
            free(module);
        }
        return false;
    }
    unsafe {
        clean_cache(loaded_base, header.memory_len);
        synchronize_instructions();
        let primask: u32;
        asm!("mrs {0}, PRIMASK", "cpsid i", out(reg) primask, options(nostack, preserves_flags));
        // Verify every registry row before changing any row or handler head.
        // `claim()` excludes competing loader transactions; this catches
        // corrupt/stale registry state without exposing a partially installed
        // multi-site module.
        for plan in &plans[..planned] {
            let current_chain = ptr::read_volatile(plan.chain);
            let slot = (dynamic_handler_table() + plan.dispatch_index * 4) as *mut u32;
            let current_head = ptr::read_volatile(slot);
            if (current_chain.address != plan.address
                && !(current_chain.count == 0 && current_chain.address == 0))
                || current_chain.count != plan.expected_count
                || (current_chain.count == 0 && current_head != 0)
                || (current_chain.count != 0 && current_head != current_chain.links[0].entry)
            {
                asm!("msr PRIMASK, {0}", in(reg) primask, options(nostack, preserves_flags));
                clear_payload(loaded_base, header.memory_len);
                free(module);
                return false;
            }
        }
        // Initialize each module continuation before exposing any handler.
        for plan in &plans[..planned] {
            let site_name = hook_catalog::FUNCTION_HOOK_SITES
                .iter()
                .filter(|site| site.dynamic_dispatch)
                .nth(plan.dispatch_index)
                .unwrap()
                .name;
            let continuation = (crate::patch_api::__continuations::address(site_name) as u32) | 1;
            ptr::write_volatile(plan.next_slot as *mut u32, continuation);
        }
        for plan in &plans[..planned] {
            let slot = (dynamic_handler_table() + plan.dispatch_index * 4) as *mut u32;
            let mut updated_chain = ptr::read_volatile(plan.chain);
            let previous_slot = updated_chain
                .append(
                    plan.address,
                    plan.entry,
                    plan.next_slot,
                    order,
                    loaded_base as u32,
                )
                .unwrap();
            ptr::write_volatile(plan.chain, updated_chain);
            if let Some(previous_slot) = previous_slot {
                ptr::write_volatile(previous_slot as *mut u32, plan.entry);
            } else {
                ptr::write_volatile(slot, plan.entry);
            }
        }
        ptr::write_volatile(module_ptr_word() as *mut u32, loaded_base as u32);
        ptr::write_volatile(module_len_word() as *mut u32, header.memory_len as u32);
        ptr::write_volatile(debug_word() as *mut u32, 0x327);
        asm!("msr PRIMASK, {0}", in(reg) primask, options(nostack, preserves_flags));
    }
    true
}

/// Enumerate the patch directory, loading each supported BIN independently.
/// Invalid or unsupported files are skipped so another candidate can load.
unsafe fn load_patch_files() -> bool {
    let mut loaded_any = false;
    let mut last_name = [0; SEARCH_NAME_CAPACITY];
    let mut last_len = 0;
    let mut path = [0u16; SEARCH_NAME_CAPACITY + 12];
    for order in 0..MAX_PATCH_FILES {
        let mut selected = [0; SEARCH_NAME_CAPACITY];
        let selected_len = match unsafe { find_next_patch(&last_name[..last_len], &mut selected) } {
            Ok(Some(len)) => len,
            Ok(None) => break,
            Err(()) => break,
        };
        if make_patch_path(&selected[..selected_len], &mut path).is_some() {
            let mut handle = 0;
            if unsafe { open(&mut handle, path.as_ptr()) } == 0 {
                let loaded = unsafe { load_dynamic(&mut FirmwareReader(handle), order as u32) };
                unsafe { close(handle) };
                loaded_any |= loaded;
            }
        }
        last_name[..selected_len].copy_from_slice(&selected[..selected_len]);
        last_len = selected_len;
    }
    loaded_any
}

/// Serialize attempts from different mount callers. The final word belongs to
/// the loader, not the module payload; the installed image initializes it to ff.
unsafe fn claim() -> bool {
    let primask: u32;
    unsafe {
        asm!("mrs {0}, PRIMASK", "cpsid i", out(reg) primask, options(nostack, preserves_flags));
    }
    let flag = claim_word() as *mut u32;
    let state = unsafe { ptr::read_volatile(flag) };
    let free = state == 0 || state == u32::MAX;
    if free {
        unsafe {
            // Reserve the loader while one file is being streamed. Without a
            // distinct in-progress state, parallel mount callers can both
            // allocate and mutate the same hook registry.
            ptr::write_volatile(flag, 2);
        }
    }
    unsafe {
        asm!("msr PRIMASK, {0}", in(reg) primask, options(nostack, preserves_flags));
    }
    free
}
unsafe fn finish(success: bool) {
    unsafe {
        ptr::write_volatile(claim_word() as *mut u32, if success { 1 } else { 0 });
    }
}

/// Runs after a startup mount call; the call-hook dispatcher preserves the
/// firmware mount result and selects only the cataloged call sites below.
unsafe fn after_sd_mount(drive: u32) {
    if !unsafe { ensure_registry() } {
        return;
    }
    if unsafe { ptr::read_volatile(claim_word() as *const u32) } == 1 {
        return;
    }
    unsafe { ptr::write_volatile(debug_word() as *mut u32, 1) };
    unsafe { ptr::write_volatile(debug_word() as *mut u32, 2) };
    if drive != 0x41 {
        unsafe { ptr::write_volatile(debug_word() as *mut u32, 3) };
        return;
    }
    let is_ready = unsafe { ready() };
    unsafe { ptr::write_volatile(debug_word() as *mut u32, 0x100 | (is_ready & 0xff)) };
    if is_ready == 0 {
        return;
    }
    if !unsafe { claim() } {
        unsafe { ptr::write_volatile(debug_word() as *mut u32, 4) };
        return;
    }
    unsafe { ptr::write_volatile(debug_word() as *mut u32, 5) };
    let success = unsafe { load_patch_files() };
    unsafe {
        if success {
            ptr::write_volatile(debug_word() as *mut u32, 0x500);
        } else {
            ptr::write_volatile(debug_word() as *mut u32, 0x400);
        }
    }
    unsafe {
        finish(success);
    }
}

#[cfg(sd_l6)]
#[function_call_post_hook(fs_mount, at = 0x8000_9a60)]
unsafe fn sd_mount_post_l6_recorder(drive: u32) {
    unsafe { after_sd_mount(drive) };
}

#[cfg(sd_l6)]
#[function_call_post_hook(fs_mount, at = 0x8000_9aa0)]
unsafe fn sd_mount_post_l6_settings(drive: u32) {
    unsafe { after_sd_mount(drive) };
}

#[cfg(not(sd_l6))]
#[function_call_post_hook(fs_mount, at = 0x8000_e9fa)]
unsafe fn sd_mount_post_l6max_recorder_a(drive: u32) {
    unsafe { after_sd_mount(drive) };
}

#[cfg(not(sd_l6))]
#[function_call_post_hook(fs_mount, at = 0x8000_ea24)]
unsafe fn sd_mount_post_l6max_recorder_b(drive: u32) {
    unsafe { after_sd_mount(drive) };
}

#[cfg(not(sd_l6))]
#[function_call_post_hook(fs_mount, at = 0x8001_0118)]
unsafe fn sd_mount_post_l6max_startup(drive: u32) {
    unsafe { after_sd_mount(drive) };
}

#[cfg(not(sd_l6))]
#[function_call_post_hook(fs_mount, at = 0x8002_bc7e)]
unsafe fn sd_mount_post_l6max_patch(drive: u32) {
    unsafe { after_sd_mount(drive) };
}

#[cfg(not(sd_l6))]
#[function_call_post_hook(fs_mount, at = 0x8000_ebac)]
unsafe fn sd_mount_post_l6max_factory(drive: u32) {
    unsafe { after_sd_mount(drive) };
}

#[cfg(sd_l6)]
#[function_offset_hook(
    sys_l6_initialize_and_start_scheduler,
    at = 0x24,
    displaced_len = 4,
    guard_sha256 = "0360cab6934417a2ec4e6d8e81a452db698d5fc5f0965ae39b13d02391dc2132"
)]
unsafe fn sd_reset_hook() {
    unsafe { sd_reset_registry() };
    unsafe { vTaskStartScheduler() };
    hook_done()
}

#[cfg(not(sd_l6))]
#[function_offset_hook(
    sys_main_startup_initialize,
    at = 0,
    displaced_len = 4,
    guard_sha256 = "07f4993f36ec2892ba7daa618956f411293bf2fc693e4a3a2aebb9abd0c2ea04"
)]
unsafe fn sd_reset_hook() {
    unsafe { sd_reset_registry() };
    hook_continue()
}
