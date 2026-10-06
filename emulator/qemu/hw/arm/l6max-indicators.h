/* SPDX-License-Identifier: GPL-2.0-or-later */
/* External serial LED matrix, independent of either MCU's firmware data.
 * Copyright (c) 2026. Licensed under GPL-2.0-or-later. */
#ifndef L6MAX_INDICATORS_H
#define L6MAX_INDICATORS_H
#include <stdint.h>
#include <stdbool.h>

typedef struct L6Indicators {
    uint16_t gpio_a, gpio_b;
    uint32_t shift, latch;
    uint32_t seen[8], published[8];
    uint32_t generation;
} L6Indicators;

static inline void l6_indicators_sample(L6Indicators *s)
{
    static const unsigned commons[8] = { 0, 1, 2, 10, 11, 12, 13, 14 };
    if (s->gpio_a & (1u << 4)) {
        return;
    }
    for (unsigned row = 0; row < 8; row++) {
        if (!(s->gpio_b & (1u << commons[row]))) {
            s->seen[row] |= s->latch;
        }
    }
}

static inline void l6_indicators_pins(L6Indicators *s, uint16_t a, uint16_t b)
{
    l6_indicators_sample(s);
    if (!(s->gpio_a & (1u << 5)) && (a & (1u << 5))) {
        s->shift = ((s->shift << 1) | ((a >> 7) & 1)) & 0xffffff;
    }
    if (!(s->gpio_a & (1u << 6)) && (a & (1u << 6))) {
        s->latch = s->shift;
    }
    s->gpio_a = a;
    s->gpio_b = b;
    l6_indicators_sample(s);
}

static inline void l6_indicators_spi_byte(L6Indicators *s, uint8_t byte)
{
    for (int bit = 7; bit >= 0; bit--) {
        uint16_t pins = (s->gpio_a & ~0xa0u) | (((byte >> bit) & 1) << 7);
        l6_indicators_pins(s, pins, s->gpio_b);
        l6_indicators_pins(s, pins | 0x20, s->gpio_b);
    }
    l6_indicators_pins(s, s->gpio_a & ~0x20u, s->gpio_b);
}
#endif
