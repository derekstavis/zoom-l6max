# Persistent device storage

[Documentation index](../README.md)

The native engine keeps device storage in `emulator/state/` by default.
`--state-dir DIR` selects an isolated device; `--volatile` starts with fresh
storage and discards changes when QEMU exits. Storage and firmware files are
excluded from Git. Stop the guest before copying or editing these files.

These files represent nonvolatile hardware. Display and input communication
still uses anonymous shared memory and inherited sockets.

| File | Size | Representation |
| --- | ---: | --- |
| `main-nor.bin` | 2 MiB | Main MCU's physical serial NOR, erased bytes `ff` |
| `panel-flash.bin` | 64 KiB | Panel executable flash at `0x08000000` |
| `panel-options.bin` | 4 bytes | Panel ROM protocol's option-byte word at `0x1fff7800` |
| `panel-rtc.bin` | 256 bytes | Panel RTC backup domain, separate from flash |

QEMU exclusively locks each file for its lifetime and rejects an existing
file of the wrong size. Missing files are seeded from the supplied firmware
and modeled peripheral defaults. Existing files retain their contents: changing
`--firmware-dir` does not reinstall firmware in an existing device.
Main execution is loaded from the installed NOR application region; panel
execution is loaded from panel flash.
On a fresh device the main firmware programs the panel and writes its version
stamp at `0x08003ffc`. Retaining panel flash prevents this programming sequence
from repeating on the next process launch.

## Main NOR map

Offsets below are physical NOR offsets, not CPU executable addresses. The
application executes at `0x80000000`; its installed NOR region starts at
`0x50000`.

| NOR region | Contents | Evidence |
| --- | --- | --- |
| `0x000000–0x04ffff` | Bootloader region; unavailable in the update package | Bootloader version read at `0x4fffc` |
| `0x050000–0x1f2ff7` | Main application and padding | Package payload layout |
| `0x1f2ff8–0x1f2fff` | Main length and four ASCII version digits | Package trailer |
| `0x1f3000–0x1f6ff7` | Packaged panel application and padding | Main's panel-programming reads |
| `0x1f6ff8–0x1f6fff` | Panel length and four ASCII version digits | Package trailer and version reader |
| `0x1f7000–0x1f7fff` | Boot-data region; installed app version at `0x1f7ffc` | Version reader `0x8005dfe0`; remaining fields unclassified |
| `0x1f8000–0x1f8fff` | Dual ADC calibration sector | Reader `0x80026400` reads 48 bytes |
| `0x1f9000–0x1fafff` | Settings bank A | Settings loader and observed erase/program traffic |
| `0x1fb000–0x1fbfff` | Unclassified | No verified assignment |
| `0x1fc000–0x1fdfff` | Settings bank B | Settings loader and observed erase/program traffic |
| `0x1fe000–0x1fefff` | Unclassified | No verified assignment |
| `0x1ff000–0x1fffff` | Firmware-update marker sector | Eight-byte `FW UPDT\0` read/write/erase routines |

The error record `FW UPDT ERR\0` starts at `0x1ff008`. The application consumes
and erases these handoff records during startup. See the
[firmware update map](../firmware/updates.md) for the package checks,
preparation callback, and missing bootloader boundary.
The native engine can fill that boundary with `--update-bootloader`: a Rust SD
installer runs before QEMU opens its storage files. The standard persistent
visual UI enables it automatically for attached media, including Power-on
after a firmware-controlled shutdown. It updates only
the supported MAIN payload region and leaves boot/calibration/settings intact.

Investigation pointed to a 32-byte calibration header beginning with
`Dual AD Calibration`, followed by 16 bytes of calibration data. Calibration
is not present in the update package and is left erased; values are not invented.

The settings loader at `0x80035c98` selects between bank markers 1, 2, and 3,
including wraparound. Each bank contains a four-byte generation marker and
`0x17e0` bytes of parameters. The writer erases the inactive bank, programs
its data at base + 4, writes the valid marker last, then erases the previous
bank. The model handles write-enable, page programming with NOR's 1-to-0
semantics, and sector erasure. Individual parameter fields remain to be mapped.

The update-marker routines are `0x80035b50` (erase), `0x80035b60` (read and
compare), and `0x80035b98` (erase and write). Its exact role in boot recovery
still needs an upgrade/recovery exercise.

## Versions

The firmware reads three separate records:

- Bootloader: `0x4fffc`.
- Installed application: `0x1f7ffc`.
- Packaged panel application: `0x1f6ffc`.

On first creation, the installed application record is seeded from the main
package trailer (`0110`, displayed as **1.10**). The panel trailer contains
`0100` (**1.00**). The bootloader is absent from this package, so its region
and version remain erased. The initial installed-version record is an emulator
seed, not evidence that the update package includes boot data.

## RTC backup domain

Date/time lives on the panel MCU in the RTC, rather than in main NOR settings.
`panel-rtc.bin` stores little-endian registers at offsets `0x00–0x5f` matching
`0x40002800–0x4000285f`; offset `0x80` stores RCC BDCR (`0x4002105c`).
The remaining bytes are reserved and zero on creation.

TR contains BCD time, DR contains BCD date and weekday, and CR bit 18 is the
firmware's calendar-valid marker. The model supplies oscillator-ready,
initialization, synchronization, prescaler, and wakeup reset behavior. Register
layout follows the [STM32G0 reference manual](https://www.st.com/resource/en/reference_manual/rm0454-stm32g0x0-advanced-armbased-32bit-mcus-stmicroelectronics.pdf).

The calendar and subsecond counter advance with QEMU virtual time. Pausing
QEMU freezes them. Date/time is retained across process restarts; time while
the emulator is stopped is not added to the calendar.
The panel's RCC oscillator-ready and clock-switch status must also be modeled.
Without them, its two startup waits time out, delaying the RTC reply beyond the
main firmware's two-second limit and reopening Date/Time setup despite a valid
calendar. Supplying those hardware responses allows startup to use the retained
calendar. Persisted settings also avoid repeating Battery Type setup.

## Validation

From the repository root, use a fresh state directory:

```sh
cargo run -p persistence-smoke -- \
  --state-dir /tmp/l6-storage-test --logs emulator/logs/persistence-smoke
```

The harness uses GPIO inputs to edit all five date/time fields, closes QEMU,
starts a fresh process with the same storage, compares the restored calendar
and time, checks the settings-bank reload and absence of repeated panel
programming, and checks that the RTC reply meets the firmware's startup
deadline. It saves the loading, setup, recorder, and Firmware screens.
`--resume` inspects an already initialized test device without repeating setup.

The information page is reached through Menu → System → Firmware, as described
in the [ZOOM operation manual](https://zoomcorp.com/manuals/l6max-en/).
SYSTEM shows 1.10 for the supplied package. With an erased bootloader region,
the firmware formats BOOT as 22.22: this is an invalid-data artifact, not an
identified bootloader version. Its CHECKSUM is computed from the emulated NOR
contents, including the erased bootloader region, and must not be treated as
the checksum of a complete hardware installation.

## Remaining limits

- The bootloader is not supplied, so bootloader execution, its version, and
  bootloader-driven application installation/recovery remain unmodeled.
- Panel option bytes persist for ROM read/write verification; their protection
  and boot-selection side effects are not modeled.
- RTC alarms, wakeup interrupts, write protection, and battery-loss behavior
  are not complete.
- NOR program/erase commands complete immediately. Power-loss timing and full
  NOR identification/protection behavior remain unmodeled.
- Main CPU warm-reset remapping after an upgrade needs validation; the current
  startup loader selects the installed application on process launch.
