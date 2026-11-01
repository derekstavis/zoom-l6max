#!/usr/bin/env python3
"""Exercise the panel MMIO path with a synthetic image and QEMU qtest."""
import os
from pathlib import Path
import select
import struct
import subprocess
import sys
import tempfile

qemu = sys.argv[1]
with tempfile.TemporaryDirectory(prefix="l6-panel-qtest-") as temporary:
    image = bytearray(0x4000)
    struct.pack_into("<II", image, 0, 0x20002000, 0x08000101)
    struct.pack_into("<I", image, 0x3ffc, 0x1234abcd)
    firmware = Path(temporary) / "panel.bin"
    firmware.write_bytes(image)
    for model, persistent in (("l6", False), ("l6max", False),
                              ("l6", True), ("l6max", True)):
        env = dict(os.environ, L6_MODEL=model)
        env.pop("L6_STATE_DIR", None)
        state_file = None
        if persistent:
            state = Path(temporary) / (model + "-existing-state")
            state.mkdir()
            state_file = state / "panel-flash.bin"
            state_file.write_bytes(image + bytes([0xa5]) * 0xc000)
            env["L6_STATE_DIR"] = str(state)
        process = subprocess.Popen(
            [qemu, "-machine", "l6max-panel", "-kernel", str(firmware),
             "-accel", "qtest", "-qtest", "stdio", "-display", "none"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1, env=env,
        )
        def command(text):
            process.stdin.write(text + "\n")
            process.stdin.flush()
            if not select.select([process.stdout], [], [], 10)[0]:
                raise RuntimeError(f"{model}: no response to {text}")
            reply = process.stdout.readline().strip()
            if not reply.startswith("OK"):
                raise RuntimeError(f"{model}: {text}: {reply}")
            return reply
        def write(address, value):
            command(f"writel {address:#x} {value:#x}")
        def read(address):
            return int(command(f"readl {address:#x}").split()[1], 16)
        try:
            assert read(0x08003ffc) == 0x1234abcd
            assert read(0x08004000) == 0
            assert read(0x00003ffc) == 0x1234abcd
            assert read(0x00004000) == 0
            # The GPIO commands update ODR, while IDR reserves its upper bits.
            gpio = 0x50000000
            write(gpio + 0x14, 0xffff)
            write(gpio + 0x28, 0x00f0)
            assert read(gpio + 0x14) == 0xff0f
            assert read(gpio + 0x28) == 0
            assert read(gpio + 0x10) <= 0xffff
            for base, irq in ((0x40000000, 15), (0x40000400, 16),
                              (0x40002000, 19)):
                # UG sets UIF even with UIE disabled; EGR self-clears.
                write(base + 0x0c, 0)
                write(base + 0x14, 1)
                assert read(base + 0x10) & 1
                assert read(base + 0x14) == 0
                # Enabling a pending flag asserts the NVIC input; clearing
                # UIF deasserts the line so the pending bit can be cleared.
                write(base + 0x0c, 1)
                assert read(0xe000e200) & (1 << irq)
                write(base + 0x10, 0)
                write(0xe000e280, 1 << irq)
                assert not (read(0xe000e200) & (1 << irq))
                write(base + 0x0c, 0)
                assert not (read(base + 0x10) & 1)
                # URS and UDIS each suppress the software update flag.
                for control in (4, 2):
                    write(base, control)
                    write(base + 0x14, 1)
                    assert not (read(base + 0x10) & 1)
                write(base, 0)
                write(base + 0x28, 0)       # PSC
                write(base + 0x2c, 4799)    # 100 us at 48 MHz
                write(base, 3)              # CEN + UDIS
                command("clock_step 200000")
                assert not (read(base + 0x10) & 1)
                assert read(base) & 1
                write(base, 0)
                write(base, 9)              # CEN + OPM
                command("clock_step 200000")
                assert read(base + 0x10) & 1, f"overflow UIF missing at {base:#x}"
                assert not (read(base) & 1), f"OPM did not stop {base:#x}"
                write(base + 0x10, 0)
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        if state_file is not None:
            saved = state_file.read_bytes()
            assert len(saved) == 0x10000
            assert saved[0x4000:] == bytes([0xa5]) * 0xc000
        print(f"{model} ({'existing state' if persistent else 'volatile'}): "
              "panel flash bounds and timer MMIO passed")
