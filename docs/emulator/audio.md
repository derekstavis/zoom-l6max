# Audio emulation

The emulator models the SAI and eDMA activity needed by L6 and L6max firmware.
The [current status](../development/current-status.md) describes qualified
scenarios and unresolved playback behavior. Emulated samples do not establish
physical jack routing, analog fidelity, or USB audio support.

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
establish sample fidelity. Sound-pad and recorder playback require a prepared
state and card. See [testing](../development/testing.md) and
[current status](../development/current-status.md).

Mixer controls reach the audio path through a 16-slot ring. A control handler
stores a destination, a word count and up to 16 words in the next slot and
advances the write index. Before each mixer callback, the audio task copies every
pending slot to its destination and advances the read index, so a change never
lands in the middle of a block. The EQ controls' filter coefficients arrive this
way on both models. The emulator leaves the ring to the firmware.

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
- Both profiles use documented RX FIFO placement. Instruction timing gives the
  guest enough instructions to run the full-rate audio task and lower-priority
  tasks; see [board timing](board.md).

## Input

The firmware mixes and records SAI receive words, which are silent until audio files
are assigned and playing. There is no live input: each channel plays a file, which is
enough to exercise the mixer and recorder without the clock and latency issues of capture.

The GUI's Audio Input window assigns a WAV file to each channel, or several in name order,
and has Play, Pause, Stop, Rewind and a position bar to seek. It opens by itself when files
need converting, with a progress bar on each converting channel. A scenario declares the same with
[`audio_input`](../development/scenario-format.md) and `audio-input-transport` steps.
The menu saves its assignments to `[emulator.MODEL.input_patch]` in the local
[`config.toml`](../../config.example.toml), and the emulator applies that table each time it
starts, converting only files that changed. Tests use each scenario's own audio instead.
16, 24 and 32-bit PCM and 32 and 64-bit float WAVs at any sample rate are accepted. Extra
channels in a file are ignored, and a stereo file on a mono channel is mixed down.

The host converts each file ahead of time, so that MAIN's audio path only reads samples.
Files convert in parallel on half of the host's cores, to 48 kHz stereo 32-bit float, in `audio-input` under the
state directory, or the directory in `L6_AUDIO_INPUT_DIR`. MAIN maps that file, and maps
every prepared channel at startup, so assignments survive restarts. Applying the assignments
removes prepared audio that no channel uses. All channels share
one position, driven by guest time, so tracks stay aligned with each other and with the
firmware's clock whatever the host's speed. Playback loops after the longest file, and
shorter files are silent past their end.

Words count across the SAI's data lines, line by line.

| Model | Mono channels | Stereo channels, left/right |
| --- | --- | --- |
| L6max (SAI1) | 1-4: words 0/1, 2/3, 12/13, 14/15 | 5-8: words 8/9, 10/11, 4/5, 6/7 |
| L6 | 1-2: SAI3 words 0/1, 2/3 | 3-6: SAI1 words 0/1, 2/3, 4/5, 6/7 |

The firmware treats each mono pair as dual gain, with the first word read 26.4 dB above
the second. It uses the first word until that word reaches about -1 dB of full scale, and
returns once it falls to about -6 dB. The second word alone never decides. The emulator
writes the source to the second word and the source scaled by 26.4 dB, clipped, to the
first, so a level continues smoothly across the switch. Source full scale is therefore
the second word's full scale.

Each channel records alone on its matching track, before its fader. Channel output also
needs a raised level: `mixer.current.level` in an L6max state spec (indices 0-3 for mono
and 4, 6, 8, 10 for stereo), or the channel encoder. These results describe the digital
stream. Which physical jack or preamp setting produces these words is not established.

## Live output

The emulator runs MAIN and the panel in separate
QEMU processes so MAIN has a host thread to itself. MAIN uses 2 ns per instruction
on L6max and 4 ns on L6. A socket pair carries panel reset and ROM mode, and MAIN
programs panel flash through the shared `panel-flash.bin`. The two clocks are not
synchronized, so runs are not deterministic.

The panel runs at 64 ns per instruction with `sleep=off`, so idle time jumps to its
next timer, and each virtual millisecond is then held to the host clock. Host sleeps
are too coarse for its 50 us LED scan, and the firmware times LED pulses in software:
ring level doubles with the instruction cost, and 64 ns reaches the rings' measured
full scale on both models.

Firmware accelerates encoders by the spacing of their reports. With two clocks that
spacing varies, so equal and opposite turns need not cancel. Ring checks assert
direction and report count.

MAIN has little real-time margin. Other host load, even two busy processes, can
underrun live output, and the waveform comparison detects this where the test case
outcome does not.

The GUI opens CoreAudio at 48 kHz Float32 so QEMU's mixer does not resample the
firmware stream. This also sets the output device to 48 kHz, and macOS keeps that
rate after the emulator exits. Output is held until 200 ms is queued, which adds
that latency. The `*-sound-pad-coreaudio` cases record the device callback through
`L6_AUDIO_COREAUDIO_CAPTURE`; compare that capture with the source WAV.

On Linux the GUI requests the PulseAudio backend at 48 kHz Float32, including
on systems using PipeWire's PulseAudio compatibility service. The selected
sink and its configuration are controlled by the host audio server. This
connection has not yet been qualified for waveform fidelity or underruns.

The demo SD card includes generated tones and a stereo float snare sample.
Its source and terms are recorded in [NOTICE.md](../../NOTICE.md). The card
contains a copy of the locally supplied firmware and is generated locally.
