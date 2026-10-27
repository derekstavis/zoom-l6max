#include "../hw/arm/l6-ccm-analog.h"

#include <assert.h>
#include <stdio.h>

int main(void)
{
    uint32_t control = 0;

    /* The L6 board seeds the observed clock registers with RM reset values
     * instead of returning zero from the generic sparse-MMIO bank. */
    assert(L6_CCM_CSCMR1_RESET == 0x04900000u);
    assert(L6_CCM_CS1CDR_RESET == 0x0ec102c1u);
    assert(L6_CCM_ANALOG_PLL_SYS_RESET == 0x00013001u);
    assert(L6_CCM_ANALOG_PLL_AUDIO_RESET == 0x00011006u);
    assert(L6_CCM_ANALOG_PFD_480_RESET == 0x13111018u);
    assert(L6_CCM_ANALOG_PFD_528_RESET == 0x1018101bu);

    /* Separate SET writes are cumulative; LOCK appears only after both the
     * documented POWER and ENABLE controls have been set. */
    control = l6_usb1_pll_apply(control, L6_USB1_PLL_POWER, L6_USB1_PLL_SET);
    assert(!(l6_usb1_pll_status(control) & L6_USB1_PLL_LOCK));
    control = l6_usb1_pll_apply(control, L6_USB1_PLL_ENABLE, L6_USB1_PLL_SET);
    assert(l6_usb1_pll_status(control) & L6_USB1_PLL_LOCK);

    /* CLR and TOG aliases update control state and deassert LOCK as soon as
     * the PLL is no longer both powered and enabled. LOCK is read-only. */
    control = l6_usb1_pll_apply(control, L6_USB1_PLL_ENABLE,
                                L6_USB1_PLL_CLEAR);
    assert(!(l6_usb1_pll_status(control) & L6_USB1_PLL_LOCK));
    control = l6_usb1_pll_apply(control, L6_USB1_PLL_ENABLE,
                                L6_USB1_PLL_TOGGLE);
    assert(l6_usb1_pll_status(control) & L6_USB1_PLL_LOCK);
    control = l6_usb1_pll_apply(control, L6_USB1_PLL_LOCK,
                                L6_USB1_PLL_DIRECT);
    assert(!(l6_usb1_pll_status(control) & L6_USB1_PLL_LOCK));

    puts("CCM_ANALOG USB1 PLL alias/status checks passed");
    return 0;
}
