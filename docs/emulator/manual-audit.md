# RT1041 board-model manual audit

The L6 MAIN processor is an i.MX RT1041. This reference compares the board's
modeled peripherals with the RT1040/RT1043/RT1046 Processor Reference Manual,
Rev. 3, and RT1041 SDK definitions. L6max silicon is not identified; its shared
profile and compatibility exceptions do not establish physical RT1041 behavior.

## Findings

| Peripheral | Current model and limits |
| --- | --- |
| GPIO | QEMU's i.MX GPIO supplies direction, input, and interrupt state. A board adapter provides the RT atomic aliases and panel reset/boot wiring. IOMUX pad electrical behavior is not modeled. |
| eDMA / DMAMUX | Models observed TCD layouts, request pacing, scatter/gather reload, completion, and interrupt state. Linked minor loops, MLOFF, unequal transfer widths, and unobserved request sources remain unsupported. |
| SAI | Models FIFO state, frame timing, masks, and the exercised RX/TX paths. L6 ignores the reserved SAI3 FIFO-combine field. See [audio emulation](audio.md). |
| CCM / CCM_ANALOG | Models relevant reset values, USB PLL control and lock state. The audio scheduler uses a nominal 24.576 MHz source and configured frame dividers. Bootloader-inherited clock roots and other peripheral rates remain unverified. |
| LPSPI3 / external NOR | Models the L6 resource-NOR read and status commands and relevant register reset values. It is not a complete LPSPI controller or physical flash model. |
| USDHC1 / SD | Reuses QEMU's i.MX USDHC/SDHCI and SD card models with a board adapter for clock, power, and register semantics. Full reset-state coverage remains open. |
| FlexSPI1 / resource flash | Models the exercised flash command sequences and reset semantics. General LUT execution and exact FIFO behavior remain incomplete. |
| LPI2C1 and codec | L6max forwards observed writes through QEMU's I²C bus to register-bank codec approximations. Repeated START, receive commands, and analog conversion remain incomplete. |
| LPUART | Models the panel and MIDI paths and documented identification/configuration reset values. Full FIFO, baud timing, errors, and pin muxing remain incomplete. |
| ADC1 | Models host-controlled samples, conversion completion, and interrupt enable. Conversion timing, calibration, and analog inputs are approximate. |

Focused peripheral checks live under `emulator/qemu/tests/`. Firmware scenarios
use the [test driver](../development/testing.md), including `audio-path` with
configured L6 and L6max fixtures. The [board model](board.md) lists QEMU reuse
and the [current status](../development/current-status.md) records runtime
qualification.
