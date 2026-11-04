/* SPDX-License-Identifier: GPL-2.0-or-later */
/* External serial LED matrix, independent of either MCU's firmware data.
 * Copyright (c) 2026. Licensed under GPL-2.0-or-later. */
#ifndef L6MAX_INDICATORS_H
#define L6MAX_INDICATORS_H
#include <stdint.h>

/* Both panels show a brightness as the number of frames, out of a repeating
 * cycle of 32, in which they latch a lamp on. A frame is one pass over the
 * eight commons, starting when common 0 is selected. */
#define L6_INDICATOR_CYCLE_FRAMES 32

typedef struct L6Indicators {
    uint16_t gpio_a, gpio_b;
    uint32_t shift, latch;
    uint64_t last_ns;
    uint8_t initialized;
    /* The frame in progress. */
    uint64_t row_active_ns[8];
    uint64_t led_on_ns[8][24];
    /* The last cycle of completed frames, and its totals. */
    uint32_t cycle_active_ns[L6_INDICATOR_CYCLE_FRAMES][8];
    uint32_t cycle_on_ns[L6_INDICATOR_CYCLE_FRAMES][8][24];
    uint64_t window_active_ns[8];
    uint64_t window_on_ns[8][24];
    unsigned cycle_slot;
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

/* Replace the oldest frame of the window with the one just finished. */
static inline void l6_indicators_end_frame(L6Indicators *s)
{
    unsigned slot = s->cycle_slot;
    for (unsigned row = 0; row < 8; row++) {
        uint32_t active = s->row_active_ns[row] > UINT32_MAX ?
            UINT32_MAX : (uint32_t)s->row_active_ns[row];
        s->window_active_ns[row] += active;
        s->window_active_ns[row] -= s->cycle_active_ns[slot][row];
        s->cycle_active_ns[slot][row] = active;
        s->row_active_ns[row] = 0;
        for (unsigned bit = 0; bit < 24; bit++) {
            uint32_t on = s->led_on_ns[row][bit] > active ?
                active : (uint32_t)s->led_on_ns[row][bit];
            s->window_on_ns[row][bit] += on;
            s->window_on_ns[row][bit] -= s->cycle_on_ns[slot][row][bit];
            s->cycle_on_ns[slot][row][bit] = on;
            s->led_on_ns[row][bit] = 0;
        }
    }
    s->cycle_slot = (slot + 1) % L6_INDICATOR_CYCLE_FRAMES;
}

/* A lamp's duty over the last cycle, 0..255. Because the window spans
 * exactly one cycle, a steady lamp reads the same at any time, and a lamp
 * switched on or off reaches its new level one cycle later. One is the most
 * a stray sliver of on-time from a latch update can reach, and less than
 * any lamp's dimmest level, so it reads as off. */
static inline uint8_t l6_indicators_level(const L6Indicators *s,
                                          unsigned row, unsigned bit)
{
    uint64_t active = s->window_active_ns[row];
    if (!active) {
        return 0;
    }
    uint64_t on = s->window_on_ns[row][bit];
    uint8_t level = (on * 255 + active / 2) / active;
    return level <= 1 ? 0 : level;
}

static inline void l6_indicators_pins(L6Indicators *s, uint16_t a, uint16_t b,
                                      uint64_t now_ns)
{
    l6_indicators_advance(s, now_ns);
    if ((s->gpio_b & 1) && !(b & 1)) {
        l6_indicators_end_frame(s);
    }
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
