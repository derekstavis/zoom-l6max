# STM32G031 panel manual audit

The panel MCU is reported as STM32G031C4T6 on both L6 and L6max. This
focused source inspection compares the shared QEMU panel model with ST
[RM0444 Rev 6](https://www.st.com/resource/en/reference_manual/rm0444-stm32g0x1-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)
and the [STM32G031 datasheet](https://www.st.com/resource/en/datasheet/stm32g031c6.pdf).
It is not a complete register audit or a physical measurement. Runtime scenario
results do not establish peripheral conformance.

| Area | Manual and implementation | Follow-up |
| --- | --- | --- |
| Memory | The datasheet specifies 16 KiB flash and 8 KiB SRAM. The updater bounds flash operations to 16 KiB and the panel maps 8 KiB SRAM. The executable flash mapping is 16 KiB; persistent backing remains 64 KiB for state compatibility. | Continue testing existing states and update/restart paths. |
| GPIO | RM0444 sections 7.5.5–7.5.7 specify 16-bit IDR/ODR and BSRR set priority. The board masks IDR and ODR to 16 bits and implements BSRR set priority and BRR output clearing. GPIO modes and electrical behavior remain approximated. | Evaluate QEMU's STM32L4x5 GPIO with matrix and indicator wiring; test additional GPIO register semantics. |
| TIM2/3/14 | RM0444 sections 22.4 and 24.4 describe write-zero-to-clear status flags, update event controls and one-pulse mode. TIM2/3/14 honor UDIS and OPM on overflow, clear status flags on zero, assert update IRQ while UIF and UIE are set, and handle software UG and URS for the modeled update path. Counter modes, preload transfers and other interrupt sources remain incomplete. | Extend register-level tests and model counter modes, preload and other events when needed. |
| DMA/SPI | RM0444 sections 10.6 and 35 describe DMA channels and SPI. The model handles the specific SPI1 LED transfer as a timed DMA callback, setting completion flags and shifting bytes to the external indicator model. It does not implement general channel scheduling, DMA request routing or SPI FIFO behavior. | Exercise any replacement against both models' LED and control scenarios; do not treat this shortcut as general DMA conformance. |
| RTC | RM0444 sections 30.3 and 30.6 define prescalers, shadow synchronization and write protection. The panel returns a fixed 256-step subsecond counter based on virtual milliseconds; it does not derive that count from programmed prescalers. Initialization and oscillator ready flags are immediate approximations. | Compare firmware configuration and add tests for prescaler, shadow and initialization behavior before expanding the RTC model. |
| RCC | RM0444 section 5.4 describes clock switching and oscillator state. The model immediately reflects oscillator enables as ready and mirrors the requested clock selection; the panel timer model uses a fixed 48 MHz clock. | Model clock enable, source validity and timing when needed by firmware; retain the fixed-rate assumption as an emulator approximation until validated. |
| USART1 | The panel uses QEMU's STM32L4x5 USART, not a dedicated G031 device. Similar register names and passing firmware exchanges do not establish full compatibility with RM0444 section 33. | Compare the firmware-used fields, interrupt and reset behavior before claiming register compatibility. |
| System bootloader | QEMU implements the observed AN3155 read, write and erase subset; the actual ROM image and revision are not present. Out-of-range reads return `0xff` and invalid page numbers are ignored by the current model. | Verify invalid-command and range responses against the applicable ST bootloader documentation or hardware; do not infer them from the MCU name alone. |

The source entry points are `emulator/qemu/hw/arm/l6max.c` and the upstream
`hw/char/stm32l4x5_usart.c`. The shared panel model and its limitations apply
to both firmware profiles; matrix wiring and MAIN UART routing are profile-specific.
