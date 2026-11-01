/* SPDX-License-Identifier: GPL-2.0-or-later */
/* External serial LED matrix, independent of either MCU's firmware data.
 * Copyright (c) 2026. Licensed under GPL-2.0-or-later. */
#ifndef L6MAX_INDICATORS_H
#define L6MAX_INDICATORS_H
#include <stdint.h>

typedef struct L6Indicators {
    uint16_t gpio_a, gpio_b;
    uint32_t shift, latch;
    uint64_t last_ns;
    uint8_t initialized;
    uint64_t row_active_ns[8];
    uint64_t led_on_ns[8][24];
    uint8_t pulse_history[8][24];
    uint8_t levels[8][24];
    uint8_t published[8][24];
    uint32_t generation;
} L6Indicators;

/* Integrate the actual multiplexed output between panel GPIO/SPI events.
 * GPIOA4 gates the LED output; GPIOB selects one of eight common rows. */
static inline void l6_indicators_advance(L6Indicators *s, uint64_t now_ns)
{
    static const unsigned commons[8] = { 0, 1, 2, 10, 11, 12, 13, 14 };
    if (!s->initialized) {
        s->last_ns = now_ns;
        s->initialized = 1;
        return;
    }
    if (now_ns <= s->last_ns) {
        return;
    }
    uint64_t elapsed = now_ns - s->last_ns;
    for (unsigned row = 0; row < 8; row++) {
        if (!(s->gpio_b & (1u << commons[row]))) {
            s->row_active_ns[row] += elapsed;
            if (!(s->gpio_a & (1u << 4))) {
                for (unsigned bit = 0; bit < 24; bit++) {
                    if (s->latch & (1u << bit)) {
                        s->led_on_ns[row][bit] += elapsed;
                    }
                }
            }
        }
    }
    s->last_ns = now_ns;
}

static inline void l6_indicators_pins(L6Indicators *s, uint16_t a, uint16_t b,
                                      uint64_t now_ns)
{
    l6_indicators_advance(s, now_ns);
    if (!(s->gpio_a & (1u << 5)) && (a & (1u << 5))) {
        s->shift = ((s->shift << 1) | ((a >> 7) & 1)) & 0xffffff;
    }
    if (!(s->gpio_a & (1u << 6)) && (a & (1u << 6))) {
        s->latch = s->shift;
    }
    s->gpio_a = a;
    s->gpio_b = b;
}

static inline void l6_indicators_spi_byte(L6Indicators *s, uint8_t byte,
                                         uint64_t now_ns)
{
    for (int bit = 7; bit >= 0; bit--) {
        uint16_t pins = (s->gpio_a & ~0xa0u) | (((byte >> bit) & 1) << 7);
        l6_indicators_pins(s, pins, s->gpio_b, now_ns);
        l6_indicators_pins(s, pins | 0x20, s->gpio_b, now_ns);
    }
    l6_indicators_pins(s, s->gpio_a & ~0x20u, s->gpio_b, now_ns);
}
#endif
