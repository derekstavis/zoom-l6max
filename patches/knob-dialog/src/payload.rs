#![no_std]
#![no_main]
use core::{
    mem::transmute,
    ptr::{read_volatile as read, write_volatile as write},
};

mod firmware;
mod values;
use firmware::*;

mod notification_hooks;
mod synchronization;
mod trampolines;
use trampolines as original;

#[repr(C)]
struct Bitmap {
    width: u16,
    height: u16,
    pixels: u32,
}
#[repr(C)]
struct State {
    // First field: the existing bitmap resource table owns the live pointer.
    bitmap: Bitmap,
    magic: u32,
    active: u32,
    pointers: [u32; 3],
    lengths: [u16; 3],
    header_y: u16,
    header_polarity: u16,
    header_font: u16,
    value_y: u16,
    original_bitmap: u32,
    pixels: [u16; 798],
    lines: [[u16; 24]; 3],
}
const MAGIC: u32 = 0x4b4e4f42;
unsafe fn state() -> *mut State {
    unsafe {
        let pointer = read(0x80207458 as *const u32);
        // The original allocator's pool, not an emulator-reserved address.
        if pointer & 7 != 0
            || pointer < 0x808a4150
            || pointer > 0x80921140 - core::mem::size_of::<State>() as u32
        {
            return core::ptr::null_mut();
        }
        let s = pointer as *mut State;
        if read(core::ptr::addr_of!((*s).magic)) == MAGIC {
            s
        } else {
            core::ptr::null_mut()
        }
    }
}

unsafe fn current_message() -> u16 {
    unsafe {
        let slot = read(0x8020a2c2 as *const u8) as usize;
        read((0x8020a2a4 + 2 * slot) as *const u16)
    }
}
unsafe fn restore() {
    unsafe {
        let pointer = state();
        if pointer.is_null() {
            return;
        }
        let s = &mut *pointer;
        // Stock pop clears text using the restored widget geometry. Our title
        // uses a different Y/font, so remove its ink AND opacity first, while
        // the custom allocation and layout still belong to the notification.
        clear_layer_rect(6, 9, 8, 119, 58);
        let lengths = read(0x802050fc as *const u32) as *mut u16;
        for i in 0..3 {
            write((0x802059b8 + 4 * i) as *mut u32, s.pointers[i]);
            write(lengths.add(229 + i), s.lengths[i]);
        }
        call2(GUI_SET_WIDGET_Y, 58, s.header_y as u32);
        call2(GUI_SET_WIDGET_POLARITY, 58, s.header_polarity as u32);
        write(0x80200990 as *mut u16, s.header_font);
        call2(GUI_SET_WIDGET_Y, 59, s.value_y as u32);
        write(0x80207458 as *mut u32, s.original_bitmap);
        s.active = 0;
        s.magic = 0;
        call1(V_PORT_FREE, pointer as u32);
    }
}
unsafe fn redraw() {
    unsafe {
        // Retain the original frame and its opaque background. Command 1
        // clears one compositor layer in a rect
        // whose final coordinates are exclusive (not width/height).

        call1(GUI_DRAW_BACKGROUND, 45);
        clear_layer_rect(6, 9, 8, 119, 58);
        call2(GUI_SET_WIDGET_Y, 58, 9);
        call2(GUI_SET_WIDGET_POLARITY, 58, 1);
        // Font 0 is the original seven-pixel font used elsewhere in the GUI.
        write(0x80200990 as *mut u16, 0);
        call2(GUI_SET_WIDGET_Y, 59, 32);
        for widget in 58..=59 {
            call1(GUI_DRAW_TEXT_WIDGET, widget);
        }
        call1(GUI_PUBLISH, 0);
    }
}
fn text(out: &mut [u16; 24], bytes: &[u8]) {
    out.fill(0);
    for (a, b) in out.iter_mut().zip(bytes.iter().take(23)) {
        *a = *b as u16;
    }
}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.knob_changed")]
pub unsafe extern "C" fn knob_changed(
    channel: u32,
    mode: u32,
    absolute: u32,
    delta: i32,
    value: u32,
) {
    unsafe {
        transmute::<usize, unsafe extern "C" fn(u32, u32, u32, i32, u32)>(
            MIXER_APPLY_CHANNEL_PARAMETER | 1,
        )(channel, mode, absolute, delta, value);
        if channel >= 8 || mode >= 10 || delta == 0 {
            return;
        }
        // Only the normal recorder window: leave setup and modal dialogs alone.
        if read(0x80202610 as *const u32) != 0x80200408 {
            return;
        }
        let audio = call1(MIXER_CHANNEL_INDEX, channel);
        let raw = match mode {
            0 => call2(MIXER_GET_EQ_GAIN, audio, 0),
            1 => call2(MIXER_GET_EQ_FREQUENCY, audio, 1),
            2 => call2(MIXER_GET_EQ_GAIN, audio, 1),
            3 => call2(MIXER_GET_EQ_GAIN, audio, 2),
            4 | 5 => call2(MIXER_GET_AUX, audio, mode - 4),
            6 => call2(MIXER_GET_EFX, audio, 0),
            7 => call1(MIXER_GET_SUBMIX, audio),
            8 => call1(MIXER_GET_PAN, audio),
            _ => call1(MIXER_GET_LEVEL, audio),
        };
        if raw > 127 {
            return;
        }
        synchronization::publish(mode, raw);
    }
}

// Only the notification callback calls this, while holding the manager lock.
unsafe fn present(mode: u32, raw: u32) {
    unsafe {
        if read(0x80202610 as *const u32) != 0x80200408 || read(0x8020a2b0 as *const u8) != 0 {
            return;
        }
        let mut pointer = state();
        if read(0x8020a2c0 as *const u8) != 0 && (pointer.is_null() || current_message() != 15) {
            return;
        }
        let titles: [&[u8]; 10] = [
            b"HIGH", b"FREQ", b"MID", b"LOW", b"AUX1", b"AUX2", b"EFX", b"SUB-MIX", b"PAN",
            b"LEVEL",
        ];
        let mut title = [0u16; 24];
        text(&mut title, titles[mode as usize]);
        let gain = if matches!(mode, 0 | 2 | 3) {
            read((EQ_GAIN_DB_TABLE + 4 * raw) as *const f32)
        } else {
            0.0
        };
        let frequency = if mode == 1 {
            read((EQ_MID_FREQUENCY_HZ_TABLE + 4 * raw) as *const f32)
        } else {
            0.0
        };
        let value_text = values::format_value(mode, raw, gain, frequency);
        if pointer.is_null() {
            pointer = call1(PV_PORT_MALLOC, core::mem::size_of::<State>() as u32) as *mut State;
            if pointer.is_null() {
                return;
            } // Mixer adjustment already succeeded.
            core::ptr::write_bytes(pointer, 0, 1);
        }
        let s = &mut *pointer;
        let lengths = read(0x802050fc as *const u32) as *mut u16;
        if s.active == 0 {
            s.header_y = read(0x8020098c as *const u16);
            s.header_polarity = read(0x80200986 as *const u16);
            s.header_font = read(0x80200990 as *const u16);
            s.value_y = read(0x8020099c as *const u16);
            s.original_bitmap = read(0x80207458 as *const u32);
            let source = read((s.original_bitmap + 4) as *const u32) as *const u16;
            for i in 0..798 {
                s.pixels[i] = read(source.add(i));
            }
            // The bitmap uses page-major words: low byte pixels, high byte
            // opacity. Erase the baked heading, keeping the original white
            // header band, opacity, and frame borders.
            for y in 1..11 {
                for x in 1..113 {
                    s.pixels[(y / 8) * 114 + x] &= !(1u16 << (y % 8));
                }
            }
            s.bitmap.width = 114;
            s.bitmap.height = 52;
            s.bitmap.pixels = s.pixels.as_ptr() as u32;
            s.magic = MAGIC;
            write(0x80207458 as *mut u32, core::ptr::addr_of!(s.bitmap) as u32);
            for i in 0..3 {
                s.pointers[i] = read((0x802059b8 + 4 * i) as *const u32);
                s.lengths[i] = read(lengths.add(229 + i));
                write((0x802059b8 + 4 * i) as *mut u32, s.lines[i].as_ptr() as u32);
                // Metadata counts characters, excluding the terminator.
                write(lengths.add(229 + i), 23);
            }
            // show() draws synchronously. Supply the text before that first
            // draw, avoiding a published blank header followed by our redraw.
            s.lines[0] = title;
            s.lines[1] = value_text;
            s.active = 1;
            call1(original::notification_show(), 15);
        } else {
            // Same copy API used by Date/Time and filename widgets. Sources
            // are stack buffers; destinations belong to the heap allocation.
            call2(GUI_SET_WIDGET_TEXT, 58, title.as_ptr() as u32);
            call2(GUI_SET_WIDGET_TEXT, 59, value_text.as_ptr() as u32);
            redraw();
        }
        // Existing 100 ms notification timer owns expiry; restart its count.
        write(0x8020a2c4 as *mut u16, 0);
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
