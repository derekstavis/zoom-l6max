/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Minimal L6/L6max firmware execution boards for QEMU 11.1.2.
 *
 * Copyright (c) 2026. Licensed under GPL-2.0-or-later.
 * These are bring-up boards, not hardware-accurate SoC models.
 */

#include "qemu/osdep.h"
#include "qapi/error.h"
#include "hw/arm/armv7m.h"
#include "hw/arm/boot.h"
#include "hw/core/cpu.h"
#include "hw/gpio/imx_gpio.h"
#include "target/arm/cpu.h"
#include "hw/arm/machines-qom.h"
#include "hw/core/boards.h"
#include "hw/core/qdev-clock.h"
#include "hw/core/qdev-properties.h"
#include "hw/core/irq.h"
#include "hw/core/sysbus.h"
#include "hw/core/resettable.h"
#include "hw/core/loader.h"
#include "exec/memop.h"
#include "hw/sd/sd.h"
#include "hw/sd/sdhci.h"
#include "hw/i2c/i2c.h"
#include "system/address-spaces.h"
#include "system/blockdev.h"
#include "qemu/log.h"
#include "qemu/module.h"
#include "qemu/timer.h"
#include "qemu/main-loop.h"
#include "qemu/audio.h"
#include "chardev/char-fe.h"
#include "hw/char/stm32l4x5_usart.h"
#include "qemu/config-file.h"
#include "qemu/option.h"
#include "system/system.h"
#include "system/runstate.h"
#include "l6max-display.h"
#include "l6max-indicators.h"
#include "l6max-usb.h"
#include "l6-profile.h"
#include "l6-panel-gpio.h"
#include "l6-panel-timer.h"
#include "l6-sai.h"
#include "l6-edma.h"
#include "l6-ccm-analog.h"
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/file.h>
#include <unistd.h>

#define L6_MAX_PANEL_KEYS 32
#define L6_BUTTON_EVENT_COUNT 54
#define L6_BUTTON_TAP_TICKS 50
#define L6_ENCODER_COUNT 8
#define L6_INPUT_MAGIC 0x4c364950u
#define L6_INPUT_MESSAGE_SIZE 24
#define L6_INPUT_INDICATOR_LEVELS 0x8000000au
#define L6_INDICATOR_INTERVAL_NS 33333333
#define L6_PANEL_PACE_INTERVAL_NS 1000000
#define L6_PANEL_CONTROL_POWER_OFF 2
#define L6_PANEL_PACE_MAX_LAG_NS 100000000
#define L6_INPUT_QUEUE_LIMIT 64
#define L6_INPUT_TX_LIMIT (64 * 1024)
#define L6_AUDIO_TRACE_WINDOW_FRAMES 64000
/* 200 ms of output reserve, as a divisor of the sample rate. */
#define L6_AUDIO_PRIME_DIVISOR 5
/* Bring-up debounce default, matching the verified minimum press duration. */
#define L6_INPUT_RELEASE_GAP_MS 50
#define L6_LPUART2_BASE 0x188000
#define L6_LPUART2_IRQ 21
#define L6_LPUART3_BASE 0x18c000
#define L6_LPUART3_IRQ 22
#define L6_LPUART1_BASE 0x184000
#define L6_LPUART1_IRQ 20
#define L6_LPUART7_BASE 0x19c000
#define L6_LPUART7_IRQ 26
#define L6_MAIN_BUTTON_MENU (1u << 0)
#define L6_MAIN_BUTTON_PLAY (1u << 1)
#define L6_MAIN_BUTTON_RECORD (1u << 2)

/* The L6max firmware's I2C register writes match a TLV320ADC6140-compatible
 * ADC configuration. Keep the emulated slave generic until the physical
 * codec marking is confirmed; it provides the observed register protocol. */
#define TYPE_L6MAX_AUDIO_ADC "l6max-audio-adc"
OBJECT_DECLARE_SIMPLE_TYPE(L6maxAudioADCState, L6MAX_AUDIO_ADC)

struct L6maxAudioADCState {
    I2CSlave parent_obj;
    uint8_t registers[128];
    uint8_t register_pointer;
    bool expect_register;
};

static int l6max_audio_adc_send(I2CSlave *i2c, uint8_t data)
{
    L6maxAudioADCState *s = L6MAX_AUDIO_ADC(i2c);

    if (s->expect_register) {
        s->register_pointer = data & 0x7f;
        s->expect_register = false;
    } else {
        s->registers[s->register_pointer] = data;
        s->register_pointer = (s->register_pointer + 1) & 0x7f;
        if (g_getenv("L6_AUDIO_TRACE")) {
            qemu_log_mask(LOG_UNIMP,
                          "l6max audio ADC I2C address=%02x register=%02x value=%02x\n",
                          i2c->address, (s->register_pointer - 1) & 0x7f,
                          data);
        }
    }
    return 0;
}

static uint8_t l6max_audio_adc_recv(I2CSlave *i2c)
{
    L6maxAudioADCState *s = L6MAX_AUDIO_ADC(i2c);
    uint8_t value = s->registers[s->register_pointer];

    s->register_pointer = (s->register_pointer + 1) & 0x7f;
    return value;
}

static int l6max_audio_adc_event(I2CSlave *i2c, enum i2c_event event)
{
    L6maxAudioADCState *s = L6MAX_AUDIO_ADC(i2c);

    if (event == I2C_START_SEND) {
        s->expect_register = true;
    }
    return 0;
}

static void l6max_audio_adc_class_init(ObjectClass *klass, const void *data)
{
    I2CSlaveClass *sc = I2C_SLAVE_CLASS(klass);

    (void)data;
    sc->send = l6max_audio_adc_send;
    sc->recv = l6max_audio_adc_recv;
    sc->event = l6max_audio_adc_event;
}

static const TypeInfo l6max_audio_adc_type_info = {
    .name = TYPE_L6MAX_AUDIO_ADC,
    .parent = TYPE_I2C_SLAVE,
    .instance_size = sizeof(L6maxAudioADCState),
    .class_init = l6max_audio_adc_class_init,
};

static void l6max_audio_adc_register_types(void)
{
    type_register_static(&l6max_audio_adc_type_info);
}
type_init(l6max_audio_adc_register_types)

typedef struct L6BoardProfile {
    const char *name;
    hwaddr main_flash_base;
    unsigned panel_matrix_columns;
    unsigned encoder_count;
    hwaddr midi_uart_base;
    int midi_uart_irq;
    unsigned midi_uart_number;
    hwaddr panel_uart_base;
    int panel_uart_irq;
    unsigned panel_uart_number;
    hwaddr panel_boot_gpio_base;
    uint32_t panel_boot_gpio_mask;
    hwaddr panel_reset_gpio_base;
    uint32_t panel_reset_gpio_mask;
    hwaddr high_ram_base;
    size_t high_ram_size;
    hwaddr audio_queue_write_offset;
    hwaddr audio_queue_read_offset;
    hwaddr sai_bases[2]; /* Offsets within the MAIN peripheral window. */
    unsigned sai_count;
    unsigned audio_output_left_slot;
    unsigned audio_output_right_slot;
    unsigned audio_sample_rate_hz;
    unsigned audio_source_clock_hz;
    bool resource_flash;
    bool oled;
    bool audio_codec_bus;
    bool audio_profile_ramp_ack;
    bool audio_dsp_wait_ack;
    unsigned main_gpio_button_mask;
    unsigned power_button_cutoff_ms;
} L6BoardProfile;

static const L6BoardProfile L6_PROFILE = {
    .name = "L6",
    .main_flash_base = 0x80001000,
    .panel_matrix_columns = 3,
    .encoder_count = 6,
    .midi_uart_base = L6_LPUART2_BASE,
    .midi_uart_irq = L6_LPUART2_IRQ,
    .midi_uart_number = 2,
    /* L6's panel service code configures SDK UART instance 6 at 115200 baud;
     * the runtime handle resolves to LPUART7 (base + IRQ verified in firmware). */
    .panel_uart_base = L6_LPUART7_BASE,
    .panel_uart_irq = L6_LPUART7_IRQ,
    .panel_uart_number = 7,
    /* The panel service sets GPIO3.7 before pulsing GPIO2.29. */
    .panel_boot_gpio_base = 0x401c0000,
    .panel_boot_gpio_mask = 1u << 7,
    .panel_reset_gpio_base = 0x401bc000,
    .panel_reset_gpio_mask = 1u << 29,
    .high_ram_base = 0x801f0000,
    .high_ram_size = 0x10000,
    /* L6's callback staging indices are in SRAM at 0x2000bb0c/bb10. */
    .audio_queue_write_offset = 0xbb0c,
    .audio_queue_read_offset = 0xbb10,
    .sai_bases = { 0x384000, 0x38c000 },
    .sai_count = 2,
    .audio_output_left_slot = 0,
    .audio_output_right_slot = 1,
    /* Both stock images pass 24.576 MHz to their SAI divider helpers. This
     * is the firmware's source-clock assumption, not a measured CCM root. */
    .audio_sample_rate_hz = 48000,
    .audio_source_clock_hz = 24576000,
    .resource_flash = true,
    .oled = false,
    .audio_codec_bus = false,
    .audio_profile_ramp_ack = false,
    .audio_dsp_wait_ack = false,
    /* Keep L6max's generic button mask limited to Play. L6 TAP and sound-pad
     * inputs use the model-specific mappings in l6_main_key_gpio(). */
    .main_gpio_button_mask = L6_MAIN_BUTTON_PLAY,
    /* L6 does not emit L6max's GPIO3.3 power-hold falling edge. Model the
     * front-panel on/off switch after a debounced press instead. */
    .power_button_cutoff_ms = 500,
};

static const L6BoardProfile L6MAX_PROFILE = {
    .name = "L6max",
    .main_flash_base = 0x80000000,
    .panel_matrix_columns = 5,
    .encoder_count = 8,
    .midi_uart_base = L6_LPUART3_BASE,
    .midi_uart_irq = L6_LPUART3_IRQ,
    .midi_uart_number = 3,
    .panel_uart_base = L6_LPUART1_BASE,
    .panel_uart_irq = L6_LPUART1_IRQ,
    .panel_uart_number = 1,
    /* Recovered L6max updater uses GPIO1.29 BOOT and GPIO5.1 RESET. */
    .panel_boot_gpio_base = 0x401b8000,
    .panel_boot_gpio_mask = 1u << 29,
    .panel_reset_gpio_base = 0x400c0000,
    .panel_reset_gpio_mask = 1u << 1,
    /* Preserve the established L6max callback-ring layout. */
    .audio_queue_write_offset = 0x7d08,
    .audio_queue_read_offset = 0x7d0c,
    .sai_bases = { 0x384000 },
    .sai_count = 1,
    /* L6max SAI1 uses an eight-slot frame. Slots 2-3 follow the MASTER knob,
     * right channel first; slots 4-5 follow the MONITOR knob with inverted
     * polarity; slots 6-7 follow neither. */
    .audio_output_left_slot = 3,
    .audio_output_right_slot = 2,
    .audio_sample_rate_hz = 48000,
    .audio_source_clock_hz = 24576000,
    .oled = true,
    .audio_codec_bus = true,
    .audio_profile_ramp_ack = false,
    .audio_dsp_wait_ack = false,
    .main_gpio_button_mask = L6_MAIN_BUTTON_MENU | L6_MAIN_BUTTON_PLAY |
                             L6_MAIN_BUTTON_RECORD,
    .power_button_cutoff_ms = 0,
};

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
    unsigned id; /* L6max: 0=Menu, 1=Play, 2=Record; L6: host target ID */
    unsigned start_ms;
    unsigned end_ms;
    bool logged;
} L6MainKey;

typedef struct L6MainGPIOAdapter {
    struct L6ChipState *chip;
    DeviceState *device;
    MemoryRegion *model_mmio;
    MemoryRegion registers;
    MemoryRegion aliases;
    unsigned bank;
} L6MainGPIOAdapter;

typedef struct L6ChipState {
    ARMv7MState armv7m;
    MemoryRegion flash;
    MemoryRegion panel_flash_window;
    MemoryRegion flash_alias;
    MemoryRegion itcm;
    MemoryRegion main_high_ram;
    MemoryRegion sram;
    MemoryRegion dtcm;
    MemoryRegion external_ram;
    MemoryRegion peripheral;
    MemoryRegion sd_adapter;
    MemoryRegion usb_region;
    L6USB usb;
    SDHCIState *sdhc;
    I2CBus *audio_i2c_bus;
    const MemoryRegionOps *sd_ops;
    uint32_t sd_sysctl;
    uint32_t sd_vend_spec2;
    uint8_t sd_wakeup;
    bool sd_present;
    Clock *sysclk;
    Clock *refclk;
    GHashTable *registers;
    size_t flash_payload_size;
    uint8_t *nor_flash;
    uint8_t *resource_flash;
    size_t resource_flash_size;
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
    unsigned resource_flash_log_count;
    uint8_t lpspi_flash_opcode;
    uint8_t lpspi_flash_header_bytes;
    uint32_t lpspi_flash_address;
    uint32_t lpspi_rx_fifo[16];
    unsigned lpspi_rx_head;
    unsigned lpspi_rx_count;
    unsigned edma_log_count;
    uint32_t edma_erq;
    bool audio_trace;
    unsigned audio_trace_sai_log_count;
    unsigned audio_trace_edma_log_count;
    unsigned audio_trace_clock_log_count;
    unsigned audio_trace_irq_elapsed_frames;
    uint64_t audio_trace_irq_count[4];
    uint64_t audio_trace_irq_ack_count[4];
    L6Sai sai[2];
    uint64_t audio_service_period_remainder;
    uint64_t audio_service_deadline_ns;
    unsigned audio_service_rate_hz;
    bool audio_sample_trace;
    unsigned audio_sample_trace_frames;
    bool audio_edma_irq_raised;
    uint64_t audio_tx_words[2];
    uint64_t audio_tx_nonzero_words[2];
    uint64_t audio_tx_underruns[2];
    uint32_t audio_tx_first_nonzero[2];
    uint32_t audio_tx_last_nonzero[2];
    uint32_t audio_tx_probe[2][8];
    unsigned audio_tx_probe_count[2];
    bool audio_tx_probe_logged[2];
    AudioBackend *audio_backend;
    SWVoiceOut *audio_voice;
    GByteArray *audio_pending;
    bool audio_primed;
    FILE *audio_raw_capture;
    FILE *audio_tdm_capture;
    FILE *audio_dma_source_capture;
    bool audio_dma_source_capture_header_written;
    unsigned audio_tdm_capture_slots;
    uint64_t audio_tdm_capture_limit_frames;
    uint64_t audio_tdm_capture_frames_written;
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
    bool main_boot_select;
    bool main_rom_mode;
    uint8_t rom_flash[0x10000];
    uint8_t *rom_shared_flash;
    /* Split-process link carrying panel hold from MAIN to the panel. */
    int panel_control_fd;
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
    QEMUTimer *panel_pace;
    int64_t panel_pace_virtual_ns;
    int64_t panel_pace_host_ns;
    QEMUTimer *panel_tim14;
    QEMUTimer *panel_spi_dma;
    QEMUTimer *indicator_timer;
    L6Indicators indicators;
    unsigned indicator_log_count;
    QEMUTimer *rom_uart_timer;
    QEMUTimer *main_audio_service_timer;
    /* Panel USART1 is the real stm32l4x5-usart device (register-compatible
     * by inspection: our own CR1/ISR bit usage already matched RM0351
     * bit-for-bit before this). Its chardev is an in-process socket pair;
     * panel_usart_fd is our end, bridging its bytes to/from the profile's
     * panel UART on the main chip (LPUART7 on L6, LPUART1 on L6max). */
    DeviceState *panel_usart1;
    int panel_usart_fd;
    CharFrontend main_uart;
    bool main_rx_pending;
    uint8_t main_rx_byte;
    /* Model-selected UART for the physical MIDI IN/OUT (DIN/TRS) jacks: an
     * inherited socket, like L6_USB_FD, rather than a Chardev. */
    int main_midi_fd;
    GByteArray *main_midi_rx;
    bool main_midi_rx_pending;
    uint8_t main_midi_rx_byte;
    const L6BoardProfile *profile;
    L6PanelKey panel_keys[L6_MAX_PANEL_KEYS];
    unsigned panel_key_count;
    L6MainKey main_keys[L6_MAX_PANEL_KEYS];
    unsigned main_key_count;
    bool service_audio_queue;
    bool service_audio_profile_ramp_ack;
    bool service_audio_dsp_ack;
    int input_fd;
    uint8_t input_rx[L6_INPUT_MESSAGE_SIZE];
    bool input_trace;
    int64_t input_trace_until;
    unsigned input_trace_matrix_count;
    bool main_gpio_trace;
    bool main_gpio_output_trace;
    bool main_gpio_pad_led_trace;
    unsigned main_gpio_output_log_count;
    uint32_t main_button_tested_bits[L6_BUTTON_EVENT_COUNT][5];
    unsigned input_rx_size;
    GByteArray *input_tx;
    GQueue input_taps[L6_BUTTON_EVENT_COUNT];
    uint32_t main_gpio[5], main_gpio_generation;
    L6MainGPIOAdapter main_gpio_adapters[5];
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

static unsigned l6_encoder_count(L6ChipState *s);
static bool l6_panel_button_valid(L6ChipState *s, unsigned target);
static bool l6_main_button_valid(L6ChipState *s, unsigned target);
static bool l6_main_key_gpio(L6ChipState *s, unsigned id,
                             hwaddr *port, unsigned *pin);
static void l6_main_gpio_refresh_inputs(L6ChipState *s);

static void l6_lpi2c_mtdr_write(L6ChipState *s, uint32_t value)
{
    uint32_t command = (value >> 8) & 7;
    uint8_t data = value & 0xff;
    gpointer msr_key = GUINT_TO_POINTER((0x3f0014 >> 2) + 1);
    uint32_t msr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                        msr_key));
    int result = 0;

    if (!s->audio_i2c_bus) {
        return;
    }
    switch (command) {
    case 0: /* Transmit data byte. */
        result = i2c_send(s->audio_i2c_bus, data);
        break;
    case 2: /* STOP. */
        i2c_end_transfer(s->audio_i2c_bus);
        msr |= 1u << 9; /* SDF */
        break;
    case 4: /* START and send/receive the 8-bit address byte. */
        result = i2c_start_transfer(s->audio_i2c_bus, data >> 1,
                                    (data & 1) != 0);
        break;
    default:
        result = -1;
        qemu_log_mask(LOG_UNIMP,
                      "L6max LPI2C1 unsupported MTDR command=%u value=%04x\n",
                      command, value & 0x7ff);
        break;
    }
    if (result < 0) {
        msr |= 1u << 10; /* NDF: no slave acknowledged the operation. */
    }
    msr |= 1u; /* TDF: the command/data word has left the transmit FIFO. */
    g_hash_table_insert(s->registers, msr_key, GUINT_TO_POINTER(msr));
}

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
#define L6_USDHC_VEND_SPEC2_RESET 0x00019006u
#define L6_USDHC_VEND_SPEC2_RW_MASK ((1u << 12) | (0x0fu << 3))
#define L6_USDHC_SYS_CTRL_RESET 0x0080800fu

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
    memset(s->main_button_tested_bits, 0, sizeof(s->main_button_tested_bits));
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
    if (kind != L6_INPUT_INDICATOR_LEVELS) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 %s input reply kind=%08x seq=%u target=%u value=%d at %" PRId64 " ms\n",
                      s->panel ? "panel" : "main", kind, seq, target, value,
                      qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
    }
    g_byte_array_append(s->input_tx, message, sizeof(message));
    l6_input_handlers(s);
}

static uint32_t l6_button_duration(L6ChipState *s, unsigned target,
                                   uint32_t duration)
{
    if (!s->panel && target == 53 && s->profile->power_button_cutoff_ms) {
        return s->profile->power_button_cutoff_ms;
    }
    return MAX(duration, L6_BUTTON_TAP_TICKS);
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
        (s->panel ? l6_panel_button_valid(s, target)
                  : l6_main_button_valid(s, target));

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
        (kind == 4 && (!s->panel || target >= l6_encoder_count(s))) ||
        kind < 1 || kind > 4 ||
        (kind == 1 && (value != 1 || duration > 60000)) ||
        (kind == 2 && (value != 0 || duration != 0)) ||
        (kind == 3 && (value != 1 || !duration || duration > 60000)) ||
        (kind == 4 && (!value || duration != 0))) {
        l6_input_reply(s, 0x80000003u, seq, target, -EINVAL);
        return;
    }
    if (s->input_trace && kind <= 3) {
        s->input_trace_until = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) + 1000;
        s->input_trace_matrix_count = 0;
    }
    switch (kind) {
    case 1:
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
            press->duration_ms = l6_button_duration(s, target, duration);
            g_queue_push_tail(&s->input_taps[target], press);
            break;
        }
        s->input_button_down[target] = true;
        s->input_release_requested[target] = false;
        s->input_down_deadline[target] = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) +
            l6_button_duration(s, target, duration);
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
        tap->duration_ms = l6_button_duration(s, target, duration);
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
        } else if (received == 0 && s->panel_control_fd >= 0) {
            /* The host owns a split engine's processes; do not outlive it. */
            exit(0);
        } else if (received == 0 ||
                   (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR)) {
            l6_input_disconnect(s);
            return;
        } else if (errno != EINTR) {
            return;
        }
    }
}

static void l6_board_power_off(L6ChipState *main);

static void l6_socket_input_tick(L6ChipState *s)
{
    int64_t now = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);
    for (unsigned button = 0; button < L6_BUTTON_EVENT_COUNT; button++) {
        bool l6_power_cutoff = !s->panel && button == 53 &&
            s->profile->power_button_cutoff_ms != 0;
        if (l6_power_cutoff && s->input_button_down[button] &&
            now >= s->input_down_deadline[button]) {
            if (s->input_release_requested[button]) {
                l6_input_reply(s, 0x80000001u,
                               s->input_release_sequence[button], button, 0);
            }
            s->input_button_down[button] = false;
            s->input_release_requested[button] = false;
            qemu_log_mask(LOG_UNIMP,
                          "l6 board Power button cutoff at %" PRId64 " ms\n",
                          now);
            if (!++s->main_gpio_generation) { ++s->main_gpio_generation; }
            l6_input_reply(s, 0x80000009u, s->main_gpio_generation, 0, 0);
            l6_board_power_off(s);
            return;
        }
        if (l6_power_cutoff && s->input_tap_deadline[button] &&
            now >= s->input_tap_deadline[button]) {
            l6_input_reply(s, 0x80000002u, s->input_tap_sequence[button],
                           button, s->input_button_down[button]);
            s->input_tap_deadline[button] = 0;
            qemu_log_mask(LOG_UNIMP,
                          "l6 board Power button cutoff at %" PRId64 " ms\n",
                          now);
            if (!++s->main_gpio_generation) { ++s->main_gpio_generation; }
            l6_input_reply(s, 0x80000009u, s->main_gpio_generation, 0, 0);
            l6_board_power_off(s);
            return;
        }
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
    if (present != s->sd_present) {
        s->sd_present = present;
        IMXGPIOState *gpio = IMX_GPIO(s->main_gpio_adapters[1].device);
        gpio->isr |= 1u << 28;
        qemu_set_irq(gpio->irq[1], !!(gpio->isr & gpio->imr & 0xffff0000u));
        qemu_log_mask(LOG_UNIMP, "l6 SD card detect=%u\n", present);
    }
}

static void l6_input_timer_tick(void *opaque)
{
    L6ChipState *s = opaque;
    l6_socket_input_tick(s);
    if (!s->panel) { l6_sd_detect_tick(s); }
    if (!s->panel) { l6_main_gpio_refresh_inputs(s); }
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

static unsigned l6_encoder_count(L6ChipState *s)
{
    return s->profile->encoder_count;
}

static bool l6_panel_button_valid(L6ChipState *s, unsigned target)
{
    if (s->profile == &L6_PROFILE) {
        /* L6: five direct inputs and a 3 x 8 switch matrix. */
        return target == 1 || target == 2 || target == 3 || target == 6 ||
               target == 47 || (target >= 7 && target <= 30);
    }
    return target == 1 || target == 2 || target == 3 || target == 6 ||
           (target >= 7 && target <= 47);
}

static bool l6_main_button_valid(L6ChipState *s, unsigned target)
{
    if (target >= 48) {
        return true;
    }
    unsigned button = target == 0 ? L6_MAIN_BUTTON_MENU :
                      target == 4 ? L6_MAIN_BUTTON_PLAY :
                      target == 5 ? L6_MAIN_BUTTON_RECORD : 0;
    return button && (s->profile->main_gpio_button_mask & button);
}

/* Timed main-GPIO keys include L6 TAP and the four sound pads. Input pins are
 * model-specific; retain the recovered reader pin even where pad attribution
 * still needs confirmation. */
static bool l6_main_key_gpio(L6ChipState *s, unsigned id,
                             hwaddr *port, unsigned *pin)
{
    if (s->profile == &L6MAX_PROFILE && id >= 48 && id <= 52) {
        /* L6max control readers 49–52 come from the recovered key table;
         * control 48 is the EventPort TAP input. GPIO2.30 is a board strap. */
        static const struct { hwaddr port; unsigned pin; } l6max_pad_keys[] = {
            { 0x1bc000, 31 }, /* TAP */
            { 0x1b8000, 21 }, /* Control 49: GPIO1 bit 21 */
            { 0x1b8000, 25 }, /* Control 50: GPIO1 bit 25 */
            { 0x1b8000, 26 }, /* Control 51: GPIO1 bit 26 */
            { 0x1b8000, 0 },  /* Control 52: GPIO1 bit 0 */
        };
        *port = l6max_pad_keys[id - 48].port;
        *pin = l6max_pad_keys[id - 48].pin;
        return true;
    }
    if (s->profile == &L6_PROFILE) {
        static const struct { unsigned id; hwaddr port; unsigned pin; } l6_keys[] = {
            { 4, 0x1bc000, 25 }, /* Play/Stop: active-low GPIO2.25 */
            { 48, 0x1bc000, 22 }, /* TAP: fifth EventPort input */
            { 49, 0x1bc000, 31 }, /* Sound pad 1 */
            { 50, 0x1bc000, 30 }, /* Sound pad 2 */
            { 51, 0x1b8000, 21 }, /* Sound pad 3 */
            { 52, 0x1b8000, 20 }, /* Sound pad 4 */
        };
        for (unsigned i = 0; i < ARRAY_SIZE(l6_keys); i++) {
            if (l6_keys[i].id == id) {
                *port = l6_keys[i].port;
                *pin = l6_keys[i].pin;
                return true;
            }
        }
        return false;
    }
    if (id > 2 || !(s->profile->main_gpio_button_mask & (1u << id))) {
        return false;
    }
    *port = id == 1 ? 0x1b8000 : 0x0c0000;
    *pin = id == 0 ? 2 : id == 1 ? 20 : 0;
    return true;
}

static const hwaddr l6_main_gpio_ports[5] = {
    0x1b8000, 0x1bc000, 0x1c0000, 0x1c4000, 0x0c0000
};

static uint32_t l6_main_gpio_input_value(L6ChipState *s, unsigned bank)
{
    hwaddr port = l6_main_gpio_ports[bank];
    uint32_t word = s->main_gpio[bank];
    uint64_t now_ms = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);

    /* Board pull-ups and static strap levels recovered by the old shim. */
    word |= bank == 4 ? 0x5u : bank == 1 ?
        ((1u << 31) | (s->profile == &L6_PROFILE ?
                       ((1u << 30) | (1u << 22)) : 0)) :
        ((1u << 20) | (1u << 21) | (1u << 25) | (1u << 26) | 1u |
         (s->profile == &L6_PROFILE ? (1u << 9) : 0));
    if (bank == 1) {
        word = (word & ~(1u << 28)) | (s->sd_present ? (1u << 28) : 0);
        if (s->profile == &L6_PROFILE) {
            word |= 1u << 25;
        } else {
            word &= ~(1u << 25);
            if (s->input_fd >= 0 && l6_button_down(s, 53)) {
                word |= 1u << 25;
            }
        }
    }

    for (unsigned i = 0; i < s->main_key_count; i++) {
        L6MainKey *key = &s->main_keys[i];
        hwaddr key_port;
        unsigned pin;
        if (!l6_main_key_gpio(s, key->id, &key_port, &pin) ||
            port != key_port || now_ms < key->start_ms ||
            now_ms >= key->end_ms) {
            continue;
        }
        if (!key->logged) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 main key id=%u port=%08x pin=%u at %" PRIu64 " ms\n",
                          key->id, (unsigned)port, pin, now_ms);
            key->logged = true;
        }
        word &= ~(1u << pin);
    }

    if (s->input_fd >= 0) {
        static const struct {
            unsigned id;
            unsigned bank;
            unsigned pin;
            bool active_high;
            const L6BoardProfile *profile;
        } buttons[] = {
            { 0, 4, 2, false, NULL },
            { 4, 1, 25, false, &L6_PROFILE },
            { 4, 0, 20, false, &L6MAX_PROFILE },
            { 5, 4, 0, false, NULL },
        };
        for (unsigned i = 0; i < ARRAY_SIZE(buttons); i++) {
            if (bank != buttons[i].bank ||
                (buttons[i].profile && s->profile != buttons[i].profile)) {
                continue;
            }
            bool pressed = l6_button_down(s, buttons[i].id);
            if (!pressed) {
                memset(s->main_button_tested_bits[buttons[i].id], 0,
                       sizeof(s->main_button_tested_bits[buttons[i].id]));
                if (buttons[i].active_high) {
                    word &= ~(1u << buttons[i].pin);
                } else {
                    word |= 1u << buttons[i].pin;
                }
            } else {
                if (buttons[i].active_high) {
                    word |= 1u << buttons[i].pin;
                } else {
                    word &= ~(1u << buttons[i].pin);
                }
            }
        }
        if (s->profile == &L6_PROFILE && bank == 0) {
            if (l6_button_down(s, 53)) {
                word &= ~(1u << 9);
            } else {
                word |= 1u << 9;
            }
        }
        for (unsigned id = 48; id <= 52; id++) {
            hwaddr key_port;
            unsigned pin;
            if (!l6_main_key_gpio(s, id, &key_port, &pin) || key_port != port) {
                continue;
            }
            bool pressed = l6_button_down(s, id);
            if (!pressed) {
                memset(s->main_button_tested_bits[id], 0,
                       sizeof(s->main_button_tested_bits[id]));
                word |= 1u << pin;
            } else {
                word &= ~(1u << pin);
            }
        }
    }

    bool l6_power_bank = s->profile == &L6_PROFILE ? bank == 0 : bank == 1;
    if (l6_power_bank) {
        unsigned power_pin = s->profile == &L6_PROFILE ? 9 : 25;
        bool pressed = (word & (1u << power_pin)) != 0;
        if (s->profile == &L6_PROFILE) {
            pressed = !pressed;
        }
        if (!s->power_seen || pressed != s->power_published) {
            s->power_seen = true;
            s->power_published = pressed;
            qemu_log_mask(LOG_UNIMP,
                          "l6 main power input=%u at %" PRIu64 " ms\n",
                          pressed, now_ms);
        }
    }
    return word;
}

static void l6_encoder_input_tick(L6ChipState *s)
{
    if (s->input_fd >= 0) {
        static const uint8_t phases[2][4] = {
            { 2, 0, 1, 3 }, /* positive: one Gray-code cycle from idle 3 */
            { 1, 0, 2, 3 }, /* negative: reverse Gray-code cycle */
        };
        for (unsigned encoder = 0; encoder < l6_encoder_count(s); encoder++) {
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

static void l6_panel_timer_irq(L6ChipState *s, hwaddr base, unsigned irq)
{
    qemu_set_irq(qdev_get_gpio_in(DEVICE(&s->armv7m), irq),
                 l6_panel_timer_irq_level(l6_panel_reg(s, base + 0x10),
                                          l6_panel_reg(s, base + 0x0c)));
}

static void l6_panel_timer_reset(L6ChipState *s)
{
    static const hwaddr bases[] = { 0, 0x400, 0x2000 };
    static const unsigned irqs[] = { 15, 16, 19 };
    for (unsigned i = 0; i < ARRAY_SIZE(bases); i++) {
        l6_panel_store(s, bases[i] + 0x0c, 0);
        l6_panel_store(s, bases[i] + 0x10, 0);
        l6_panel_timer_irq(s, bases[i], irqs[i]);
    }
}

/* External indicator matrix: 24 serial columns, eight one-hot commons.
 * These outputs come from the peripheral pins, never UART or guest variables. */
static void l6_indicator_tick(void *opaque)
{
    L6ChipState *s = opaque;
    L6Indicators *indicators = &s->indicators;
    if (s->cpu_held) {
        timer_del(s->indicator_timer);
        return;
    }
    l6_indicators_advance(indicators, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL));
    if (!++indicators->generation) {
        ++indicators->generation;
    }
    l6_input_reply(s, 0x80000008u, indicators->generation, 0, 0);
    for (unsigned row = 0; row < 8; row++) {
        uint64_t active = indicators->row_active_ns[row];
        if (active) {
            for (unsigned bit = 0; bit < 24; bit++) {
                uint64_t on = MIN(indicators->led_on_ns[row][bit], active);
                uint8_t level = (on * 255 + active / 2) / active;
                /* The dimmest segments get too few PWM pulses per window to
                 * round above zero, and some windows miss them. They recur,
                 * unlike a stray pulse from a latch update, which stays off.
                 * Bright windows do not count, so a blinking LED reaches zero. */
                indicators->pulse_history[row][bit] =
                    indicators->pulse_history[row][bit] << 1 |
                    (on != 0 && level <= 1);
                if (level <= 1) {
                    level = ctpop8(indicators->pulse_history[row][bit]) >= 2;
                }
                indicators->levels[row][bit] = level;
            }
        }
        indicators->row_active_ns[row] = 0;
        memset(indicators->led_on_ns[row], 0,
               sizeof(indicators->led_on_ns[row]));
        for (unsigned group = 0; group < 8; group++) {
            unsigned first = group * 3;
            if (memcmp(&indicators->levels[row][first],
                       &indicators->published[row][first], 3) == 0) {
                continue;
            }
            uint32_t packed = indicators->levels[row][first] |
                ((uint32_t)indicators->levels[row][first + 1] << 8) |
                ((uint32_t)indicators->levels[row][first + 2] << 16);
            l6_input_reply(s, L6_INPUT_INDICATOR_LEVELS,
                           indicators->generation, row * 8 + group, packed);
            memcpy(&indicators->published[row][first],
                   &indicators->levels[row][first], 3);
        }
    }
    timer_mod(s->indicator_timer,
              qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  L6_INDICATOR_INTERVAL_NS);
}

extern bool icount_timers_stay_on_vcpu;

/* A standalone panel runs with -icount sleep=off, so idle time jumps to the next
 * timer and its PWM keeps exact virtual timing. Host wakeups are far too coarse for
 * its 50 us scan, so instead hold each virtual millisecond to the host clock. */
static void l6_panel_pace_tick(void *opaque)
{
    L6ChipState *s = opaque;
    int64_t virtual_ns = qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL);
    int64_t host_ns = get_clock();
    int64_t ahead = (virtual_ns - s->panel_pace_virtual_ns) -
                    (host_ns - s->panel_pace_host_ns);
    if (!s->panel_pace_host_ns || ahead < -L6_PANEL_PACE_MAX_LAG_NS) {
        s->panel_pace_virtual_ns = virtual_ns;
        s->panel_pace_host_ns = host_ns;
    } else if (ahead > 0) {
        struct timespec delay = {
            .tv_sec = ahead / NANOSECONDS_PER_SECOND,
            .tv_nsec = ahead % NANOSECONDS_PER_SECOND,
        };
        /* Keep the BQL: dropping it inside a timer deadlocks with vm_stop, which
         * holds it while waiting for running timers to finish. */
        nanosleep(&delay, NULL);
    }
    timer_mod(s->panel_pace, virtual_ns + L6_PANEL_PACE_INTERVAL_NS);
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
    uint32_t status = l6_panel_reg(s, 0x410);
    l6_panel_timer_update(&cr1, &status, false);
    l6_panel_store(s, 0x410, status);
    l6_panel_timer_irq(s, 0x400, 16);
    if (!(cr1 & 1)) { /* one-pulse mode */
        l6_panel_store(s, 0x400, cr1 & ~1u);
    } else {
        timer_mod(s->panel_tim3, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  l6_panel_tim3_period(s));
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
    uint32_t status = l6_panel_reg(s, 0x2010);
    l6_panel_timer_update(&cr1, &status, false);
    l6_panel_store(s, 0x2010, status);
    l6_panel_timer_irq(s, 0x2000, 19);
    if (!(cr1 & 1)) {
        l6_panel_store(s, 0x2000, cr1 & ~1u);
    } else {
        timer_mod(s->panel_tim14, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  l6_panel_tim14_period(s));
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
        l6_indicators_spi_byte(&s->indicators, byte,
                               qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL));
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
    gpointer sr_key = GUINT_TO_POINTER((0x0010 >> 2) + 1);
    uint32_t sr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, sr_key));
    bool update = l6_panel_timer_update(&cr1, &sr, false);
    l6_panel_store(s, 0, cr1);
    l6_panel_store(s, 0x10, sr);
    if (update && (dier & 1)) {
        if (s->input_trace &&
            qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL) < s->input_trace_until) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 panel TIM2 assert at %" PRId64 " ns period=%" PRId64 " ns\n",
                          qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL),
                          l6_panel_tim2_period(s));
        }
    }
    l6_panel_timer_irq(s, 0, 15);
    if (cr1 & 1) {
        timer_mod(s->panel_tim2, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  l6_panel_tim2_period(s));
    }
}

static int l6_main_uart_can_receive(void *opaque)
{
    L6ChipState *s = opaque;
    return s->main_rx_pending || s->main_rom_mode ? 0 : 1;
}

/* Both the normal panel/service link and the ROM/update exchange use the
 * model's physical panel UART: LPUART7 on L6, LPUART1 on L6max. */
static hwaddr l6_panel_uart_base(const L6ChipState *s)
{
    return s->profile->panel_uart_base;
}

static int l6_panel_uart_irq(const L6ChipState *s)
{
    return s->profile->panel_uart_irq;
}

static unsigned l6_panel_uart_number(const L6ChipState *s)
{
    return s->profile->panel_uart_number;
}

static void l6_main_uart_receive(void *opaque, const uint8_t *buf, int size)
{
    L6ChipState *s = opaque;
    if (!s->peer) {
        /* Tools grep this exact line, as for the in-process link. */
        for (int i = 0; i < size; i++) {
            qemu_log_mask(LOG_UNIMP, "l6 panel USART1 TX %02x\n", buf[i]);
            if (buf[i] == 0xb0) {
                qemu_log_mask(LOG_UNIMP, "l6 panel RTC response start at %" PRId64 " ms\n",
                              qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
            }
        }
    }
    if (size <= 0 || s->main_rx_pending) {
        return;
    }
    hwaddr base = l6_panel_uart_base(s);
    uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER(((base + 0x18) >> 2) + 1)));
    /* i.MX RT LPUART CTRL.RE gates reception. Consume but discard bytes that
     * arrive while disabled; holding one until RE is enabled would invent a
     * receive FIFO and can stall the UART link behind a stale byte. */
    if (!(ctrl & (1u << 18))) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 main LPUART%u (panel link) RX discarded (RE=0) %02x\n",
                      l6_panel_uart_number(s), buf[0]);
        return;
    }
    s->main_rx_byte = buf[0];
    s->main_rx_pending = true;
    s->main_rx_count++;
    qemu_log_mask(LOG_UNIMP, "l6 main LPUART%u (panel link) RX accepted %02x\n",
                  l6_panel_uart_number(s), buf[0]);
    if (ctrl & (1u << 21)) {
        qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), l6_panel_uart_irq(s)));
    }
}

/* The SDK UART instance and IRQ are selected by the booted board profile. */
static hwaddr l6_midi_uart_base(const L6ChipState *s)
{
    return s->profile->midi_uart_base;
}

static int l6_midi_uart_irq(const L6ChipState *s)
{
    return s->profile->midi_uart_irq;
}

/* Physical MIDI bytes arrive over the inherited socket and are handed to
 * the model's UART instance one at a time through STAT.RDRF. */
static void l6_main_midi_read_host(void *opaque)
{
    L6ChipState *s = opaque;
    hwaddr base = l6_midi_uart_base(s);
    uint8_t buf[256];
    ssize_t n = read(s->main_midi_fd, buf, sizeof(buf));
    if (n <= 0) {
        if (n == 0 || (errno != EAGAIN && errno != EWOULDBLOCK)) {
            qemu_set_fd_handler(s->main_midi_fd, NULL, NULL, NULL);
            close(s->main_midi_fd);
            s->main_midi_fd = -1;
        }
        return;
    }
    uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER(((base + 0x18) >> 2) + 1)));
    qemu_log_mask(LOG_UNIMP,
                  "l6 main LPUART%u (MIDI jacks) host RX %d bytes CTRL %08x RE %u RIE %u\n",
                  s->profile->midi_uart_number, (int)n, ctrl,
                  !!(ctrl & (1u << 18)), !!(ctrl & (1u << 21)));
    /* The DIN socket is the wire-facing endpoint, so disabled receivers lose
     * these bytes just like the panel UART path above. */
    if (!(ctrl & (1u << 18))) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 main LPUART%u (MIDI jacks) RX discarded (RE=0) %d bytes\n",
                      s->profile->midi_uart_number, (int)n);
        return;
    }
    bool was_pending = s->main_midi_rx_pending;
    g_byte_array_append(s->main_midi_rx, buf, n);
    if (!s->main_midi_rx_pending && s->main_midi_rx->len > 0) {
        s->main_midi_rx_pending = true;
        s->main_midi_rx_byte = s->main_midi_rx->data[0];
        g_byte_array_remove_index(s->main_midi_rx, 0);
    }
    if (!was_pending && s->main_midi_rx_pending) {
        if (ctrl & (1u << 21)) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), l6_midi_uart_irq(s)));
        }
    }
}

static void l6_uart_link_tick(void *opaque)
{
    L6UartLink *link = opaque;
    L6ChipState *destination = link->destination;
    if (!link->queue->len || destination->cpu_held) {
        return;
    }
    /* The panel's real USART device has no "can receive" gate of ours to
     * check; its own chardev handlers apply CR1.RE/overrun on arrival. */
    if (!destination->panel && !l6_main_uart_can_receive(destination)) {
        return;
    }
    uint8_t byte = link->queue->data[0];
    g_byte_array_remove_range(link->queue, 0, 1);
    if (destination->panel) {
        /* Tools grep this exact line (see tools/qemu-run) the same way
         * as the old hand-rolled receive path did. */
        qemu_log_mask(LOG_UNIMP, "l6 panel USART1 RX %02x\n", byte);
        destination->panel_rx_count++;
        if (destination->panel_usart_fd >= 0) {
            ssize_t n;
            do {
                n = write(destination->panel_usart_fd, &byte, 1);
            } while (n < 0 && errno == EINTR);
        }
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

/* Bytes sent by panel USART1 arrive here on our end of the in-process socket
 * pair and are forwarded to the model-selected main-chip panel UART. */
static void l6_panel_usart_bridge_read(void *opaque)
{
    L6ChipState *s = opaque;
    uint8_t buf[256];
    ssize_t n = read(s->panel_usart_fd, buf, sizeof(buf));
    if (n <= 0) {
        if (n == 0 || (errno != EAGAIN && errno != EWOULDBLOCK)) {
            qemu_set_fd_handler(s->panel_usart_fd, NULL, NULL, NULL);
            close(s->panel_usart_fd);
            s->panel_usart_fd = -1;
        }
        return;
    }
    s->panel_tx_count += n;
    for (ssize_t i = 0; i < n; i++) {
        /* Tools grep this exact line (see tools, "-smoke" binaries) to
         * verify panel output without their own USART client. */
        qemu_log_mask(LOG_UNIMP, "l6 panel USART1 TX %02x\n", buf[i]);
        if (buf[i] == 0xb0) {
            qemu_log_mask(LOG_UNIMP, "l6 panel RTC response start at %" PRId64 " ms\n",
                          qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        }
        l6_uart_link_send(s, buf[i]);
    }
}

static void l6_uart_accept_input(L6ChipState *destination)
{
    /* The panel's real USART device drives its own qemu_chr_fe_accept_input
     * internally; this path serves the model-selected main-chip panel UART. */
    if (destination->peer) {
        L6UartLink *incoming = destination->peer->uart_link;
        if (incoming->queue->len && !timer_pending(incoming->timer)) {
            timer_mod(incoming->timer,
                      qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + L6_UART_BYTE_NS);
        }
    } else {
        /* Both call sites below are guarded by `!s->panel`. */
        qemu_chr_fe_accept_input(&destination->main_uart);
    }
}

static void l6_storage_flush(void *bytes, size_t size)
{
    if (bytes && msync(bytes, size, MS_SYNC)) {
        error_setg(&error_fatal, "cannot flush device storage: %s", strerror(errno));
    }
}

/* In a split-process run, MAIN reaches panel flash through its state file. */
static void l6_main_map_panel_flash(L6ChipState *main)
{
    const char *directory = g_getenv("L6_STATE_DIR");

    if (main->rom_shared_flash || !directory) {
        return;
    }
    g_autofree char *path = g_build_filename(directory, "panel-flash.bin", NULL);
    int fd = open(path, O_RDWR);
    if (fd < 0) {
        return;
    }
    uint8_t *bytes = mmap(NULL, sizeof(main->rom_flash), PROT_READ | PROT_WRITE,
                          MAP_SHARED, fd, 0);
    close(fd);
    if (bytes != MAP_FAILED) {
        main->rom_shared_flash = bytes;
    }
}

static uint8_t *l6_rom_flash(L6ChipState *main)
{
    if (main->peer) {
        return memory_region_get_ram_ptr(&main->peer->flash);
    }
    l6_main_map_panel_flash(main);
    return main->rom_shared_flash ? main->rom_shared_flash : main->rom_flash;
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
        memcpy(l6_rom_flash(main) + offset, bytes, count);
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
    if (chip->panel) {
        device_cold_reset(chip->panel_usart1);
    }
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
        timer_del(chip->indicator_timer);
        l6_panel_timer_reset(chip);
        memset(&chip->indicators, 0, sizeof(chip->indicators));
        memset(chip->indicators.published, 0xff,
               sizeof(chip->indicators.published));
        l6_panel_store(chip, 0x400, 0);
        l6_panel_store(chip, 0x2000, 0);
        l6_panel_store(chip, 0x20008, 0);
        l6_panel_store(chip, 0x10000014, 0);
        l6_panel_store(chip, 0x10000414, 0);
        g_hash_table_insert(chip->registers, GUINT_TO_POINTER(1), NULL);
    } else {
        timer_del(chip->rom_uart_timer);
        chip->rom_phase = 0;
        chip->rom_reply_read = chip->rom_reply_visible = chip->rom_reply_write = 0;
        chip->main_rx_pending = false;
    }
    async_run_on_cpu(CPU(chip->armv7m.cpu), l6_chip_cpu_reset,
                     RUN_ON_CPU_HOST_PTR(chip));
}

static void l6_panel_set_held(L6ChipState *panel, bool held)
{
    if (panel->cpu_held == held) {
        return;
    }
    panel->cpu_held = held;
    timer_del(panel->panel_tim2);
    timer_del(panel->panel_tim3);
    timer_del(panel->panel_tim14);
    timer_del(panel->panel_spi_dma);
    timer_del(panel->indicator_timer);
    l6_panel_timer_reset(panel);
    l6_panel_store(panel, 0x400, 0);
    l6_panel_store(panel, 0x2000, 0);
    l6_panel_store(panel, 0x20008, 0);
    memset(&panel->indicators, 0, sizeof(panel->indicators));
    memset(panel->indicators.published, 0xff,
           sizeof(panel->indicators.published));
    l6_panel_store(panel, 0x10000014, 0);
    l6_panel_store(panel, 0x10000414, 0);
    if (panel->peer) {
        timer_del(panel->peer->uart_link->timer);
        timer_del(panel->uart_link->timer);
        g_byte_array_set_size(panel->peer->uart_link->queue, 0);
        g_byte_array_set_size(panel->uart_link->queue, 0);
    } else if (!held && panel->persistent_panel) {
        /* MAIN programs this file directly in a split-process run. */
        address_space_write_rom(CPU(panel->armv7m.cpu)->as, 0x08000000,
                                MEMTXATTRS_UNSPECIFIED,
                                panel->persistent_panel, 0x10000);
    }
    /* Registers outside this sparse peripheral model (NVIC/SysTick and CPU)
     * reset through their QEMU device. Preserve the RTC calendar domain. */
    g_hash_table_insert(panel->registers,
        GUINT_TO_POINTER((0x013800 >> 2) + 1), NULL);
    g_hash_table_insert(panel->registers,
        GUINT_TO_POINTER((0x0000 >> 2) + 1), NULL);
    if (!held) {
        timer_mod(panel->indicator_timer,
                  qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                      L6_INDICATOR_INTERVAL_NS);
    }
    async_run_on_cpu(CPU(panel->armv7m.cpu), l6_chip_cpu_reset,
                     RUN_ON_CPU_HOST_PTR(panel));
}

static void l6_panel_control_read(void *opaque)
{
    L6ChipState *panel = opaque;
    uint8_t command;
    ssize_t n = read(panel->panel_control_fd, &command, 1);

    if (n == 1 && command == L6_PANEL_CONTROL_POWER_OFF) {
        qemu_system_shutdown_request(SHUTDOWN_CAUSE_GUEST_SHUTDOWN);
    } else if (n == 1) {
        l6_panel_set_held(panel, command);
    } else if (n == 0 || (errno != EAGAIN && errno != EINTR)) {
        qemu_set_fd_handler(panel->panel_control_fd, NULL, NULL, NULL);
    }
}

static void l6_panel_control_send(L6ChipState *main, uint8_t command)
{
    ssize_t n;

    do {
        n = write(main->panel_control_fd, &command, 1);
    } while (n < 0 && errno == EINTR);
}

static void l6_update_panel_boot(L6ChipState *main)
{
    bool held = main->panel_reset_asserted || main->main_rom_mode;

    if (main->peer) {
        l6_panel_set_held(main->peer, held);
    } else if (main->panel_control_fd >= 0) {
        l6_panel_control_send(main, held);
    }
}

/* Power is board-wide, and a standalone panel has its own VM to stop. */
static void l6_board_power_off(L6ChipState *main)
{
    if (main->panel_control_fd >= 0) {
        l6_panel_control_send(main, L6_PANEL_CONTROL_POWER_OFF);
    }
    qemu_system_shutdown_request(SHUTDOWN_CAUSE_GUEST_SHUTDOWN);
}

static void l6_rom_uart_tick(void *opaque)
{
    L6ChipState *s = opaque;
    if (!s->main_rom_mode || s->rom_reply_visible == s->rom_reply_write) {
        return;
    }
    s->rom_reply_visible++;
    uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER(((l6_panel_uart_base(s) + 0x18) >> 2) + 1)));
    if (ctrl & (1u << 21)) {
        qemu_irq_raise(qdev_get_gpio_in(DEVICE(&s->armv7m),
                                        l6_panel_uart_irq(s)));
    }
    if (s->rom_reply_visible < s->rom_reply_write) {
        timer_mod(s->rom_uart_timer,
                  qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 100000);
    }
}

/* Optional diagnostic acknowledgments remain available via environment
 * overrides. Normal profiles use the clocked SAI/eDMA paths instead. */
static unsigned l6_sai_audio_clock_advance(L6ChipState *s, unsigned frames);

static unsigned l6_sai_master_sample_rate(L6ChipState *s)
{
    if (!s->profile->audio_source_clock_hz) {
        return 0;
    }
    for (unsigned i = 0; i < s->profile->sai_count; i++) {
        L6Sai *sai = &s->sai[i];
        uint32_t tcr2 = sai->regs[L6_SAI_TCR2 / 4];
        uint32_t tx = sai->regs[L6_SAI_TCSR / 4];
        uint32_t rx = sai->regs[L6_SAI_RCSR / 4];
        if ((tcr2 & L6_SAI_BCD) && ((tx & L6_SAI_TE) || (rx & L6_SAI_RE))) {
            return l6_sai_sample_rate(s->profile->audio_source_clock_hz,
                                      tcr2,
                                      sai->regs[L6_SAI_TCR4 / 4],
                                      sai->regs[L6_SAI_TCR5 / 4]);
        }
    }
    return 0;
}

static void l6_audio_output_flush(L6ChipState *s, size_t available)
{
    /* Queue a little audio before the first write. The guest cannot run ahead
     * of wall time, so this reserve is what absorbs a brief host stall. */
    if (!s->audio_primed) {
        if (s->audio_pending->len <
            s->profile->audio_sample_rate_hz / L6_AUDIO_PRIME_DIVISOR *
            2 * sizeof(int32_t)) {
            return;
        }
        s->audio_primed = true;
    }
    while (s->audio_pending && s->audio_pending->len && available) {
        size_t requested = MIN(s->audio_pending->len, available);
        size_t written = audio_be_write(s->audio_backend, s->audio_voice,
                                        s->audio_pending->data, requested);
        if (!written) {
            break;
        }
        g_byte_array_remove_range(s->audio_pending, 0, written);
        available -= written;
    }
}

static void l6_audio_output_callback(void *opaque, int available)
{
    L6ChipState *s = opaque;
    if (available > 0) {
        l6_audio_output_flush(s, available);
    }
}

static void l6_audio_output_write(L6ChipState *s, const int32_t *samples,
                                  unsigned frames)
{
    if (!frames) {
        return;
    }

    if (s->audio_raw_capture) {
        uint8_t raw[64 * 2 * sizeof(uint32_t)];
        size_t bytes = frames * 2 * sizeof(uint32_t);

        g_assert(bytes <= sizeof(raw));
        for (unsigned frame = 0; frame < frames; frame++) {
            stl_le_p(raw + frame * 2 * sizeof(uint32_t),
                     (uint32_t)samples[frame * 2]);
            stl_le_p(raw + (frame * 2 + 1) * sizeof(uint32_t),
                     (uint32_t)samples[frame * 2 + 1]);
        }
        if (fwrite(raw, 1, bytes, s->audio_raw_capture) != bytes ||
            fflush(s->audio_raw_capture)) {
            qemu_log_mask(LOG_GUEST_ERROR,
                          "l6 raw audio capture write failed: %s\n",
                          strerror(errno));
            fclose(s->audio_raw_capture);
            s->audio_raw_capture = NULL;
        }
    }

    if (!s->audio_voice) {
        return;
    }

    g_byte_array_append(s->audio_pending, (const uint8_t *)samples,
                         frames * 2 * sizeof(*samples));
    l6_audio_output_flush(s, s->audio_pending->len);

    /* Keep the frontend close to guest real time if a backend stalls. */
    const unsigned max_pending = 48000 * 2 * sizeof(*samples);
    if (s->audio_pending->len > max_pending) {
        unsigned drop = s->audio_pending->len - max_pending;
        drop -= drop % (2 * sizeof(*samples));
        g_byte_array_remove_range(s->audio_pending, 0, drop);
        qemu_log_mask(LOG_UNIMP,
                      "l6 audio backend fell behind; discarded %u queued bytes\n",
                      drop);
    }
}

static void l6_audio_sample_trace_tick(L6ChipState *s, unsigned frames)
{
    if (!s->audio_sample_trace) {
        return;
    }
    s->audio_sample_trace_frames += frames;
    if (s->audio_sample_trace_frames < L6_AUDIO_TRACE_WINDOW_FRAMES) {
        return;
    }
    s->audio_sample_trace_frames = 0;
    for (unsigned i = 0; i < s->profile->sai_count; i++) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 audio TX samples sai=%08" HWADDR_PRIx " words=%" PRIu64
                      " nonzero=%" PRIu64 " underruns=%" PRIu64
                      " first=%08x last=%08x at=%" PRId64 "ms\n",
                      s->profile->sai_bases[i],
                      s->audio_tx_words[i], s->audio_tx_nonzero_words[i],
                      s->audio_tx_underruns[i],
                      s->audio_tx_first_nonzero[i], s->audio_tx_last_nonzero[i],
                      qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        s->audio_tx_words[i] = 0;
        s->audio_tx_nonzero_words[i] = 0;
        s->audio_tx_underruns[i] = 0;
        s->audio_tx_first_nonzero[i] = 0;
        s->audio_tx_last_nonzero[i] = 0;
    }
}

static void l6_audio_irq_trace_tick(L6ChipState *s, unsigned frames)
{
    if (!s->audio_trace) {
        return;
    }
    s->audio_trace_irq_elapsed_frames += frames;
    if (s->audio_trace_irq_elapsed_frames < L6_AUDIO_TRACE_WINDOW_FRAMES) {
        return;
    }
    s->audio_trace_irq_elapsed_frames = 0;
    for (unsigned i = 0; i < ARRAY_SIZE(s->audio_trace_irq_count); i++) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 audio eDMA IRQ channel=%u assertions=%" PRIu64
                      " clears=%" PRIu64 " pending=%u at=%" PRId64 "ms\n",
                      i + 12, s->audio_trace_irq_count[i],
                      s->audio_trace_irq_ack_count[i],
                      (GPOINTER_TO_UINT(g_hash_table_lookup(
                           s->registers,
                           GUINT_TO_POINTER((0x0e8024 >> 2) + 1))) >>
                       (i + 12)) & 1,
                      qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        s->audio_trace_irq_count[i] = 0;
        s->audio_trace_irq_ack_count[i] = 0;
    }
}

static void l6_main_audio_service_tick(void *opaque)
{
    L6ChipState *s = opaque;
    uint64_t now = qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL);
    unsigned sample_rate = l6_sai_master_sample_rate(s);
    unsigned frames = 64;
    if (sample_rate) {
        /* Service at most one 64-frame guest block, ending early on an eDMA
         * interrupt. The firmware must see that interrupt before the TX ring
         * advances further, or eDMA reads a half it has not refilled yet. */
        if (s->audio_service_rate_hz != sample_rate) {
            s->audio_service_rate_hz = sample_rate;
            s->audio_service_period_remainder = 0;
            s->audio_service_deadline_ns = now;
        }
        frames = l6_sai_audio_clock_advance(s, 64);
        const uint64_t frame_period_numerator = frames * 1000000000ULL;
        s->audio_service_deadline_ns +=
            frame_period_numerator / sample_rate;
        s->audio_service_period_remainder +=
            frame_period_numerator % sample_rate;
        if (s->audio_service_period_remainder >= sample_rate) {
            s->audio_service_deadline_ns++;
            s->audio_service_period_remainder -= sample_rate;
        }
        timer_mod(s->main_audio_service_timer, s->audio_service_deadline_ns);
    } else {
        s->audio_service_rate_hz = 0;
        s->audio_service_period_remainder = 0;
        s->audio_service_deadline_ns = now + 1000000;
        timer_mod(s->main_audio_service_timer, s->audio_service_deadline_ns);
    }
    l6_audio_sample_trace_tick(s, frames);
    l6_audio_irq_trace_tick(s, frames);
    if (s->service_audio_profile_ramp_ack) {
        uint8_t *ram = memory_region_get_ram_ptr(&s->external_ram);
        if (ldl_le_p(ram + 0xa3e8) == 1) {
            stl_le_p(ram + 0xa3e8, 0);
        }
    }
    if (s->service_audio_dsp_ack && s->profile == &L6_PROFILE &&
        s->profile->high_ram_size >= 0x8cd8) {
        uint8_t *main_high_ram = memory_region_get_ram_ptr(&s->main_high_ram);
        if (ldl_le_p(main_high_ram + 0x8cd4) == 1) {
            /* This explicit override lets probes expose the DSP wait even
             * though normal profiles now advance the SAI/eDMA callback. */
            stl_le_p(main_high_ram + 0x8cd4, 0);
        }
    }
    if (s->service_audio_queue) {
        /* Diagnostic stand-in for the absent audio transfer consumer. The
         * firmware's 15-slot staging queue otherwise fills during setup.
         * The index layout differs between firmware models. Acknowledge the
         * current write index without processing audio data. */
        uint8_t *sram = memory_region_get_ram_ptr(&s->sram);
        uint32_t write_index = ldl_le_p(sram + s->profile->audio_queue_write_offset);
        stl_le_p(sram + s->profile->audio_queue_read_offset, write_index);
    }
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
                        address >= 0x08000000 && address < 0x08004000 ?
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
                (uint64_t)s->rom_address + count <= 0x08004000;
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
                l6_rom_erase_flash(s, 0, 0x4000);
                qemu_log_mask(LOG_UNIMP, "l6 panel ROM mass erase\n");
                l6_rom_reply_byte(s, 0x79);
            } else if (pages < 128) {
                for (unsigned i = 0; i <= pages; i++) {
                    unsigned page = ((unsigned)s->rom_input[2 + 2 * i] << 8) |
                                     s->rom_input[3 + 2 * i];
                    if (page < 0x4000 / 2048) {
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

/* RT USDHC clock/power semantics around QEMU's reusable card and ADMA model.
 * Unlike SDHCI, USDHC derives clock/power from the SoC; SYS_CTRL[2:0]
 * are reserved. Firmware 0x80080fc8 waits for PRES_STATE.SDSTB before
 * programming its divider and writes INITA to issue initialization clocks.
 * Card removal therefore does not gate the card clock: a command to an empty
 * slot completes with a command timeout. PROT_CTRL WECINT/WECINS/WECRM only
 * enable wakeup events, so they are kept out of SDHCI's interrupt decision.
 */
static uint64_t l6_sd_read(void *opaque, hwaddr offset, unsigned size)
{
    L6ChipState *s = opaque;
    if (offset == 0x2c && size == 4) {
        return s->sd_sysctl;
    }
    if (offset == 0xc8 && size == 4) {
        return s->sd_vend_spec2;
    }
    if (offset == 0x28 && size == 4) {
        return s->sd_ops->read(s->sdhc, offset, size) |
               ((uint32_t)s->sd_wakeup << 24);
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
    if (offset == 0xc8 && size == 4) {
        /* VEND_SPEC2 is RW; reserved bits retain their reset values. */
        s->sd_vend_spec2 = (s->sd_vend_spec2 & ~L6_USDHC_VEND_SPEC2_RW_MASK) |
                           ((uint32_t)value & L6_USDHC_VEND_SPEC2_RW_MASK);
        return;
    }
    if (offset == 0x28 && size == 4) {
        s->sd_ops->write(s->sdhc, offset, value & 0x00ffffffu, size);
        s->sd_wakeup = (value >> 24) & 0x07;
        return;
    }
    if (offset == 0x0c) {
        s->sdhc->clkcon |= 7u;
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

static bool l6_audio_trace_register(hwaddr offset)
{
    /* Clock reconstruction needs the SAI roots, PLL_AUDIO setup and the
     * IOMUX GPR MCLK source selectors alongside SAI register writes. */
    if (offset == 0x0fc01c || offset == 0x0fc028 || offset == 0x0fc02c ||
        (offset >= 0x0ac004 && offset < 0x0ac008) ||
        (offset >= 0x0d8070 && offset < 0x0d80c0) ||
        (offset >= 0x0d8030 && offset < 0x0d8040) ||
        offset == L6_CCM_ANALOG_PFD_480 ||
        offset == L6_CCM_ANALOG_PFD_528 ||
        (offset >= 0x3a0000 && offset < 0x3a0100) ||
        (offset >= 0x2a8000 && offset < 0x2a8300)) {
        return true;
    }
    if ((offset >= 0x384000 && offset < 0x384100) ||
        (offset >= 0x388000 && offset < 0x388100) ||
        (offset >= 0x38c000 && offset < 0x38c100) ||
        (offset >= 0x3f0000 && offset < 0x3f0100)) {
        return true;
    }

    /* The L6 audio paths use channels 12–15. L6max also uses eDMA 2–3 for
     * OLED refresh; including every TCD and DMAMUX channel lets display
     * traffic fill the audio trace and hide later SAI/eDMA activity. */
    if (offset >= 0x0e9000 && offset < 0x0e9200) {
        unsigned channel = (offset - 0x0e9000) / 0x20;
        return channel >= 12 && channel <= 15;
    }
    return offset >= 0x0ec030 && offset < 0x0ec040;
}

/* Clock setup registers are read repeatedly while firmware polls PLL locks.
 * Keep those records bounded independently so they cannot hide the SAI and
 * eDMA configuration that explains audio service rates. */
static bool l6_audio_trace_sai_register(hwaddr offset)
{
    return (offset >= 0x384000 && offset < 0x384100) ||
           (offset >= 0x388000 && offset < 0x388100) ||
           (offset >= 0x38c000 && offset < 0x38c100) ||
           (offset >= 0x3f0000 && offset < 0x3f0100);
}

static bool l6_audio_trace_edma_register(hwaddr offset)
{
    return (offset >= 0x0e9000 && offset < 0x0e9200) ||
           (offset >= 0x0ec030 && offset < 0x0ec040) ||
           (offset >= 0x0e801a && offset <= 0x0e8024);
}

static bool l6_audio_trace_allow(L6ChipState *s, hwaddr offset)
{
    if (l6_audio_trace_sai_register(offset)) {
        return s->audio_trace_sai_log_count++ < 20000;
    }
    if (l6_audio_trace_edma_register(offset)) {
        return s->audio_trace_edma_log_count++ < 20000;
    }
    return s->audio_trace_clock_log_count++ < 2000;
}

static bool l6_audio_trace_edma_global(hwaddr offset, uint64_t value,
                                       unsigned size, bool write)
{
    if (write && size == 1 &&
        (offset == 0x0e801a || offset == 0x0e801b ||
         offset == 0x0e801e || offset == 0x0e801f)) {
        /* Keep channel-command writes only for the observed audio channels. */
        return value >= 12 && value <= 15;
    }
    if (offset == 0x0e8024) {
        /* INT is a per-channel pending bitmap; ignore OLED channels 2–3. */
        return (value & 0x0000f000u) != 0;
    }
    return false;
}

static void l6_audio_trace_mmio(const char *operation, hwaddr offset,
                                uint64_t value, unsigned size)
{
    vaddr pc = current_cpu && CPU_GET_CLASS(current_cpu)->get_pc ?
               CPU_GET_CLASS(current_cpu)->get_pc(current_cpu) : 0;
    CPUARMState *env = current_cpu ? (CPUARMState *)cpu_env(current_cpu) : NULL;

    qemu_log_mask(LOG_UNIMP,
                  "l6 audio MMIO %s addr=%08x size=%u value=%08x pc=%08" PRIx64
                  " lr=%08x at=%" PRId64 "ms\n",
                  operation, (unsigned)(0x40000000u + offset), size,
                  (uint32_t)value, (uint64_t)pc, env ? env->regs[14] : 0,
                  qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
}

static L6Sai *l6_sai_at(L6ChipState *s, hwaddr offset, hwaddr *reg_offset)
{
    if (s->panel) {
        return NULL;
    }
    for (unsigned i = 0; i < s->profile->sai_count; i++) {
        hwaddr base = s->profile->sai_bases[i];
        if (offset >= base && offset < base + 0x100) {
            *reg_offset = offset - base;
            return &s->sai[i];
        }
    }
    return NULL;
}

static uint64_t l6_mmio_read(void *opaque, hwaddr offset, unsigned size)
{
    L6ChipState *s = opaque;
    if (!s->panel && offset == 0x0d8010 && size == 4) {
        uint32_t control = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((0x0d8010 >> 2) + 1)));
        return l6_usb1_pll_status(control);
    }
    if (s->profile->resource_flash && offset == 0x3a005c && size == 4) {
        /* The L6 firmware polls FSR.RXCOUNT (bits 16..20) before draining
         * the LPSPI receive data register. */
        return s->lpspi_rx_count << 16;
    }
    if (s->profile->resource_flash && offset == 0x3a0074 && size == 4) {
        if (!s->lpspi_rx_count) {
            return 0;
        }
        uint32_t value = s->lpspi_rx_fifo[s->lpspi_rx_head];
        s->lpspi_rx_head = (s->lpspi_rx_head + 1) % ARRAY_SIZE(s->lpspi_rx_fifo);
        s->lpspi_rx_count--;
        return value;
    }
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
        /* Target 1 is the row-5/column-4 key. Log its selected-row samples,
         * including released samples, to distinguish a missed matrix scan
         * from a later firmware UART/reporting failure. */
        if (s->input_trace && now_ms < s->input_trace_until &&
            offset == columns[4] && s->input_trace_matrix_count < 128) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 panel matrix scan target=1 row_code=%02x down=%u idr=%08x at=%" PRIu64 " ns\n",
                          odr & 0x70, l6_button_down(s, 1), idr,
                          qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL));
            s->input_trace_matrix_count++;
        }
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
                if (s->profile == &L6_PROFILE) {
                    for (unsigned column = 0; column < s->profile->panel_matrix_columns; column++) {
                        unsigned event_id = 7 + row * 3 + column;
                        if (l6_button_down(s, event_id) &&
                            offset == columns[column]) {
                            idr &= ~(1u << pins[column]);
                            if (s->input_trace && now_ms < s->input_trace_until) {
                                qemu_log_mask(LOG_UNIMP,
                                              "l6 panel matrix key target=%u row=%u column=%u idr=%08x at %" PRId64 " ns\n",
                                              event_id, row, column, idr,
                                              qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL));
                            }
                        }
                    }
                } else {
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
        }
        if (s->input_fd >= 0) {
            if (s->profile == &L6_PROFILE) {
                static const struct { unsigned id; hwaddr port; unsigned pin; } direct[] = {
                    { 1, 0x10000810, 13 }, { 2, 0x10000410, 15 },
                    { 3, 0x10000010, 8 }, { 6, 0x10000810, 6 },
                    { 47, 0x10000810, 7 },
                };
                for (unsigned i = 0; i < ARRAY_SIZE(direct); i++) {
                    if (offset == direct[i].port && l6_button_down(s, direct[i].id)) {
                        idr &= ~(1u << direct[i].pin);
                    }
                }
            } else {
                /* Static switch reader 0x08000aa0 samples PA3 outside the matrix. */
                if (offset == 0x10000010 && l6_button_down(s, 47)) {
                    idr &= ~(1u << 3);
                }
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
            for (unsigned encoder = 0; encoder < l6_encoder_count(s); encoder++) {
                uint8_t phase = s->encoder_phase[encoder];
                if (offset == phase_a[encoder].port && !(phase & 2)) {
                    idr &= ~(1u << phase_a[encoder].pin);
                }
                if (offset == phase_b[encoder].port && !(phase & 1)) {
                    idr &= ~(1u << phase_b[encoder].pin);
                }
            }
        }
        return l6_panel_gpio_idr(idr);
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
        /* With no queued TX word, LPSPI3 requests data. */
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
    if (!s->panel && offset == l6_panel_uart_base(s) + 0x14 && size == 4) {
        return (1u << 23) | (1u << 22) |
               ((s->main_rom_mode ? s->rom_reply_read < s->rom_reply_visible :
                  s->main_rx_pending) ? (1u << 21) : 0);
    }
    if (!s->panel && offset == l6_midi_uart_base(s) + 0x14 && size == 4) {
        /* The selected MIDI UART: RDRF follows a byte
         * queued from the inherited socket. */
        return (1u << 23) | (1u << 22) |
               (s->main_midi_rx_pending ? (1u << 21) : 0);
    }
    if (!s->panel && offset == l6_midi_uart_base(s) + 0x1c && size == 4) {
        uint8_t byte = s->main_midi_rx_byte;
        qemu_log_mask(LOG_UNIMP,
                      "l6 main LPUART%u (MIDI jacks) DATA read %02x\n",
                      s->profile->midi_uart_number, byte);
        s->main_midi_rx_pending = false;
        if (s->main_midi_rx->len > 0) {
            s->main_midi_rx_pending = true;
            s->main_midi_rx_byte = s->main_midi_rx->data[0];
            g_byte_array_remove_index(s->main_midi_rx, 0);
            /* Each byte refilled from the queue is its own RDRF edge, the
             * same as a byte arriving fresh from the socket. */
            uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                GUINT_TO_POINTER(((l6_midi_uart_base(s) + 0x18) >> 2) + 1)));
            if (ctrl & (1u << 21)) {
                qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), l6_midi_uart_irq(s)));
            }
        }
        return byte;
    }
    if (!s->panel && size == 4 && offset >= 0x188014 &&
        offset <= 0x1a0014 && (offset - 0x188014) % 0x4000 == 0) {
        /* Unconnected LPUART instances discard TX synchronously and are idle.
         * Serial close loop at 0x800325b8 waits for STAT.TC (bit 22).
         * Reporting zero leaves preparation blocked before Power dispatch.
         */
        return (1u << 23) | (1u << 22);
    }
    if (!s->panel && size == 4 && offset >= 0x18801c &&
        offset <= 0x1a001c && (offset - 0x18801c) % 0x4000 == 0 &&
        offset != l6_midi_uart_base(s) + 0x1c &&
        offset != l6_panel_uart_base(s) + 0x1c) {
        uint32_t value = GPOINTER_TO_UINT(g_hash_table_lookup(
            s->registers, GUINT_TO_POINTER((offset >> 2) + 1)));
        qemu_log_mask(LOG_UNIMP,
                      "l6 main LPUART (other) DATA read +%05lx = %08x\n",
                      (unsigned long)offset, value);
        return value;
    }
    if (!s->panel && offset == l6_panel_uart_base(s) + 0x1c && size == 4) {
        if (s->main_rom_mode) {
            if (s->rom_reply_read < s->rom_reply_visible) {
                uint8_t byte = s->rom_reply[s->rom_reply_read++];
                qemu_log_mask(LOG_UNIMP, "l6 main LPUART%u ROM RX %02x\n",
                              l6_panel_uart_number(s), byte);
                qemu_irq_lower(qdev_get_gpio_in(DEVICE(&s->armv7m),
                                                l6_panel_uart_irq(s)));
                if (s->rom_reply_read == s->rom_reply_write) {
                    s->rom_reply_read = s->rom_reply_visible =
                        s->rom_reply_write = 0;
                }
                return byte;
            }
            return 0;
        }
        s->main_rx_pending = false;
        qemu_log_mask(LOG_UNIMP,
                      "l6 main LPUART%u (panel link) DATA read %02x\n",
                      l6_panel_uart_number(s), s->main_rx_byte);
        l6_uart_accept_input(s);
        return s->main_rx_byte;
    }
    hwaddr sai_offset;
    L6Sai *sai = l6_sai_at(s, offset, &sai_offset);
    if (sai) {
        uint32_t value = l6_sai_read(sai, sai_offset, size);
        if (s->audio_trace && l6_audio_trace_register(offset) &&
            l6_audio_trace_allow(s, offset)) {
            l6_audio_trace_mmio("read", offset, value, size);
        }
        return value;
    }
    if (!s->panel && offset == 0x0e800c && size == 4) {
        return s->edma_erq;
    }
    if (!s->panel && offset == 0x0e801f && size == 1) {
        return 0; /* CINT is write-only. */
    }
    uint32_t word = GPOINTER_TO_UINT(g_hash_table_lookup(
        s->registers, GUINT_TO_POINTER((offset >> 2) + 1)));
    unsigned shift = (offset & 3) * 8;
    uint32_t value = size == 4 ? word :
                     (word >> shift) & ((1u << (size * 8)) - 1);
    if (s->audio_trace && !s->panel &&
        (l6_audio_trace_register(offset) ||
         l6_audio_trace_edma_global(offset, value, size, false)) &&
        l6_audio_trace_allow(s, offset)) {
        l6_audio_trace_mmio("read", offset, value, size);
    }
    return value;
}

/* eDMA assigns one NVIC input per channel. The L6 firmware's IRQ table maps
 * channels 12–15 to IRQs 12–15, and L6max's OLED channel 3 uses IRQ 3. */
static void l6_edma_raise_interrupt(L6ChipState *s, unsigned channel)
{
    gpointer key = GUINT_TO_POINTER((0x0e8024 >> 2) + 1);
    uint32_t pending = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));

    g_hash_table_insert(s->registers, key,
                        GUINT_TO_POINTER(pending | (1u << channel)));
    if (s->audio_trace && channel >= 12 && channel <= 15) {
        s->audio_trace_irq_count[channel - 12]++;
    }
    /* RT eDMA INT bits directly drive the interrupt controller. Hold each
     * channel high until firmware clears its pending bit through CINT/INT. */
    qemu_set_irq(qdev_get_gpio_in(DEVICE(&s->armv7m), channel), 1);
    s->audio_edma_irq_raised = true;
}

static void l6_edma_update_irq_level(L6ChipState *s, unsigned channel,
                                     uint32_t pending)
{
    qemu_set_irq(qdev_get_gpio_in(DEVICE(&s->armv7m), channel),
                 (pending >> channel) & 1);
}

static bool l6_edma_as_read(void *opaque, uint32_t address, void *buffer,
                            size_t size)
{
    AddressSpace *as = opaque;
    return address_space_read(as, address, MEMTXATTRS_UNSPECIFIED,
                              buffer, size) == MEMTX_OK;
}

static bool l6_edma_as_write(void *opaque, uint32_t address,
                             const void *buffer, size_t size)
{
    AddressSpace *as = opaque;
    return address_space_write(as, address, MEMTXATTRS_UNSPECIFIED,
                               buffer, size) == MEMTX_OK;
}

static uint32_t l6_edma_register(L6ChipState *s, hwaddr offset)
{
    return GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
        GUINT_TO_POINTER((offset >> 2) + 1)));
}

static bool l6_sai_edma_request_active(L6ChipState *s, uint8_t request)
{
    for (unsigned i = 0; i < s->profile->sai_count; i++) {
        hwaddr base = s->profile->sai_bases[i];
        bool tx = request == (base == 0x384000 ? 20 : 84);
        bool rx = request == (base == 0x384000 ? 19 : 83);
        if (tx) {
            uint32_t control = l6_sai_read(&s->sai[i], L6_SAI_TCSR, 4);
            if ((control & (L6_SAI_TE | L6_SAI_FRDE | L6_SAI_FRF)) ==
                (L6_SAI_TE | L6_SAI_FRDE | L6_SAI_FRF)) {
                return true;
            }
        } else if (rx) {
            uint32_t control = l6_sai_read(&s->sai[i], L6_SAI_RCSR, 4);
            if ((control & (L6_SAI_RE | L6_SAI_FRDE | L6_SAI_FRF)) ==
                (L6_SAI_RE | L6_SAI_FRDE | L6_SAI_FRF)) {
                return true;
            }
        }
    }
    return false;
}

/* A DMAMUX request services exactly one RT1041 32-bit-TCD minor loop. The
 * peripheral request is the pacing edge; this routine never drains an entire
 * cyclic audio ring just because ERQ was set. */
static bool l6_edma_service_request(L6ChipState *s, uint8_t request)
{
    AddressSpace *as = CPU(s->armv7m.cpu)->as;
    bool serviced = false;

    if (!l6_sai_edma_request_active(s, request)) {
        return false;
    }

    for (unsigned channel = 0; channel < 32; channel++) {
        hwaddr mux_offset = 0x0ec000 + channel * 4;
        uint32_t mux = l6_edma_register(s, mux_offset);
        if (!(mux & (1u << 31)) || (mux & 0x7f) != request ||
            !(s->edma_erq & (1u << channel))) {
            continue;
        }

        uint32_t peripheral = request == 20 ? 0x40384020 :
                              request == 84 ? 0x4038c020 :
                              request == 19 ? 0x403840a0 : 0x4038c0a0;
        uint8_t raw[L6_EDMA_TCD_BYTES];
        uint32_t live_tcd = 0x400e9000 + channel * L6_EDMA_TCD_BYTES;
        if (!l6_edma_as_read(as, live_tcd, raw, sizeof(raw))) {
            return false;
        }
        L6EdmaTcd tcd;
        l6_edma_tcd_decode(raw, &tcd);
        /* Hardware clears DONE and START when it accepts a channel request. */
        tcd.csr &= ~(L6_EDMA_TCD_DONE | 1u);
        if ((request == 19 || request == 83) ? tcd.saddr != peripheral :
                                                tcd.daddr != peripheral) {
            continue;
        }

        /* A request remains asserted while its FIFO threshold remains true.
         * Bound one service turn to the modeled FIFO depth. */
        for (unsigned minor = 0; minor < L6_SAI_FIFO_DEPTH &&
             l6_sai_edma_request_active(s, request); minor++) {
            if (s->audio_dma_source_capture && channel == 14 && request == 20) {
                uint8_t source[L6_EDMA_TCD_BYTES * 128];
                uint8_t record[8];
                uint32_t size = tcd.nbytes;
                if (size <= sizeof(source) &&
                    l6_edma_as_read(as, tcd.saddr, source, size)) {
                    if (!s->audio_dma_source_capture_header_written) {
                        static const uint8_t header[4] = { 'L', '6', 'D', 'M' };
                        uint8_t metadata[12];
                        stl_le_p(metadata, s->profile->audio_sample_rate_hz);
                        stl_le_p(metadata + 4, s->profile->sai_bases[0]);
                        stl_le_p(metadata + 8, 1); /* address/length records */
                        if (fwrite(header, 1, sizeof(header),
                                   s->audio_dma_source_capture) != sizeof(header) ||
                            fwrite(metadata, 1, sizeof(metadata),
                                   s->audio_dma_source_capture) != sizeof(metadata)) {
                            qemu_log_mask(LOG_GUEST_ERROR,
                                          "l6 SAI DMA source capture header write failed: %s\n",
                                          strerror(errno));
                            fclose(s->audio_dma_source_capture);
                            s->audio_dma_source_capture = NULL;
                        } else {
                            s->audio_dma_source_capture_header_written = true;
                        }
                    }
                    if (s->audio_dma_source_capture) {
                        stl_le_p(record, tcd.saddr);
                        stl_le_p(record + 4, size);
                        if (fwrite(record, 1, sizeof(record),
                                   s->audio_dma_source_capture) != sizeof(record) ||
                            fwrite(source, 1, size,
                                   s->audio_dma_source_capture) != size) {
                            qemu_log_mask(LOG_GUEST_ERROR,
                                          "l6 SAI DMA source capture write failed: %s\n",
                                          strerror(errno));
                            fclose(s->audio_dma_source_capture);
                            s->audio_dma_source_capture = NULL;
                        }
                    }
                }
            }
            L6EdmaMinorResult result = l6_edma_transfer_minor(
                &tcd, l6_edma_as_read, l6_edma_as_write, as);
            if (result == L6_EDMA_MINOR_ERROR) {
                qemu_log_mask(LOG_UNIMP,
                              "l6 eDMA unsupported request source=%u channel=%u TCD=%08x/%08x nbytes=%u citer=%04x attr=%04x\n",
                              request, channel, tcd.saddr, tcd.daddr,
                              tcd.nbytes, tcd.citer, tcd.attr);
                return false;
            }

            if (result == L6_EDMA_MINOR_MAJOR &&
                (tcd.csr & L6_EDMA_TCD_ESG)) {
                uint32_t next = (uint32_t)tcd.dlast_sga;
                if ((next & (L6_EDMA_TCD_BYTES - 1)) ||
                    !l6_edma_as_read(as, next, raw, sizeof(raw))) {
                    return false;
                }
            } else {
                if (result == L6_EDMA_MINOR_MAJOR) {
                    if (l6_edma_complete_major(&tcd)) {
                        s->edma_erq &= ~(1u << channel);
                    }
                }
                l6_edma_tcd_encode(raw, &tcd);
            }
            if (!l6_edma_as_write(as, live_tcd, raw, sizeof(raw))) {
                return false;
            }
            if ((result == L6_EDMA_MINOR_HALF &&
                 (tcd.csr & L6_EDMA_TCD_INTHALF)) ||
                (result == L6_EDMA_MINOR_MAJOR &&
                 (tcd.csr & L6_EDMA_TCD_INTMAJOR))) {
                l6_edma_raise_interrupt(s, channel);
            }
            serviced = true;
            if (result == L6_EDMA_MINOR_MAJOR &&
                !(tcd.csr & L6_EDMA_TCD_ESG)) {
                break;
            }
            if (result == L6_EDMA_MINOR_MAJOR &&
                !l6_edma_as_read(as, live_tcd, raw, sizeof(raw))) {
                return false;
            }
            if (result == L6_EDMA_MINOR_MAJOR) {
                l6_edma_tcd_decode(raw, &tcd);
            }
        }
    }
    return serviced;
}

/* The stock firmware configures these SAI streams for 48 kHz. Its RT1041
 * SAI3 (L6) or SAI1 (L6max) master supplies the frame clock; synchronous
 * slave instances on L6 share that clock. Each frame advances the configured
 * TDM word count, then lets the active FIFO watermark pace eDMA. */
static unsigned l6_sai_audio_clock_advance(L6ChipState *s, unsigned frames)
{
    bool master_clock = false;
    for (unsigned i = 0; i < s->profile->sai_count; i++) {
        L6Sai *sai = &s->sai[i];
        uint32_t tcr2 = sai->regs[L6_SAI_TCR2 / 4];
        uint32_t tx = sai->regs[L6_SAI_TCSR / 4];
        uint32_t rx = sai->regs[L6_SAI_RCSR / 4];
        if ((tcr2 & L6_SAI_BCD) &&
            ((tx & L6_SAI_TE) || (rx & L6_SAI_RE))) {
            master_clock = true;
            break;
        }
    }
    if (!master_clock) {
        return frames;
    }

    uint32_t silence[L6_SAI_CHANNELS * L6_SAI_FIFO_DEPTH] = { 0 };
    uint32_t tx_samples[L6_SAI_FIFO_DEPTH];
    int32_t audio_frames[64 * 2];
    uint32_t tdm_frames[64][L6_SAI_FIFO_DEPTH] = { 0 };
    unsigned audio_frame_count = 0;
    unsigned frame;
    s->audio_edma_irq_raised = false;
    for (frame = 0; frame < frames && !s->audio_edma_irq_raised; frame++) {
        for (unsigned i = 0; i < s->profile->sai_count; i++) {
            L6Sai *sai = &s->sai[i];
            unsigned words = l6_sai_frame_words(sai, true);
            if (words > L6_SAI_FIFO_DEPTH) {
                words = L6_SAI_FIFO_DEPTH;
            }
            unsigned rx_words = l6_sai_frame_words(sai, false);
            if (rx_words > L6_SAI_FIFO_DEPTH) {
                rx_words = L6_SAI_FIFO_DEPTH;
            }
            uint32_t tx = l6_sai_read(sai, L6_SAI_TCSR, 4);
            uint32_t rx = l6_sai_read(sai, L6_SAI_RCSR, 4);
            uint8_t tx_request = s->profile->sai_bases[i] == 0x384000 ? 20 : 84;
            uint8_t rx_request = s->profile->sai_bases[i] == 0x384000 ? 19 : 83;

            if (tx & L6_SAI_TE) {
                if (s->audio_sample_trace) {
                    uint32_t enabled = l6_sai_channel_mask(sai, true);
                    uint32_t masked = sai->regs[L6_SAI_TMR / 4];
                    for (unsigned channel = 0; channel < L6_SAI_CHANNELS;
                         channel++) {
                        if (!(enabled & (1u << channel))) {
                            continue;
                        }
                        unsigned required = 0;
                        for (unsigned slot = 0; slot < words; slot++) {
                            required += !(masked & (1u << slot));
                        }
                        if (required > sai->tx_count[channel]) {
                            s->audio_tx_underruns[i] +=
                                required - sai->tx_count[channel];
                        }
                    }
                }
                words = l6_sai_tx_frame(sai, tx_samples, words);
                if (i == 0 && s->audio_tdm_capture) {
                    if (!s->audio_tdm_capture_slots) {
                        s->audio_tdm_capture_slots = words;
                    }
                    for (unsigned slot = 0;
                         slot < MIN(words, s->audio_tdm_capture_slots); slot++) {
                        tdm_frames[frame][slot] = tx_samples[slot];
                    }
                }
                /* Firmware may leave leading TDM slots silent before the
                 * stereo pair routed to the product's output. */
                unsigned left_slot = s->profile->audio_output_left_slot;
                unsigned right_slot = s->profile->audio_output_right_slot;
                if (i == 0 && (s->audio_voice || s->audio_raw_capture) &&
                    words > MAX(left_slot, right_slot) &&
                    audio_frame_count < ARRAY_SIZE(audio_frames) / 2) {
                    /* These are already signed S32 samples at the firmware's
                     * playback gain. Rescaling here clips the source waveform. */
                    audio_frames[audio_frame_count * 2] =
                        (int32_t)tx_samples[left_slot];
                    audio_frames[audio_frame_count * 2 + 1] =
                        (int32_t)tx_samples[right_slot];
                    audio_frame_count++;
                }
                if (s->audio_sample_trace) {
                    for (unsigned word = 0; word < words; word++) {
                        uint32_t sample = tx_samples[word];
                        s->audio_tx_words[i]++;
                        /* The uninitialized SAI FIFO sentinel is all ones;
                         * do not count it as nonzero audio or let it hide the
                         * first/last real sample in this trace window. */
                        if (sample && sample != UINT32_MAX) {
                            if (!s->audio_tx_nonzero_words[i]) {
                                s->audio_tx_first_nonzero[i] = sample;
                            }
                            s->audio_tx_nonzero_words[i]++;
                            s->audio_tx_last_nonzero[i] = sample;
                            if (!s->audio_tx_probe_logged[i] &&
                                sample != UINT32_MAX) {
                                s->audio_tx_probe[i][s->audio_tx_probe_count[i]++] =
                                    sample;
                                if (s->audio_tx_probe_count[i] ==
                                    ARRAY_SIZE(s->audio_tx_probe[i])) {
                                    qemu_log_mask(LOG_UNIMP,
                                                  "l6 audio TX probe sai=%08" HWADDR_PRIx
                                                  " words=%08x,%08x,%08x,%08x,%08x,%08x,%08x,%08x\n",
                                                  s->profile->sai_bases[i],
                                                  s->audio_tx_probe[i][0],
                                                  s->audio_tx_probe[i][1],
                                                  s->audio_tx_probe[i][2],
                                                  s->audio_tx_probe[i][3],
                                                  s->audio_tx_probe[i][4],
                                                  s->audio_tx_probe[i][5],
                                                  s->audio_tx_probe[i][6],
                                                  s->audio_tx_probe[i][7]);
                                    s->audio_tx_probe_logged[i] = true;
                                }
                            }
                        }
                    }
                }
                /* TX FIFO requests are level-triggered from the post-shift
                 * FIFO count. Re-read status after consuming this frame so
                 * eDMA refills an empty FIFO before the next frame starts. */
                if (l6_sai_read(sai, L6_SAI_TCSR, 4) & L6_SAI_FRDE) {
                    l6_edma_service_request(s, tx_request);
                }
            }
            if (rx & L6_SAI_RE) {
                l6_sai_rx_frame(sai, silence, rx_words);
                /* RX requests assert after the frame has been written into
                 * the FIFO, so evaluate the updated count for this edge. */
                if (l6_sai_read(sai, L6_SAI_RCSR, 4) & L6_SAI_FRDE) {
                    l6_edma_service_request(s, rx_request);
                }
            }
        }
    }
    l6_audio_output_write(s, audio_frames, audio_frame_count);
    if (s->audio_tdm_capture && s->audio_tdm_capture_slots) {
        uint8_t raw_tdm[64 * L6_SAI_FIFO_DEPTH * sizeof(uint32_t)];
        if (ftell(s->audio_tdm_capture) == 0) {
            static const uint8_t header[4] = { 'L', '6', 'T', 'D' };
            uint8_t metadata[12];
            stl_le_p(metadata, s->profile->audio_sample_rate_hz);
            stl_le_p(metadata + 4, s->audio_tdm_capture_slots);
            stl_le_p(metadata + 8, 1); /* signed 32-bit little-endian samples */
            if (fwrite(header, 1, sizeof(header), s->audio_tdm_capture) !=
                    sizeof(header) ||
                fwrite(metadata, 1, sizeof(metadata), s->audio_tdm_capture) !=
                    sizeof(metadata)) {
                qemu_log_mask(LOG_GUEST_ERROR,
                              "l6 TDM capture header write failed: %s\n",
                              strerror(errno));
                fclose(s->audio_tdm_capture);
                s->audio_tdm_capture = NULL;
                return frame;
            }
        }
        unsigned captured_frames = MIN(frame, ARRAY_SIZE(tdm_frames));
        if (s->audio_tdm_capture_limit_frames) {
            uint64_t remaining = s->audio_tdm_capture_limit_frames -
                                 s->audio_tdm_capture_frames_written;
            captured_frames = MIN((uint64_t)captured_frames, remaining);
        }
        for (unsigned row = 0; row < captured_frames; row++) {
            for (unsigned slot = 0; slot < s->audio_tdm_capture_slots; slot++) {
                stl_le_p(raw_tdm +
                             (row * s->audio_tdm_capture_slots + slot) *
                                 sizeof(uint32_t),
                         tdm_frames[row][slot]);
            }
        }
        size_t bytes = captured_frames * s->audio_tdm_capture_slots *
                       sizeof(uint32_t);
        if (fwrite(raw_tdm, 1, bytes, s->audio_tdm_capture) != bytes) {
            qemu_log_mask(LOG_GUEST_ERROR,
                          "l6 TDM capture write failed: %s\n",
                          strerror(errno));
            fclose(s->audio_tdm_capture);
            s->audio_tdm_capture = NULL;
        } else {
            s->audio_tdm_capture_frames_written += captured_frames;
            if (s->audio_tdm_capture_limit_frames &&
                s->audio_tdm_capture_frames_written >=
                    s->audio_tdm_capture_limit_frames) {
                qemu_log_mask(LOG_UNIMP,
                              "l6 TDM capture reached its configured frame limit: %" PRIu64 "\n",
                              s->audio_tdm_capture_frames_written);
                fclose(s->audio_tdm_capture);
                s->audio_tdm_capture = NULL;
            }
        }
    }
    return frame;
}

/* DMAMUX routing and ERQ may be enabled after SAI has already asserted its
 * FIFO request. Recheck each profile route after either side is programmed. */
static void l6_edma_service_active_sai(L6ChipState *s)
{
    for (unsigned i = 0; i < s->profile->sai_count; i++) {
        uint8_t tx = s->profile->sai_bases[i] == 0x384000 ? 20 : 84;
        uint8_t rx = s->profile->sai_bases[i] == 0x384000 ? 19 : 83;
        l6_edma_service_request(s, tx);
        l6_edma_service_request(s, rx);
    }
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
    if (s->profile->oled) {
        static unsigned display_trace_count;
        if (g_getenv("L6_DISPLAY_TRACE") && display_trace_count++ < 256) {
            GString *hex = g_string_sized_new(wire_size * 3);
            for (unsigned i = 0; i < wire_size && i < 64; i++) {
                g_string_append_printf(hex, "%s%02x", i ? " " : "", wire[i]);
            }
            qemu_log_mask(LOG_UNIMP,
                          "l6 OLED SPI dc=%u bytes=%u rx=%02x data=%s%s\n",
                          s->display_dc, wire_size, rx_byte, hex->str,
                          wire_size > 64 ? " ..." : "");
            g_string_free(hex, TRUE);
        }
        l6_display_bytes(&s->display, s->display_dc, wire, wire_size);
        l6_display_flush(&s->display);
    }
    s->edma_erq &= ~0xc;
    if (rx_csr & 2) {
        l6_edma_raise_interrupt(s, 3);
    }
    return true;
}

static void l6_lpspi_flash_tx_byte(L6ChipState *s, uint8_t byte)
{
    if (!s->lpspi_flash_header_bytes ||
        (s->lpspi_flash_header_bytes >= 5 &&
         (byte == 0x05 || byte == 0x0b))) {
        s->lpspi_flash_opcode = byte;
        s->lpspi_flash_header_bytes = byte == 0x0b ? 1 : 5;
        s->lpspi_flash_address = 0;
        return;
    }
    if (s->lpspi_flash_opcode == 0x0b && s->lpspi_flash_header_bytes < 5) {
        if (s->lpspi_flash_header_bytes <= 3) {
            s->lpspi_flash_address = (s->lpspi_flash_address << 8) | byte;
        }
        s->lpspi_flash_header_bytes++;
    }
}

static uint8_t l6_lpspi_flash_rx_byte(L6ChipState *s)
{
    if (s->lpspi_flash_opcode == 0x05) {
        return 0; /* WIP/WEL are clear for the read-only resource image. */
    }
    if (s->lpspi_flash_opcode == 0x0b &&
        s->lpspi_flash_header_bytes >= 5) {
        uint32_t address = s->lpspi_flash_address++;
        uint8_t value = address < s->resource_flash_size ?
                        s->resource_flash[address] : 0xff;
        if (s->resource_flash_log_count++ < 16) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 resource NOR read %06x = %02x\n",
                          address, value);
        }
        return value;
    }
    return 0xff;
}

static unsigned l6_lpspi_frame_bytes(uint32_t tcr)
{
    return ((tcr & 0xfffu) + 8u) / 8u;
}

static void l6_rtc_store(L6ChipState *s);

static void l6_mmio_write(void *opaque, hwaddr offset,
                          uint64_t value, unsigned size)
{
    L6ChipState *s = opaque;
    bool store_word = true;
    if (s->audio_trace && !s->panel &&
        (l6_audio_trace_register(offset) ||
         l6_audio_trace_edma_global(offset, value, size, true)) &&
        l6_audio_trace_allow(s, offset)) {
        l6_audio_trace_mmio("write", offset, value, size);
    }
    gpointer key = GUINT_TO_POINTER((offset >> 2) + 1);
    uint32_t word = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));
    uint32_t previous_word = word;
    unsigned shift = (offset & 3) * 8;
    uint32_t mask = size == 4 ? UINT32_MAX : ((1u << (size * 8)) - 1) << shift;
    word = (word & ~mask) | (((uint32_t)value << shift) & mask);
    if (!s->panel && size == 4 && offset >= 0x0d8010 &&
        offset <= 0x0d801c && !(offset & 3)) {
        gpointer control_key = GUINT_TO_POINTER((0x0d8010 >> 2) + 1);
        uint32_t control = GPOINTER_TO_UINT(g_hash_table_lookup(
            s->registers, control_key));
        L6Usb1PllAlias alias = offset == 0x0d8014 ? L6_USB1_PLL_SET :
                               offset == 0x0d8018 ? L6_USB1_PLL_CLEAR :
                               offset == 0x0d801c ? L6_USB1_PLL_TOGGLE :
                               L6_USB1_PLL_DIRECT;
        control = l6_usb1_pll_apply(control, value, alias);
        g_hash_table_insert(s->registers, control_key,
                            GUINT_TO_POINTER(control));
        if (alias == L6_USB1_PLL_DIRECT) {
            word = control;
        } else {
            store_word = false; /* SET/CLR/TOG addresses are write-only. */
        }
    }
    hwaddr sai_offset;
    L6Sai *sai = l6_sai_at(s, offset, &sai_offset);
    if (sai) {
        l6_sai_write(sai, sai_offset, value, size);
        hwaddr sai_base = offset - sai_offset;
        if (sai_offset == L6_SAI_TCSR) {
            uint32_t control = l6_sai_read(sai, sai_offset, 4);
            if ((control & (L6_SAI_TE | L6_SAI_FRDE | L6_SAI_FRF)) ==
                (L6_SAI_TE | L6_SAI_FRDE | L6_SAI_FRF)) {
                uint8_t source = sai_base == 0x384000 ? 20 : 84;
                l6_edma_service_request(s, source);
            }
        } else if (sai_offset == L6_SAI_RCSR) {
            uint32_t control = l6_sai_read(sai, sai_offset, 4);
            if ((control & (L6_SAI_RE | L6_SAI_FRDE | L6_SAI_FRF)) ==
                (L6_SAI_RE | L6_SAI_FRDE | L6_SAI_FRF)) {
                uint8_t source = sai_base == 0x384000 ? 19 : 83;
                l6_edma_service_request(s, source);
            }
        }
    }
    if (!s->panel && offset == 0x1bc018 && size == 4) {
        word = previous_word & ~(uint32_t)value; /* GPIO ISR is W1C */
    }
    if (s->panel && offset >= 0x10000000 && offset < 0x10001800) {
        hwaddr base = offset & ~0x3ff;
        unsigned reg = offset & 0x3ff;
        if ((reg >= 0x14 && reg < 0x1c) ||
            (reg >= 0x28 && reg < 0x2c)) {
            uint32_t old = l6_panel_reg(s, base + 0x14);
            uint32_t next = old;
            if ((reg & ~3u) == 0x18) {
                uint32_t bits = (uint32_t)value << shift;
                next = l6_panel_gpio_bsrr(old, bits);
                word = 0; /* BSRR is a command register, not output storage. */
            } else if ((reg & ~3u) == 0x28) {
                uint32_t bits = (uint32_t)value << shift;
                next = l6_panel_gpio_brr(old, bits);
                word = 0; /* BRR is write-only. */
            } else {
                next = word & 0xffff;
            }
            l6_panel_store(s, base + 0x14, next);
            if (base == 0x10000000 || base == 0x10000400) {
                l6_indicators_pins(&s->indicators,
                    l6_panel_reg(s, 0x10000014),
                    l6_panel_reg(s, 0x10000414),
                    qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL));
            }
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
    /* The L6 firmware polls MCR0.SWRESET for bit 0 to clear before it
     * configures the rest of the FlexSPI controller. */
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
    if (s->profile->resource_flash && offset == 0x3a0064 && size == 4) {
        /* CPU-driven L6 SPI-window transfers read the external resource NOR.
         * Decode the observed 0x0b fast-read and 0x05 status commands; the
         * display is handled separately by the eDMA path below. */
        gpointer sr_key = GUINT_TO_POINTER((0x3a0014 >> 2) + 1);
        uint32_t sr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                            sr_key));
        g_hash_table_insert(s->registers, sr_key,
                            GUINT_TO_POINTER(sr | 1u | (1u << 10)));
        uint32_t tcr = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER((0x3a0060 >> 2) + 1)));
        unsigned bytes = l6_lpspi_frame_bytes(tcr);
        for (unsigned i = 0; i < bytes; i++) {
            uint8_t tx = value >> (i * 8);
            l6_lpspi_flash_tx_byte(s, tx);
            /* TCR.RXMSK is bit 19. Bit 20 is CONT and must not mask RX. */
            if (!(tcr & (1u << 19)) &&
                s->lpspi_rx_count < ARRAY_SIZE(s->lpspi_rx_fifo)) {
                unsigned tail = (s->lpspi_rx_head + s->lpspi_rx_count) %
                                ARRAY_SIZE(s->lpspi_rx_fifo);
                s->lpspi_rx_fifo[tail] = l6_lpspi_flash_rx_byte(s);
                s->lpspi_rx_count++;
            }
        }
    }
    if (!s->panel && offset == 0x3f0014 && size == 4) {
        /* LPI2C1 MSR event/error flags are write-one-to-clear. Treating the
         * firmware's 0x7f00 clear as stored data invents PLTF (bit 13) and
         * makes every startup transaction fail with a pin-low timeout.
         */
        word = previous_word & ~((uint32_t)value & 0x7f00u);
    }
    if (!s->panel && offset == 0x3f0060 && (size == 2 || size == 4)) {
        /* Execute the observed START/address, data, and STOP command stream
         * against QEMU's I2C bus and the L6max-compatible codec slaves. */
        l6_lpi2c_mtdr_write(s, value);
    }
    if (offset == 0x0e8024 && size == 4) {
        /* eDMA INT is write-one-to-clear. */
        uint32_t old = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers, key));
        word = old & ~(uint32_t)value;
        for (unsigned channel = 0; channel < 32; channel++) {
            if ((uint32_t)value & (1u << channel)) {
                if (s->audio_trace && channel >= 12 && channel <= 15 &&
                    (old & (1u << channel))) {
                    s->audio_trace_irq_ack_count[channel - 12]++;
                }
                l6_edma_update_irq_level(s, channel, word);
            }
        }
    }
    if (!s->panel && offset == 0x0e801f && size == 1) {
        /* CINT bit 7 is NOP, bit 6 clears every channel, otherwise bits 4:0
         * select the single channel whose request is being acknowledged. */
        word = previous_word; /* CINT is write-only, not stored state. */
        if (!(value & 0x80)) {
            gpointer int_key = GUINT_TO_POINTER((0x0e8024 >> 2) + 1);
            uint32_t pending = GPOINTER_TO_UINT(g_hash_table_lookup(
                s->registers, int_key));
            uint32_t clear_mask = (value & 0x40) ? UINT32_MAX :
                                  1u << (value & 0x1f);
            for (unsigned channel = 12; channel <= 15; channel++) {
                if (s->audio_trace && (clear_mask & pending & (1u << channel))) {
                    s->audio_trace_irq_ack_count[channel - 12]++;
                }
            }
            pending &= ~clear_mask;
            g_hash_table_insert(s->registers, int_key,
                                GUINT_TO_POINTER(pending));
            for (unsigned channel = 0; channel < 32; channel++) {
                if (clear_mask & (1u << channel)) {
                    l6_edma_update_irq_level(s, channel, pending);
                }
            }
        }
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
    if (s->panel && ((offset & ~3u) == 0x10 ||
                     (offset & ~3u) == 0x410 ||
                     (offset & ~3u) == 0x2010)) {
        /* TIMx_SR flags clear on zero; ones leave existing flags unchanged. */
        word = l6_panel_timer_clear_status(previous_word, word, mask);
    }
    if (!sai && store_word) {
        g_hash_table_insert(s->registers, key, GUINT_TO_POINTER(word));
    }
    if (s->panel) {
        static const hwaddr timer_bases[] = { 0, 0x400, 0x2000 };
        static const unsigned timer_irqs[] = { 15, 16, 19 };
        QEMUTimer *timers[] = { s->panel_tim2, s->panel_tim3, s->panel_tim14 };
        for (unsigned i = 0; i < ARRAY_SIZE(timer_bases); i++) {
            hwaddr base = timer_bases[i];
            hwaddr reg = offset & ~3u;
            if (reg == base + 0x14) {
                /* UG is self-clearing and resets the counter and prescaler. */
                if (((uint32_t)value << shift) & 1u) {
                    uint32_t cr1 = l6_panel_reg(s, base);
                    uint32_t status = l6_panel_reg(s, base + 0x10);
                    uint32_t counter = (i != 2 && (cr1 & (1u << 4))) ?
                                       l6_panel_reg(s, base + 0x2c) : 0;
                    l6_panel_store(s, base + 0x24, counter);
                    l6_panel_timer_update(&cr1, &status, true);
                    l6_panel_store(s, base, cr1);
                    l6_panel_store(s, base + 0x10, status);
                    timer_del(timers[i]);
                    if ((cr1 & 1) && !s->cpu_held) {
                        int64_t period = i == 0 ? l6_panel_tim2_period(s) :
                                         i == 1 ? l6_panel_tim3_period(s) :
                                                  l6_panel_tim14_period(s);
                        timer_mod(timers[i], qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + period);
                    }
                }
                l6_panel_store(s, base + 0x14, 0);
                l6_panel_timer_irq(s, base, timer_irqs[i]);
            } else if (reg == base + 0x0c || reg == base + 0x10) {
                l6_panel_timer_irq(s, base, timer_irqs[i]);
            }
        }
    }
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
    if (s->panel && (offset == 0x28 || offset == 0x2c)) {
        uint32_t cr1 = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
                                                         GUINT_TO_POINTER(1)));
        if ((cr1 & 1) && !s->cpu_held) {
            timer_mod(s->panel_tim2,
                      qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + l6_panel_tim2_period(s));
        }
    }
    if (!s->panel && offset == l6_panel_uart_base(s) + 0x18 && size == 4) {
        if (s->main_ctrl_log_count++ < 40) {
            qemu_log_mask(LOG_UNIMP, "l6 main LPUART%u (panel link) CTRL %08x\n",
                          l6_panel_uart_number(s), word);
        }
        if ((word & (1u << 23)) && !(previous_word & (1u << 23))) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m),
                                            l6_panel_uart_irq(s)));
        }
        if (s->main_rx_pending && (word & (1u << 21)) &&
            !(previous_word & (1u << 21))) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m),
                                            l6_panel_uart_irq(s)));
        }
        if (s->main_rom_mode && s->rom_reply_read < s->rom_reply_visible) {
            qemu_set_irq(qdev_get_gpio_in(DEVICE(&s->armv7m),
                                          l6_panel_uart_irq(s)),
                         !!(word & (1u << 21)));
        }
    }
    if (!s->panel && offset == l6_panel_uart_base(s) + 0x1c && size == 4) {
        uint8_t byte = value;
        s->main_tx_count++;
        vaddr pc = current_cpu && CPU_GET_CLASS(current_cpu)->get_pc ?
                   CPU_GET_CLASS(current_cpu)->get_pc(current_cpu) : 0;
        qemu_log_mask(LOG_UNIMP,
                      "l6 main LPUART%u (panel link) TX %02x at=%" PRId64
                      "ms pc=%08" PRIx64 "\n",
                      l6_panel_uart_number(s), byte,
                      qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL), (uint64_t)pc);
        if (byte == 0xb1) {
            qemu_log_mask(LOG_UNIMP, "l6 main RTC request at %" PRId64 " ms\n",
                          qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        }
        if (s->main_rom_mode) {
            l6_rom_write_byte(s, byte);
        } else if (s->uart_link) {
            l6_uart_link_send(s, byte);
        } else {
            /* Tools grep this exact line, as for the in-process link. */
            qemu_log_mask(LOG_UNIMP, "l6 panel USART1 RX %02x\n", byte);
            qemu_chr_fe_write(&s->main_uart, &byte, 1);
        }
        uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER(((l6_panel_uart_base(s) + 0x18) >> 2) + 1)));
        if (ctrl & (1u << 23)) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m),
                                            l6_panel_uart_irq(s)));
        }
    }
    if (!s->panel && offset == l6_midi_uart_base(s) + 0x18 && size == 4 &&
        word != previous_word) {
        qemu_log_mask(LOG_UNIMP, "l6 main LPUART%u (MIDI jacks) CTRL %08x\n",
                      s->profile->midi_uart_number, word);
        if ((word & (1u << 23)) && !(previous_word & (1u << 23))) {
            /* TIE newly enabled: real hardware's TDRE is always set while
             * idle, so this interrupts immediately, the same way the panel UART's
             * CTRL handler above already does. Without this, firmware's
             * enable-TIE-then-wait-for-interrupt driver pattern never gets
             * its first interrupt, so it never transmits the MIDI OUT jack
             * at all. */
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), l6_midi_uart_irq(s)));
        }
    }
    if (!s->panel && size == 4 && offset >= 0x188018 &&
        offset != l6_midi_uart_base(s) + 0x18 &&
        offset != l6_panel_uart_base(s) + 0x18 &&
        offset <= 0x1a0018 && (offset - 0x188018) % 0x4000 == 0 &&
        word != previous_word) {
        /* A boot-time check found none of these touched despite each
         * having its own distinct vector table entry (the SDK wires all
         * eight uniformly); kept in case a later firmware path differs. */
        qemu_log_mask(LOG_UNIMP, "l6 main LPUART (other) CTRL +%05lx = %08x\n",
                      (unsigned long)offset, word);
    }
    if (!s->panel && size == 4 && offset >= 0x18801c &&
        offset <= 0x1a001c && (offset - 0x18801c) % 0x4000 == 0 &&
        offset != l6_midi_uart_base(s) + 0x1c &&
        offset != l6_panel_uart_base(s) + 0x1c) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 main LPUART (other) DATA write +%05lx = %02x\n",
                      (unsigned long)offset, (uint8_t)value);
    }
    if (!s->panel && offset == l6_midi_uart_base(s) + 0x1c && size == 4) {
        /* The selected MIDI UART's TX: forward bytes to the MIDI OUT jack
         * inherited socket like L6_USB_FD, one byte at a time. */
        uint8_t byte = value;
        qemu_log_mask(LOG_UNIMP, "l6 main LPUART%u (MIDI jacks) TX %02x\n",
                      s->profile->midi_uart_number, byte);
        if (s->main_midi_fd >= 0) {
            ssize_t n;
            do {
                n = write(s->main_midi_fd, &byte, 1);
            } while (n < 0 && errno == EINTR);
        }
        uint32_t ctrl = GPOINTER_TO_UINT(g_hash_table_lookup(s->registers,
            GUINT_TO_POINTER(((l6_midi_uart_base(s) + 0x18) >> 2) + 1)));
        if (ctrl & (1u << 23)) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), l6_midi_uart_irq(s)));
        }
    }
    if (offset >= 0x3a0000 && offset < 0x3a0100 &&
        s->lpspi_log_count++ < 128) {
        qemu_log_mask(LOG_UNIMP, "l6 LPSPI3 MMIO +%02x = %08x\n",
                      (unsigned)(offset - 0x3a0000), (uint32_t)value);
    }
    if (offset >= 0x0e8000 && offset < 0x0f0000 &&
        s->edma_log_count++ < 256) {
        qemu_log_mask(LOG_UNIMP, "l6 eDMA MMIO +%04x = %08x (%u bytes)\n",
                      (unsigned)(offset - 0x0e8000), (uint32_t)value, size);
    }
    if (offset == 0x0e801b && size == 1 && value < 32) {
        s->edma_erq |= 1u << value;
        l6_edma_service_active_sai(s);
    }
    if (offset == 0x0e801a && size == 1 && value < 32) {
        s->edma_erq &= ~(1u << value);
    }
    if (offset == 0x0e800c && size == 4) {
        s->edma_erq = value;
        l6_edma_service_active_sai(s);
    }
    if (offset >= 0x0ec000 && offset < 0x0ec080) {
        l6_edma_service_active_sai(s);
    }
    if (!s->panel && s->profile->oled && offset == 0x3a001c &&
        size == 4 && (word & 3) == 3 &&
        (s->edma_erq & 0xc) == 0xc) {
        if (!l6_display_dma_transfer(s)) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 unsupported SPI-window/eDMA transfer, completion withheld\n");
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
        (value & 0x1f) != 0x1f) {
        /* ADC1 HC0 starts a conversion. Physical channels 8 and 13 are the
         * board-ID ladder inputs; low samples select the firmware's ID 6.
         * Five physical potentiometers use channels 3..7, with 10-bit
         * results. AIEN enables the completion interrupt; it does not gate
         * the software-triggered conversion.
         */
        uint32_t channel = value & 0x1f;
        s->adc_channel = channel;
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x0c4020 >> 2) + 1), GUINT_TO_POINTER(1));
        g_hash_table_insert(s->registers,
            GUINT_TO_POINTER((0x0c4024 >> 2) + 1),
            GUINT_TO_POINTER(channel >= 3 && channel <= 7 ? s->analog_inputs[channel - 3] :
                channel == 8 || channel == 13 ? 0 : 0x800));
        if (value & 0x80) {
            qemu_irq_pulse(qdev_get_gpio_in(DEVICE(&s->armv7m), 67));
        }
    }
}

static uint64_t l6_main_gpio_read(void *opaque, hwaddr offset, unsigned size)
{
    L6MainGPIOAdapter *adapter = opaque;
    L6ChipState *s = adapter->chip;
    hwaddr aligned = offset & ~3ULL;
    uint64_t value = 0;
    if (aligned >= IMX_GPIO_MEM_SIZE || size == 0 || size > 4) {
        return 0;
    }
    MemTxResult result = memory_region_dispatch_read(
        adapter->model_mmio, aligned, &value, size_memop(4) | MO_LE,
        MEMTXATTRS_UNSPECIFIED);
    if (result != MEMTX_OK) {
        return 0;
    }
    unsigned shift = (offset & 3) * 8;
    uint64_t mask = size == 4 ? UINT32_MAX : (1ULL << (size * 8)) - 1;
    uint64_t result_value = (value >> shift) & mask;
    /* GPIOx_DR (0x00) and GPIOx_PSR (0x08) expose input pin levels. Report
     * candidate pins that are low during an actual guest port read, together
     * with the generic GPIO reader's selected bit. The latter is needed to
     * distinguish a whole-port read from the firmware testing that pin. */
    if (s->main_gpio_trace && (aligned == 0x00 || aligned == 0x08)) {
        for (unsigned id = 0; id <= 52; id++) {
            hwaddr candidate_port;
            unsigned pin;
            if (id == 0 || id == 5) {
                candidate_port = l6_main_gpio_ports[4];
                pin = id == 0 ? 2 : 0;
            } else if (s->profile == &L6MAX_PROFILE && id == 4) {
                /* Host target 4 is the L6max Play candidate in the live
                 * input API; trace it independently from L6's target 4. */
                candidate_port = 0x1b8000;
                pin = 20;
            } else if (!l6_main_key_gpio(s, id, &candidate_port, &pin)) {
                continue;
            }
            if (candidate_port != l6_main_gpio_ports[adapter->bank] ||
                !l6_button_down(s, id) ||
                ((value >> pin) & 1) != 0) {
                continue;
            }
            vaddr pc = current_cpu && CPU_GET_CLASS(current_cpu)->get_pc ?
                       CPU_GET_CLASS(current_cpu)->get_pc(current_cpu) : 0;
            CPUARMState *env = current_cpu ? (CPUARMState *)cpu_env(current_cpu) : NULL;
            unsigned tested_port = env ? env->regs[0] : 0;
            unsigned tested_bit = env ? env->regs[1] : 32;
            unsigned reader_index = env ? env->regs[4] : ~0u;
            unsigned expected_port = 0x40000000 + l6_main_gpio_ports[adapter->bank];
            if (tested_port != expected_port || tested_bit >= 32) {
                continue;
            }
            uint32_t tested_mask = 1u << tested_bit;
            if (s->main_button_tested_bits[id][adapter->bank] & tested_mask) {
                continue;
            }
            s->main_button_tested_bits[id][adapter->bank] |= tested_mask;
            qemu_log_mask(LOG_UNIMP,
                          "l6 MAIN GPIO port-read candidate=%u reader=%u port=%08x reg=%s value=%08x pin=%u pc=%08" PRIx64 " lr=%08x tested_port=%08x tested_bit=%u matches_candidate=%u at %" PRId64 " ms\n",
                          id, reader_index, (unsigned)l6_main_gpio_ports[adapter->bank],
                          aligned == 0x00 ? "DR" : "PSR", (uint32_t)value, pin,
                          (uint64_t)pc, env ? env->regs[14] : 0,
                          tested_port, tested_bit, tested_bit == pin,
                          qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        }
    }
    return result_value;
}

static void l6_main_gpio_publish(L6ChipState *s, unsigned bank,
                                 uint32_t before, uint32_t after)
{
    if (before == after) {
        return;
    }
    s->main_gpio[bank] = after;
    g_hash_table_insert(s->registers,
        GUINT_TO_POINTER((l6_main_gpio_ports[bank] >> 2) + 1),
        GUINT_TO_POINTER(after));
    if ((s->main_gpio_output_trace || s->main_gpio_pad_led_trace) &&
        s->main_gpio_output_log_count++ < 50000) {
        int64_t now_ms = qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL);
        if (s->main_gpio_output_trace) {
            qemu_log_mask(LOG_UNIMP,
                          "l6 MAIN GPIO output bank=%u before=%08x after=%08x at %" PRId64 " ms\n",
                          bank, before, after, now_ms);
        }
        if (s->main_gpio_pad_led_trace && bank == 1) {
            uint8_t pad_leds[8];
            AddressSpace *as = CPU(s->armv7m.cpu)->as;
            if (address_space_read(as, 0x806293f4, MEMTXATTRS_UNSPECIFIED,
                                   pad_leds, sizeof(pad_leds)) == MEMTX_OK) {
                qemu_log_mask(LOG_UNIMP,
                              "l6 MAIN GPIO pad-led sample bank=%u output=%08x records=%02x%02x%02x%02x%02x%02x%02x%02x at=%" PRId64 " ms\n",
                              bank, after, pad_leds[0], pad_leds[1],
                              pad_leds[2], pad_leds[3], pad_leds[4],
                              pad_leds[5], pad_leds[6], pad_leds[7], now_ms);
            }
        }
    }
    if (!++s->main_gpio_generation) {
        ++s->main_gpio_generation;
    }
    l6_input_reply(s, 0x80000006u, s->main_gpio_generation, bank, after);
    if (bank == 2 && (before & 8u) && !(after & 8u)) {
        qemu_log_mask(LOG_UNIMP,
                      "l6 board power hold released at %" PRId64 " ms\n",
                      qemu_clock_get_ms(QEMU_CLOCK_VIRTUAL));
        l6_input_reply(s, 0x80000009u, s->main_gpio_generation, 0, 0);
        l6_board_power_off(s);
    }
}

static uint32_t l6_main_gpio_read_latch(L6MainGPIOAdapter *adapter)
{
    return IMX_GPIO(adapter->device)->dr;
}

static void l6_main_gpio_write_latch(L6MainGPIOAdapter *adapter,
                                     uint32_t value)
{
    L6ChipState *s = adapter->chip;
    IMXGPIOState *gpio = IMX_GPIO(adapter->device);
    uint32_t before = gpio->dr;
    memory_region_dispatch_write(adapter->model_mmio, DR_ADDR, value,
                                 size_memop(4) | MO_LE,
                                 MEMTXATTRS_UNSPECIFIED);
    l6_main_gpio_publish(s, adapter->bank, before, gpio->dr);
    if (adapter->bank == 1) {
        s->display_dc = !!(gpio->dr & (1u << 27));
    }
}

static void l6_main_gpio_write(void *opaque, hwaddr offset,
                               uint64_t value, unsigned size)
{
    L6MainGPIOAdapter *adapter = opaque;
    IMXGPIOState *gpio = IMX_GPIO(adapter->device);
    hwaddr aligned = offset & ~3ULL;
    if (aligned >= IMX_GPIO_MEM_SIZE || size == 0 || size > 4) {
        return;
    }
    uint32_t write_value = value;
    if (size < 4) {
        uint32_t old = aligned == DR_ADDR ? gpio->dr :
            (uint32_t)l6_main_gpio_read(adapter, aligned, 4);
        uint32_t lane_mask = ((1u << (size * 8)) - 1) << ((offset & 3) * 8);
        write_value = (old & ~lane_mask) |
                      (((uint32_t)value << ((offset & 3) * 8)) & lane_mask);
        if (aligned == ISR_ADDR) {
            write_value = ((uint32_t)value << ((offset & 3) * 8)) & lane_mask;
        }
    }
    uint32_t before = gpio->dr;
    MemTxResult result = memory_region_dispatch_write(
        adapter->model_mmio, aligned, write_value,
        size_memop(4) | MO_LE, MEMTXATTRS_UNSPECIFIED);
    if (result == MEMTX_OK && aligned == DR_ADDR) {
        l6_main_gpio_publish(adapter->chip, adapter->bank, before, gpio->dr);
        if (adapter->bank == 1) {
            adapter->chip->display_dc = !!(gpio->dr & (1u << 27));
        }
    }
}

static void l6_main_gpio_alias_write(void *opaque, hwaddr offset,
                                     uint64_t value, unsigned size)
{
    L6MainGPIOAdapter *adapter = opaque;
    L6ChipState *s = adapter->chip;
    uint32_t mask = value;
    uint32_t before = l6_main_gpio_read_latch(adapter);
    uint32_t after = before;
    /* This window starts at GPIO + 0x80. That first word is reserved;
     * DR_SET/CLEAR/TOGGLE therefore arrive at local offsets 4/8/12. */
    hwaddr reg = offset & ~3ULL;
    if (reg == 0x04) {
        after |= mask;
    } else if (reg == 0x08) {
        after &= ~mask;
    } else if (reg == 0x0c) {
        after ^= mask;
    } else {
        return;
    }
    if (size < 4) {
        after = before;
        uint32_t shifted_mask = mask << ((offset & 3) * 8);
        if (reg == 0x04) {
            after |= shifted_mask;
        } else if (reg == 0x08) {
            after &= ~shifted_mask;
        } else {
            after ^= shifted_mask;
        }
    }
    l6_main_gpio_write_latch(adapter, after);
    hwaddr port_address = 0x40000000 + l6_main_gpio_ports[adapter->bank];
    uint32_t pin_mask = mask;
    if (port_address == s->profile->panel_boot_gpio_base && pin_mask) {
        if (reg == 0x04 && (pin_mask & s->profile->panel_boot_gpio_mask)) {
            s->main_boot_select = true;
        } else if (reg == 0x08 &&
                   (pin_mask & s->profile->panel_boot_gpio_mask)) {
            s->main_boot_select = false;
        }
    }
    if (port_address == s->profile->panel_reset_gpio_base &&
        (pin_mask & s->profile->panel_reset_gpio_mask)) {
        if (reg == 0x04) {
            s->panel_reset_asserted = true;
            s->main_rom_mode = false;
            l6_update_panel_boot(s);
        } else if (reg == 0x08) {
            s->panel_reset_asserted = false;
            s->main_rom_mode = s->main_boot_select;
            s->main_rx_pending = false;
            s->rom_phase = 0;
            s->rom_reply_read = s->rom_reply_visible = s->rom_reply_write = 0;
            timer_del(s->rom_uart_timer);
            qemu_irq_lower(qdev_get_gpio_in(DEVICE(&s->armv7m),
                                            l6_panel_uart_irq(s)));
            if (s->main_rom_mode) {
                qemu_log_mask(LOG_UNIMP, "l6 main panel reset into ROM mode\n");
            }
            l6_update_panel_boot(s);
        }
    }
    if (adapter->bank == 1 && (mask & (1u << 27)) &&
        (reg == 0x04 || reg == 0x08)) {
        s->display_dc = reg == 0x04;
    }
}

static const MemoryRegionOps l6_main_gpio_register_ops = {
    .read = l6_main_gpio_read,
    .write = l6_main_gpio_write,
    .endianness = DEVICE_NATIVE_ENDIAN,
    .valid = { .min_access_size = 1, .max_access_size = 4 },
};

static const MemoryRegionOps l6_main_gpio_alias_ops = {
    .write = l6_main_gpio_alias_write,
    .endianness = DEVICE_NATIVE_ENDIAN,
    .valid = { .min_access_size = 1, .max_access_size = 4 },
};

static void l6_main_gpio_refresh_inputs(L6ChipState *s)
{
    if (s->panel) {
        return;
    }
    for (unsigned bank = 0; bank < ARRAY_SIZE(s->main_gpio_adapters); bank++) {
        L6MainGPIOAdapter *adapter = &s->main_gpio_adapters[bank];
        if (!adapter->device) {
            return;
        }
        uint32_t value = l6_main_gpio_input_value(s, bank);
        for (unsigned pin = 0; pin < IMX_GPIO_PIN_COUNT; pin++) {
            qemu_set_irq(qdev_get_gpio_in(adapter->device, pin),
                         (value >> pin) & 1);
        }
    }
}

static void l6_main_gpio_setup(L6ChipState *s, MemoryRegion *system_memory)
{
    static const unsigned gpio_irqs[5][2] = {
        { 80, 81 }, { 82, 83 }, { 84, 85 }, { 86, 87 }, { 88, 89 },
    };
    for (unsigned bank = 0; bank < ARRAY_SIZE(s->main_gpio_adapters); bank++) {
        L6MainGPIOAdapter *adapter = &s->main_gpio_adapters[bank];
        adapter->chip = s;
        adapter->bank = bank;
        adapter->device = qdev_new(TYPE_IMX_GPIO);
        qdev_prop_set_bit(adapter->device, "has-upper-pin-irq", true);
        sysbus_realize_and_unref(SYS_BUS_DEVICE(adapter->device), &error_fatal);
        adapter->model_mmio = sysbus_mmio_get_region(
            SYS_BUS_DEVICE(adapter->device), 0);
        g_autofree char *region_name = g_strdup_printf("l6-main-gpio%u", bank + 1);
        g_autofree char *alias_name = g_strdup_printf("l6-main-gpio%u-aliases", bank + 1);
        memory_region_init_io(&adapter->registers, NULL,
                              &l6_main_gpio_register_ops, adapter,
                              region_name, IMX_GPIO_MEM_SIZE);
        memory_region_init_io(&adapter->aliases, NULL,
                              &l6_main_gpio_alias_ops, adapter,
                              alias_name, 0x10);
        hwaddr base = 0x40000000 + l6_main_gpio_ports[bank];
        memory_region_add_subregion_overlap(system_memory, base,
                                            &adapter->registers, 2);
        memory_region_add_subregion_overlap(system_memory, base + 0x80,
                                            &adapter->aliases, 2);
        if (bank == 1) {
            /* GPIO2 upper-bank IRQ 83 is the card-detect interrupt used by
             * the existing L6/L6max SD hotplug path. */
            sysbus_connect_irq(SYS_BUS_DEVICE(adapter->device), 1,
                qdev_get_gpio_in(DEVICE(&s->armv7m), gpio_irqs[bank][1]));
        }
    }
    l6_main_gpio_refresh_inputs(s);
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

/* The raw MAIN image is the authority for board selection. Both observed
 * models use the same CPU/peripheral family, but the L6 vector table starts
 * at 0x80001000 while the L6max vector table starts at 0x80000000. */
static hwaddr l6_main_flash_base(const char *kernel)
{
    g_autofree char *contents = NULL;
    gsize size = 0;
    if (!kernel || !g_file_get_contents(kernel, &contents, &size, NULL) ||
        size < 8) {
        error_setg(&error_fatal, "cannot identify L6 MAIN firmware image");
        return 0;
    }
    uint32_t stack = ldl_le_p((uint8_t *)contents);
    uint32_t reset = ldl_le_p((uint8_t *)contents + 4);
    L6BoardModel model = l6_board_model_from_vectors(stack, reset);

    if (stack < 0x20000000 || stack >= 0x40000000 || !(reset & 1)) {
        error_setg(&error_fatal,
                   "invalid MAIN vector table (%08x, %08x)", stack, reset);
        return 0;
    }
    if (model != L6_BOARD_MODEL_INVALID) {
        return l6_board_model_flash_base(model);
    }
    error_setg(&error_fatal,
               "unsupported MAIN reset vector %08x (expected L6 or L6max)",
               reset);
    return 0;
}

/* Persistent MAIN NOR replaces the package image at reset. When it contains
 * an installed image, that image is the authority for the board wiring too. */
static L6BoardModel l6_persistent_main_model(void)
{
    const char *directory = g_getenv("L6_STATE_DIR");
    if (!directory) {
        return L6_BOARD_MODEL_INVALID;
    }
    g_autofree char *path = g_build_filename(directory, "main-nor.bin", NULL);
    if (!g_file_test(path, G_FILE_TEST_EXISTS)) {
        return L6_BOARD_MODEL_INVALID;
    }
    g_autofree char *contents = NULL;
    gsize size = 0;
    if (!g_file_get_contents(path, &contents, &size, NULL)) {
        error_setg(&error_fatal, "cannot read persistent MAIN NOR %s", path);
        return L6_BOARD_MODEL_INVALID;
    }
    /* A zero-length file is initialized from the selected package later. */
    if (!size) {
        return L6_BOARD_MODEL_INVALID;
    }
    if (size != 0x200000) {
        error_setg(&error_fatal, "persistent MAIN NOR %s must contain 2 MiB", path);
        return L6_BOARD_MODEL_INVALID;
    }
    uint32_t stack = ldl_le_p((uint8_t *)contents + 0x50000);
    uint32_t reset = ldl_le_p((uint8_t *)contents + 0x50004);
    L6BoardModel model = l6_board_model_from_vectors(stack, reset);
    if (model == L6_BOARD_MODEL_INVALID) {
        error_setg(&error_fatal,
                   "unsupported persistent MAIN vector table (%08x, %08x)",
                   stack, reset);
    }
    return model;
}

static const L6BoardProfile *l6_main_board_profile(const char *kernel)
{
    L6BoardModel installed = l6_persistent_main_model();
    const L6BoardProfile *profile = installed != L6_BOARD_MODEL_INVALID ?
        (installed == L6_BOARD_MODEL_L6 ? &L6_PROFILE : &L6MAX_PROFILE) :
        (l6_main_flash_base(kernel) == L6_PROFILE.main_flash_base ?
            &L6_PROFILE : &L6MAX_PROFILE);
    qemu_log_mask(LOG_UNIMP,
                  installed != L6_BOARD_MODEL_INVALID ?
                    "l6 board profile %s selected from installed MAIN NOR\n" :
                    "l6 board profile %s selected from MAIN reset vector\n",
                  profile->name);
    return profile;
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

static void l6_load_resource_flash(L6ChipState *s, const char *directory)
{
    g_autofree char *path = g_build_filename(directory, "resource_region.bin", NULL);
    g_autofree char *contents = NULL;
    gsize size = 0;
    if (!g_file_get_contents(path, &contents, &size, NULL) || size > 0x400000) {
        error_setg(&error_fatal, "cannot load L6 resource flash %s", path);
        return;
    }
    uint8_t *seed = g_malloc(0x400000);
    memset(seed, 0xff, 0x400000);
    memcpy(seed, contents, size);
    uint8_t *persistent = l6_persistent_bytes("resource-nor.bin", seed, 0x400000);
    if (persistent) {
        g_free(seed);
        s->resource_flash = persistent;
    } else {
        s->resource_flash = seed;
    }
    s->resource_flash_size = 0x400000;
}

static void l6_chip_init(MachineState *machine, L6ChipState *s, bool panel,
                         MemoryRegion *system_memory, const char *kernel_filename,
                         const char *cpu_type, const char *prefix,
                         const char *input_variable, bool external_serial,
                         const L6BoardProfile *profile)
{
    hwaddr flash_base = panel ? 0x08000000 : profile->main_flash_base;
    /* Keep the 64 KiB panel backing for existing state files, but only map
     * the STM32G031C4T6's 16 KiB physical flash into the guest. */
    uint64_t flash_size = panel ? 0x10000 : 0x200000;
    uint64_t flash_map_size = panel ? 0x4000 : flash_size;
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
    s->profile = profile;
    if (!panel) {
        static const struct {
            hwaddr offset;
            uint32_t value;
        } flexspi1_reset[] = {
            /* RT1040 RM Rev. 3, FlexSPI1 reset values. */
            { 0x2a8000, 0xffff8002u }, { 0x2a8004, 0xffffffffu },
            { 0x2a8008, 0x200081f7u }, { 0x2a800c, 0x00000018u },
            { 0x2a8018, 0x5af05af0u }, { 0x2a801c, 0x00000002u },
            { 0x2a8020, 0x80000020u }, { 0x2a8024, 0x80010020u },
            { 0x2a8028, 0x80020020u }, { 0x2a802c, 0x80030020u },
            { 0x2a8060, 0x00010000u }, { 0x2a8064, 0x00010000u },
            { 0x2a8068, 0x00010000u }, { 0x2a806c, 0x00010000u },
            { 0x2a8070, 0x00000063u }, { 0x2a8074, 0x00000063u },
            { 0x2a8078, 0x00000063u }, { 0x2a807c, 0x00000063u },
            { 0x2a80c0, 0x00000100u }, { 0x2a80c4, 0x00000100u },
            { 0x2a80e0, 0x00000002u }, { 0x2a80e8, 0x01000100u },
        };
        static const struct {
            hwaddr offset;
            uint32_t value;
        } lpspi3_reset[] = {
            /* RT1040 RM Rev. 3, LPSPI3 nonzero reset values. */
            { 0x3a0000, 0x01020004u }, { 0x3a0004, 0x00040404u },
            { 0x3a0014, 0x00000001u }, { 0x3a0060, 0x0000001fu },
            { 0x3a0070, 0x00000002u },
        };
        static const struct {
            hwaddr offset;
            uint32_t value;
        } lpi2c1_reset[] = {
            /* RT1040 RM Rev. 3, LPI2C1 nonzero reset values. */
            { 0x3f0000, 0x01000003u }, { 0x3f0004, 0x00000202u },
            { 0x3f0070, 0x00004000u },
        };
        static const struct {
            hwaddr offset;
            uint32_t value;
        } ccm_reset[] = {
            { L6_CCM_CSCMR1_OFFSET, L6_CCM_CSCMR1_RESET },
            { L6_CCM_CS1CDR_OFFSET, L6_CCM_CS1CDR_RESET },
            { L6_CCM_CS2CDR_OFFSET, L6_CCM_CS2CDR_RESET },
            { L6_CCM_ANALOG_PLL_SYS, L6_CCM_ANALOG_PLL_SYS_RESET },
            { L6_CCM_ANALOG_PLL_AUDIO, L6_CCM_ANALOG_PLL_AUDIO_RESET },
            { L6_CCM_ANALOG_PLL_AUDIO_NUM,
              L6_CCM_ANALOG_PLL_AUDIO_NUM_RESET },
            { L6_CCM_ANALOG_PLL_AUDIO_DENOM,
              L6_CCM_ANALOG_PLL_AUDIO_DENOM_RESET },
            { L6_CCM_ANALOG_PFD_480, L6_CCM_ANALOG_PFD_480_RESET },
            { L6_CCM_ANALOG_PFD_528, L6_CCM_ANALOG_PFD_528_RESET },
        };
        static const struct {
            hwaddr offset;
            uint32_t value;
        } lpuart_reset[] = {
            /* RT1040 RM Rev. 3, common LPUART instance reset values. */
            { 0x0000, 0x04010003u }, /* VERID */
            { 0x0004, 0x00000202u }, /* PARAM */
            { 0x0010, 0x0f000004u }, /* BAUD */
            { 0x0014, 0x00c00000u }, /* STAT */
            { 0x001c, 0x00001000u }, /* DATA */
            { 0x0028, 0x00c00011u }, /* FIFO */
        };
        for (unsigned i = 0; i < ARRAY_SIZE(flexspi1_reset); i++) {
            g_hash_table_insert(s->registers,
                GUINT_TO_POINTER((flexspi1_reset[i].offset >> 2) + 1),
                GUINT_TO_POINTER(flexspi1_reset[i].value));
        }
        for (unsigned i = 0; i < ARRAY_SIZE(lpspi3_reset); i++) {
            g_hash_table_insert(s->registers,
                GUINT_TO_POINTER((lpspi3_reset[i].offset >> 2) + 1),
                GUINT_TO_POINTER(lpspi3_reset[i].value));
        }
        for (unsigned i = 0; i < ARRAY_SIZE(lpi2c1_reset); i++) {
            g_hash_table_insert(s->registers,
                GUINT_TO_POINTER((lpi2c1_reset[i].offset >> 2) + 1),
                GUINT_TO_POINTER(lpi2c1_reset[i].value));
        }
        for (unsigned i = 0; i < ARRAY_SIZE(ccm_reset); i++) {
            g_hash_table_insert(s->registers,
                GUINT_TO_POINTER((ccm_reset[i].offset >> 2) + 1),
                GUINT_TO_POINTER(ccm_reset[i].value));
        }
        for (unsigned instance = 0; instance < 8; instance++) {
            hwaddr base = 0x184000 + instance * 0x4000;
            for (unsigned i = 0; i < ARRAY_SIZE(lpuart_reset); i++) {
                hwaddr offset = base + lpuart_reset[i].offset;
                g_hash_table_insert(s->registers,
                    GUINT_TO_POINTER((offset >> 2) + 1),
                    GUINT_TO_POINTER(lpuart_reset[i].value));
            }
        }
        for (unsigned i = 0; i < profile->sai_count; i++) {
            /* RT1041's reference manual gives SAI1 a four-line PARAM value;
             * SAI2/SAI3 expose one line and reserve FIFO combine. */
            l6_sai_init(&s->sai[i],
                        profile->sai_bases[i] == 0x384000);
        }
    }
    s->input_trace = g_getenv("L6_INPUT_TRACE") != NULL;
    s->audio_trace = g_getenv("L6_AUDIO_TRACE") != NULL;
    s->audio_sample_trace = g_getenv("L6_AUDIO_SAMPLE_TRACE") != NULL;
    if (!panel) {
        const char *raw_capture_path = g_getenv("L6_AUDIO_RAW_CAPTURE");
        const char *tdm_capture_path = g_getenv("L6_AUDIO_TDM_CAPTURE");
        const char *dma_source_capture_path =
            g_getenv("L6_AUDIO_DMA_SOURCE_CAPTURE");

        if (raw_capture_path && *raw_capture_path) {
            s->audio_raw_capture = g_fopen(raw_capture_path, "wb");
            if (!s->audio_raw_capture) {
                error_setg_errno(&error_fatal, errno,
                                 "could not open L6_AUDIO_RAW_CAPTURE '%s'",
                                 raw_capture_path);
            }
        }
        if (tdm_capture_path && *tdm_capture_path) {
            s->audio_tdm_capture = g_fopen(tdm_capture_path, "wb");
            if (!s->audio_tdm_capture) {
                error_setg_errno(&error_fatal, errno,
                                 "could not open L6_AUDIO_TDM_CAPTURE '%s'",
                                 tdm_capture_path);
            }
            const char *limit_seconds =
                g_getenv("L6_AUDIO_TDM_CAPTURE_SECONDS");
            if (limit_seconds && *limit_seconds) {
                char *end = NULL;
                guint64 seconds = g_ascii_strtoull(limit_seconds, &end, 10);
                if (!end || *end || !seconds || seconds > 3600) {
                    error_setg(&error_fatal,
                               "L6_AUDIO_TDM_CAPTURE_SECONDS must be an integer from 1 to 3600");
                } else {
                    s->audio_tdm_capture_limit_frames =
                        seconds * profile->audio_sample_rate_hz;
                }
            }
        }
        if (dma_source_capture_path && *dma_source_capture_path) {
            s->audio_dma_source_capture =
                g_fopen(dma_source_capture_path, "wb");
            if (!s->audio_dma_source_capture) {
                error_setg_errno(&error_fatal, errno,
                                 "could not open L6_AUDIO_DMA_SOURCE_CAPTURE '%s'",
                                 dma_source_capture_path);
            }
        }
    }
    s->main_gpio_trace = g_getenv("L6_MAIN_GPIO_TRACE") != NULL;
    s->main_gpio_output_trace = g_getenv("L6_MAIN_GPIO_OUTPUT_TRACE") != NULL;
    s->main_gpio_pad_led_trace = g_getenv("L6_MAIN_GPIO_PAD_LED_TRACE") != NULL;
    s->input_fd = -1;
    s->panel_usart_fd = -1;
    s->panel_control_fd = -1;
    const char *control_fd = g_getenv("L6_PANEL_CONTROL_FD");
    if (control_fd && !object_dynamic_cast(OBJECT(machine), TYPE_L6_DUAL_MACHINE)) {
        char *end;
        long fd = strtol(control_fd, &end, 10);
        if (!*control_fd || *end || fd < 0 || fd > INT_MAX ||
            fcntl(fd, F_GETFD) < 0) {
            error_setg(&error_fatal, "L6_PANEL_CONTROL_FD must name an inherited socket");
        }
        s->panel_control_fd = fd;
        if (panel) {
            qemu_set_fd_handler(fd, l6_panel_control_read, NULL, s);
        }
    }
    if (!panel && machine->audiodev) {
        struct audsettings settings = {
            .freq = profile->audio_sample_rate_hz,
            .nchannels = 2,
            .fmt = AUDIO_FORMAT_S32,
            .big_endian = false,
        };
        s->audio_backend = audio_be_by_name(machine->audiodev, &error_fatal);
        s->audio_voice = audio_be_open_out(s->audio_backend, NULL,
                                           "l6.sai1.tx", s,
                                           l6_audio_output_callback,
                                           &settings);
        if (!s->audio_voice) {
            error_setg(&error_fatal, "could not open L6 SAI1 audio output");
        }
        s->audio_pending = g_byte_array_new();
        audio_be_set_active_out(s->audio_backend, s->audio_voice, true);
    }
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
    if (!profile->oled && display_fd >= 0) {
        /* The L6 has no display peripheral. Do not retain the host's shared
         * OLED frame descriptor in its QEMU process. */
        close(display_fd);
        display_fd = -1;
        qemu_log_mask(LOG_UNIMP,
                      "l6 board profile %s has no OLED; display descriptor closed\n",
                      profile->name);
    }
    l6_display_init(&s->display, display_fd, l6_display_frame_ready,
                    s, &error_fatal);
    if (panel) {
        s->panel_tim3 = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_panel_tim3_tick, s);
        s->panel_tim14 = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_panel_tim14_tick, s);
        s->panel_spi_dma = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_panel_spi_dma_tick, s);
        s->indicator_timer = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_indicator_tick, s);
        timer_mod(s->indicator_timer, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                  L6_INDICATOR_INTERVAL_NS);
        if (g_getenv("L6_PANEL_REALTIME")) {
            icount_timers_stay_on_vcpu = true;
            s->panel_pace = timer_new_ns(QEMU_CLOCK_VIRTUAL, l6_panel_pace_tick, s);
            timer_mod(s->panel_pace, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) +
                      L6_PANEL_PACE_INTERVAL_NS);
        }
        const char *keys = g_getenv("L6_PANEL_KEYS");
        if (keys) {
            g_auto(GStrv) entries = g_strsplit(keys, ",", -1);
            for (unsigned i = 0; entries[i]; i++) {
                unsigned row = 0, column = 0, start_ms = 0, end_ms = 0;
                int consumed = 0;
                if (i >= L6_MAX_PANEL_KEYS ||
                    sscanf(entries[i], "%u:%u:%u:%u%n", &row, &column,
                           &start_ms, &end_ms, &consumed) != 4 ||
                    entries[i][consumed] || row >= 8 ||
                    column >= s->profile->panel_matrix_columns ||
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
        const char *bringup_ack = g_getenv("L6_AUDIO_BRINGUP_ACK");
        const char *dsp_ack = g_getenv("L6_AUDIO_DSP_ACK");
        bool master_ack =
            bringup_ack ? g_strcmp0(bringup_ack, "0") != 0 : true;
        s->service_audio_profile_ramp_ack = bringup_ack
            ? master_ack : s->profile->audio_profile_ramp_ack;
        s->service_audio_dsp_ack =
            dsp_ack ? g_strcmp0(dsp_ack, "0") != 0
                    : (bringup_ack ? master_ack : s->profile->audio_dsp_wait_ack);
        s->service_audio_queue = g_strcmp0(g_getenv("L6_SERVICE_AUDIO_QUEUE"),
                                             "1") == 0;
        const char *keys = g_getenv("L6_MAIN_KEYS");
        if (keys) {
            g_auto(GStrv) entries = g_strsplit(keys, ",", -1);
            for (unsigned i = 0; entries[i]; i++) {
                unsigned id = 0, start_ms = 0, end_ms = 0;
                int consumed = 0;
                hwaddr port;
                unsigned pin;
                if (i >= L6_MAX_PANEL_KEYS ||
                    sscanf(entries[i], "%u:%u:%u%n", &id, &start_ms,
                           &end_ms, &consumed) != 3 ||
                    entries[i][consumed] ||
                    !l6_main_key_gpio(s, id, &port, &pin) ||
                    start_ms >= end_ms) {
                    error_setg(&error_fatal,
                               "L6_MAIN_KEYS has an unsupported target or invalid interval");
                }
                s->main_keys[i] = (L6MainKey) { id, start_ms, end_ms };
                s->main_key_count++;
            }
        }
    }
    memset(s->rom_flash, 0xff, sizeof(s->rom_flash));
    {
        /* Without a state file, a split-process MAIN still has to see the
         * panel image that its own panel process booted from. */
        const char *image = panel ? NULL : g_getenv("L6_PANEL_FIRMWARE");
        g_autofree char *contents = NULL;
        gsize size = 0;

        if (image && *image && !object_dynamic_cast(OBJECT(machine), TYPE_L6_DUAL_MACHINE) &&
            g_file_get_contents(image, &contents, &size, NULL) &&
            size <= sizeof(s->rom_flash)) {
            if (profile == &L6_PROFILE) {
                memset(s->rom_flash, 0, sizeof(s->rom_flash));
            }
            memcpy(s->rom_flash, contents, size);
        }
    }
    memset(s->rom_option, 0xff, sizeof(s->rom_option));
    if (!panel) {
        s->persistent_options = l6_persistent_bytes("panel-options.bin", s->rom_option, 4);
        if (s->persistent_options) { memcpy(s->rom_option, s->persistent_options, 4); }
    }
    if (panel) {
        s->panel_tim2 = timer_new_ns(QEMU_CLOCK_VIRTUAL,
                                     l6_panel_tim2_tick, s);
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
    if (!panel && s->profile->resource_flash) {
        g_autofree char *directory = g_path_get_dirname(kernel_filename);
        l6_load_resource_flash(s, directory);
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
    if (panel) {
        memory_region_init_alias(&s->panel_flash_window, NULL,
                                 "panel.flash-window", &s->flash, 0, flash_map_size);
        memory_region_add_subregion(system_memory, flash_base, &s->panel_flash_window);
        memory_region_init_alias(&s->flash_alias, NULL,
                                 boot_alias_name, &s->flash, 0, flash_map_size);
        memory_region_add_subregion(system_memory, 0, &s->flash_alias);
    } else {
        memory_region_add_subregion(system_memory, flash_base, &s->flash);
        memory_region_init_ram(&s->itcm, NULL, itcm_name, 0x100000,
                               &error_fatal);
        memory_region_add_subregion(system_memory, 0, &s->itcm);
        memcpy(memory_region_get_ram_ptr(&s->itcm), s->nor_flash + 0x50000, 0x100000);
        if (s->profile->high_ram_size) {
            /* L6 keeps writable heap state in the final 64 KiB of its MAIN
             * image window; expose that profile-specific bank over NOR. */
            memory_region_init_ram(&s->main_high_ram, NULL,
                                   "l6-main-high-ram", s->profile->high_ram_size,
                                   &error_fatal);
            memory_region_add_subregion_overlap(system_memory,
                                                s->profile->high_ram_base,
                                                &s->main_high_ram, 1);
        }
    }

    l6_make_ram(system_memory, &s->sram, sram_name, 0x20000000,
                panel ? 0x2000 : 0x200000);
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
    if (!panel && profile->audio_codec_bus) {
        s->audio_i2c_bus = i2c_init_bus(cpu, "lpi2c1");
        i2c_slave_create_simple(s->audio_i2c_bus, TYPE_L6MAX_AUDIO_ADC, 0x4c);
        i2c_slave_create_simple(s->audio_i2c_bus, TYPE_L6MAX_AUDIO_ADC, 0x4d);
    }
    if (!panel) {
        l6_main_gpio_setup(s, system_memory);
    }
    if (panel) {
        /* The dual-chip machine routes the real panel USART through an
         * in-process socket pair to the main UART bridge. The standalone
         * panel machine instead uses its -serial backend to connect to the
         * independently launched main QEMU process. */
        Chardev *usart_chr;
        if (external_serial) {
            usart_chr = serial_hd(0);
            if (!usart_chr) {
                error_setg(&error_fatal, "panel USART1 requires a serial backend");
            }
        } else {
            int usart_fds[2];
            if (socketpair(AF_UNIX, SOCK_STREAM, 0, usart_fds) < 0) {
                error_setg(&error_fatal, "cannot create panel USART1 socket pair: %s",
                           strerror(errno));
            }
            if (fcntl(usart_fds[1], F_SETFL, O_NONBLOCK) < 0) {
                error_setg(&error_fatal, "cannot set panel USART1 bridge non-blocking");
            }
            g_autofree char *usart_chr_opts = g_strdup_printf(
                "socket,id=l6-panel-usart1-link,fd=%d,server=off", usart_fds[0]);
            QemuOpts *usart_chr_opts_parsed = qemu_opts_parse_noisily(
                qemu_find_opts("chardev"), usart_chr_opts, true);
            if (!usart_chr_opts_parsed) {
                error_setg(&error_fatal, "cannot parse panel USART1 chardev options");
            }
            usart_chr = qemu_chr_new_from_opts(usart_chr_opts_parsed, NULL, &error_fatal);
            if (!usart_chr) {
                error_setg(&error_fatal, "cannot create panel USART1 chardev");
            }
            s->panel_usart_fd = usart_fds[1];
            qemu_set_fd_handler(s->panel_usart_fd,
                                l6_panel_usart_bridge_read, NULL, s);
        }
        s->panel_usart1 = qdev_new(TYPE_STM32L4X5_USART);
        qdev_prop_set_chr(s->panel_usart1, "chardev", usart_chr);
        qdev_connect_clock_in(s->panel_usart1, "clk", s->sysclk);
        sysbus_realize_and_unref(SYS_BUS_DEVICE(s->panel_usart1), &error_fatal);
        memory_region_add_subregion_overlap(system_memory, 0x40013800,
            sysbus_mmio_get_region(SYS_BUS_DEVICE(s->panel_usart1), 0), 2);
        sysbus_connect_irq(SYS_BUS_DEVICE(s->panel_usart1), 0,
                           qdev_get_gpio_in(DEVICE(&s->armv7m), 27));
    }
    if (panel && g_getenv("L6_STATE_DIR")) {
        g_autofree char *contents = NULL;
        gsize size;
        g_autofree uint8_t *seed = g_malloc0(0x10000);
        /* L6's volatile ROM loader leaves an omitted panel-image tail zeroed;
         * L6max uses an erased flash tail. Keep first-run persistent state
         * consistent with each model's existing boot behavior. */
        if (profile != &L6_PROFILE) { memset(seed, 0xff, 0x10000); }
        if (!g_file_get_contents(kernel_filename, &contents, &size, NULL) || size > 0x10000) {
            error_setg(&error_fatal, "cannot seed persistent panel flash");
        }
        memcpy(seed, contents, size);
        s->persistent_panel = l6_persistent_bytes("panel-flash.bin", seed, 0x10000);
        if (s->persistent_panel) {
            /* The L6 package omits the rest of panel flash. Its volatile ROM
             * image leaves that tail zeroed, and the L6 panel firmware only
             * reaches its ready path with the same tail in persistent mode.
             * Migrate only the old default seed (exact package prefix plus an
             * erased tail); preserve any panel image that has since changed. */
            if (profile == &L6_PROFILE && size < 0x10000 &&
                !memcmp(s->persistent_panel, contents, size)) {
                bool erased_tail = true;
                for (size_t offset = size; offset < 0x10000; offset++) {
                    if (s->persistent_panel[offset] != 0xff) {
                        erased_tail = false;
                        break;
                    }
                }
                if (erased_tail) {
                    memset(s->persistent_panel + size, 0, 0x10000 - size);
                    l6_storage_flush(s->persistent_panel, 0x10000);
                }
            }
        }
    }
    armv7m_load_kernel(s->armv7m.cpu,
                       panel && !s->persistent_panel ? kernel_filename : NULL,
                       flash_base, flash_map_size);
    if (panel && s->persistent_panel) {
        rom_add_blob_fixed_as("l6-panel-installed", s->persistent_panel,
                              flash_map_size, flash_base, CPU(s->armv7m.cpu)->as);
    }
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
        s->main_midi_fd = -1;
        s->main_midi_rx = g_byte_array_new();
        const char *midi_fd = g_getenv("L6_MAIN_MIDI_UART_FD");
        if (midi_fd) {
            char *end;
            long fd = strtol(midi_fd, &end, 10);
            if (!*midi_fd || *end || fd < 0 || fd > INT_MAX ||
                fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK) < 0) {
                error_setg(&error_fatal,
                           "L6_MAIN_MIDI_UART_FD must be an inherited socket descriptor");
            }
            s->main_midi_fd = fd;
            qemu_set_fd_handler(s->main_midi_fd, l6_main_midi_read_host, NULL, s);
        }
        memory_region_init_io(&s->usb_region, NULL, &l6_usb_ops, &s->usb,
                              "l6-chipidea-device", 0x200);
        memory_region_add_subregion_overlap(system_memory, 0x402e0000,
                                            &s->usb_region, 2);
        /* RT USDHC1 overlays the sparse peripheral region. The SD slot
         * itself, like the real card-edge connector, is always present; an
         * inherited `-drive if=sd,id=l6-sd` with no `file=` is an empty
         * slot. Whether a card is actually in it is a runtime QMP
         * eject/insert (`blockdev-change-medium`), not a boot-time choice.
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
        s->sd_sysctl = L6_USDHC_SYS_CTRL_RESET;
        s->sd_vend_spec2 = L6_USDHC_VEND_SPEC2_RESET;
        memory_region_init_io(&s->sd_adapter, NULL, &l6_sd_ops, s,
                              "l6-usdhc", 0x100);
        memory_region_add_subregion_overlap(system_memory, 0x402c0000,
                                            &s->sd_adapter, 2);
        memory_region_add_subregion_overlap(system_memory, 0x402c0000,
            sysbus_mmio_get_region(SYS_BUS_DEVICE(usdhc), 0), 1);
        sysbus_connect_irq(SYS_BUS_DEVICE(usdhc), 0,
            qdev_get_gpio_in(DEVICE(&s->armv7m), 110));
        DeviceState *card = qdev_new(TYPE_SD_CARD);
        if (sd_drive) {
            qdev_prop_set_drive_err(card, "drive",
                                    blk_by_legacy_dinfo(sd_drive),
                                    &error_fatal);
        }
        qdev_realize_and_unref(card, qdev_get_child_bus(usdhc, "sd-bus"),
                               &error_fatal);
        s->sd_present = sdbus_get_inserted(&s->sdhc->sdbus);
    }
}

static void l6_main_init(MachineState *machine)
{
    L6ChipState *s = &L6_MACHINE(machine)->chip;
    const L6BoardProfile *profile =
        l6_main_board_profile(machine->kernel_filename);
    l6_chip_init(machine, s, false, get_system_memory(),
                 machine->kernel_filename, machine->cpu_type, "main",
                 "L6_INPUT_FD", true, profile);
}

static void l6_panel_init(MachineState *machine)
{
    L6ChipState *s = &L6_MACHINE(machine)->chip;
    /* The standalone panel board has no MAIN image to identify its family. */
    const L6BoardProfile *profile =
        g_strcmp0(g_getenv("L6_MODEL"), "l6") == 0 ? &L6_PROFILE : &L6MAX_PROFILE;
    l6_chip_init(machine, s, true, get_system_memory(),
                 machine->kernel_filename, machine->cpu_type, "panel",
                 "L6_INPUT_FD", true, profile);
}

static void l6_dual_init(MachineState *machine)
{
    L6DualMachineState *s = L6_DUAL_MACHINE(machine);
    const char *panel_firmware = g_getenv("L6_PANEL_FIRMWARE");
    if (!panel_firmware || !*panel_firmware || !machine->kernel_filename) {
        error_setg(&error_fatal,
                   "l6max-dual requires -kernel MAIN image and L6_PANEL_FIRMWARE");
    }
    /* MAIN is the authority for the complete board, including the panel
     * wiring profile. Frontends do not need to duplicate model selection. */
    const L6BoardProfile *profile =
        l6_main_board_profile(machine->kernel_filename);
    /* These MCUs overlap SRAM, peripheral and boot-vector addresses. Keep
     * separate address spaces while sharing QEMU_CLOCK_VIRTUAL and its loop. */
    memory_region_init(&s->main_memory, OBJECT(machine),
                       "l6-main-address-space", UINT64_MAX);
    memory_region_init(&s->panel_memory, OBJECT(machine),
                       "l6-panel-address-space", UINT64_MAX);
    l6_chip_init(machine, &s->main, false, &s->main_memory,
                 machine->kernel_filename, ARM_CPU_TYPE_NAME("cortex-m7"),
                 "main", "L6_MAIN_INPUT_FD", false, profile);
    l6_chip_init(machine, &s->panel, true, &s->panel_memory,
                 panel_firmware, ARM_CPU_TYPE_NAME("cortex-m0"),
                 "panel", "L6_PANEL_INPUT_FD", false, profile);
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
    mc->desc = "ZOOM L6/L6max dual-MCU board (model detected from MAIN firmware)";
    mc->init = l6_dual_init;
    mc->default_cpu_type = ARM_CPU_TYPE_NAME("cortex-m7");
    mc->default_ram_size = 0x200000;
    mc->min_cpus = 2;
    mc->default_cpus = 2;
    mc->max_cpus = 2;
    machine_add_audiodev_property(mc);
}

static void l6_main_class_init(ObjectClass *oc, const void *data)
{
    MachineClass *mc = MACHINE_CLASS(oc);
    static const char * const cpus[] = { ARM_CPU_TYPE_NAME("cortex-m7"), NULL };
    mc->desc = "ZOOM L6/L6max MAIN MCU bring-up board (model detected from firmware)";
    mc->init = l6_main_init;
    mc->valid_cpu_types = cpus;
    mc->default_cpu_type = cpus[0];
    mc->default_ram_size = 0x200000;
    machine_add_audiodev_property(mc);
}

static void l6_panel_class_init(ObjectClass *oc, const void *data)
{
    MachineClass *mc = MACHINE_CLASS(oc);
    static const char * const cpus[] = { ARM_CPU_TYPE_NAME("cortex-m0"), NULL };
    mc->desc = "ZOOM L6/L6max panel MCU bring-up board (Cortex-M0 stand-in for M0+)";
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
