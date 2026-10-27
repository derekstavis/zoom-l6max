/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Firmware-vector based board selection shared by QEMU and its unit test. */
#ifndef L6_PROFILE_H
#define L6_PROFILE_H

#include <stdbool.h>
#include <stdint.h>

typedef enum L6BoardModel {
    L6_BOARD_MODEL_INVALID,
    L6_BOARD_MODEL_L6,
    L6_BOARD_MODEL_L6MAX,
} L6BoardModel;

static inline L6BoardModel l6_board_model_from_vectors(uint32_t stack,
                                                       uint32_t reset)
{
    uint32_t reset_address = reset & ~1u;

    if (stack < 0x20000000 || stack >= 0x40000000 || !(reset & 1)) {
        return L6_BOARD_MODEL_INVALID;
    }
    if (reset_address >= 0x80001000 && reset_address < 0x80200000) {
        return L6_BOARD_MODEL_L6;
    }
    if (reset_address >= 0x80000000 && reset_address < 0x80001000) {
        return L6_BOARD_MODEL_L6MAX;
    }
    return L6_BOARD_MODEL_INVALID;
}

static inline uint32_t l6_board_model_flash_base(L6BoardModel model)
{
    switch (model) {
    case L6_BOARD_MODEL_L6:
        return 0x80001000;
    case L6_BOARD_MODEL_L6MAX:
        return 0x80000000;
    default:
        return 0;
    }
}

#endif
