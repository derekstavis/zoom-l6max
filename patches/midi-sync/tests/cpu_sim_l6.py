#!/usr/bin/env python3
"""Exercise the L6 1.10 MIDI-sync hook and its displaced-prologue veneer.

This runs the patched hook in Unicorn. It stubs the six-channel mapper, state
getters, configured-channel getter and mapped-CC sender; the hook, payload and
trampoline bytes are the real compiled image. It does not model USB or DIN.

    python3 -m pip install unicorn
    python3 patches/midi-sync/tests/cpu_sim_l6.py /path/to/main_firmware.bin
"""
import sys
from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
from unicorn.arm_const import UC_ARM_REG_LR, UC_ARM_REG_PC, UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_SP

MAIN_BASE = 0x80001000
HANDLER = 0x8002F490
AFTER_PROLOGUE = 0x8002F494
MAPPED_CC_SENDER = 0x80008330
CHANNEL_INDEX = 0x80040AE8
MIDI_CHANNEL = 0x80005ED8
GETTERS = {
    0x80005FA8: "gain",
    0x80005F90: "frequency",
    0x800060E8: "aux",
    0x80005F60: "efx",
    0x80006028: "pan",
    0x80005F40: "level",
}
CHANNEL_MAP = [0, 1, 2, 4, 6, 8]
EXPECTED = [
    ("gain", 0), ("frequency", 1), ("gain", 1), ("gain", 2),
    ("aux", 0), ("aux", 1), ("efx", 0), ("pan", None), ("level", None),
]


def run(image, message, configured_channel=0, out_of_range_channel=None, invalid_state_channel=None):
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.mem_map(0x80000000, 0xA00000)
    uc.mem_write(MAIN_BASE, open(image, "rb").read())
    event, ret = 0x80800000, 0x80900100
    uc.mem_write(event, bytes([0, 0, 0, 0, *message, 0]))
    uc.reg_write(UC_ARM_REG_SP, 0x80900000)
    uc.reg_write(UC_ARM_REG_LR, ret | 1)
    uc.reg_write(UC_ARM_REG_R0, event)
    log = {"sent": [], "reads": [], "stock_handler": 0}
    stubs = {MAPPED_CC_SENDER, AFTER_PROLOGUE, CHANNEL_INDEX, MIDI_CHANNEL, *GETTERS}

    def on_code(uc, address, _size, _):
        if address == ret:
            uc.emu_stop()
        if address not in stubs:
            return
        r0 = uc.reg_read(UC_ARM_REG_R0)
        r1 = uc.reg_read(UC_ARM_REG_R1)
        r2 = uc.reg_read(UC_ARM_REG_R2)
        value = 0
        if address == MAPPED_CC_SENDER:
            log["sent"].append((r0, r1, r2))
        elif address == AFTER_PROLOGUE:
            log["stock_handler"] += 1
        elif address == CHANNEL_INDEX:
            value = 11 if r0 == out_of_range_channel else CHANNEL_MAP[r0]
        elif address == MIDI_CHANNEL:
            value = configured_channel
        else:
            log["reads"].append((GETTERS[address], r0, r1))
            if invalid_state_channel is not None and r0 == CHANNEL_MAP[invalid_state_channel]:
                value = 0xFFFFFFFF
            else:
                value = (len(log["reads"]) * 3) & 0x7F
        uc.reg_write(UC_ARM_REG_R0, value)
        uc.reg_write(UC_ARM_REG_PC, uc.reg_read(UC_ARM_REG_LR) | 1)

    uc.hook_add(UC_HOOK_CODE, on_code)
    uc.emu_start(HANDLER | 1, ret, count=200_000)
    return log


def main(image):
    failures = 0

    def check(condition, text):
        nonlocal failures
        print(("PASS " if condition else "FAIL ") + text)
        failures += not condition

    log = run(image, [0xB0, 127, 127])
    sent = log["sent"]
    check(len(sent) == 54 and len({(channel, group) for channel, group, _ in sent}) == 54
          and not log["stock_handler"], "sync request consumes the message and sends all 54 channel/group pairs")
    ordered = len(log["reads"]) == len(sent) == 54
    for index, ((getter, channel, argument), (sent_channel, sent_group, value)) in enumerate(zip(log["reads"], sent)):
        expected_getter, expected_argument = EXPECTED[index % len(EXPECTED)]
        physical_channel = index // len(EXPECTED)
        ordered &= getter == expected_getter and channel == CHANNEL_MAP[physical_channel]
        ordered &= expected_argument is None or argument == expected_argument
        ordered &= (sent_channel, sent_group) == (physical_channel, index % len(EXPECTED)) and value <= 127
    check(ordered, "each L6 mode reads its mapped state field and sends through the matching mapped-CC group")

    skipped = run(image, [0xB0, 127, 127], out_of_range_channel=2)
    check(len(skipped["sent"]) == 45 and not any(channel == 2 for channel, _, _ in skipped["sent"]),
          "invalid L6 mapped channels are skipped")
    invalid_values = run(image, [0xB0, 127, 127], invalid_state_channel=1)
    check(len(invalid_values["sent"]) == 45 and not any(channel == 1 for channel, _, _ in invalid_values["sent"]),
          "negative or out-of-range signed state values are skipped")
    check(run(image, [0xB5, 127, 64], configured_channel=5)["sent"]
          and run(image, [0xB0, 127, 127], configured_channel=5)["stock_handler"] == 1,
          "request matches the configured MIDI channel")
    check(run(image, [0xB0, 126, 127])["stock_handler"] == 1
          and run(image, [0xB0, 127, 63])["stock_handler"] == 1
          and run(image, [0x90, 127, 127])["stock_handler"] == 1,
          "other messages and request releases fall through to the stock handler")
    return failures


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: cpu_sim_l6.py PATCHED_MAIN_FIRMWARE")
    raise SystemExit(main(sys.argv[1]))
