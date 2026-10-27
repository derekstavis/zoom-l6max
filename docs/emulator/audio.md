# Audio emulation

The emulator models the SAI and eDMA activity needed by L6 and L6max firmware.
The [current status](../development/current-status.md) describes qualified
scenarios and unresolved playback and recording behavior. Emulated samples do
not establish physical jack routing, analog fidelity, or USB audio support.

## Observed firmware configuration

The inspected L6 uses an RT1041; its SAI register semantics follow the RT1040
reference manual. The L6max physical processor has not been identified. Both
firmware profiles use a nominal 24.576 MHz source and 48 kHz audio cadence.
The physical clock source remains unverified. L6 configures SAI1 and SAI3;
L6max configures SAI1.

## Firmware audio data path

The modeled SAI FIFO and request-paced eDMA paths can deliver transmit samples
to a configured QEMU audio backend. L6 and L6max use different DMA routes.
The `audio-path` smoke case checks callbacks and SAI/eDMA traffic; it does not
establish sample fidelity. Sound-pad and recorder playback require appropriately
prepared fixtures. See [testing](../development/testing.md) and
[current status](../development/current-status.md).

## Modeling direction and remaining work

The L6max profile enables `sai1_rx_read_stream_compat`, distributing receive
words across enabled FIFOs when `FCOMB=2`. This is an emulator compatibility
exception, not a claim about RT1040 silicon. L6 leaves it disabled. Physical
SAI capture or further analysis is needed to explain the difference. The
[peripheral audit](manual-audit.md) records the register behavior and scope.

The demo SD card includes generated tones and a stereo float snare sample.
Its source and terms are recorded in [NOTICE.md](../../NOTICE.md). The card
contains a copy of the locally supplied firmware and is generated locally.
