/* SPDX-License-Identifier: GPL-2.0-or-later */
/* STM32G031 GPIO data-register operations, RM0444 sections 7.5.5–7.5.7, 7.5.11. */
#ifndef L6_PANEL_GPIO_H
#define L6_PANEL_GPIO_H
#include <stdint.h>

static inline uint32_t l6_panel_gpio_idr(uint32_t pins)
{
    return pins & 0xffffu;
}

static inline uint32_t l6_panel_gpio_bsrr(uint32_t odr, uint32_t command)
{
    return ((odr & ~(command >> 16)) | (command & 0xffffu)) & 0xffffu;
}

static inline uint32_t l6_panel_gpio_brr(uint32_t odr, uint32_t command)
{
    return (odr & ~(command & 0xffffu)) & 0xffffu;
}
#endif
