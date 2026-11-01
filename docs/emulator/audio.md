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

## Output fidelity

The `l6-sound-pad-waveform` and `l6max-sound-pad-waveform` cases compare captured
SAI output with the source WAV, fitting one constant gain per channel. A source
that exceeds full scale after firmware gain clips, and fails this comparison. L6max
host output and capture use the SAI slots that follow MASTER, which carry the right
channel first; a high MASTER position clips, so its waveform cases hold MASTER at
768. The slots that follow MONITOR have inverted polarity. These results describe
the digital SAI stream, not the analog outputs.

Two model properties are required for a faithful stream:

- SAI service stops at the frame that raises an eDMA interrupt, so firmware can
  refill its TX ring before DMA advances further.
- Both profiles use documented RX FIFO placement. Paced timing gives the
  guest enough instructions to run the full-rate audio task and lower-priority
  tasks; see [board timing](board.md).

## Live output

The GUI defaults to `--timing split`, which runs MAIN and the panel in separate
QEMU processes so MAIN has a host thread to itself. MAIN uses 2 ns per instruction
on L6max and 4 ns on L6. A socket pair carries panel reset and ROM mode, and MAIN
programs panel flash through the shared `panel-flash.bin`. These runs are not
deterministic, so qualification keeps the single-process timing.

The panel runs at 64 ns per instruction with `sleep=off`, so idle time jumps to its
next timer, and each virtual millisecond is then held to the host clock. Host sleeps
are too coarse for its 50 us LED scan, and the firmware times LED pulses in software:
ring level doubles with the instruction cost, and 64 ns reaches the rings' measured
full scale on both models.

Firmware accelerates encoders by the spacing of their reports. With two clocks that
spacing varies, so equal and opposite turns need not cancel. Split-mode ring checks
assert direction and report count, and single-process runs keep the exact check.

MAIN has little real-time margin. Other host load, even two busy processes, can
underrun live output, and the waveform comparison detects this where the test case
outcome does not.

The GUI opens CoreAudio at 48 kHz Float32 so QEMU's mixer does not resample the
firmware stream. This also sets the output device to 48 kHz, and macOS keeps that
rate after the emulator exits. Output is held until 200 ms is queued, which adds
that latency. The `*-sound-pad-coreaudio` cases record the device callback through
`L6_AUDIO_COREAUDIO_CAPTURE`; compare that capture with the source WAV.

The demo SD card includes generated tones and a stereo float snare sample.
Its source and terms are recorded in [NOTICE.md](../../NOTICE.md). The card
contains a copy of the locally supplied firmware and is generated locally.
