/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Pin-level checks; compile with: cc -Wall -Wextra -Werror indicators.c */
#include <assert.h>
#include <string.h>
#include "../hw/arm/l6max-indicators.h"
int main(void)
{
    const unsigned commons[8] = {0,1,2,10,11,12,13,14};
    L6Indicators s = {0};
    l6_indicators_pins(&s, 0x10, 0x7c07); /* blank, all commons off */
    l6_indicators_spi_byte(&s, 0x12);
    l6_indicators_spi_byte(&s, 0x34);
    l6_indicators_spi_byte(&s, 0x56);
    assert(s.shift == 0x123456 && s.latch == 0);
    l6_indicators_pins(&s, s.gpio_a | 0x40, s.gpio_b);
    assert(s.latch == 0x123456);
    for (unsigned row = 0; row < 8; row++) {
        memset(s.seen, 0, sizeof(s.seen));
        l6_indicators_pins(&s, s.gpio_a | 0x10, 0x7c07);
        l6_indicators_pins(&s, s.gpio_a, 0x7c07 & ~(1u << commons[row]));
        for (unsigned i = 0; i < 8; i++) { assert(s.seen[i] == 0); }
        l6_indicators_pins(&s, s.gpio_a & ~0x10u, s.gpio_b);
        for (unsigned i = 0; i < 8; i++) {
            assert(s.seen[i] == (i == row ? 0x123456u : 0));
        }
        l6_indicators_pins(&s, s.gpio_a | 0x10, s.gpio_b);
        memset(s.seen, 0, sizeof(s.seen));
    }
    /* New serial data cannot change the display before a latch edge. */
    l6_indicators_spi_byte(&s, 0xff);
    assert(s.shift == 0x3456ff && s.latch == 0x123456);
    l6_indicators_pins(&s, s.gpio_a & ~0x40u, s.gpio_b);
    assert(s.latch == 0x123456);
    l6_indicators_pins(&s, s.gpio_a | 0x40, s.gpio_b);
    assert(s.latch == 0x3456ff);
    return 0;
}
