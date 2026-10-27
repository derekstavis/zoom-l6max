# RT1041 board-model manual audit

This audit compares the custom MAIN-MCU blocks used by the L6 and L6max
firmware against the RT1040/RT1043/RT1046 Processor Reference Manual, Rev. 3
(November 2024), alongside RT1041 SDK feature definitions and observed firmware
accesses. The available reference manual is for the closely related RT1040
family, so instance-specific claims are checked against the RT1041 SDK where
possible. This is a register-semantics review, not a claim that the complete
RT1041 SoC or board is modeled.

## Findings

| Block | Manual check | Board-model result |
| --- | --- | --- |
| GPIO | Chapter 12 documents DR/GDIR/PSR and atomic aliases at `+0x84`, `+0x88`, and `+0x8c`; DR reads reflect input pins when configured as inputs. | The QEMU i.MX GPIO model supplies the register behavior and the board adapter routes inputs. Alias offsets match the manual. The board does not model IOMUXC mux/pad electrical behavior. |
| eDMA / DMAMUX | Chapter 6 defines the 32-byte TCD layout, CITER/BITER major-loop reload, DONE and DREQ behavior, CINT NOP/CAIR/channel commands, INT write-one-to-clear, and direct INT output levels. Chapter 5 places DMAMUX at `0x400e_c000`. | The model now reloads CITER from BITER and applies SLAST/DLASTSGA on non-ESG major completion, sets DONE, and clears ERQ only for DREQ. A newly accepted request clears DONE/START as hardware does. ESG reload, request pacing, and observed descriptor layouts are modeled; linked minor loops, MLOFF, unequal transfer widths, and unobserved request sources remain unsupported. Focused checks cover completion side effects; the guest's exercised audio TCDs use ESG. |
| SAI | Chapter 37 defines FIFO request/warning aggregation, per-instance PARAM reset values, RCR4 FCOMB support on SAI1 only, and TMR semantics: masked frame words do not read the TX FIFO (§37.3.5). | SAI reports `VERID=0x03000000`, `PARAM=0x00050504` on SAI1 and `0x00050501` on SAI2/SAI3. TX frame advancement honors TMR before consuming FIFO entries; the standalone check covers a two-word payload after masked slots 0–1. The shared SAI model implements the manual's `FCOMB=2` behavior: each receive line fills its own FIFO while software reads rotate among enabled FIFOs. **Exception:** the L6max profile enables `sai1_rx_read_stream_compat`, which distributes incoming words across enabled FIFOs even with `FCOMB=2`. Removing this exception restored about 750 channel-15 IRQ assertions per guest second but froze the panel and encoder indicators; with it enabled the measured rate was 187–188/s, and the eight-ring indicator smoke exercises the path. The exception is an empirical firmware compatibility behavior, not a claim about RT silicon semantics. Keep it until firmware/eDMA analysis or physical SAI capture explains the freeze and supports a replacement. L6 leaves it disabled. The L6 firmware writes `FCOMB=2` to SAI3, where the field is reserved; the model ignores it and reads zero. SAI3 currently enables only channel 0. Both stock firmware images pass `0x01770000` (24.576 MHz) and 48 kHz into their SAI divider helpers. The physical source inherited from the bootloader remains unverified; QEMU derives frame cadence from the configured master divider, frame size, word width, and that nominal source value. See the [audio investigation](audio.md#observed-firmware-configuration). |
| CCM / CCM_ANALOG clocks | Chapter 14 defines `CCM_CSCMR1` and `CCM_CS1CDR` reset values (`0x04900000` and `0x0ec102c1`), PLL controls, and PFD_480/PFD_528 fraction registers. | MAIN startup reads `CCM_CSCMR1` and PLL_SYS but does not write the clock roots in the observed trace. The sparse MMIO model now seeds the relevant CSCMR1, CS1CDR/CS2CDR, PLL_AUDIO/PLL_SYS, and PFD registers with manual reset values. USB1 PLL SET/CLR/TOG and LOCK semantics are modeled. These defaults describe reset state only; actual values inherited from the instrument's bootloader are unknown. The SAI scheduler derives its cadence from the active master divider and frame format using the nominal 24.576 MHz source passed by firmware; other peripheral clock roots are not modeled. |
| LPSPI3 / external NOR | Chapter 46 defines TCR frame size in bits, RXMSK at bit 19, and LPSPI3 at `0x403a0000`; VERID, PARAM, SR, TCR, and RSR have nonzero reset values. | The sparse MMIO bank now seeds those documented reset values, and LPSPI accesses are included in the firmware trace. The observed L6 8-bit commands (`0x0b` fast read and `0x05` status) are decoded; this is not a general LPSPI controller or complete NOR model. Bundled QEMU's `imx_spi` implements ECSPI, not LPSPI, so it is not a drop-in controller. QEMU's generic SSI bus and serial-NOR devices remain candidates after flash identity, capacity, and write behavior are confirmed. |
| USDHC1 / SD | Chapter 26 lists USDHC1 at `0x402c0000`, specifies 32-bit register accesses, SYS_CTRL reset `0x0080800f`, and VEND_SPEC2 reset `0x00019006`; VEND_SPEC2 has writable ACMD23/tuning/card-interrupt controls. | The board reuses QEMU's i.MX USDHC/SDHCI and SD card models, with a board adapter for clock/power semantics. The adapter now returns the manual SYS_CTRL reset value and implements VEND_SPEC2 reset/read/write behavior; upstream QEMU previously logged the observed `+0xc8` access as unimplemented. Full register reset-state comparison remains open. |
| FlexSPI1 / resource flash | Chapter 27 places FlexSPI1 at `0x402a8000`, defines nonzero reset values for MCR0/MCR1/MCR2, LUT controls, buffer controls, and status, and specifies that MCR0.SWRESET self-clears without resetting configuration registers. | The sparse MMIO bank now seeds the documented nonzero FlexSPI1 reset values. The L6 firmware first reads MCR0, then performs the SWRESET sequence; the adapter self-clears the bit while preserving configuration, as specified. Flash commands remain limited to the observed LUT sequences and synthesize completion/watermark state. Bundled QEMU has no RT FlexSPI controller; its generic Versal OSPI model is a different controller. Full LUT-driven execution and exact FIFO behavior remain open. |
| LPI2C1 and codec | Chapter 45 defines LPI2C1 at `0x403f0000` and gives nonzero reset values for VERID, PARAM, and MRDR; MSR is W1C. | The sparse bank now seeds those nonzero reset values. Startup clears MSR flags with write-one-to-clear semantics and configures MCFGR registers from zero. L6max forwards the observed START/address/data/STOP stream through QEMU's I2C bus. The register-bank codec slaves are protocol approximations; repeated START, receive commands, and analog conversion remain open. L6 made no LPI2C1 accesses in the tested boot. |
| LPUART | Chapter 47 lists eight instances and common reset values: VERID `0x04010003`, PARAM `0x202`, BAUD `0x0f000004`, STAT `0x00c00000`, DATA `0x1000`, and FIFO `0x00c00011`. | The custom panel/MIDI links synthesize active STAT and DATA behavior. All eight instances now start with the documented common reset values for identification/configuration registers; link-specific status/data reads retain their existing behavior. Full FIFO, baud timing, error flags, and pin muxing are not modeled. The RT1040 manual is a cross-reference; instance count and chosen bases follow the observed RT1041 board map. |
| ADC1 | Chapter 64 defines HC0.ADCH as the software-trigger input and AIEN as the conversion-complete interrupt enable; reading R0 clears COCO0. | The host-controlled sample shim now starts a conversion for a valid HC0.ADCH regardless of AIEN, and only pulses the ADC IRQ when AIEN is set. Reading R0 clears COCO0. Conversion timing, calibration, and analog source/reference behavior remain approximate. No matching QEMU RT ADC exists. |

## Follow-up order

1. Establish the clock values left by the L6/L6max bootloaders. The MAIN
   firmware reads the mux state but the current boot trace shows no writes to
   the SAI roots or PLL_AUDIO. Compare inherited CCM values with the RT1040 RM,
   PLL_AUDIO frequency equations, TCR2/RCR2 dividers, and SAI sync fields before
   replacing the current 48 kHz scheduler.
2. Capture and compare FlexSPI LUT/IP command sequences with chapter 27, then
   decide whether its current command-specific adapter should be generalized
   or replaced by an upstream QEMU block.
3. Verify the physical resource NOR and L6max codec parts before modeling
   capacity, write status, reset behavior, conversion data, or analog routing.
4. Compare RT1041-specific register headers with this RT1040 reference manual
   for every instance-specific peripheral decision, especially where the RM
   says a feature is unsupported on one SAI instance.

The focused standalone checks are `emulator/qemu/tests/sai.c`,
`emulator/qemu/tests/edma.c`, and `emulator/qemu/tests/ccm-analog.c`. Firmware
smoke validation uses `cargo run -p l6test -- run --case audio-path` with
configured L6 and L6max fixtures.
