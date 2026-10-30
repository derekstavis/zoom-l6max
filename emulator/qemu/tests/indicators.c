/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Pin-level checks; compile with: cc -std=c11 -Wall -Wextra -Werror indicators.c */
#include <assert.h>
#include <string.h>
#include "../hw/arm/l6max-indicators.h"

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

    return 0;
}
