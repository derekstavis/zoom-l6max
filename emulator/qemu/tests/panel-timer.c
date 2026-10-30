/* SPDX-License-Identifier: GPL-2.0-or-later */
#include "../hw/arm/l6-panel-timer.h"
#include <assert.h>
#include <stdio.h>

int main(void)
{
    uint32_t cr1 = 1, status = 0;
    assert(l6_panel_timer_update(&cr1, &status, 0));
    assert(cr1 == 1 && status == 1);
    cr1 = 9; status = 0;
    assert(l6_panel_timer_update(&cr1, &status, 0));
    assert(cr1 == 8 && status == 1);
    cr1 = 11; status = 0;
    assert(!l6_panel_timer_update(&cr1, &status, 0));
    assert(cr1 == 11 && status == 0);
    assert(l6_panel_timer_clear_status(0x103, 0x101, 0xffffffffu) == 0x101);
    assert(l6_panel_timer_clear_status(0x103, 0x00, 0xffu) == 0x100);
    assert(l6_panel_timer_clear_status(0, 0xffffffffu, 0xffffffffu) == 0);
    cr1 = 1; status = 0;
    assert(l6_panel_timer_update(&cr1, &status, 1));
    assert(status == 1 && l6_panel_timer_irq_level(status, 1));
    assert(!l6_panel_timer_irq_level(status, 0));
    status = l6_panel_timer_clear_status(status, 0, 0xffffffffu);
    assert(!l6_panel_timer_irq_level(status, 1));
    cr1 = 5; status = 0;
    assert(l6_panel_timer_update(&cr1, &status, 1));
    assert(status == 0);
    cr1 = 3; status = 0;
    assert(!l6_panel_timer_update(&cr1, &status, 1));
    assert(status == 0);
    puts("STM32G031 panel timer status and TIM2 overflow passed");
    return 0;
}
