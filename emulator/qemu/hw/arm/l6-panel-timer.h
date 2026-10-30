/* SPDX-License-Identifier: GPL-2.0-or-later */
/* STM32G031 timer update and status rules, RM0444 sections 22.4 and 24.4. */
#ifndef L6_PANEL_TIMER_H
#define L6_PANEL_TIMER_H
#include <stdint.h>

static inline uint32_t l6_panel_timer_clear_status(uint32_t status,
                                                   uint32_t value,
                                                   uint32_t mask)
{
    return status & (value | ~mask);
}

/* Apply an overflow or software update event. */
static inline int l6_panel_timer_update(uint32_t *cr1, uint32_t *status, int software)
{
    if (*cr1 & 2u) { /* UDIS */
        return 0;
    }
    if (!software || !(*cr1 & 4u)) { /* URS gates software update requests. */
        *status |= 1u;
    }
    if (*cr1 & 8u) { /* OPM stops on an update event. */
        *cr1 &= ~1u;
    }
    return 1;
}
static inline int l6_panel_timer_irq_level(uint32_t status, uint32_t enable)
{
    return (status & enable & 1u) != 0;
}
#endif
