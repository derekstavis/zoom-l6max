/* SPDX-License-Identifier: GPL-2.0-or-later */
#include "../hw/arm/l6-panel-gpio.h"
#include <assert.h>
#include <stdio.h>

int main(void)
{
    assert(l6_panel_gpio_idr(0xffffffffu) == 0xffffu);
    assert(l6_panel_gpio_idr(0xffff1234u) == 0x1234u);
    assert(l6_panel_gpio_bsrr(0x00f0u, 0x00300005u) == 0x00c5u);
    /* Set wins when both halves name the same pin. */
    assert(l6_panel_gpio_bsrr(0, 0x00010001u) == 1u);
    assert(l6_panel_gpio_bsrr(0xffffu, 0xffff0000u) == 0);
    assert(l6_panel_gpio_brr(0xffffu, 0xffff00f0u) == 0xff0fu);
    assert(l6_panel_gpio_brr(0xffff0001u, 0xffff0000u) == 1u);
    puts("STM32G031 panel GPIO data registers passed");
    return 0;
}
