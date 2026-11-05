/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Pin-level checks; compile with: cc -std=c11 -Wall -Wextra -Werror indicators.c */
#include <assert.h>
#include <string.h>
#include "../hw/arm/l6max-indicators.h"

/* One frame: each common for 100 ns with output enabled for its first 10. */
static void scan(L6Indicators *s, uint64_t *t, uint32_t latch)
{
    static const unsigned commons[8] = {0, 1, 2, 10, 11, 12, 13, 14};
    s->latch = latch;
    for (unsigned row = 0; row < 8; row++) {
        l6_indicators_pins(s, 0x00, 0x7c07 & ~(1u << commons[row]), *t);
        *t += 10;
        l6_indicators_pins(s, 0x10, s->gpio_b, *t);
        *t += 90;
        l6_indicators_pins(s, 0x10, 0x7c07, *t);
    }
}

int main(void)
{
    static const unsigned commons[8] = {0, 1, 2, 10, 11, 12, 13, 14};
    L6Indicators s = {0};
    const uint16_t all_rows_disabled = 0x7c07;
    uint64_t now_ns = 0;

    l6_indicators_pins(&s, 0x10, all_rows_disabled, now_ns); /* blank, rows off */
    l6_indicators_spi_byte(&s, 0x12, now_ns);
    l6_indicators_spi_byte(&s, 0x34, now_ns);
    l6_indicators_spi_byte(&s, 0x56, now_ns);
    assert(s.shift == 0x123456 && s.latch == 0);

    /* The serial register changes only on the latch rising edge. */
    l6_indicators_pins(&s, s.gpio_a | 0x40, s.gpio_b, now_ns);
    assert(s.latch == 0x123456);

    for (unsigned row = 0; row < 8; row++) {
        now_ns += 100;
        l6_indicators_pins(&s, s.gpio_a | 0x10, all_rows_disabled, now_ns);
        memset(s.row_active_ns, 0, sizeof(s.row_active_ns));
        memset(s.led_on_ns, 0, sizeof(s.led_on_ns));
        l6_indicators_pins(&s, s.gpio_a,
                           all_rows_disabled & ~(1u << commons[row]), now_ns);
        l6_indicators_pins(&s, s.gpio_a & ~0x10u, s.gpio_b, now_ns);

        /* Hold this row and pattern active for 100 ns, then blank it. */
        now_ns += 100;
        l6_indicators_pins(&s, s.gpio_a | 0x10, s.gpio_b, now_ns);

        assert(s.row_active_ns[row] == 100);
        for (unsigned other = 0; other < 8; other++) {
            if (other != row) {
                assert(s.row_active_ns[other] == 0);
            }
        }
        for (unsigned bit = 0; bit < 24; bit++) {
            const uint64_t expected = (s.latch & (1u << bit)) ? 100 : 0;
            assert(s.led_on_ns[row][bit] == expected);
        }
    }

    /* Shifting new serial data cannot change the latched output. */
    l6_indicators_spi_byte(&s, 0xff, now_ns);
    assert(s.shift == 0x3456ff && s.latch == 0x123456);
    l6_indicators_pins(&s, s.gpio_a & ~0x40u, s.gpio_b, now_ns);
    assert(s.latch == 0x123456);
    l6_indicators_pins(&s, s.gpio_a | 0x40, s.gpio_b, now_ns);
    assert(s.latch == 0x3456ff);

    /* Scan frames of eight 100 ns commons, each lit for the first 10 ns.
     * Lamp 0 is on in 2 of every 32 frames, like a dim ring segment. */
    L6Indicators w = {0};
    uint64_t t = 0;
    unsigned frame = 0;
    for (; frame < 3 * L6_INDICATOR_CYCLE_FRAMES; frame++) {
        scan(&w, &t, frame % 32 < 2 ? 1 : 0);
        if (frame >= L6_INDICATOR_CYCLE_FRAMES) {
            /* Every window of one cycle holds the pattern once. */
            assert(l6_indicators_level(&w, 0, 0) ==
                   (uint8_t)((2 * 10 * 255 + 1600) / 3200));
        }
    }
    /* Switched fully on, the lamp reaches its level one cycle later. */
    for (unsigned n = 0; n < L6_INDICATOR_CYCLE_FRAMES; n++, frame++) {
        assert(l6_indicators_level(&w, 0, 0) < 26);
        scan(&w, &t, 1);
    }
    scan(&w, &t, 1);
    assert(l6_indicators_level(&w, 0, 0) == 26);
    return 0;
}

