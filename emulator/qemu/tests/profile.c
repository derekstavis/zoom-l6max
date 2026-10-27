/* SPDX-License-Identifier: GPL-2.0-or-later */
#include "../hw/arm/l6-profile.h"

#include <assert.h>
#include <stdio.h>

int main(void)
{
    const uint32_t stack = 0x20010000;

    assert(l6_board_model_from_vectors(stack, 0x80001415) ==
           L6_BOARD_MODEL_L6);
    assert(l6_board_model_flash_base(L6_BOARD_MODEL_L6) == 0x80001000);
    assert(l6_board_model_from_vectors(stack, 0x80000415) ==
           L6_BOARD_MODEL_L6MAX);
    assert(l6_board_model_flash_base(L6_BOARD_MODEL_L6MAX) == 0x80000000);

    /* Exercise the exclusive flash-range boundaries and Thumb-bit handling. */
    assert(l6_board_model_from_vectors(stack, 0x80001001) ==
           L6_BOARD_MODEL_L6);
    assert(l6_board_model_from_vectors(stack, 0x801fffff) ==
           L6_BOARD_MODEL_L6);
    assert(l6_board_model_from_vectors(stack, 0x80000001) ==
           L6_BOARD_MODEL_L6MAX);
    assert(l6_board_model_from_vectors(stack, 0x80000fff) ==
           L6_BOARD_MODEL_L6MAX);
    assert(l6_board_model_from_vectors(stack, 0x80200001) ==
           L6_BOARD_MODEL_INVALID);
    assert(l6_board_model_from_vectors(stack, 0x7fffffff) ==
           L6_BOARD_MODEL_INVALID);
    assert(l6_board_model_from_vectors(0x20000000, 0x80001415) ==
           L6_BOARD_MODEL_L6);
    assert(l6_board_model_from_vectors(0x1fffffff, 0x80001415) ==
           L6_BOARD_MODEL_INVALID);
    assert(l6_board_model_from_vectors(0x40000000, 0x80001415) ==
           L6_BOARD_MODEL_INVALID);
    assert(l6_board_model_from_vectors(stack, 0x80001414) ==
           L6_BOARD_MODEL_INVALID);

    puts("L6/L6max MAIN vector profile selection passed");
    return 0;
}
