/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Minimal L6max firmware execution boards for QEMU 11.1.2.
 *
 * Copyright (c) 2026. Licensed under GPL-2.0-or-later.
 * These are bring-up boards, not hardware-accurate SoC models.
 */

#include "qemu/osdep.h"
#include "qapi/error.h"
#include "hw/arm/armv7m.h"
#include "hw/arm/boot.h"
#include "hw/arm/machines-qom.h"
#include "hw/core/boards.h"
#include "hw/core/qdev-clock.h"
#include "hw/core/qdev-properties.h"
#include "hw/core/irq.h"
#include "hw/core/sysbus.h"
#include "hw/core/resettable.h"
#include "hw/core/loader.h"
#include "hw/sd/sd.h"
#include "hw/sd/sdhci.h"
#include "system/address-spaces.h"
#include "system/blockdev.h"
#include "qemu/log.h"
#include "qemu/module.h"
#include "qemu/timer.h"
#include "qemu/main-loop.h"
#include "chardev/char-fe.h"
#include "system/system.h"
#include "system/runstate.h"
#include "l6max-display.h"
#include "l6max-indicators.h"
#include "l6max-usb.h"
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/file.h>
#include <unistd.h>

#define L6_MAX_PANEL_KEYS 32
#define L6_BUTTON_EVENT_COUNT 54
#define L6_BUTTON_TAP_TICKS 50
#define L6_ENCODER_COUNT 8
#define L6_INPUT_MAGIC 0x4c364950u
#define L6_INPUT_MESSAGE_SIZE 24
#define L6_INPUT_QUEUE_LIMIT 64
#define L6_INPUT_TX_LIMIT (64 * 1024)
/* Bring-up debounce default, matching the verified minimum press duration. */
#define L6_INPUT_RELEASE_GAP_MS 50

typedef struct L6InputTap {
    uint32_t sequence;
    uint32_t duration_ms;
    bool automatic;
    bool release_requested;
    uint32_t release_sequence;
} L6InputTap;

typedef struct L6PanelKey {
    unsigned row;
    unsigned column;
    unsigned start_ms;
    unsigned end_ms;
    bool logged;
} L6PanelKey;

typedef struct L6MainKey {
    unsigned id; /* 0=Menu/FUNC1, 1=Play, 2=Record */
    unsigned start_ms;
    unsigned end_ms;
    bool logged;
} L6MainKey;

typedef struct L6ChipState {
    ARMv7MState armv7m;
    MemoryRegion flash;
    MemoryRegion flash_alias;
    MemoryRegion itcm;
    MemoryRegion sram;
    MemoryRegion dtcm;
    MemoryRegion external_ram;
    MemoryRegion peripheral;
    MemoryRegion sd_adapter;
    MemoryRegion usb_region;
    L6USB usb;
    SDHCIState *sdhc;
    const MemoryRegionOps *sd_ops;
    uint32_t sd_sysctl;
    bool sd_present;
    Clock *sysclk;
    Clock *refclk;
    GHashTable *registers;
    size_t flash_payload_size;
    uint8_t *nor_flash;
    uint8_t *persistent_panel;
    uint8_t *persistent_rtc;
    uint8_t *persistent_options;
    unsigned rtc_reads;
    int64_t rtc_anchor_ms;
    unsigned flexspi_log_count;
    /* IPCR1 carries a 16-bit transfer size; startup checks 32 KiB chunks. */
    uint8_t flexspi_rx[0x10000];
    unsigned flexspi_rx_size;
    unsigned flexspi_rx_cursor;
    bool nor_write_enable;
    bool nor_program;
    unsigned nor_program_address;
    unsigned nor_program_size;
    unsigned nor_program_cursor;
    unsigned lpspi_log_count;
    unsigned edma_log_count;
    uint32_t edma_erq;
    L6Display display;
    bool display_dc;
    bool panel;
    struct L6ChipState *peer;
    struct L6UartLink *uart_link;
    bool cpu_held;
    bool panel_reset_asserted;
    unsigned panel_tx_count;
    unsigned panel_rx_count;
    unsigned main_rx_count;
    unsigned main_ctrl_log_count;
    unsigned main_tx_count;
    unsigned main_gpio_log_count;
    bool main_boot_select;
    bool main_rom_mode;
    uint8_t rom_flash[0x10000];
    uint8_t rom_option[4];
    uint8_t rom_reply[1024];
    unsigned rom_reply_read;
    unsigned rom_reply_visible;
    unsigned rom_reply_write;
    uint8_t rom_command;
    uint8_t rom_input[260];
    unsigned rom_input_size;
    unsigned rom_phase;
    uint32_t rom_address;
    QEMUTimer *panel_tim2;
    QEMUTimer *panel_tim3;
    QEMUTimer *panel_tim14;
    QEMUTimer *panel_spi_dma;
    QEMUTimer *indicator_timer;
    L6Indicators indicators;
    unsigned indicator_log_count;
    QEMUTimer *rom_uart_timer;
    QEMUTimer *main_audio_service_timer;
    CharFrontend panel_uart;
    bool panel_rx_pending;
    uint8_t panel_rx_byte;
    CharFrontend main_uart;
    bool main_rx_pending;
    uint8_t main_rx_byte;
    L6PanelKey panel_keys[L6_MAX_PANEL_KEYS];
    unsigned panel_key_count;
    L6MainKey main_keys[L6_MAX_PANEL_KEYS];
    unsigned main_key_count;
    bool service_audio_queue;
    int input_fd;
    uint8_t input_rx[L6_INPUT_MESSAGE_SIZE];
    bool input_trace;
    int64_t input_trace_until;
    unsigned input_rx_size;
    GByteArray *input_tx;
    GQueue input_taps[L6_BUTTON_EVENT_COUNT];
    uint32_t main_gpio[5], main_gpio_generation;
    uint16_t analog_inputs[5], analog_published[5];
    uint32_t analog_generation, adc_channel;
    bool analog_seen[5];
    bool power_seen, power_published;
    bool input_button_down[L6_BUTTON_EVENT_COUNT];
    int64_t input_down_deadline[L6_BUTTON_EVENT_COUNT];
    bool input_release_requested[L6_BUTTON_EVENT_COUNT];
    uint32_t input_release_sequence[L6_BUTTON_EVENT_COUNT];
    int64_t input_next_press[L6_BUTTON_EVENT_COUNT];
    QEMUTimer *input_timer;
    uint32_t input_tap_sequence[L6_BUTTON_EVENT_COUNT];
    int64_t input_tap_deadline[L6_BUTTON_EVENT_COUNT];
    int32_t encoder_target[L6_ENCODER_COUNT];
    uint8_t encoder_phase[L6_ENCODER_COUNT];
    int32_t encoder_position[L6_ENCODER_COUNT];
    uint8_t encoder_sequence[L6_ENCODER_COUNT];
    uint8_t encoder_phase_wait[L6_ENCODER_COUNT];
    int8_t encoder_direction[L6_ENCODER_COUNT];
    bool encoder_active[L6_ENCODER_COUNT];
} L6ChipState;

typedef struct L6MachineState {
    MachineState parent;
    L6ChipState chip;
} L6MachineState;

typedef struct L6UartLink {
    L6ChipState *source;
    L6ChipState *destination;
    GByteArray *queue;
    QEMUTimer *timer;
} L6UartLink;

typedef struct L6DualMachineState {
    MachineState parent;
    L6ChipState main;
    L6ChipState panel;
    MemoryRegion main_memory;
    MemoryRegion panel_memory;
    MemoryRegion debug_alias;
    L6UartLink main_to_panel;
    L6UartLink panel_to_main;
} L6DualMachineState;

#define TYPE_L6_MACHINE MACHINE_TYPE_NAME("l6max-base")
OBJECT_DECLARE_SIMPLE_TYPE(L6MachineState, L6_MACHINE)
#define TYPE_L6_DUAL_MACHINE MACHINE_TYPE_NAME("l6max-dual")
OBJECT_DECLARE_SIMPLE_TYPE(L6DualMachineState, L6_DUAL_MACHINE)

/* Bring-up framing delay: 100 us per byte matches the existing ROM endpoint.
 * It is not yet calculated from the UART divisor registers. */
#define L6_UART_BYTE_NS 100000
#define L6_UART_QUEUE_LIMIT (64 * 1024)

static void l6_flexspi_rx_window(L6ChipState *s)
{
    unsigned remaining = s->flexspi_rx_size - s->flexspi_rx_cursor;
    g_hash_table_insert(s->registers,
        GUINT_TO_POINTER((0x2a80f0 >> 2) + 1),
        GUINT_TO_POINTER((remaining + 7) / 8));
    for (unsigned word = 0; word < 2; word++) {
        uint32_t data = 0;
        for (unsigned byte = 0; byte < 4; byte++) {
            unsigned i = s->flexspi_rx_cursor + word * 4 + byte;
            if (i < s->flexspi_rx_size) {
                data |= (uint32_t)s->flexspi_rx[i] << (byte * 8);
            }
        }
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER(((0x2a8100 + word * 4) >> 2) + 1),
            GUINT_TO_POINTER(data));
    }
}

/* Native input messages run on QEMU's main loop. Device levels and releases
 * use virtual time; the panel firmware continues scanning its GPIO matrix.
 * Timed diagnostic keys use the same device GPIO path.
 */
static void l6_input_read(void *opaque);
static void l6_input_write(void *opaque);

static void l6_input_handlers(L6ChipState *s)
{
    qemu_set_fd_handler(s->input_fd, l6_input_read,
                       s->input_tx->len ? l6_input_write : NULL, s);
}

static void l6_input_disconnect(L6ChipState *s)
{
    qemu_set_fd_handler(s->input_fd, NULL, NULL, NULL);
    close(s->input_fd);
    s->input_fd = -1;
    if (!s->panel) {
        l6_display_close(&s->display);
    }
    memset(s->input_button_down, 0, sizeof(s->input_button_down));
    memset(s->input_release_requested, 0, sizeof(s->input_release_requested));
    memset(s->input_tap_deadline, 0, sizeof(s->input_tap_deadline));
    for (unsigned button = 0; button < L6_BUTTON_EVENT_COUNT; button++) {
        g_queue_clear_full(&s->input_taps[button], g_free);
    }
}

static void l6_input_reply(L6ChipState *s, uint32_t kind, uint32_t seq,
                           uint32_t target, int32_t value)
{
    uint8_t message[L6_INPUT_MESSAGE_SIZE];
    if (s->input_fd < 0) {
        return;
    }
    if (s->input_tx->len > L6_INPUT_TX_LIMIT - sizeof(message)) {
        l6_input_disconnect(s);
        return;
    }
    stl_le_p(message, L6_INPUT_MAGIC);
    stl_le_p(message + 4, kind);
    stl_le_p(message + 8, seq);
    stl_le_p(message + 12, target);
    stl_le_p(message + 16, value);
    stl_le_p(message + 20, qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
    qemu_log_mask(LOG_UNIMP,
                  "l6 %s input reply kind=%08x seq=%u target=%u value=%d at %" PRId64 " ms\n",
                  s->panel ? "panel" : "main", kind, seq, target, value,
                  qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
    g_byte_array_append(s->input_tx, message, sizeof(message));
    l6_input_handlers(s);
}

static void l6_display_frame_ready(void *opaque, uint32_t generation,
                                   uint32_t slot, uint32_t dirty_pages)
{
    l6_input_reply(opaque, 0x80000004u, generation, slot, dirty_pages);
}

static void l6_input_write(void *opaque)
{
    L6ChipState *s = opaque;
    ssize_t written = write(s->input_fd, s->input_tx->data, s->input_tx->len);
    if (written > 0) {
        g_byte_array_remove_range(s->input_tx, 0, written);
    } else if (written < 0 && errno != EAGAIN && errno != EWOULDBLOCK &&
               errno != EINTR) {
        l6_input_disconnect(s);
        return;
    }
    l6_input_handlers(s);
}

static void l6_input_command(L6ChipState *s, const uint8_t *message)
{
    uint32_t kind = ldl_le_p(message + 4);
    uint32_t seq = ldl_le_p(message + 8);
    uint32_t target = ldl_le_p(message + 12);
    int32_t value = (int32_t)ldl_le_p(message + 16);
    uint32_t duration = ldl_le_p(message + 20);
    bool valid_button = target < L6_BUTTON_EVENT_COUNT &&
        (s->panel ? (target == 1 || target == 2 || target == 3 || target == 6 || (target >= 7 && target <= 47))
                  : (target == 0 || target == 4 || target == 5 || target >= 48));

    if (ldl_le_p(message) != L6_INPUT_MAGIC || !seq) {
        l6_input_disconnect(s);
        return;
    }
    if (kind == 5) {
        if (s->panel || target >= 2 || value || duration ||
            !l6_display_ack(&s->display, seq, target)) {
            l6_input_disconnect(s);
        }
        return;
    }
    if (kind == 6) {
        if (s->panel || target < 3 || target > 7 || value < 0 || value > 1023 || duration) {
            l6_input_reply(s, 0x80000003u, seq, target, -EINVAL);
        } else {
            s->analog_inputs[target - 3] = value;
            l6_input_reply(s, 0x80000001u, seq, target, value);
        }
        return;
    }
    if (((kind >= 1 && kind <= 3) && !valid_button) ||
        (kind == 4 && (!s->panel || target >= L6_ENCODER_COUNT)) ||
        kind < 1 || kind > 4 ||
        (kind == 1 && (value != 1 || duration > 60000)) ||
        (kind == 2 && (value != 0 || duration != 0)) ||
        (kind == 3 && (value != 1 || !duration || duration > 60000)) ||
        (kind == 4 && (!value || duration != 0))) {
        l6_input_reply(s, 0x80000003u, seq, target, -EINVAL);
        return;
    }
    switch (kind) {
    case 1:
        s->input_trace_until = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) + 1000;
        if ((s->input_button_down[target] &&
             !s->input_release_requested[target] &&
             g_queue_is_empty(&s->input_taps[target])) ||
            (!g_queue_is_empty(&s->input_taps[target]) &&
             !((L6InputTap *)g_queue_peek_tail(&s->input_taps[target]))->automatic &&
             !((L6InputTap *)g_queue_peek_tail(&s->input_taps[target]))->release_requested)) {
            l6_input_reply(s, 0x80000003u, seq, target, -EALREADY);
            break;
        }
        if (s->input_release_requested[target] ||
            !g_queue_is_empty(&s->input_taps[target]) ||
            s->input_tap_deadline[target] ||
            qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) < s->input_next_press[target]) {
            if (g_queue_get_length(&s->input_taps[target]) >= L6_INPUT_QUEUE_LIMIT) {
                l6_input_reply(s, 0x80000003u, seq, target, -ENOBUFS);
                break;
            }
            L6InputTap *press = g_new0(L6InputTap, 1);
            press->sequence = seq;
            press->duration_ms = MAX(duration, L6_BUTTON_TAP_TICKS);
            g_queue_push_tail(&s->input_taps[target], press);
            break;
        }
        s->input_button_down[target] = true;
        s->input_release_requested[target] = false;
        s->input_down_deadline[target] = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) +
            MAX(duration, L6_BUTTON_TAP_TICKS);
        l6_input_reply(s, 0x80000001u, seq, target, 1);
        break;
    case 2:
        if (!g_queue_is_empty(&s->input_taps[target])) {
            L6InputTap *press = g_queue_peek_tail(&s->input_taps[target]);
            if (!press->automatic && !press->release_requested) {
                press->release_requested = true;
                press->release_sequence = seq;
                break;
            }
            if (!press->automatic && press->release_requested) {
                l6_input_reply(s, 0x80000003u, seq, target, -EALREADY);
                break;
            }
        }
        if (s->input_release_requested[target] ||
            !s->input_button_down[target]) {
            l6_input_reply(s, 0x80000003u, seq, target, -EALREADY);
            break;
        }
        s->input_release_requested[target] = true;
        s->input_release_sequence[target] = seq;
        break;
    case 3: {
        if (g_queue_get_length(&s->input_taps[target]) >= L6_INPUT_QUEUE_LIMIT) {
            l6_input_reply(s, 0x80000003u, seq, target, -ENOBUFS);
            break;
        }
        L6InputTap *tap = g_new0(L6InputTap, 1);
        tap->sequence = seq;
        tap->duration_ms = MAX(duration, L6_BUTTON_TAP_TICKS);
        tap->automatic = true;
        g_queue_push_tail(&s->input_taps[target], tap);
        break;
    }
    case 4:
        s->encoder_target[target] = CLAMP(
            (int64_t)s->encoder_target[target] + value, INT32_MIN, INT32_MAX);
        l6_input_reply(s, 0x80000001u, seq, target,
                       s->encoder_target[target]);
        break;
    }
}

static void l6_input_read(void *opaque)
{
    L6ChipState *s = opaque;
    /* Bound work per event-loop iteration so input cannot starve timers. */
    for (unsigned count = 0; count < 256; count++) {
        ssize_t received = read(s->input_fd, s->input_rx + s->input_rx_size,
                               sizeof(s->input_rx) - s->input_rx_size);
        if (received > 0) {
            s->input_rx_size += received;
            if (s->input_rx_size == sizeof(s->input_rx)) {
                l6_input_command(s, s->input_rx);
                s->input_rx_size = 0;
                if (s->input_fd < 0) {
                    return;
                }
            }
        } else if (received == 0 ||
                   (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR)) {
            l6_input_disconnect(s);
            return;
        } else if (errno != EINTR) {
            return;
        }
    }
}

static void l6_socket_input_tick(L6ChipState *s)
{
    int64_t now = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);
    for (unsigned button = 0; button < L6_BUTTON_EVENT_COUNT; button++) {
        if (s->input_release_requested[button] &&
            now >= s->input_down_deadline[button]) {
            s->input_button_down[button] = false;
            s->input_release_requested[button] = false;
            l6_input_reply(s, 0x80000001u, s->input_release_sequence[button],
                           button, 0);
            s->input_next_press[button] = now + L6_INPUT_RELEASE_GAP_MS;
            continue;
        }
        if (s->input_tap_deadline[button] &&
            now >= s->input_tap_deadline[button]) {
            s->input_tap_deadline[button] = 0;
            l6_input_reply(s, 0x80000002u, s->input_tap_sequence[button],
                           button, s->input_button_down[button]);
            s->input_next_press[button] = now + L6_INPUT_RELEASE_GAP_MS;
            continue;
        }
        if (!s->input_tap_deadline[button] &&
            !s->input_button_down[button] &&
            now >= s->input_next_press[button] &&
            !g_queue_is_empty(&s->input_taps[button])) {
            L6InputTap *tap = g_queue_pop_head(&s->input_taps[button]);
            if (tap->automatic) {
                s->input_tap_sequence[button] = tap->sequence;
                s->input_tap_deadline[button] = now + tap->duration_ms;
            } else {
                s->input_button_down[button] = true;
                s->input_down_deadline[button] = now + tap->duration_ms;
                s->input_release_requested[button] = tap->release_requested;
                s->input_release_sequence[button] = tap->release_sequence;
            }
            l6_input_reply(s, 0x80000001u, tap->sequence, button, 1);
            g_free(tap);
        }
    }
}

static void l6_encoder_input_tick(L6ChipState *s);

/* The removable QEMU SD medium also drives the board's card-detect pin.
 * Poll at the existing virtual millisecond tick, latch GPIO2 ISR and deliver
 * its upper-bank interrupt. Medium changes still go through QEMU's SDBus.
 */
static void l6_sd_detect_tick(L6ChipState *s)
{
    if (!s->sdhc) { return; }
    bool present = sdbus_get_inserted(&s->sdhc->sdbus);
    gpointer isr_key = GUINT_TO_POINTER((0x1bc018 >> 2) + 1);
    gpointer imr_key = GUINT_TO_POINTER((0x1bc014 >> 2) + 1);
    uint32_t isr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, isr_key));
    uint32_t imr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, imr_key));
    if (present != s->sd_present) {
        s->sd_present = present;
        isr |= 1u << 28;
        g_hash_table_insert(s->registers, isr_key, GUINT_TO_POINTER(isr));
        qemu_log_mask(LOG_UNIMP, "l6 SD card detect=%u\n", present);
    }
    qemu_set_irq(qdev_get_gpio_in(DEVICE(&s->armv7m), 83),
                 !!(isr & imr & (1u << 28)));
}

static void l6_input_timer_tick(void *opaque)
{
    L6ChipState *s = opaque;
    l6_socket_input_tick(s);
    if (!s->panel) { l6_sd_detect_tick(s); }
    if (s->panel && !s->cpu_held) {
        l6_encoder_input_tick(s);
    }
    timer_mod(s->input_timer,
              qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 1000000);
}

static bool l6_button_down(L6ChipState *s, unsigned button)
{
    return s->input_button_down[button] ||
        s->input_tap_deadline[button] != 0;
}

static void l6_encoder_input_tick(L6ChipState *s)
{
    if (s->input_fd >= 0) {
        static const uint8_t phases[2][4] = {
            { 2, 0, 1, 3 }, /* positive: one Gray-code cycle from idle 3 */
            { 1, 0, 2, 3 }, /* negative: reverse Gray-code cycle */
        };
        for (unsigned encoder = 0; encoder < L6_ENCODER_COUNT; encoder++) {
            int32_t target = s->encoder_target[encoder];
            if (!s->encoder_active[encoder] &&
                target != s->encoder_position[encoder]) {
                s->encoder_direction[encoder] =
                    target > s->encoder_position[encoder] ? 1 : -1;
                s->encoder_sequence[encoder] = 0;
                s->encoder_active[encoder] = true;
                qemu_log_mask(LOG_UNIMP,
                    "l6 panel encoder %u target=%d position=%" PRId32 " direction=%d\n",
                    encoder + 1, target, s->encoder_position[encoder],
                    s->encoder_direction[encoder]);
            }
            if (!s->encoder_active[encoder]) {
                continue;
            }
            if (s->encoder_phase_wait[encoder]) {
                s->encoder_phase_wait[encoder]--;
                continue;
            }
            unsigned direction = s->encoder_direction[encoder] > 0 ? 0 : 1;
            s->encoder_phase[encoder] =
                phases[direction][s->encoder_sequence[encoder]++];
            /* Shorter 2 ms and 5 ms phases lose firmware encoder reports.
             * Preserve the verified 10 ms dwell (40 ms per detent). */
            s->encoder_phase_wait[encoder] = 9;
            if (s->encoder_sequence[encoder] == 4) {
                s->encoder_position[encoder] +=
                    s->encoder_direction[encoder];
                s->encoder_active[encoder] = false;
            }
        }
    }
}

static int64_t l6_panel_tim2_period(L6ChipState *s)
{
    uint64_t prescale = (GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x0028 >> 2) + 1))) & 0xffff) + 1ULL;
    uint64_t reload = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x002c >> 2) + 1))) + 1ULL;
    /* Startup configures the STM32 timer clock to 48 MHz. Divide before
     * multiplying nanoseconds to stay in range for the 32-bit TIM2 ARR. */
    uint64_t ticks = prescale * reload;
    uint64_t ns = (ticks / 48000000) * 1000000000ULL +
                  (ticks % 48000000) * 1000000000ULL / 48000000;
    return MAX(ns, 1);
}

static uint32_t l6_panel_reg(L6ChipState *s, hwaddr offset)
{
    return GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                           GUINT_TO_POINTER((offset >> 2) + 1)));
}

static void l6_panel_store(L6ChipState *s, hwaddr offset, uint32_t value)
{
    g_hash_table_insert(s->registers, GUINT_TO_POINTER((offset >> 2) + 1),
                        GUINT_TO_POINTER(value));
}

/* External indicator matrix: 24 serial columns, eight one-hot commons.
 * These outputs come from the peripheral pins, never UART or guest variables. */
static void l6_indicator_tick(void *opaque)
{
    L6ChipState *s = opaque;
    l6_indicators_sample(&s->indicators);
    bool initial = !s->indicators.generation;
    for (unsigned row = 0; row < 8; row++) {
        if (s->indicators.seen[row] != s->indicators.published[row] || initial) {
            if (!++s->indicators.generation) {
                ++s->indicators.generation;
            }
            l6_input_reply(s, 0x80000005u, s->indicators.generation, row,
                           s->indicators.seen[row]);
            s->indicators.published[row] = s->indicators.seen[row];
        }
        s->indicators.seen[row] = 0;
    }
    /* The display only publishes changed pixels; it is not a clock source. */
    if (!++s->indicators.generation) { ++s->indicators.generation; }
    l6_input_reply(s, 0x80000008u, s->indicators.generation, 0, 0);
    timer_mod(s->indicator_timer,
              qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 50000000);
}

static int64_t l6_panel_tim3_period(L6ChipState *s)
{
    uint64_t ticks = ((l6_panel_reg(s, 0x428) & 0xffff) + 1ULL) *
                     (l6_panel_reg(s, 0x42c) + 1ULL);
    return MAX((ticks / 48000000) * 1000000000ULL +
               (ticks % 48000000) * 1000000000ULL / 48000000, 1);
}

static void l6_panel_tim3_tick(void *opaque)
{
    L6ChipState *s = opaque;
    uint32_t cr1 = l6_panel_reg(s, 0x400);
    if (!(cr1 & 1) || s->cpu_held) {
        return;
    }
    l6_panel_store(s, 0x410, l6_panel_reg(s, 0x410) | 1);
    if (cr1 & 8) { /* one-pulse mode */
        l6_panel_store(s, 0x400, cr1 & ~1u);
    } else {
        timer_mod(s->panel_tim3, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  l6_panel_tim3_period(s));
    }
    if (l6_panel_reg(s, 0x40c) & 1) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 16));
    }
}

static int64_t l6_panel_tim14_period(L6ChipState *s)
{
    uint64_t ticks = ((l6_panel_reg(s, 0x2028) & 0xffff) + 1ULL) *
                     (l6_panel_reg(s, 0x202c) + 1ULL);
    return MAX((ticks / 48000000) * 1000000000ULL +
               (ticks % 48000000) * 1000000000ULL / 48000000, 1);
}

static void l6_panel_tim14_tick(void *opaque)
{
    L6ChipState *s = opaque;
    uint32_t cr1 = l6_panel_reg(s, 0x2000);
    if (!(cr1 & 1) || s->cpu_held) {
        return;
    }
    l6_panel_store(s, 0x2010, l6_panel_reg(s, 0x2010) | 1);
    if (cr1 & 8) {
        l6_panel_store(s, 0x2000, cr1 & ~1u);
    } else {
        timer_mod(s->panel_tim14, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  l6_panel_tim14_period(s));
    }
    if (l6_panel_reg(s, 0x200c) & 1) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 19));
    }
}

static void l6_panel_spi_dma_tick(void *opaque)
{
    L6ChipState *s = opaque;
    uint32_t ccr = l6_panel_reg(s, 0x20008);
    if (s->cpu_held || !(ccr & 1)) {
        return;
    }
    AddressSpace *as = CPU(s->armv7m.cpu)->as;
    uint32_t source = l6_panel_reg(s, 0x20014);
    unsigned count = l6_panel_reg(s, 0x2000c);
    if (s->indicator_log_count++ < 8) {
        qemu_log_mask(LOG_UNIMP, "l6 panel LED DMA ccr=%x src=%x dst=%x count=%u\n",
                      ccr, source, l6_panel_reg(s, 0x20010), count);
    }
    if (count > 4096 || l6_panel_reg(s, 0x20010) != 0x4001300c ||
        !(ccr & (1u << 4))) {
        return;
    }
    for (unsigned i = 0; i < count; i++) {
        uint8_t byte;
        if (address_space_read(as, source, MEMTXATTRS_UNSPECIFIED,
                               &byte, 1) != MEMTX_OK) {
            return;
        }
        /* SPI1's PA5 clock / PA7 data shift MSB first, exactly as a
         * three-byte external shift register would consume the pin edges. */
        l6_indicators_spi_byte(&s->indicators, byte);
        if (ccr & (1u << 7)) {
            source++;
        }
    }
    l6_panel_store(s, 0x2000c, 0);
    l6_panel_store(s, 0x20000, l6_panel_reg(s, 0x20000) | 3);
    if (ccr & 2) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 9));
    }
}

static void l6_panel_tim2_tick(void *opaque)
{
    L6ChipState *s = opaque;
    uint32_t cr1 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER(1)));
    uint32_t dier = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x000c >> 2) + 1)));
    if (!(cr1 & 1) || s->cpu_held) {
        return;
    }
    if (dier & 1) {
        if (s->input_trace &&
            qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) < s->input_trace_until) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 panel TIM2 assert at %" PRId64 " ns period=%" PRId64 " ns\n",
                          qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL),
                          l6_panel_tim2_period(s));
        }
        gpointer sr_key = GUINT_TO_POINTER((0x0010 >> 2) + 1);
        uint32_t sr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, sr_key));
        g_hash_table_insert(s->registers, sr_key, GUINT_TO_POINTER(sr | 1));
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 15));
    }
    timer_mod(s->panel_tim2, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + l6_panel_tim2_period(s));
}

static int l6_panel_uart_can_receive(void *opaque)
{
    L6ChipState *s = opaque;
    return s->panel_rx_pending ? 0 : 1;
}

static void l6_panel_uart_receive(void *opaque, const uint8_t *buf, int size)
{
    L6ChipState *s = opaque;
    if (size <= 0 || s->panel_rx_pending) {
        return;
    }
    s->panel_rx_byte = buf[0];
    s->panel_rx_pending = true;
    s->panel_rx_count++;
    qemu_log_mask(LOG_UNIMP, "l6 panel USART1 RX %02x\n", buf[0]);
    uint32_t cr1 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x013800 >> 2) + 1)));
    if (cr1 & (1u << 5)) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 27));
    }
}

static int l6_main_uart_can_receive(void *opaque)
{
    L6ChipState *s = opaque;
    return s->main_rx_pending || s->main_rom_mode ? 0 : 1;
}

static void l6_main_uart_receive(void *opaque, const uint8_t *buf, int size)
{
    L6ChipState *s = opaque;
    if (size <= 0 || s->main_rx_pending) {
        return;
    }
    s->main_rx_byte = buf[0];
    s->main_rx_pending = true;
    s->main_rx_count++;
    qemu_log_mask(LOG_UNIMP, "l6 main LPUART1 RX %02x\n", buf[0]);
    uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x184018 >> 2) + 1)));
    if (ctrl & (1u << 21)) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 20));
    }
}

static void l6_uart_link_tick(void *opaque)
{
    L6UartLink *link = opaque;
    L6ChipState *destination = link->destination;
    if (!link->queue->len || destination->cpu_held) {
        return;
    }
    int ready = destination->panel ?
        l6_panel_uart_can_receive(destination) :
        l6_main_uart_can_receive(destination);
    if (!ready) {
        return;
    }
    uint8_t byte = link->queue->data[0];
    g_byte_array_remove_range(link->queue, 0, 1);
    if (destination->panel) {
        l6_panel_uart_receive(destination, &byte, 1);
    } else {
        l6_main_uart_receive(destination, &byte, 1);
    }
    if (link->queue->len) {
        timer_mod(link->timer,
                  qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + L6_UART_BYTE_NS);
    }
}

static void l6_uart_link_send(L6ChipState *source, uint8_t byte)
{
    L6UartLink *link = source->uart_link;
    if (link->queue->len >= L6_UART_QUEUE_LIMIT) {
        error_setg(&error_fatal, "L6 internal UART queue overflow");
    }
    g_byte_array_append(link->queue, &byte, 1);
    if (!timer_pending(link->timer)) {
        timer_mod(link->timer,
                  qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + L6_UART_BYTE_NS);
    }
}

static void l6_uart_accept_input(L6ChipState *destination)
{
    if (destination->peer) {
        L6UartLink *incoming = destination->peer->uart_link;
        if (incoming->queue->len && !timer_pending(incoming->timer)) {
            timer_mod(incoming->timer,
                      qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + L6_UART_BYTE_NS);
        }
    } else {
        qemu_chr_fe_accept_input(destination->panel ?
                                 &destination->panel_uart :
                                 &destination->main_uart);
    }
}

static void l6_storage_flush(void *bytes, size_t size)
{
    if (bytes && msync(bytes, size, MS_SYNC)) {
        error_setg(&error_fatal, "cannot flush device storage: %s", strerror(errno));
    }
}

static uint8_t *l6_rom_flash(L6ChipState *main)
{
    return main->peer ? memory_region_get_ram_ptr(&main->peer->flash) :
                        main->rom_flash;
}

static void l6_rom_write_flash(L6ChipState *main, unsigned offset,
                              const uint8_t *bytes, unsigned count)
{
    if (main->peer) {
        address_space_write_rom(CPU(main->peer->armv7m.cpu)->as,
                                0x08000000 + offset, MEMTXATTRS_UNSPECIFIED,
                                bytes, count);
        if (main->peer->persistent_panel) {
            memcpy(main->peer->persistent_panel + offset, bytes, count);
            l6_storage_flush(main->peer->persistent_panel, 0x10000);
        }
    } else {
        memcpy(main->rom_flash + offset, bytes, count);
    }
}

static void l6_rom_erase_flash(L6ChipState *main, unsigned offset,
                              unsigned count)
{
    g_autofree uint8_t *blank = g_malloc(count);
    memset(blank, 0xff, count);
    l6_rom_write_flash(main, offset, blank, count);
}

static void l6_chip_cpu_reset(CPUState *cpu, run_on_cpu_data data)
{
    L6ChipState *chip = data.host_ptr;
    cpu->start_powered_off = chip->cpu_held;
    device_cold_reset(DEVICE(&chip->armv7m));
    device_cold_reset(DEVICE(&chip->armv7m.nvic));
    device_cold_reset(DEVICE(&chip->armv7m.systick[M_REG_NS]));
    cpu_reset(cpu);
    qemu_log_mask(LOG_UNIMP, "l6 dual %s CPU %s\n",
                  chip->panel ? "panel" : "main",
                  chip->cpu_held ? "held in reset/ROM" : "booted application");
}

static void l6_local_reset_request(void *opaque, int n, int level)
{
    L6ChipState *chip = opaque;
    if (!level) {
        return;
    }
    if (chip->panel) {
        timer_del(chip->panel_tim2);
        timer_del(chip->panel_tim3);
        timer_del(chip->panel_tim14);
        timer_del(chip->panel_spi_dma);
        memset(&chip->indicators, 0, sizeof(chip->indicators));
        l6_panel_store(chip, 0x400, 0);
        l6_panel_store(chip, 0x2000, 0);
        l6_panel_store(chip, 0x20008, 0);
        l6_panel_store(chip, 0x10000014, 0);
        l6_panel_store(chip, 0x10000414, 0);
        g_hash_table_insert(chip->registers, GUINT_TO_POINTER(1), NULL);
        g_hash_table_insert(chip->registers,
            GUINT_TO_POINTER((0x013800 >> 2) + 1), NULL);
        chip->panel_rx_pending = false;
    } else {
        timer_del(chip->rom_uart_timer);
        chip->rom_phase = 0;
        chip->rom_reply_read = chip->rom_reply_visible = chip->rom_reply_write = 0;
        chip->main_rx_pending = false;
    }
    async_run_on_cpu(CPU(chip->armv7m.cpu), l6_chip_cpu_reset,
                     RUN_ON_CPU_HOST_PTR(chip));
}

static void l6_update_panel_boot(L6ChipState *main)
{
    if (!main->peer) {
        return;
    }
    L6ChipState *panel = main->peer;
    bool held = main->panel_reset_asserted || main->main_rom_mode;
    if (panel->cpu_held == held) {
        return;
    }
    panel->cpu_held = held;
    panel->panel_rx_pending = false;
    timer_del(panel->panel_tim2);
    timer_del(panel->panel_tim3);
    timer_del(panel->panel_tim14);
    timer_del(panel->panel_spi_dma);
    l6_panel_store(panel, 0x400, 0);
    l6_panel_store(panel, 0x2000, 0);
    l6_panel_store(panel, 0x20008, 0);
    memset(&panel->indicators, 0, sizeof(panel->indicators));
    l6_panel_store(panel, 0x10000014, 0);
    l6_panel_store(panel, 0x10000414, 0);
    timer_del(main->uart_link->timer);
    timer_del(panel->uart_link->timer);
    g_byte_array_set_size(main->uart_link->queue, 0);
    g_byte_array_set_size(panel->uart_link->queue, 0);
    /* Registers outside this sparse peripheral model (NVIC/SysTick and CPU)
     * reset through their QEMU device. Preserve the RTC calendar domain. */
    g_hash_table_insert(panel->registers,
        GUINT_TO_POINTER((0x013800 >> 2) + 1), NULL);
    g_hash_table_insert(panel->registers,
        GUINT_TO_POINTER((0x0000 >> 2) + 1), NULL);
    async_run_on_cpu(CPU(panel->armv7m.cpu), l6_chip_cpu_reset,
                     RUN_ON_CPU_HOST_PTR(panel));
}

static void l6_rom_uart_tick(void *opaque)
{
    L6ChipState *s = opaque;
    if (!s->main_rom_mode || s->rom_reply_visible == s->rom_reply_write) {
        return;
    }
    s->rom_reply_visible++;
    uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x184018 >> 2) + 1)));
    if (ctrl & (1u << 21)) {
        qemu_irq_raise(qdev_get_gpio_in(DEVICE(&s->armv7m), 20));
    }
    if (s->rom_reply_visible < s->rom_reply_write) {
        timer_mod(s->rom_uart_timer,
                  qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 100000);
    }
}

/* The audio DSP callback is not yet reached without a SAI/eDMA stream.
 * Complete its pending ramp request so firmware startup can continue.
 * This is a bring-up approximation, not an audio signal model.
 */
static void l6_main_audio_service_tick(void *opaque)
{
    L6ChipState *s = opaque;
    uint8_t *ram = memory_region_get_ram_ptr(&s->external_ram);
    if (ldl_le_p(ram + 0xa3e8) == 1) {
        stl_le_p(ram + 0xa3e8, 0);
    }
    if (s->service_audio_queue) {
        /* Diagnostic stand-in for the absent audio transfer consumer. The
         * firmware's 15-slot staging queue otherwise fills during setup.
         * This acknowledges its read index without processing audio data. */
        uint8_t *sram = memory_region_get_ram_ptr(&s->sram);
        uint32_t write_index = ldl_le_p(sram + 0x7d08);
        stl_le_p(sram + 0x7d0c, write_index);
    }
    timer_mod(s->main_audio_service_timer,
              qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 1000000);
}

static void l6_rom_reply_byte(L6ChipState *s, uint8_t byte)
{
    if (s->rom_reply_write < sizeof(s->rom_reply)) {
        s->rom_reply[s->rom_reply_write++] = byte;
        qemu_log_mask(LOG_UNIMP, "l6 panel ROM TX %02x\n", byte);
        if (!timer_pending(s->rom_uart_timer)) {
            timer_mod(s->rom_uart_timer,
                      qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 100000);
        }
    }
}

/* The STM32's system ROM is absent from the extracted panel application.
 * Model the AN3155 Read, Write, and Extended Erase exchanges used here.
 */
static void l6_rom_write_byte(L6ChipState *s, uint8_t byte)
{
    switch (s->rom_phase) {
    case 0: /* 0x7f autobaud sync */
        if (byte == 0x7f) {
            l6_rom_reply_byte(s, 0x79);
            s->rom_phase = 1;
        }
        break;
    case 1: /* command */
        s->rom_command = byte;
        s->rom_phase = 2;
        break;
    case 2: /* complemented command */
        if ((s->rom_command ^ byte) == 0xff &&
            (s->rom_command == 0x11 || s->rom_command == 0x31 ||
             s->rom_command == 0x44)) {
            l6_rom_reply_byte(s, 0x79);
            s->rom_input_size = 0;
            s->rom_phase = s->rom_command == 0x44 ? 7 : 3;
        } else {
            l6_rom_reply_byte(s, 0x1f);
            s->rom_phase = 1;
        }
        break;
    case 3: /* address and XOR checksum */
        s->rom_input[s->rom_input_size++] = byte;
        if (s->rom_input_size == 5) {
            uint8_t checksum = s->rom_input[0] ^ s->rom_input[1] ^
                               s->rom_input[2] ^ s->rom_input[3];
            if (checksum == s->rom_input[4]) {
                s->rom_address = ((uint32_t)s->rom_input[0] << 24) |
                                 ((uint32_t)s->rom_input[1] << 16) |
                                 ((uint32_t)s->rom_input[2] << 8) |
                                  s->rom_input[3];
                l6_rom_reply_byte(s, 0x79);
                s->rom_input_size = 0;
                s->rom_phase = s->rom_command == 0x11 ? 4 : 5;
            } else {
                l6_rom_reply_byte(s, 0x1f);
                s->rom_phase = 1;
            }
        }
        break;
    case 4: /* N-1 and complement, then N bytes of flash data */
        s->rom_input[s->rom_input_size++] = byte;
        if (s->rom_input_size == 2) {
            if ((s->rom_input[0] ^ s->rom_input[1]) == 0xff) {
                unsigned count = s->rom_input[0] + 1;
                l6_rom_reply_byte(s, 0x79);
                for (unsigned i = 0; i < count; i++) {
                    uint32_t address = s->rom_address + i;
                    l6_rom_reply_byte(s,
                        address >= 0x08000000 && address < 0x08010000 ?
                        l6_rom_flash(s)[address - 0x08000000] :
                        address >= 0x1fff7800 && address < 0x1fff7804 ?
                        s->rom_option[address - 0x1fff7800] : 0xff);
                }
            } else {
                l6_rom_reply_byte(s, 0x1f);
            }
            s->rom_phase = 1;
        }
        break;
    case 5: /* Write Memory byte count minus one */
        s->rom_input_size = 0;
        s->rom_input[0] = byte;
        s->rom_phase = 6;
        break;
    case 6: { /* Write Memory data followed by XOR checksum */
        unsigned count = s->rom_input[0] + 1;
        s->rom_input[++s->rom_input_size] = byte;
        if (s->rom_input_size == count + 1) {
            uint8_t checksum = s->rom_input[0];
            for (unsigned i = 1; i <= count; i++) {
                checksum ^= s->rom_input[i];
            }
            bool flash_range = s->rom_address >= 0x08000000 &&
                (uint64_t)s->rom_address + count <= 0x08010000;
            bool option_range = s->rom_address == 0x1fff7800 && count == 4;
            if (checksum != s->rom_input[count + 1] ||
                (count & 3) || (!flash_range && !option_range)) {
                l6_rom_reply_byte(s, 0x1f);
            } else {
                uint8_t *target = flash_range ?
                    l6_rom_flash(s) + s->rom_address - 0x08000000 :
                    s->rom_option;
                if (flash_range) {
                    l6_rom_write_flash(s, s->rom_address - 0x08000000,
                                       s->rom_input + 1, count);
                } else {
                    memcpy(target, s->rom_input + 1, count);
                    if (s->persistent_options) {
                        memcpy(s->persistent_options, target, count);
                        l6_storage_flush(s->persistent_options, 4);
                    }
                }
                qemu_log_mask(LOG_UNIMP,
                    "l6 panel ROM write %08x %u bytes: %02x %02x %02x %02x\n",
                    s->rom_address, count, target[0], target[1],
                    target[2], target[3]);
                if (s->rom_address + count == 0x08004000 &&
                    s->nor_flash) {
                    qemu_log_mask(LOG_UNIMP,
                        "l6 panel ROM 16 KiB image match: %s\n",
                        memcmp(l6_rom_flash(s), s->nor_flash + 0x1f3000,
                               0x4000) == 0 ? "yes" : "no");
                }
                l6_rom_reply_byte(s, 0x79);
            }
            s->rom_phase = 1;
        }
        break;
    }
    case 7: /* Extended Erase 16-bit page count */
        s->rom_input[s->rom_input_size++] = byte;
        if (s->rom_input_size == 2) {
            s->rom_phase = 8;
        }
        break;
    case 8: { /* Page numbers (or special command) and XOR checksum */
        uint16_t pages = ((uint16_t)s->rom_input[0] << 8) |
                         s->rom_input[1];
        unsigned remaining = pages >= 0xfff0 ? 1 : 2 * (pages + 1) + 1;
        if (remaining > sizeof(s->rom_input) - 2) {
            l6_rom_reply_byte(s, 0x1f);
            s->rom_phase = 1;
            break;
        }
        s->rom_input[s->rom_input_size++] = byte;
        if (s->rom_input_size == remaining + 2) {
            uint8_t checksum = 0;
            for (unsigned i = 0; i < s->rom_input_size; i++) {
                checksum ^= s->rom_input[i];
            }
            if (checksum != 0) {
                l6_rom_reply_byte(s, 0x1f);
            } else if (pages == 0xffff) {
                l6_rom_erase_flash(s, 0, sizeof(s->rom_flash));
                qemu_log_mask(LOG_UNIMP, "l6 panel ROM mass erase\n");
                l6_rom_reply_byte(s, 0x79);
            } else if (pages < 128) {
                for (unsigned i = 0; i <= pages; i++) {
                    unsigned page = ((unsigned)s->rom_input[2 + 2 * i] << 8) |
                                     s->rom_input[3 + 2 * i];
                    if (page < sizeof(s->rom_flash) / 2048) {
                        l6_rom_erase_flash(s, page * 2048, 2048);
                    }
                }
                qemu_log_mask(LOG_UNIMP, "l6 panel ROM erase %u pages\n",
                              pages + 1);
                l6_rom_reply_byte(s, 0x79);
            } else {
                l6_rom_reply_byte(s, 0x1f);
            }
            s->rom_phase = 1;
        }
        break;
    }
    }
}

/* RT1052 clock/power semantics around QEMU's reusable card and ADMA model.
 * Unlike SDHCI, USDHC derives clock/power from the SoC; SYS_CTRL[2:0]
 * are reserved. Firmware 0x80080fc8 waits for PRES_STATE.SDSTB before
 * programming its divider and writes INITA to issue initialization clocks.
 */
static uint64_t l6_sd_read(void *opaque, hwaddr offset, unsigned size)
{
    L6ChipState *s = opaque;
    if (offset == 0x2c && size == 4) {
        return s->sd_sysctl;
    }
    return s->sd_ops->read(s->sdhc, offset, size);
}

static void l6_sd_write(void *opaque, hwaddr offset, uint64_t value, unsigned size)
{
    L6ChipState *s = opaque;
    if (offset == 0x2c && size == 4) {
        /* RSTA/RSTC/RSTD and INITA are self-clearing. Preserve divider and
         * timeout bits for reads, then translate to the SDHCI clock contract.
         */
        s->sd_sysctl = value & ~0x0f000000u;
        s->sd_ops->write(s->sdhc, offset, (value & 0x070f0000u) | 5u, size);
        s->sdhc->pwrcon = 0x0f; /* board supplies 3.3 V */
        s->sdhc->clkcon |= 3u; /* SoC clock enabled and stable */
        return;
    }
    s->sd_ops->write(s->sdhc, offset, value, size);
}

static const MemoryRegionOps l6_sd_ops = {
    .read = l6_sd_read,
    .write = l6_sd_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid = { .min_access_size = 4, .max_access_size = 4 },
};

/* Sparse MMIO storage is intentionally crude. Peripheral models replace it. */
static void l6_rtc_tick(L6ChipState *s);

static uint64_t l6_mmio_read(void *opaque, hwaddr offset, unsigned size)
{
    L6ChipState *s = opaque;
    if (s->panel && offset >= 0x2800 && offset < 0x2860) {
        l6_rtc_tick(s);
        if (offset == 0x2808 && size == 4) {
            /* 256 Hz synchronous prescaler; calendar uses the same virtual clock. */
            return 255 - ((qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) - s->rtc_anchor_ms) % 1000) * 256 / 1000;
        }
        if (s->rtc_reads++ < 48) {
            qemu_log_mask(LOG_UNIMP, "l6 RTC read offset=%04x value=%08x\n",
                (unsigned)offset, GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                GUINT_TO_POINTER((offset >> 2) + 1))));
        }
    }
    if (s->panel && offset == 0x10 && s->input_trace &&
        qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) < s->input_trace_until) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 panel TIM2 handler SR read at %" PRId64 " ns\n",
                      qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL));
    }
    if (!s->panel && size == 4 &&
        (offset == 0x0c0000 || offset == 0x1b8000 || offset == 0x1bc000)) {
        /* Eight active-low buttons and the active-high Power input are wired
         * to the main MCU, outside the panel's scanned matrix. */
        uint32_t word = GPOINTER_TO_UINT(g_hash_table_lookup(
            s->registers, GUINT_TO_POINTER((offset >> 2) + 1)));
        uint64_t now_ms = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);
        word |= offset == 0x0c0000 ? 0x5u : offset == 0x1bc000 ? (1u << 31) :
            ((1u << 20) | (1u << 21) | (1u << 25) | (1u << 26) | 1u);
        if (offset == 0x1bc000) {
            /* Card detect is active high at GPIO2.28. Reader 0x80022a90
             * invokes the insertion callback only when this bit is set.
             */
            word = (word & ~(1u << 28)) | (s->sd_present ? (1u << 28) : 0);
            /* Power reader 0x80021d58 inverts GPIO2.25: released is low. */
            word &= ~(1u << 25);
            if (s->input_fd >= 0 && l6_button_down(s, 53)) {
                word |= 1u << 25;
            }
        }
        for (unsigned i = 0; i < s->main_key_count; i++) {
            L6MainKey *key = &s->main_keys[i];
            hwaddr port = key->id == 1 ? 0x1b8000 : 0x0c0000;
            unsigned pin = key->id == 0 ? 2 : key->id == 1 ? 20 : 0;
            if (offset != port || now_ms < key->start_ms ||
                now_ms >= key->end_ms) {
                continue;
            }
            if (!key->logged) {
                qemu_log_mask(LOG_UNIMP,
                              "l6 main key id=%u at %" PRIu64 " ms\n",
                              key->id, now_ms);
                key->logged = true;
            }
            word &= ~(1u << pin);
        }
        if (s->input_fd >= 0) {
            static const struct { unsigned id; hwaddr port; unsigned pin; } buttons[] = {
                { 0, 0x0c0000, 2 }, { 4, 0x1b8000, 20 }, { 5, 0x0c0000, 0 },
                { 48, 0x1bc000, 31 }, { 49, 0x1b8000, 21 }, { 50, 0x1b8000, 25 },
                { 51, 0x1b8000, 26 }, { 52, 0x1b8000, 0 },
            };
            for (unsigned i = 0; i < ARRAY_SIZE(buttons); i++) {
                if (offset == buttons[i].port && l6_button_down(s, buttons[i].id)) {
                    word &= ~(1u << buttons[i].pin);
                }
            }
        }
        if (offset == 0x1bc000) {
            bool pressed = (word & (1u << 25)) != 0;
            if (!s->power_seen || pressed != s->power_published) {
                s->power_seen = true;
                s->power_published = pressed;
                qemu_log_mask(LOG_UNIMP, "l6 main power input=%u at %" PRIu64 " ms\n", pressed, now_ms);
            }
        }
        return word;
    }
    if (!s->panel && offset == 0x1bc008 && size == 4) {
        return (s->main_gpio[1] & ~(1u << 28)) |
               (s->sd_present ? (1u << 28) : 0);
    }
    if (s->panel && size == 4 &&
        (offset == 0x10000010 || offset == 0x10000410 ||
         offset == 0x10000c10 || offset == 0x10001410 ||
         offset == 0x10000810)) {
        /* Pull-ups keep the key matrix and encoder inputs released. The
         * optional diagnostic key follows the row selected on GPIOB[6:4].
         */
        static const uint8_t rows[8] = {
            0, 0x40, 0x20, 0x60, 0x10, 0x50, 0x30, 0x70
        };
        static const hwaddr columns[5] = {
            0x10000410, 0x10000410, 0x10000410,
            0x10000810, 0x10001410
        };
        static const unsigned pins[5] = { 7, 8, 9, 13, 0 };
        uint32_t idr = UINT32_MAX;
        uint64_t now_ms = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);
        uint32_t odr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((0x10000414 >> 2) + 1)));
        for (unsigned i = 0; i < s->panel_key_count; i++) {
            L6PanelKey *key = &s->panel_keys[i];
            if (now_ms < key->start_ms || now_ms >= key->end_ms ||
                offset != columns[key->column] ||
                (odr & 0x70) != rows[key->row]) {
                continue;
            }
            if (!key->logged) {
                qemu_log_mask(LOG_UNIMP,
                              "l6 panel key row=%u column=%u at %" PRIu64 " ms\n",
                              key->row, key->column, now_ms);
                key->logged = true;
            }
            idr &= ~(1u << pins[key->column]);
        }
        if (s->input_fd >= 0) {
            for (unsigned row = 0; row < 8; row++) {
                if ((odr & 0x70) != rows[row]) {
                    continue;
                }
                for (unsigned column = 0; column < 5; column++) {
                    unsigned key = row * 5 + column;
                    /* Physical Up is row 5; Down is row 4. */
                    unsigned event_id = key == 24 ? 2 : key == 29 ? 1 :
                        key == 39 ? 3 : key == 34 ? 6 : 7 + key;
                    bool down = l6_button_down(s, event_id);
                    if (down && offset == columns[column]) {
                        idr &= ~(1u << pins[column]);
                    }
                }
            }
        }
        if (s->input_fd >= 0) {
            /* Static switch reader 0x08000aa0 samples PA3 outside the matrix. */
            if (offset == 0x10000010 && l6_button_down(s, 47)) {
                idr &= ~(1u << 3);
            }
            static const struct { hwaddr port; unsigned pin; } phase_a[] = {
                { 0x10000010, 0 }, { 0x10000010, 2 },
                { 0x10000010, 11 }, { 0x10000010, 15 },
                { 0x10000c10, 1 }, { 0x10000c10, 3 },
                { 0x10000410, 15 }, { 0x10000810, 6 },
            };
            static const struct { hwaddr port; unsigned pin; } phase_b[] = {
                { 0x10000010, 1 }, { 0x10001410, 1 },
                { 0x10000010, 12 }, { 0x10000c10, 0 },
                { 0x10000c10, 2 }, { 0x10000410, 3 },
                { 0x10000010, 8 }, { 0x10000810, 7 },
            };
            for (unsigned encoder = 0; encoder < L6_ENCODER_COUNT; encoder++) {
                uint8_t phase = s->encoder_phase[encoder];
                if (offset == phase_a[encoder].port && !(phase & 2)) {
                    idr &= ~(1u << phase_a[encoder].pin);
                }
                if (offset == phase_b[encoder].port && !(phase & 1)) {
                    idr &= ~(1u << phase_b[encoder].pin);
                }
            }
        }
        if (offset == 0x10001410 && s->input_trace &&
            now_ms < s->input_trace_until) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 panel matrix sample row=%02x up=%u down=%u idr=%08x at %" PRId64 " ns\n",
                          odr & 0x70, l6_button_down(s, 1),
                          l6_button_down(s, 2), idr,
                          qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL));
        }
        return idr;
    }
    if (offset == 0x0c4024 && size == 4) {
        /* Reading ADC1 R0 acknowledges the completed conversion. */
        gpointer hs_key = GUINT_TO_POINTER((0x0c4020 >> 2) + 1);
        uint32_t hs = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                            hs_key));
        if (!s->panel && (hs & 1) && s->adc_channel >= 3 && s->adc_channel <= 7) {
            unsigned knob = s->adc_channel - 3;
            uint32_t sample = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                GUINT_TO_POINTER((offset >> 2) + 1)));
            if (!s->analog_seen[knob] || sample != s->analog_published[knob]) {
                s->analog_seen[knob] = true;
                s->analog_published[knob] = sample;
                if (!++s->analog_generation) { ++s->analog_generation; }
                l6_input_reply(s, 0x80000007u, s->analog_generation, s->adc_channel, sample);
                qemu_log_mask(LOG_UNIMP, "l6 main ADC channel=%u sample=%u at %" PRId64 " ms\n",
                    s->adc_channel, sample, qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
            }
        }
        g_hash_table_insert(s->registers, hs_key, GUINT_TO_POINTER(hs & ~1u));
    }
    /* FlexSPI STS0 reports the idle command sequencer and arbiter. */
    if (offset == 0x2a80e0 && size == 4) {
        return 3;
    }
    if (offset == 0x3a0014 && size == 4) {
        /* With no queued TX word, LPSPI4 requests data. */
        uint32_t sr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((offset >> 2) + 1)));
        return sr | 1u;
    }
    if (!s->panel && offset == 0x3f0014 && size == 4) {
        /* LPI2C1 MSR.TDF stays set while the transmit FIFO is empty. */
        uint32_t msr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((offset >> 2) + 1)));
        return msr | 1u;
    }
    if (!s->panel && offset == 0x0d8070 && size == 4) {
        /* CCM_ANALOG PLL_AUDIO locks after ENABLE with POWERDOWN clear.
         * The startup routine waits for LOCK (bit 31).
         */
        uint32_t pll = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((offset >> 2) + 1)));
        return (pll & (1u << 13)) && !(pll & (1u << 12)) ?
               pll | (1u << 31) : pll & ~(1u << 31);
    }
    if (s->panel && offset == 0x280c && size == 4) {
        /* RTC ICSR.INITF follows INIT. RSF reports synchronized calendar
         * shadow registers after leaving initialization mode.
         */
        uint32_t icsr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((offset >> 2) + 1)));
        return (icsr & (1u << 7)) ? icsr | (1u << 6) :
               (icsr | (1u << 5)) & ~(1u << 6);
    }
    if (s->panel && offset == 0x21000 && size == 4) {
        /* RCC CR ready flags follow the corresponding oscillator enables. */
        uint32_t cr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((offset >> 2) + 1)));
        cr &= ~((1u << 10) | (1u << 17) | (1u << 25));
        if (cr & (1u << 8)) { cr |= 1u << 10; } /* HSI */
        if (cr & (1u << 16)) { cr |= 1u << 17; } /* HSE */
        if (cr & (1u << 24)) { cr |= 1u << 25; } /* PLL */
        return cr;
    }
    if (s->panel && offset == 0x21008 && size == 4) {
        /* RCC CFGR SWS acknowledges the configured clock switch. The board
         * clock remains 48 MHz; oscillator settling is not modeled. */
        uint32_t cfgr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((offset >> 2) + 1)));
        return (cfgr & ~0x38u) | ((cfgr & 7) << 3);
    }
    if (s->panel && offset == 0x2105c && size == 4) {
        /* RCC BDCR: the modeled low-speed external oscillator becomes ready
         * when enabled. Without LSERDY the firmware abandons RTC setup. */
        uint32_t bdcr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((offset >> 2) + 1)));
        return (bdcr & 1) ? bdcr | 2 : bdcr & ~2u;
    }
    if (s->panel && offset == 0x01381c && size == 4) {
        /* An idle STM32 USART reports transmit FIFO empty and complete. */
        return (1u << 7) | (1u << 6) |
               (s->panel_rx_pending ? (1u << 5) : 0);
    }
    if (s->panel && offset == 0x013824 && size == 4) {
        s->panel_rx_pending = false;
        l6_uart_accept_input(s);
        return s->panel_rx_byte;
    }
    if (!s->panel && offset == 0x184014 && size == 4) {
        return (1u << 23) | (1u << 22) |
               ((s->main_rom_mode ? s->rom_reply_read < s->rom_reply_visible :
                  s->main_rx_pending) ? (1u << 21) : 0);
    }
    if (!s->panel && size == 4 && offset >= 0x188014 &&
        offset <= 0x1a0014 && (offset - 0x188014) % 0x4000 == 0) {
        /* Unconnected LPUART2-8 discard TX synchronously and are idle.
         * Serial close loop at 0x800325b8 waits for STAT.TC (bit 22),
         * including LPUART3 while closing DIN MIDI for update/shutdown.
         * Reporting zero leaves preparation blocked before Power dispatch.
         */
        return (1u << 23) | (1u << 22);
    }
    if (!s->panel && offset == 0x18401c && size == 4) {
        if (s->main_rom_mode) {
            if (s->rom_reply_read < s->rom_reply_visible) {
                uint8_t byte = s->rom_reply[s->rom_reply_read++];
                qemu_log_mask(LOG_UNIMP, "l6 main LPUART1 ROM RX %02x\n", byte);
                qemu_irq_lower(qdev_get_gpio_in(DEVICE(&s->armv7m), 20));
                if (s->rom_reply_read == s->rom_reply_write) {
                    s->rom_reply_read = s->rom_reply_visible =
                        s->rom_reply_write = 0;
                }
                return byte;
            }
            return 0;
        }
        s->main_rx_pending = false;
        l6_uart_accept_input(s);
        return s->main_rx_byte;
    }
    uint32_t word = GPOINTER_TO_UINT(g_hash_table_lookup(
        s->registers, GUINT_TO_POINTER((offset >> 2) + 1)));
    unsigned shift = (offset & 3) * 8;
    return size == 4 ? word : (word >> shift) & ((1u << (size * 8)) - 1);
}

static bool l6_display_dma_transfer(L6ChipState *s)
{
    AddressSpace *as = CPU(s->armv7m.cpu)->as;
    uint8_t tx[32], rx[32], wire[4096];
    unsigned wire_size = 0;
    uint32_t tcr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x3a0060 >> 2) + 1)));
    if (address_space_read(as, 0x400e9040, MEMTXATTRS_UNSPECIFIED,
                           tx, sizeof(tx)) != MEMTX_OK ||
        address_space_read(as, 0x400e9060, MEMTXATTRS_UNSPECIFIED,
                           rx, sizeof(rx)) != MEMTX_OK ||
        (tcr & 0xfff) != 7 || !(tcr & (1u << 22))) {
        return false;
    }
    bool finished = false;
    for (unsigned chain = 0; chain < 8; chain++) {
        uint32_t source = ldl_le_p(tx);
        int16_t soff = (int16_t)lduw_le_p(tx + 4);
        uint16_t attr = lduw_le_p(tx + 6);
        uint32_t nbytes = ldl_le_p(tx + 8);
        uint32_t destination = ldl_le_p(tx + 16);
        int16_t doff = (int16_t)lduw_le_p(tx + 20);
        uint16_t count = lduw_le_p(tx + 22);
        uint16_t csr = lduw_le_p(tx + 28);
        if (!count || (count & 0x8000) || doff) {
            return false;
        }
        if (destination == 0x403a0067 && attr == 0 && nbytes == 1) {
            if (wire_size + count > sizeof(wire)) {
                return false;
            }
            for (unsigned i = 0; i < count; i++) {
                if (address_space_read(as, source, MEMTXATTRS_UNSPECIFIED,
                                       wire + wire_size++, 1) != MEMTX_OK) {
                    return false;
                }
                source += soff;
            }
        } else if (destination == 0x403a0060 && attr == 0x0202 &&
                   nbytes == 4 && count == 1) {
            uint8_t command[4];
            if (address_space_read(as, source, MEMTXATTRS_UNSPECIFIED,
                                   command, sizeof(command)) != MEMTX_OK) {
                return false;
            }
            tcr = ldl_le_p(command);
            source += soff;
        } else {
            return false;
        }
        stl_le_p(tx, source + (int32_t)ldl_le_p(tx + 12));
        stw_le_p(tx + 22, 0);
        if (!(csr & 0x10)) {
            stw_le_p(tx + 28, csr | 0x80); /* DONE after the final major loop. */
            finished = true;
            break;
        }
        uint32_t next = ldl_le_p(tx + 24);
        if ((next & 31) ||
            address_space_read(as, next, MEMTXATTRS_UNSPECIFIED,
                               tx, sizeof(tx)) != MEMTX_OK) {
            return false;
        }
    }
    /* This initial device model supports the observed byte RX sink only. */
    uint16_t rx_count = lduw_le_p(rx + 22);
    if (!finished || !wire_size || ldl_le_p(rx) != 0x403a0077 ||
        lduw_le_p(rx + 6) || ldl_le_p(rx + 8) != 1 ||
        lduw_le_p(rx + 4) || (rx_count & 0x8000) ||
        rx_count != wire_size || (lduw_le_p(rx + 28) & 0x10)) {
        return false;
    }
    uint32_t rx_destination = ldl_le_p(rx + 16);
    int16_t rx_doff = (int16_t)lduw_le_p(rx + 20);
    uint8_t rx_byte = 0; /* OLED serial interface has no useful response. */
    for (unsigned i = 0; i < rx_count; i++) {
        if (address_space_write(as, rx_destination, MEMTXATTRS_UNSPECIFIED,
                                &rx_byte, 1) != MEMTX_OK) {
            return false;
        }
        rx_destination += rx_doff;
    }
    stl_le_p(rx + 16, rx_destination + (int32_t)ldl_le_p(rx + 24));
    stw_le_p(rx + 22, 0);
    uint16_t rx_csr = lduw_le_p(rx + 28);
    stw_le_p(rx + 28, rx_csr | 0x80);
    /* Update TCD storage directly, avoiding a recursive peripheral write. */
    for (unsigned offset = 0; offset < 32; offset += 4) {
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER(((0x0e9040 + offset) >> 2) + 1),
            GUINT_TO_POINTER(ldl_le_p(tx + offset)));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER(((0x0e9060 + offset) >> 2) + 1),
            GUINT_TO_POINTER(ldl_le_p(rx + offset)));
    }
    g_hash_table_insert(s->registers,
        GUINT_TO_POINTER((0x3a0060 >> 2) + 1), GUINT_TO_POINTER(tcr));
    /* Consume the bus bytes, not a guessed framebuffer address. Transfers
     * are presently instantaneous on the virtual clock; serial baud timing
     * can be introduced without changing this publication boundary. */
    l6_display_bytes(&s->display, s->display_dc, wire, wire_size);
    l6_display_flush(&s->display);
    s->edma_erq &= ~0xc;
    if (rx_csr & 2) {
        gpointer key = GUINT_TO_POINTER((0x0e8024 >> 2) + 1);
        uint32_t pending = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));
        g_hash_table_insert(s->registers, key,
                            GUINT_TO_POINTER(pending | (1u << 3)));
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 3));
    }
    return true;
}

static void l6_rtc_store(L6ChipState *s);

static void l6_mmio_write(void *opaque, hwaddr offset,
                          uint64_t value, unsigned size)
{
    L6ChipState *s = opaque;
    gpointer key = GUINT_TO_POINTER((offset >> 2) + 1);
    uint32_t word = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));
    uint32_t previous_word = word;
    unsigned shift = (offset & 3) * 8;
    uint32_t mask = size == 4 ? UINT32_MAX : ((1u << (size * 8)) - 1) << shift;
    word = (word & ~mask) | (((uint32_t)value << shift) & mask);
    if (!s->panel && offset == 0x1bc018 && size == 4) {
        word = previous_word & ~(uint32_t)value; /* GPIO ISR is W1C */
    }
    if (s->panel && offset >= 0x10000000 && offset < 0x10001800) {
        hwaddr base = offset & ~0x3ff;
        unsigned reg = offset & 0x3ff;
        if (reg >= 0x14 && reg < 0x1c) {
            l6_indicators_sample(&s->indicators);
            uint32_t old = l6_panel_reg(s, base + 0x14);
            uint32_t next = old;
            if ((reg & ~3u) == 0x18) {
                uint32_t bits = (uint32_t)value << shift;
                next = (old & ~(bits >> 16)) | (bits & 0xffff);
                word = 0; /* BSRR is a command register, not output storage. */
            } else {
                next = word & 0xffff;
            }
            l6_panel_store(s, base + 0x14, next);
            if (base == 0x10000000 || base == 0x10000400) {
                l6_indicators_pins(&s->indicators,
                    l6_panel_reg(s, 0x10000014),
                    l6_panel_reg(s, 0x10000414));
            }
            l6_indicators_sample(&s->indicators);
            if ((reg & ~3u) == 0x14) {
                word = next;
            }
        }
    }
    /* FlexSPI INTR is write-one-to-clear. TX watermark acknowledgment
     * consumes the FIFO window; the last window completes page programming. */
    if (offset == 0x2a8014 && size == 4) {
        word = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));
        word &= ~(uint32_t)value;
        if ((value & (1u << 6)) && s->nor_program) {
            unsigned count = MIN(8, s->nor_program_size - s->nor_program_cursor);
            for (unsigned i = 0; i < count; i++) {
                uint32_t data = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                    GUINT_TO_POINTER(((0x2a8180 + (i / 4) * 4) >> 2) + 1)));
                s->nor_flash[s->nor_program_address + s->nor_program_cursor + i] &=
                    data >> ((i & 3) * 8);
            }
            s->nor_program_cursor += count;
            if (s->nor_program_cursor == s->nor_program_size) {
                s->nor_program = false;
                s->nor_write_enable = false;
                word |= 1;
                if (g_getenv("L6_STATE_DIR")) { l6_storage_flush(s->nor_flash, 0x200000); }
                qemu_log_mask(LOG_UNIMP, "l6 NOR program address=%06x size=%u\n",
                              s->nor_program_address, s->nor_program_size);
            } else {
                word |= 1u << 6;
            }
        }
        if (value & (1u << 5) && s->flexspi_rx_size) {
            s->flexspi_rx_cursor = MIN(s->flexspi_rx_cursor + 8,
                                       s->flexspi_rx_size);
            l6_flexspi_rx_window(s);
            if (s->flexspi_rx_size - s->flexspi_rx_cursor >= 8) {
                word |= 1u << 5;
            }
        }
    }
    if (offset == 0x2a80b0 && size == 4 && (value & 1)) {
        gpointer intr_key = GUINT_TO_POINTER((0x2a8014 >> 2) + 1);
        uint32_t ipcr0 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                              GUINT_TO_POINTER((0x2a80a0 >> 2) + 1)));
        uint32_t ipcr1 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                              GUINT_TO_POINTER((0x2a80a4 >> 2) + 1)));
        unsigned seq = (ipcr1 >> 16) & 0xf;
        uint32_t lut0 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                              GUINT_TO_POINTER(((0x2a8200 + seq * 16) >> 2) + 1)));
        uint32_t intr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                              intr_key));
        if (s->flexspi_log_count++ < 4096) {
            qemu_log_mask(LOG_UNIMP, "l6 FlexSPI IP command: addr=%08x "
                          "ipcr1=%08x seq=%u lut0=%08x\n",
                          ipcr0, ipcr1, seq, lut0);
        }
        /* LUT sequence 5 in this image is CMD_SDR 0x05, READ_SDR 4.
         * Return an idle NOR status byte (WIP=0) in one 8-byte FIFO entry.
         */
        if (lut0 == 0x24040405 && (ipcr1 & 0xffff) == 1) {
            s->flexspi_rx_size = 1;
            s->flexspi_rx_cursor = 0;
            s->flexspi_rx[0] = s->nor_write_enable ? 2 : 0;
            l6_flexspi_rx_window(s);
        }
        /* Sequence 0 starts with 0x0b fast-read and a 24-bit address.
         * Present long reads through successive eight-byte FIFO windows.
         */
        unsigned read_size = ipcr1 & 0xffff;
        if (lut0 == 0x0818040b && read_size &&
            read_size <= sizeof(s->flexspi_rx)) {
            s->flexspi_rx_size = read_size;
            s->flexspi_rx_cursor = 0;
            for (unsigned i = 0; i < read_size; i++) {
                uint64_t address = (uint64_t)ipcr0 + i;
                s->flexspi_rx[i] = address < 0x200000 ?
                                    s->nor_flash[address] : 0xff;
            }
            if (ipcr0 == 0x1f6ffc && read_size == 4) {
                qemu_log_mask(LOG_UNIMP,
                              "l6 panel package version %02x %02x %02x %02x\n",
                              s->flexspi_rx[0], s->flexspi_rx[1],
                              s->flexspi_rx[2], s->flexspi_rx[3]);
            }
            l6_flexspi_rx_window(s);
            if (read_size >= 8) {
                intr |= 1u << 5;
            }
        }
        unsigned command = lut0 & 0xff;
        if (command == 0x06) {
            s->nor_write_enable = true;
        } else if ((command == 0x20 || command == 0xd8) && s->nor_write_enable) {
            unsigned length = command == 0x20 ? 0x1000 : 0x10000;
            unsigned address = ipcr0 & ~(length - 1);
            if (address <= 0x200000 - length) {
                memset(s->nor_flash + address, 0xff, length);
                qemu_log_mask(LOG_UNIMP, "l6 NOR erase address=%06x size=%u\n", address, length);
                if (g_getenv("L6_STATE_DIR")) { l6_storage_flush(s->nor_flash, 0x200000); }
            }
            s->nor_write_enable = false;
        } else if (command == 0x02 && s->nor_write_enable && read_size && read_size <= 256 &&
                   ipcr0 <= 0x200000 - read_size && (ipcr0 & 255) + read_size <= 256) {
            s->nor_program = true;
            s->nor_program_address = ipcr0;
            s->nor_program_size = read_size;
            s->nor_program_cursor = 0;
            intr |= 1u << 6; /* TX FIFO watermark */
        }
        if (s->nor_program) {
            intr &= ~1u; /* Completion follows the data phase. */
        } else {
            intr |= 1u;
        }
        g_hash_table_insert(s->registers, intr_key, GUINT_TO_POINTER(intr));
    }
    if (offset == 0x2a80b8 && size == 4 && (value & 1)) {
        s->flexspi_rx_size = 0;
        s->flexspi_rx_cursor = 0;
        l6_flexspi_rx_window(s);
        word &= ~1u;
    }
    /* RT105x FlexSPI MCR0.SWRESET completes in hardware. The firmware polls
     * for bit 0 to clear before configuring the rest of the controller.
     */
    if (offset == 0x2a8000 && size == 4) {
        word &= ~1u;
    }
    /* GPT1 CR.SWR is another self-clearing reset request. */
    if (offset == 0x1ec000 && size == 4) {
        word &= ~(1u << 15);
    }
    if (!s->panel && offset == 0x2e0140 && size == 4) {
        /* The post-setup reset request completes before its polling loop. */
        word &= ~2u;
    }
    if (!s->panel && offset == 0x2e01b4 && size == 4) {
        /* Status bits in the same block are cleared by writing ones. */
        word = previous_word & ~(uint32_t)value;
    }
    if (s->panel && offset == 0x280c && size == 4) {
        if (word & (1u << 7)) {
            word |= 1u << 6;
        } else {
            word &= ~(1u << 6);
            if (previous_word & (1u << 7)) {
                word |= 1u << 4; /* Calendar initialized. */
            }
            word |= 1u << 5; /* Shadow registers synchronized. */
        }
    }
    if (offset == 0x3a0014 && size == 4) {
        uint32_t old = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));
        word = old & ~(uint32_t)value;
    }
    if (!s->panel && offset == 0x3f0014 && size == 4) {
        /* LPI2C1 MSR event/error flags are write-one-to-clear. Treating the
         * firmware's 0x7f00 clear as stored data invents PLTF (bit 13) and
         * makes every startup transaction fail with a pin-low timeout.
         */
        word = previous_word & ~((uint32_t)value & 0x7f00u);
    }
    if (!s->panel && offset == 0x3f0060 && (size == 2 || size == 4)) {
        /* MTDR command 2 emits STOP. With an empty TX FIFO, hardware then
         * reports both SDF and TDF in MSR; the synchronous SDK transfer
         * waits for precisely that pair before returning.
         */
        if ((((uint32_t)value >> 8) & 7u) == 2u) {
            gpointer msr_key = GUINT_TO_POINTER((0x3f0014 >> 2) + 1);
            uint32_t msr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                                 msr_key));
            g_hash_table_insert(s->registers, msr_key,
                                GUINT_TO_POINTER(msr | (1u << 9)));
        }
    }
    if (!s->panel && offset == 0x0d8014 && size == 4 &&
        (value & 0x3000u)) {
        /* The post-setup power-control sequence waits for status bit 31
         * after requesting the transition through this control register. */
        gpointer status_key = GUINT_TO_POINTER((0x0d8010 >> 2) + 1);
        uint32_t status = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                                status_key));
        g_hash_table_insert(s->registers, status_key,
                            GUINT_TO_POINTER(status | (1u << 31)));
    }
    if (offset == 0x0e8024 && size == 4) {
        /* eDMA INT is write-one-to-clear. */
        uint32_t old = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));
        word = old & ~(uint32_t)value;
    }
    if (s->panel && offset == 0 && size == 4) {
        if ((word & 1) && !(GPOINTER_TO_UINT(g_hash_table_lookup(
                s->registers, key)) & 1)) {
            timer_mod(s->panel_tim2,
                      qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + l6_panel_tim2_period(s));
        } else if (!(word & 1)) {
            timer_del(s->panel_tim2);
        }
    }
    g_hash_table_insert(s->registers, key, GUINT_TO_POINTER(word));
    if (s->panel && ((offset >= 0x2800 && offset < 0x2860) || offset == 0x2105c)) {
        if (offset == 0x2800 || offset == 0x2804 || offset == 0x280c) {
            s->rtc_anchor_ms = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);
        }
        l6_rtc_store(s);
        qemu_log_mask(LOG_UNIMP, "l6 RTC write offset=%04x value=%08x\n",
                      (unsigned)offset, word);
    }
    if (s->panel && offset == 0x20004) {
        l6_panel_store(s, 0x20000, l6_panel_reg(s, 0x20000) & ~(word | ((word & 1) ? 15 : 0)));
    }
    if (s->panel && offset == 0x20008 && (word & 1) && !(previous_word & 1)) {
        /* SPI1 baud divider is encoded in CR1[5:3], on the 48 MHz bus. */
        unsigned divider = 2u << ((l6_panel_reg(s, 0x13000) >> 3) & 7);
        uint64_t bits = l6_panel_reg(s, 0x2000c) * 8ULL;
        timer_mod(s->panel_spi_dma, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  MAX(bits * divider * 1000000000ULL / 48000000, 1));
    }
    if (s->panel && (offset == 0x400 || offset == 0x428 || offset == 0x42c)) {
        if (l6_panel_reg(s, 0x400) & 1) {
            timer_mod(s->panel_tim3, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                      l6_panel_tim3_period(s));
        } else {
            timer_del(s->panel_tim3);
        }
    }
    if (s->panel && (offset == 0x2000 || offset == 0x2028 || offset == 0x202c)) {
        if (l6_panel_reg(s, 0x2000) & 1) {
            timer_mod(s->panel_tim14, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                      l6_panel_tim14_period(s));
        } else {
            timer_del(s->panel_tim14);
        }
    }
    if (!s->panel && (offset == 0x1bc084 || offset == 0x1bc088) &&
        (value & (1u << 27))) {
        s->display_dc = offset == 0x1bc084;
    }
    if (!s->panel && offset == 0x1bc000 && size == 4) {
        s->display_dc = word & (1u << 27);
    }
    if (s->panel && (offset == 0x28 || offset == 0x2c)) {
        uint32_t cr1 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                         GUINT_TO_POINTER(1)));
        if ((cr1 & 1) && !s->cpu_held) {
            timer_mod(s->panel_tim2,
                      qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + l6_panel_tim2_period(s));
        }
    }
    if (!s->panel) {
        static const hwaddr ports[5] = { 0x1b8000, 0x1bc000, 0x1c0000, 0x1c4000, 0x0c0000 };
        for (unsigned bank = 0; bank < ARRAY_SIZE(ports); bank++) {
            hwaddr reg = offset - ports[bank];
            if (size != 4 || (reg != 0 && reg != 0x84 && reg != 0x88 && reg != 0x8c)) {
                continue;
            }
            uint32_t before = s->main_gpio[bank];
            uint32_t after = reg == 0 ? word : reg == 0x84 ? before | word :
                reg == 0x88 ? before & ~word : before ^ word;
            s->main_gpio[bank] = after;
            g_hash_table_insert(s->registers, GUINT_TO_POINTER((ports[bank] >> 2) + 1), GUINT_TO_POINTER(after));
            if (after != before) {
                if (!++s->main_gpio_generation) { ++s->main_gpio_generation; }
                l6_input_reply(s, 0x80000006u, s->main_gpio_generation, bank, after);
                /* System power event routine 0x80037328 finishes by clearing
                 * output 1: callback 0x80021ea8 drives GPIO3.3 (power hold).
                 * This falling edge occurs after storage and panel teardown.
                 */
                if (bank == 2 && (before & 8u) && !(after & 8u)) {
                    qemu_log_mask(LOG_UNIMP, "l6 board power hold released at %" PRId64 " ms\n",
                                  qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
                    l6_input_reply(s, 0x80000009u, s->main_gpio_generation, 0, 0);
                    qemu_system_shutdown_request(SHUTDOWN_CAUSE_GUEST_SHUTDOWN);
                }
            }
            break;
        }
    }
    if (!s->panel && offset >= 0x1b4000 && offset < 0x1c4000 &&
        s->main_gpio_log_count++ < 180) {
        qemu_log_mask(LOG_UNIMP, "l6 main GPIO %08x = %08x\n",
                      (unsigned)(0x40000000 + offset), (uint32_t)value);
    }
    if (!s->panel && (offset == 0x1b8084 || offset == 0x1b8088 ||
                      offset == 0x0c0084 || offset == 0x0c0088)) {
        qemu_log_mask(LOG_UNIMP, "l6 sub-MCU pin %08x = %08x\n",
                      (unsigned)(0x40000000 + offset), (uint32_t)value);
    }
    if (!s->panel && offset == 0x1b8084 && (value & (1u << 29))) {
        s->main_boot_select = true;
    }
    if (!s->panel && offset == 0x1b8088 && (value & (1u << 29))) {
        s->main_boot_select = false;
    }
    if (!s->panel && offset == 0x0c0084 && (value & 2)) {
        /* Recovered updater pulses this GPIO high, then low, and starts the
         * ROM exchange after the low edge. Model the high phase as reset;
         * the wiring's physical RESET polarity is not yet established. */
        s->panel_reset_asserted = true;
        s->main_rom_mode = false;
        l6_update_panel_boot(s);
    }
    if (!s->panel && offset == 0x0c0088 && (value & 2)) {
        s->panel_reset_asserted = false;
        s->main_rom_mode = s->main_boot_select;
        s->main_rx_pending = false;
        s->rom_phase = 0;
        s->rom_reply_read = s->rom_reply_visible = s->rom_reply_write = 0;
        timer_del(s->rom_uart_timer);
        qemu_irq_lower(qdev_get_gpio_in(DEVICE(&s->armv7m), 20));
        if (s->main_rom_mode) {
            qemu_log_mask(LOG_UNIMP, "l6 main panel reset into ROM mode\n");
        }
        l6_update_panel_boot(s);
    }
    if (s->panel && offset == 0x013800 && size == 4 &&
        (word & 0x80) && !(previous_word & 0x80)) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 27));
    }
    if (s->panel && offset == 0x013800 && size == 4 &&
        (word & (1u << 5)) && !(previous_word & (1u << 5)) &&
        s->panel_rx_pending) {
        /* A byte can arrive while the real panel CPU is still booting.
         * Enabling RXNEIE must expose its already pending RXNE condition. */
        qemu_log_mask(LOG_UNIMP,
                      "l6 panel USART1 enabling RXNEIE with pending byte %02x\n",
                      s->panel_rx_byte);
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 27));
    }
    if (s->panel && offset == 0x013828 && size == 4) {
        uint8_t byte = value;
        s->panel_tx_count++;
        qemu_log_mask(LOG_UNIMP, "l6 panel USART1 TX %02x\n", byte);
        if (byte == 0xb0) {
            qemu_log_mask(LOG_UNIMP, "l6 panel RTC response start at %" PRId64 " ms\n",
                          qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        }
        if (s->uart_link) {
            l6_uart_link_send(s, byte);
        } else {
            qemu_chr_fe_write(&s->panel_uart, &byte, 1);
        }
        uint32_t cr1 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((0x013800 >> 2) + 1)));
        if (cr1 & 0x80) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 27));
        }
    }
    if (!s->panel && offset == 0x184018 && size == 4) {
        if (s->main_ctrl_log_count++ < 40) {
            qemu_log_mask(LOG_UNIMP, "l6 main LPUART1 CTRL %08x\n", word);
        }
        if ((word & (1u << 23)) && !(previous_word & (1u << 23))) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 20));
        }
        if (s->main_rx_pending && (word & (1u << 21)) &&
            !(previous_word & (1u << 21))) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 20));
        }
        if (s->main_rom_mode && s->rom_reply_read < s->rom_reply_visible) {
            qemu_set_irq(qdev_get_gpio_in(DEVICE(&s->armv7m), 20),
                         !!(word & (1u << 21)));
        }
    }
    if (!s->panel && offset == 0x18401c && size == 4) {
        uint8_t byte = value;
        s->main_tx_count++;
        qemu_log_mask(LOG_UNIMP, "l6 main LPUART1 TX %02x\n", byte);
        if (byte == 0xb1) {
            qemu_log_mask(LOG_UNIMP, "l6 main RTC request at %" PRId64 " ms\n",
                          qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        }
        if (s->main_rom_mode) {
            l6_rom_write_byte(s, byte);
        } else if (s->uart_link) {
            l6_uart_link_send(s, byte);
        } else {
            qemu_chr_fe_write(&s->main_uart, &byte, 1);
        }
        uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((0x184018 >> 2) + 1)));
        if (ctrl & (1u << 23)) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 20));
        }
    }
    if (offset >= 0x3a0000 && offset < 0x3a0100 &&
        s->lpspi_log_count++ < 128) {
        qemu_log_mask(LOG_UNIMP, "l6 LPSPI4 MMIO +%02x = %08x\n",
                      (unsigned)(offset - 0x3a0000), (uint32_t)value);
    }
    if (offset >= 0x0e8000 && offset < 0x0f0000 &&
        s->edma_log_count++ < 256) {
        qemu_log_mask(LOG_UNIMP, "l6 eDMA MMIO +%04x = %08x (%u bytes)\n",
                      (unsigned)(offset - 0x0e8000), (uint32_t)value, size);
    }
    if (offset == 0x0e801b && size == 1 && value < 32) {
        s->edma_erq |= 1u << value;
    }
    if (offset == 0x0e801f && size == 1 && value < 32) {
        gpointer int_key = GUINT_TO_POINTER((0x0e8024 >> 2) + 1);
        uint32_t pending = GPOINTER_TO_UINT(g_hash_table_lookup(
            s->registers, int_key));
        g_hash_table_insert(s->registers, int_key,
                            GUINT_TO_POINTER(pending & ~(1u << value)));
    }
    if (!s->panel && offset == 0x3a001c && size == 4 && (word & 3) == 3 &&
        (s->edma_erq & 0xc) == 0xc) {
        if (!l6_display_dma_transfer(s)) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 unsupported LPSPI4/eDMA transfer, completion withheld\n");
        }
    }
    if (offset == 0x3a0018 && size == 4 && (word & 1)) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 35));
    }
    if (offset == 0x3a0064 && size == 4) {
        uint32_t ier = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((0x3a0018 >> 2) + 1)));
        if (ier & 1) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 35));
        }
    }
    if (offset == 0x0c4000 && size == 4 &&
        (value & 0x80) && ((value & 0x1f) != 0x1f)) {
        /* ADC1 HC0 starts a conversion. Physical channels 8 and 13 are the
         * board-ID ladder inputs; low samples select the firmware's ID 6.
         * Five physical potentiometers use channels 3..7, with 10-bit results.
         */
        uint32_t channel = value & 0x1f;
        s->adc_channel = channel;
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x0c4020 >> 2) + 1), GUINT_TO_POINTER(1));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x0c4024 >> 2) + 1),
            GUINT_TO_POINTER(channel >= 3 && channel <= 7 ? s->analog_inputs[channel - 3] :
                channel == 8 || channel == 13 ? 0 : 0x800));
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 67));
    }
}

static const MemoryRegionOps l6_mmio_ops = {
    .read = l6_mmio_read,
    .write = l6_mmio_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid.min_access_size = 1,
    .valid.max_access_size = 4,
};

static void l6_make_ram(MemoryRegion *root, MemoryRegion *region,
                        const char *name, hwaddr base, uint64_t size)
{
    memory_region_init_ram(region, NULL, name, size, &error_fatal);
    memory_region_add_subregion(root, base, region);
}

/* These files are device storage, not host/guest communication. Keep the fd
 * open for its exclusive lock for the lifetime of the QEMU process. */
static uint8_t *l6_persistent_bytes(const char *name, uint8_t *seed, size_t size)
{
    const char *directory = g_getenv("L6_STATE_DIR");
    if (!directory) { return NULL; }
    g_autofree char *path = g_build_filename(directory, name, NULL);
    int fd = open(path, O_RDWR | O_CREAT, 0600);
    struct stat st;
    if (fd < 0 || flock(fd, LOCK_EX | LOCK_NB) || fstat(fd, &st)) {
        error_setg(&error_fatal, "cannot lock device storage %s: %s", path, strerror(errno));
    }
    if (!st.st_size) {
        if (ftruncate(fd, size)) {
            error_setg(&error_fatal, "cannot initialize device storage %s", path);
        }
        size_t done = 0;
        while (done < size) {
            ssize_t count = pwrite(fd, seed + done, size - done, done);
            if (count < 0 && errno == EINTR) { continue; }
            if (count <= 0) {
                error_setg(&error_fatal, "cannot initialize device storage %s", path);
            }
            done += count;
        }
        if (fsync(fd)) {
            error_setg(&error_fatal, "cannot flush initial device storage %s", path);
        }
    } else if (st.st_size != size) {
        error_setg(&error_fatal, "device storage %s must contain %zu bytes", path, size);
    }
    uint8_t *bytes = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (bytes == MAP_FAILED) {
        error_setg(&error_fatal, "cannot map device storage %s", path);
    }
    return bytes;
}

static void l6_rtc_store(L6ChipState *s)
{
    if (!s->persistent_rtc) { return; }
    for (unsigned offset = 0; offset < 0x60; offset += 4) {
        uint32_t word = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER(((0x2800 + offset) >> 2) + 1)));
        stl_le_p(s->persistent_rtc + offset, word);
    }
    stl_le_p(s->persistent_rtc + 0x80, GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x2105c >> 2) + 1))));
    l6_storage_flush(s->persistent_rtc, 0x100);
}

static unsigned l6_bcd(unsigned value) { return (value / 10) * 16 + value % 10; }
static unsigned l6_unbcd(unsigned value) { return (value >> 4) * 10 + (value & 15); }
static void l6_rtc_tick(L6ChipState *s)
{
    int64_t now = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);
    uint32_t icsr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x280c >> 2) + 1)));
    uint32_t bdcr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x2105c >> 2) + 1)));
    if ((icsr & 0x80) || !(bdcr & 0x8000)) { s->rtc_anchor_ms = now; return; }
    int64_t seconds = (now - s->rtc_anchor_ms) / 1000;
    if (seconds <= 0) { return; }
    uint32_t tr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x2800 >> 2) + 1)));
    uint32_t dr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((0x2804 >> 2) + 1)));
    g_autoptr(GDateTime) date = g_date_time_new_utc(2000 + l6_unbcd((dr >> 16) & 255),
        l6_unbcd((dr >> 8) & 31), l6_unbcd(dr & 63), l6_unbcd((tr >> 16) & 63),
        l6_unbcd((tr >> 8) & 127), l6_unbcd(tr & 127));
    if (!date) { s->rtc_anchor_ms = now; return; }
    g_autoptr(GDateTime) advanced = g_date_time_add_seconds(date, seconds);
    tr = (l6_bcd(g_date_time_get_hour(advanced)) << 16) |
         (l6_bcd(g_date_time_get_minute(advanced)) << 8) | l6_bcd(g_date_time_get_second(advanced));
    dr = (l6_bcd(g_date_time_get_year(advanced) % 100) << 16) |
         (g_date_time_get_day_of_week(advanced) << 13) |
         (l6_bcd(g_date_time_get_month(advanced)) << 8) | l6_bcd(g_date_time_get_day_of_month(advanced));
    g_hash_table_insert(s->registers, GUINT_TO_POINTER((0x2800 >> 2) + 1), GUINT_TO_POINTER(tr));
    g_hash_table_insert(s->registers, GUINT_TO_POINTER((0x2804 >> 2) + 1), GUINT_TO_POINTER(dr));
    s->rtc_anchor_ms += seconds * 1000;
    l6_rtc_store(s);
}

static void l6_load_nor_segment(L6ChipState *s, const char *directory,
                                const char *name, size_t offset, size_t capacity)
{
    g_autofree char *path = g_build_filename(directory, name, NULL);
    g_autofree char *contents = NULL;
    gsize size = 0;
    if (!g_file_get_contents(path, &contents, &size, NULL) ||
        size > capacity || offset + size > 0x200000) {
        error_setg(&error_fatal, "cannot load NOR segment %s", path);
    }
    memcpy(s->nor_flash + offset, contents, size);
}

static void l6_prepare_nor(L6ChipState *s, const char *kernel)
{
    g_autofree char *directory = g_path_get_dirname(kernel);
    s->nor_flash = g_malloc(0x200000);
    memset(s->nor_flash, 0xff, 0x200000);
    g_autofree char *kernel_name = g_path_get_basename(kernel);
    l6_load_nor_segment(s, directory, kernel_name, 0x50000, 0x1a2ff8);
    l6_load_nor_segment(s, directory, "main_trailer.bin", 0x1f2ff8, 8);
    l6_load_nor_segment(s, directory, "secondary_firmware.bin", 0x1f3000, 0x3ff8);
    l6_load_nor_segment(s, directory, "secondary_trailer.bin", 0x1f6ff8, 8);
    /* Installed application version is a boot-data record, separate from
     * the update trailer. Its initial value comes from the supplied package. */
    memcpy(s->nor_flash + 0x1f7ffc, s->nor_flash + 0x1f2ffc, 4);
    uint8_t *persistent = l6_persistent_bytes("main-nor.bin", s->nor_flash, 0x200000);
    if (persistent) {
        g_free(s->nor_flash);
        s->nor_flash = persistent;
    }
}

static void l6_chip_init(MachineState *machine, L6ChipState *s, bool panel,
                         MemoryRegion *system_memory, const char *kernel_filename,
                         const char *cpu_type, const char *prefix,
                         const char *input_variable, bool external_serial)
{
    hwaddr flash_base = panel ? 0x08000000 : 0x80000000;
    uint64_t flash_size = panel ? 0x10000 : 0x200000;
    DeviceState *cpu;

    g_autofree char *flash_name = g_strdup_printf("%s.flash", prefix);
    g_autofree char *boot_alias_name = g_strdup_printf("%s.boot_alias", prefix);
    g_autofree char *itcm_name = g_strdup_printf("%s.itcm", prefix);
    g_autofree char *sram_name = g_strdup_printf("%s.sram", prefix);
    g_autofree char *dtcm_name = g_strdup_printf("%s.dtcm", prefix);
    g_autofree char *external_ram_name = g_strdup_printf("%s.external_ram", prefix);
    g_autofree char *unmodeled_mmio_name = g_strdup_printf("%s.unmodeled_mmio", prefix);
    s->registers = g_hash_table_new(g_direct_hash, g_direct_equal);
    s->panel = panel;
    s->input_trace = g_getenv("L6_INPUT_TRACE") != NULL;
    s->input_fd = -1;
    for (unsigned i = 0; i < 5; i++) { s->analog_inputs[i] = 512; }
    memset(s->encoder_phase, 3, sizeof(s->encoder_phase));
    const char *input_fd = g_getenv(input_variable);
    if (input_fd) {
        char *end;
        long fd = strtol(input_fd, &end, 10);
        if (!*input_fd || *end || fd < 0 || fd > INT_MAX ||
            fcntl(fd, F_GETFD) < 0) {
            error_setg(&error_fatal, "L6_INPUT_FD must name an inherited socket");
        }
        int flags = fcntl(fd, F_GETFL);
        if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) {
            error_setg_errno(&error_fatal, errno, "cannot configure input socket");
        }
        s->input_fd = fd;
        s->input_tx = g_byte_array_new();
        l6_input_handlers(s);
    }
    s->input_timer = timer_new_ns(QEMU_CLOCK_VIRTUAL,
                                 l6_input_timer_tick, s);
    timer_mod(s->input_timer,
              qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 1000000);
    int display_fd = -1;
    const char *display_fd_env = panel ? NULL : g_getenv("L6_DISPLAY_FD");
    if (display_fd_env) {
        char *end;
        long fd = strtol(display_fd_env, &end, 10);
        if (!*display_fd_env || *end || fd < 0 || fd > INT_MAX ||
            s->input_fd < 0) {
            error_setg(&error_fatal,
                       "L6_DISPLAY_FD requires a valid inherited descriptor and input socket");
        }
        display_fd = fd;
    }
    l6_display_init(&s->display, display_fd, l6_display_frame_ready,
                    s, &error_fatal);
    if (panel) {
        s->panel_tim3 = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_panel_tim3_tick, s);
        s->panel_tim14 = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_panel_tim14_tick, s);
        s->panel_spi_dma = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_panel_spi_dma_tick, s);
        s->indicator_timer = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_indicator_tick, s);
        timer_mod(s->indicator_timer, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 50000000);
        const char *keys = g_getenv("L6_PANEL_KEYS");
        if (keys) {
            g_auto(GStrv) entries = g_strsplit(keys, ",", -1);
            for (unsigned i = 0; entries[i]; i++) {
                unsigned row = 0, column = 0, start_ms = 0, end_ms = 0;
                int consumed = 0;
                if (i >= L6_MAX_PANEL_KEYS ||
                    sscanf(entries[i], "%u:%u:%u:%u%n", &row, &column,
                           &start_ms, &end_ms, &consumed) != 4 ||
                    entries[i][consumed] || row >= 8 || column >= 5 ||
                    start_ms >= end_ms) {
                    error_setg(&error_fatal,
                               "L6_PANEL_KEYS expects comma-separated row:column:start_ms:end_ms entries (maximum %d)",
                               L6_MAX_PANEL_KEYS);
                }
                s->panel_keys[i] = (L6PanelKey) { row, column, start_ms, end_ms };
                s->panel_key_count++;
            }
        }
        /* Seed a deterministic calendar in the STM32 RTC. The firmware
         * treats CR.BKP as its calendar-valid marker before reading TR/DR.
         * 2026-09-30 (Wednesday), 00:00:00, in RTC BCD register format.
         */
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x2800 >> 2) + 1), GUINT_TO_POINTER(0));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x2804 >> 2) + 1), GUINT_TO_POINTER(0x00266930));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x2818 >> 2) + 1), GUINT_TO_POINTER(0));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x280c >> 2) + 1),
            GUINT_TO_POINTER((1u << 4) | (1u << 5)));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x2810 >> 2) + 1), GUINT_TO_POINTER(0x007f00ff));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x2814 >> 2) + 1), GUINT_TO_POINTER(0xffff));
        uint8_t rtc_seed[0x100] = {0};
        for (unsigned offset = 0; offset < 0x60; offset += 4) {
            stl_le_p(rtc_seed + offset, GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                GUINT_TO_POINTER(((0x2800 + offset) >> 2) + 1))));
        }
        s->persistent_rtc = l6_persistent_bytes("panel-rtc.bin", rtc_seed, sizeof(rtc_seed));
        if (s->persistent_rtc) {
            for (unsigned offset = 0; offset < 0x60; offset += 4) {
                g_hash_table_insert(s->registers,
                    GUINT_TO_POINTER(((0x2800 + offset) >> 2) + 1),
                    GUINT_TO_POINTER(ldl_le_p(s->persistent_rtc + offset)));
            }
            g_hash_table_insert(s->registers, GUINT_TO_POINTER((0x2105c >> 2) + 1),
                GUINT_TO_POINTER(ldl_le_p(s->persistent_rtc + 0x80)));
        }
    }
    if (!panel) {
        s->service_audio_queue = g_strcmp0(g_getenv("L6_SERVICE_AUDIO_QUEUE"),
                                             "1") == 0;
        const char *keys = g_getenv("L6_MAIN_KEYS");
        if (keys) {
            g_auto(GStrv) entries = g_strsplit(keys, ",", -1);
            for (unsigned i = 0; entries[i]; i++) {
                unsigned id = 0, start_ms = 0, end_ms = 0;
                int consumed = 0;
                if (i >= L6_MAX_PANEL_KEYS ||
                    sscanf(entries[i], "%u:%u:%u%n", &id, &start_ms,
                           &end_ms, &consumed) != 3 ||
                    entries[i][consumed] || id > 2 || start_ms >= end_ms) {
                    error_setg(&error_fatal,
                               "L6_MAIN_KEYS expects comma-separated id:start_ms:end_ms entries (id 0=Menu, 1=Play, 2=Record)");
                }
                s->main_keys[i] = (L6MainKey) { id, start_ms, end_ms };
                s->main_key_count++;
            }
        }
    }
    memset(s->rom_flash, 0xff, sizeof(s->rom_flash));
    memset(s->rom_option, 0xff, sizeof(s->rom_option));
    if (!panel) {
        s->persistent_options = l6_persistent_bytes("panel-options.bin", s->rom_option, 4);
        if (s->persistent_options) { memcpy(s->rom_option, s->persistent_options, 4); }
    }
    if (panel) {
        s->panel_tim2 = timer_new_ns(QEMU_CLOCK_VIRTUAL,
                                     l6_panel_tim2_tick, s);
        Chardev *serial = external_serial ? serial_hd(0) : NULL;
        if (serial) {
            qemu_chr_fe_init(&s->panel_uart, serial, &error_fatal);
            qemu_chr_fe_set_handlers(&s->panel_uart,
                                     l6_panel_uart_can_receive,
                                     l6_panel_uart_receive,
                                     NULL, NULL, s, NULL, true);
        }
    } else {
        s->rom_uart_timer = timer_new_ns(QEMU_CLOCK_VIRTUAL,
                                         l6_rom_uart_tick, s);
        s->main_audio_service_timer = timer_new_ns(QEMU_CLOCK_VIRTUAL,
                                                   l6_main_audio_service_tick, s);
        timer_mod(s->main_audio_service_timer,
                  qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 1000000);
        Chardev *serial = external_serial ? serial_hd(0) : NULL;
        if (serial) {
            qemu_chr_fe_init(&s->main_uart, serial, &error_fatal);
            qemu_chr_fe_set_handlers(&s->main_uart,
                                     l6_main_uart_can_receive,
                                     l6_main_uart_receive,
                                     NULL, NULL, s, NULL, true);
        }
    }
    if (kernel_filename) {
        struct stat st;
        if (stat(kernel_filename, &st) == 0) {
            s->flash_payload_size = st.st_size;
        }
    }
    g_autofree char *sysclk_name = g_strdup_printf("%s-SYSCLK", prefix);
    g_autofree char *refclk_name = g_strdup_printf("%s-REFCLK", prefix);
    s->sysclk = clock_new(OBJECT(machine), sysclk_name);
    s->refclk = clock_new(OBJECT(machine), refclk_name);
    clock_set_hz(s->sysclk, panel ? 48000000 : 600000000);
    clock_set_hz(s->refclk, 1000000);
    if (!panel) { l6_prepare_nor(s, kernel_filename); }

    memory_region_init_rom(&s->flash, NULL, flash_name,
                           flash_size, &error_fatal);
    memory_region_add_subregion(system_memory, flash_base, &s->flash);
    if (panel) {
        memory_region_init_alias(&s->flash_alias, NULL,
                                 boot_alias_name, &s->flash, 0, flash_size);
        memory_region_add_subregion(system_memory, 0, &s->flash_alias);
    } else {
        memory_region_init_ram(&s->itcm, NULL, itcm_name, 0x100000,
                               &error_fatal);
        memory_region_add_subregion(system_memory, 0, &s->itcm);
        memcpy(memory_region_get_ram_ptr(&s->itcm), s->nor_flash + 0x50000, 0x100000);
    }

    l6_make_ram(system_memory, &s->sram, sram_name, 0x20000000,
                panel ? 0x10000 : 0x200000);
    if (!panel) {
        l6_make_ram(system_memory, &s->dtcm, dtcm_name, 0x20200000, 0x100000);
        const char *shared_ram_fd = g_getenv("L6_EXTERNAL_RAM_FD");
        if (shared_ram_fd) {
            char *end;
            long fd = strtol(shared_ram_fd, &end, 10);
            struct stat ram_stat;
            int flags;
            if (!*shared_ram_fd || *end || fd < 0 || fd > INT_MAX) {
                error_setg(&error_fatal,
                           "L6_EXTERNAL_RAM_FD must name an inherited RAM descriptor");
            }
            flags = fcntl(fd, F_GETFL);
            if (flags < 0 || (flags & O_ACCMODE) != O_RDWR ||
                fstat(fd, &ram_stat) < 0 || ram_stat.st_size < 0x4000000) {
                error_setg(&error_fatal,
                           "L6_EXTERNAL_RAM_FD must be writable and at least 64 MiB");
            }
            /* QEMU owns this inherited descriptor for the RAM region's life. */
            memory_region_init_ram_from_fd(
                &s->external_ram, NULL, external_ram_name, 0x4000000,
                RAM_SHARED, fd, 0, &error_fatal);
            memory_region_add_subregion(system_memory, 0x80200000,
                                        &s->external_ram);
        } else {
            l6_make_ram(system_memory, &s->external_ram, external_ram_name,
                        0x80200000, 0x4000000);
        }
    }

    memory_region_init_io(&s->peripheral, NULL, &l6_mmio_ops,
                          s, unmodeled_mmio_name, 0x20000000);
    memory_region_add_subregion(system_memory, 0x40000000, &s->peripheral);

    object_initialize_child(OBJECT(machine), prefix, &s->armv7m,
                            TYPE_ARMV7M);
    cpu = DEVICE(&s->armv7m);
    qdev_prop_set_uint32(cpu, "num-irq", panel ? 32 : 160);
    /* The main image probes the implemented NVIC priority bits at startup.
     * Its expected four bits differ from QEMU's generic M7 default of eight.
     */
    qdev_prop_set_uint8(cpu, "num-prio-bits", panel ? 2 : 4);
    if (!panel) {
        qdev_prop_set_uint32(cpu, "mpu-ns-regions", 16);
    }
    qdev_prop_set_string(cpu, "cpu-type", cpu_type);
    qdev_connect_clock_in(cpu, "cpuclk", s->sysclk);
    qdev_connect_clock_in(cpu, "refclk", s->refclk);
    object_property_set_link(OBJECT(&s->armv7m), "memory",
                             OBJECT(system_memory), &error_abort);
    sysbus_realize(SYS_BUS_DEVICE(&s->armv7m), &error_fatal);
    g_autofree char *persistent_kernel = NULL;
    if (panel && g_getenv("L6_STATE_DIR")) {
        g_autofree char *contents = NULL;
        gsize size;
        g_autofree uint8_t *seed = g_malloc0(0x10000);
        memset(seed, 0xff, 0x10000);
        if (!g_file_get_contents(kernel_filename, &contents, &size, NULL) || size > 0x10000) {
            error_setg(&error_fatal, "cannot seed persistent panel flash");
        }
        memcpy(seed, contents, size);
        s->persistent_panel = l6_persistent_bytes("panel-flash.bin", seed, 0x10000);
        persistent_kernel = g_build_filename(g_getenv("L6_STATE_DIR"), "panel-flash.bin", NULL);
        kernel_filename = persistent_kernel;
    }
    armv7m_load_kernel(s->armv7m.cpu, panel ? kernel_filename : NULL,
                       flash_base, flash_size);
    if (!panel) {
        rom_add_blob_fixed_as("l6-main-installed", s->nor_flash + 0x50000,
                             0x1a2ff8, flash_base, CPU(s->armv7m.cpu)->as);
    }
    if (!panel) {
        s->usb.as = CPU(s->armv7m.cpu)->as;
        s->usb.irq = qdev_get_gpio_in(DEVICE(&s->armv7m), 113);
        s->usb.fd = -1;
        s->usb.tx = g_byte_array_new();
        s->usb.sof_timer = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_usb_sof, &s->usb);
        timer_mod(s->usb.sof_timer, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 1000000);
        const char *usb_fd = g_getenv("L6_USB_FD");
        if (usb_fd) {
            char *end;
            long fd = strtol(usb_fd, &end, 10);
            if (!*usb_fd || *end || fd < 0 || fd > INT_MAX ||
                fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK) < 0) {
                error_setg(&error_fatal, "L6_USB_FD must be an inherited socket descriptor");
            }
            s->usb.fd = fd;
            l6_usb_handlers(&s->usb);
        }
        memory_region_init_io(&s->usb_region, NULL, &l6_usb_ops, &s->usb,
                              "l6-chipidea-device", 0x200);
        memory_region_add_subregion_overlap(system_memory, 0x402e0000,
                                            &s->usb_region, 2);
        /* RT1052 USDHC1 overlays the sparse peripheral region. An SD card
         * is present only when the runner supplies an SD drive.
         */
        DeviceState *usdhc = qdev_new(TYPE_IMX_USDHC);
        DriveInfo *sd_drive = drive_get(IF_SD, 0, 0);
        object_property_set_uint(OBJECT(usdhc), "capareg", 0x057834b4,
                                 &error_fatal);
        sysbus_realize_and_unref(SYS_BUS_DEVICE(usdhc), &error_fatal);
        s->sdhc = SYSBUS_SDHCI(usdhc);
        s->sd_ops = s->sdhc->io_ops;
        s->sdhc->pwrcon = 0x0f;
        s->sdhc->clkcon = 3;
        memory_region_init_io(&s->sd_adapter, NULL, &l6_sd_ops, s,
                              "l6-rt1052-usdhc", 0x100);
        memory_region_add_subregion_overlap(system_memory, 0x402c0000,
                                            &s->sd_adapter, 2);
        memory_region_add_subregion_overlap(system_memory, 0x402c0000,
            sysbus_mmio_get_region(SYS_BUS_DEVICE(usdhc), 0), 1);
        sysbus_connect_irq(SYS_BUS_DEVICE(usdhc), 0,
            qdev_get_gpio_in(DEVICE(&s->armv7m), 110));
        if (sd_drive) {
            DeviceState *card = qdev_new(TYPE_SD_CARD);
            qdev_prop_set_drive_err(card, "drive",
                                    blk_by_legacy_dinfo(sd_drive),
                                    &error_fatal);
            qdev_realize_and_unref(card, qdev_get_child_bus(usdhc, "sd-bus"),
                                   &error_fatal);
        }
        s->sd_present = sdbus_get_inserted(&s->sdhc->sdbus);
    }
}

static void l6_main_init(MachineState *machine)
{
    L6ChipState *s = &L6_MACHINE(machine)->chip;
    l6_chip_init(machine, s, false, get_system_memory(),
                 machine->kernel_filename, machine->cpu_type, "main",
                 "L6_INPUT_FD", true);
}

static void l6_panel_init(MachineState *machine)
{
    L6ChipState *s = &L6_MACHINE(machine)->chip;
    l6_chip_init(machine, s, true, get_system_memory(),
                 machine->kernel_filename, machine->cpu_type, "panel",
                 "L6_INPUT_FD", true);
}

static void l6_dual_init(MachineState *machine)
{
    L6DualMachineState *s = L6_DUAL_MACHINE(machine);
    const char *panel_firmware = g_getenv("L6_PANEL_FIRMWARE");
    if (!panel_firmware || !*panel_firmware || !machine->kernel_filename) {
        error_setg(&error_fatal,
                   "l6max-dual requires -kernel main image and L6_PANEL_FIRMWARE");
    }
    /* These MCUs overlap SRAM, peripheral and boot-vector addresses. Keep
     * separate address spaces while sharing QEMU_CLOCK_VIRTUAL and its loop. */
    memory_region_init(&s->main_memory, OBJECT(machine),
                       "l6-main-address-space", UINT64_MAX);
    memory_region_init(&s->panel_memory, OBJECT(machine),
                       "l6-panel-address-space", UINT64_MAX);
    l6_chip_init(machine, &s->main, false, &s->main_memory,
                 machine->kernel_filename, ARM_CPU_TYPE_NAME("cortex-m7"),
                 "main", "L6_MAIN_INPUT_FD", false);
    l6_chip_init(machine, &s->panel, true, &s->panel_memory,
                 panel_firmware, ARM_CPU_TYPE_NAME("cortex-m0"),
                 "panel", "L6_PANEL_INPUT_FD", false);
    s->main.peer = &s->panel;
    s->panel.peer = &s->main;
    qdev_connect_gpio_out_named(DEVICE(&s->main.armv7m), "SYSRESETREQ", 0,
        qemu_allocate_irq(l6_local_reset_request, &s->main, 0));
    qdev_connect_gpio_out_named(DEVICE(&s->panel.armv7m), "SYSRESETREQ", 0,
        qemu_allocate_irq(l6_local_reset_request, &s->panel, 0));
    s->main_to_panel = (L6UartLink) {
        .source = &s->main,
        .destination = &s->panel,
        .queue = g_byte_array_new(),
    };
    s->panel_to_main = (L6UartLink) {
        .source = &s->panel,
        .destination = &s->main,
        .queue = g_byte_array_new(),
    };
    s->main_to_panel.timer = timer_new_ns(QEMU_CLOCK_VIRTUAL,
                                          l6_uart_link_tick, &s->main_to_panel);
    s->panel_to_main.timer = timer_new_ns(QEMU_CLOCK_VIRTUAL,
                                          l6_uart_link_tick, &s->panel_to_main);
    s->main.uart_link = &s->main_to_panel;
    s->panel.uart_link = &s->panel_to_main;
    /* HMP/GDB physical memory commands inspect the main address space; CPUs
     * remain attached to their own roots. The alias does not share MCU SRAM. */
    memory_region_init_alias(&s->debug_alias, OBJECT(machine),
                             "l6-main-debug-view", &s->main_memory,
                             0, UINT64_MAX);
    memory_region_add_subregion(get_system_memory(), 0, &s->debug_alias);
}

static void l6_dual_class_init(ObjectClass *oc, const void *data)
{
    MachineClass *mc = MACHINE_CLASS(oc);
    mc->desc = "L6max dual MCU board with a common virtual clock";
    mc->init = l6_dual_init;
    mc->default_cpu_type = ARM_CPU_TYPE_NAME("cortex-m7");
    mc->default_ram_size = 0x200000;
    mc->min_cpus = 2;
    mc->default_cpus = 2;
    mc->max_cpus = 2;
}

static void l6_main_class_init(ObjectClass *oc, const void *data)
{
    MachineClass *mc = MACHINE_CLASS(oc);
    static const char * const cpus[] = { ARM_CPU_TYPE_NAME("cortex-m7"), NULL };
    mc->desc = "L6max main MCU bring-up board (incomplete i.MX RT model)";
    mc->init = l6_main_init;
    mc->valid_cpu_types = cpus;
    mc->default_cpu_type = cpus[0];
    mc->default_ram_size = 0x200000;
}

static void l6_panel_class_init(ObjectClass *oc, const void *data)
{
    MachineClass *mc = MACHINE_CLASS(oc);
    static const char * const cpus[] = { ARM_CPU_TYPE_NAME("cortex-m0"), NULL };
    mc->desc = "L6max panel MCU bring-up board (Cortex-M0 stand-in for M0+)";
    mc->init = l6_panel_init;
    mc->valid_cpu_types = cpus;
    mc->default_cpu_type = cpus[0];
    mc->default_ram_size = 0x10000;
}

static const TypeInfo l6_base_info = {
    .name = TYPE_L6_MACHINE,
    .parent = TYPE_MACHINE,
    .abstract = true,
    .instance_size = sizeof(L6MachineState),
};

static const TypeInfo l6_main_info = {
    .name = MACHINE_TYPE_NAME("l6max-main"),
    .parent = TYPE_L6_MACHINE,
    .class_init = l6_main_class_init,
    .interfaces = arm_machine_interfaces,
};

static const TypeInfo l6_panel_info = {
    .name = MACHINE_TYPE_NAME("l6max-panel"),
    .parent = TYPE_L6_MACHINE,
    .class_init = l6_panel_class_init,
    .interfaces = arm_machine_interfaces,
};

static const TypeInfo l6_dual_info = {
    .name = TYPE_L6_DUAL_MACHINE,
    .parent = TYPE_MACHINE,
    .instance_size = sizeof(L6DualMachineState),
    .class_init = l6_dual_class_init,
    .interfaces = arm_machine_interfaces,
};

static void l6_register_types(void)
{
    type_register_static(&l6_base_info);
    type_register_static(&l6_main_info);
    type_register_static(&l6_panel_info);
    type_register_static(&l6_dual_info);
}

type_init(l6_register_types)
