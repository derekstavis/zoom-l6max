/* SPDX-License-Identifier: GPL-2.0-or-later */
/* CCM_ANALOG USB1 PLL control semantics shared by the L6 board model. */
#ifndef HW_ARM_L6_CCM_ANALOG_H
#define HW_ARM_L6_CCM_ANALOG_H

#include <stdint.h>

/* RT1040 RM Rev. 3 reset values for the clock registers needed to resolve
 * SAI1/SAI3 roots. The firmware's bootloader may override them before MAIN. */
#define L6_CCM_CSCMR1_OFFSET       0x0fc01c
#define L6_CCM_CSCMR1_RESET        0x04900000u
#define L6_CCM_CS1CDR_OFFSET       0x0fc028
#define L6_CCM_CS1CDR_RESET        0x0ec102c1u
#define L6_CCM_CS2CDR_OFFSET       0x0fc02c
#define L6_CCM_CS2CDR_RESET        0x007336c1u
#define L6_CCM_ANALOG_PLL_SYS      0x0d8030
#define L6_CCM_ANALOG_PLL_SYS_RESET 0x00013001u
#define L6_CCM_ANALOG_PLL_AUDIO    0x0d8070
#define L6_CCM_ANALOG_PLL_AUDIO_RESET 0x00011006u
#define L6_CCM_ANALOG_PLL_AUDIO_NUM 0x0d8080
#define L6_CCM_ANALOG_PLL_AUDIO_NUM_RESET 0x05f5e100u
#define L6_CCM_ANALOG_PLL_AUDIO_DENOM 0x0d8090
#define L6_CCM_ANALOG_PLL_AUDIO_DENOM_RESET 0x2964619cu
#define L6_CCM_ANALOG_PFD_480 0x0d80f0
#define L6_CCM_ANALOG_PFD_480_RESET 0x13111018u
#define L6_CCM_ANALOG_PFD_528 0x0d8100
#define L6_CCM_ANALOG_PFD_528_RESET 0x1018101bu

#define L6_USB1_PLL_LOCK       (1u << 31)
#define L6_USB1_PLL_BYPASS     (1u << 16)
#define L6_USB1_PLL_ENABLE     (1u << 13)
#define L6_USB1_PLL_POWER      (1u << 12)
#define L6_USB1_PLL_BYPASS_SRC (3u << 14)
#define L6_USB1_PLL_EN_USB_CLKS (1u << 6)
#define L6_USB1_PLL_DIV_SELECT (1u << 1)
#define L6_USB1_PLL_WRITABLE   (L6_USB1_PLL_BYPASS | \
                                L6_USB1_PLL_BYPASS_SRC | \
                                L6_USB1_PLL_ENABLE | L6_USB1_PLL_POWER | \
                                L6_USB1_PLL_EN_USB_CLKS | \
                                L6_USB1_PLL_DIV_SELECT)

typedef enum L6Usb1PllAlias {
    L6_USB1_PLL_DIRECT,
    L6_USB1_PLL_SET,
    L6_USB1_PLL_CLEAR,
    L6_USB1_PLL_TOGGLE,
} L6Usb1PllAlias;

static inline uint32_t l6_usb1_pll_apply(uint32_t control, uint32_t value,
                                         L6Usb1PllAlias alias)
{
    value &= L6_USB1_PLL_WRITABLE;
    switch (alias) {
    case L6_USB1_PLL_DIRECT:
        return value;
    case L6_USB1_PLL_SET:
        return control | value;
    case L6_USB1_PLL_CLEAR:
        return control & ~value;
    case L6_USB1_PLL_TOGGLE:
        return control ^ value;
    }
    return control;
}

/* The modeled PLL locks once powered and enabled; its lock settles
 * immediately on the virtual clock. LOCK remains a read-only status bit. */
static inline uint32_t l6_usb1_pll_status(uint32_t control)
{
    control &= ~L6_USB1_PLL_LOCK;
    if ((control & (L6_USB1_PLL_POWER | L6_USB1_PLL_ENABLE)) ==
        (L6_USB1_PLL_POWER | L6_USB1_PLL_ENABLE)) {
        control |= L6_USB1_PLL_LOCK;
    }
    return control;
}

#endif /* HW_ARM_L6_CCM_ANALOG_H */
