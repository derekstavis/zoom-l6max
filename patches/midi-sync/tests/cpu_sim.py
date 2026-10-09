#!/usr/bin/env python3
"""Run the patched inbound-MIDI handler in a Cortex-M CPU emulator.

Loads a patched `main_firmware.bin`, enters the stock handler the way its USB
caller does, and replaces only the mixer getters, the channel lookup, the stock
Control Change sender and the stock handler body with recording stubs. Everything
in between (the hook, its veneer and the displaced prologue) is real code.

    python3 -m pip install unicorn
    python3 patches/midi-sync/tests/cpu_sim.py firmware-midi-sync-v113/main_firmware.bin

This checks the patch logic and the veneer. It does not model the USB stack,
the CC map or timing; use the QEMU smoke test for those.
"""
import sys
from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
from unicorn.arm_const import UC_ARM_REG_LR, UC_ARM_REG_PC, UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_SP

BASE = 0x80000000
HANDLER = 0x8003F7C8  # stock inbound message handler (hooked)
AFTER_PROLOGUE = 0x8003F7CC  # where the veneer resumes the stock handler
SENDER = 0x8003F9B0  # stock Control Change sender
CHANNEL_INDEX = 0x800564B0
MIDI_CHANNEL = 0x8003FEB0
GETTERS = {
    0x8000B080: "aux", 0x8000B0B0: "level", 0x8000B0F0: "efx", 0x8000B120: "freq",
    0x8000B140: "gain", 0x8000B1D0: "pan", 0x8000B2A0: "submix",
}
# Setting -> (getter, extra argument, first parameter identifier).
EXPECTED = [
    ("gain", 0, 0x00), ("freq", 1, 0x08), ("gain", 1, 0x10), ("gain", 2, 0x18),
    ("aux", 0, 0x28), ("aux", 1, 0x30), ("efx", None, 0x38), ("submix", None, 0x20),
    ("pan", None, 0x40), ("level", None, 0x48),
]


def run(image, message, configured_channel=0, out_of_range_channel=None):
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.mem_map(BASE, 0xA00000)
    uc.mem_write(BASE, open(image, "rb").read())
    event, ret = 0x80800000, 0x80900100
    uc.mem_write(event, bytes([0, 0, 0, 0, *message, 0]))
    uc.reg_write(UC_ARM_REG_SP, 0x80900000)
    uc.reg_write(UC_ARM_REG_LR, ret | 1)
    uc.reg_write(UC_ARM_REG_R0, event)
    log = {"sent": [], "reads": [], "stock_handler": 0}
    stubs = {SENDER, AFTER_PROLOGUE, CHANNEL_INDEX, MIDI_CHANNEL, *GETTERS}

    def on_code(uc, address, size, _):
        if address == ret:
            uc.emu_stop()
        if address not in stubs:
            return
        r0, r1 = uc.reg_read(UC_ARM_REG_R0), uc.reg_read(UC_ARM_REG_R1)
        value = 0
        if address == SENDER:
            log["sent"].append((r0, r1))
        elif address == AFTER_PROLOGUE:
            log["stock_handler"] += 1
        elif address == CHANNEL_INDEX:
            value = r0
        elif address == MIDI_CHANNEL:
            value = configured_channel
        else:
            log["reads"].append((GETTERS[address], r0, r1))
            value = 200 if r0 == out_of_range_channel else (r0 * 7 + len(log["reads"])) % 128
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
    ids = [identifier for identifier, _ in log["sent"]]
    check(len(set(ids)) == 80 == len(ids) and not log["stock_handler"],
          "sync request sends 80 distinct parameters and consumes the message")
    ordered = len(log["reads"]) == len(log["sent"]) == 80
    for index, ((getter, channel, extra), (sent_id, sent_value)) in enumerate(zip(log["reads"], log["sent"])):
        name, argument, base = EXPECTED[index % 10]
        ordered &= (getter == name and channel == index // 10 and sent_id == base + channel
                    and (argument is None or extra == argument) and sent_value <= 127)
    check(ordered, "each setting reads its own getter and sends base[setting] + channel")
    for text, message, channel in [
        ("another controller", [0xB0, 5, 127], 0), ("button release", [0xB0, 127, 0], 0),
        ("another MIDI channel", [0xB3, 127, 127], 0), ("note on", [0x90, 127, 127], 0),
    ]:
        log = run(image, message, channel)
        check(log["stock_handler"] == 1 and not log["sent"], f"{text}: forwarded to the stock handler")
    check(len(run(image, [0xB3, 127, 127], 3)["sent"]) == 80, "configured MIDI channel is honoured")
    check(len(run(image, [0xB0, 127, 127], 0, out_of_range_channel=2)["sent"]) == 70,
          "stored values above 127 are skipped")
    return failures


if __name__ == "__main__":
    sys.exit(1 if main(sys.argv[1]) else 0)
