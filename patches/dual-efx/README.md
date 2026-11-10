# Dual-EFX: Velvet Noise Decorrelator & Plateau Reverb

**Experimental in-progress port:** The patch can be built into a firmware
update or loaded as an SD module. Its dynamic version supports live parameter
editing on the recorder screen and uses the shared knob-dialog service for
parameter feedback. It uses the shared L6max hook API to follow
the stock effect workspace clear, callback selection, type update, and DSP
stream paths. The `DualEfxProcessor` lives in the stock shared effect workspace;
the patch replaces the Room and Spring callbacks with its own processing
callback. That callback reconstructs three sends from emulator-observed channel
buffers and gains, copies the stock control stream, and writes wet stereo
output. Emulator smoke cases observe nonzero input, send, and output samples
for both effects, and confirm the stock callback resumes for other effects.
These are L6max MAIN 1.10 emulator observations; physical memory ownership,
signal mapping, callback concurrency, audio quality, and timing still need
qualification.

Replaces the stock mono/reconstructed send effects with high-end modern DSP engines:
1. **Velvet Noise Decorrelator (VND)** (Effect A):
   Ported from [115dkk/Velvet-Noise-Decorrelator-VST3](https://github.com/115dkk/Velvet-Noise-Decorrelator-VST3) to bare-metal `no_std` ARM Cortex-M7. Replaces Spring Reverb (`efx_type = 2`). Generates a sparse, unit-energy impulse sequence to decorrelate stereo audio channels without comb filtering or transient smearing.
   Each generated bank begins with the reference's positive zero-delay tap.
   The initial banks share their generation number across channels; later
   renewals use consecutive generation numbers for left and right, as in the
   reference. Filter edits use this port's incremental transition behavior.
   This send-effect adaptation uses a wet-only output, single-precision math,
   and up to 400 taps (100 ms at 4,000 impulses/s), matching the Multistomp
   range; it is not bit-identical to the reference plug-in.
   An out-of-range bank tap count silences that bank instead of panicking;
   invalid or overlapping active/pending bank indices silence both audio paths
   and reject parameter edits when the bank-zero initialization marker is
   nonzero, until reset. Other malformed state is not generally validated.
2. **Valley Plateau Plate Reverb** (Effect B):
   Ported from [digitalartifactmusic/PlateauNEVersio](https://github.com/digitalartifactmusic/PlateauNEVersio) / Dattorro 1997 topology to bare-metal `no_std` ARM Cortex-M7. Replaces Room Reverb (`efx_type = 1`). Features 4 input diffusers, 2 cross-coupled modulated feedback delay loops, predelay, damping, and freeze mode.
3. **Dual Concurrent Send & Routing Matrix**:
   Allows running both effects simultaneously with flexible per-channel routing (`Neither`, `Send A`, `Send B`, `Send A -> B` [cascade]) and optional Pre-AINR (AI Noise Reduction) chaining.
4. **Extended Parameter Control**:
   Expands beyond the stock 2-parameter limitation using all 8 physical channel
   encoders. In dynamic mode, a long press on EFX SEL enters or exits parameter
   edit mode; encoder turns update the selected Room or Spring DSP bank, and
   the shared knob-dialog patch displays the parameter name and value.

## Model Support

Targeted exclusively for **LiveTrak L6max** firmware revision 1.10 (`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`).

## CPU & Memory Budget

The original CPU estimates and 160 KiB memory figure were not measured and
must not be used for target qualification. Cascaded sends retain a separate
audio history while sharing Velvet's filter state. The processor's memory
ownership, stack use and worst-case execution time remain unproven on hardware.

The current ARM target reports `size_of::<DualEfxProcessor>()` as 283,160
bytes, before the compiled module code:

| State | Bytes | Main storage |
| --- | ---: | --- |
| Plateau | 212,424 | 211,884 bytes of fixed delay arrays |
| Velvet | 51,424 | 38,416 bytes of sample history; 12,800 bytes of tap banks |
| Cascade history | 19,208 | Separate mono sample history |
| Other fields and alignment | 104 | Routing, parameters, and control state |

The Dual EFX dynamic module currently requests about 28.2 KiB for code and
module state. Its `module.json` requires knob-dialog's `KDLG` v1 API, so the SD
bundle builder includes that provider once and places it before Dual EFX. The
dialog API table and text mailbox live in knob-dialog's module allocation;
Dual EFX stores only the resolved API address. The loader's fixed 128-byte API
registry is part of its reserved registry area and does not allocate from the
FreeRTOS heap. DSP state is initialized in place in the stock shared effect
workspace at `0x80d00000`, whose observed
span is 768,000 bytes. This avoids allocating the 283 KiB processor from the
FreeRTOS heap. The SD smoke fixture reports a 167,528-byte heap low-water mark
while requiring at least 131,072 bytes free. The stock firmware also clears and
reuses this workspace when effects change. The stock selector fades the
current effect before clearing the workspace and publishes the next callback
before it updates the selected-type field. A static hook that replaces the
callback at selector entry can prevent that fade from completing; the emulator
showed the selector waiting in its semaphore path. Both patch modes therefore
leave the callback in place through the fade and drain it at the later
workspace-clear hook. The static patch rebuilds its state in the DSP entry hook
before the newly published Room or Spring callback runs. Its small control
words occupy the first `0x100` bytes of the workspace, and the processor starts
at `0x80d00100`; those control words are rebuilt after stock clears the area.
The dynamic module keeps control state in its SD allocation and rebuilds the
processor after each clear, restoring its last applied parameter values while
letting DSP histories and reverb tails reset. Stock effect parameters are held
separately in the mixer settings image, so clearing the DSP workspace does not
erase the built-in parameter values. Emulator checks cover dynamic selection
and hook chaining; a static emulator run completed a six-selection cycle and
returned to Room with the custom callback active and finite, nonzero output.
Workspace ownership and synchronization still need validation on hardware and
along unobserved firmware paths.

## Source Layout

- `l6max/src/math.rs`: Fast, standalone math functions (`sin`, `cos`, `sqrt`, `pow10`, `exp2`, `clamp`).
- `l6max/src/velvet.rs`: Velvet Noise Decorrelator core DSP engine.
- `l6max/src/plateau.rs`: Valley Plateau Dattorro plate reverb core DSP engine.
- `l6max/src/routing.rs`: Routing matrix and `DualEfxProcessor` audio engine.
  Weighted routing can use a caller-owned 768-byte workspace to avoid its
  block-sized temporary buses on the stack; simultaneous callers need separate
  workspaces. Firmware memory ownership remains unproven.
- `l6max/src/parameters.rs`: Parameter edits and copies of applied values.
- `l6max/src/admission.rs`: Standalone callback admission model; not connected to firmware.
- `l6max/src/parameter_view.rs`: Validation and formatting of copied display values.
- `l6max/src/firmware.rs`: Includes shared L6max firmware declarations.
- `l6max/src/payload.rs`: Static and dynamic DSP/workspace hooks and the
  Room/Spring replacement callback.
- `l6max/linker/link.ld`: Dynamic module layout.
- `l6max/linker/link-static.ld`: Static control state placement in the stock
  effect workspace.

The shared L6max API exposes the fixed DSP stream and effect-workspace clear
functions as dynamic hook sites. The callback pointer is selected by the stock
firmware; the dynamic stream hook interposes only when Room or Spring is active.
The old `hooks.tsv` call-site hook at `0x8000277c` was removed because it
intercepts effect selection and parameter handling, not audio processing.

On the recorder screen, a short EFX SEL tap retains stock Room/Spring selection.
A long hold toggles edit mode without changing the selected effect. Encoder
turns are queued for the audio callback, which updates the selected DSP bank
and publishes the applied value through knob-dialog's versioned runtime API.
The SD bundle declares knob-dialog as a dependency and loads it once before
Dual EFX; Dual EFX stores only the resolved API address. Accepted parameter
edits are retained in the Dual EFX module allocation across effect-workspace
reinitialization. The emulator smoke case verifies all 16 encoder mappings,
checks the resulting DSP values, and captures the dialog and timeout
screenshots. The encoders map as follows:

| Encoder | Velvet bank | Plateau bank |
| --- | --- | --- |
| 1 | Amount | Predelay |
| 2 | Time spread | Size |
| 3 | Density | Decay |
| 4 | Decay | Low cut |
| 5 | Refresh time | High cut |
| 6 | Dynamic mode | Diffusion |
| 7 | Transition time | Modulation depth |
| 8 | Cascade amount | Freeze |

The Velvet refresh control spans 0.1–60 seconds.

For the two on/off settings, clockwise selects on and counterclockwise selects
off. The active stock effect selects the Velvet or Plateau bank, so no extra
bank-selection screen is needed. Parameter editing uses the recorder screen
and the shared transient dialog; per-channel routing controls are still not
exposed in the UI.
Plateau holds its tank modulation delays fixed while frozen to preserve the
tail, and resumes modulation when freeze is released.
The default 48 kHz Plateau size range is 0.01–1.2 and predelay is 0–190 ms; the
Multistomp version supports size through 4.0 and predelay through 500 ms. The
larger ranges still need a viable, qualified memory and processing design. An
experimental 32 kHz core, converter, and inspector model support them in
standalone host tests. The converter adds latency to Plateau; effect-return
alignment, target timing, and firmware allocation are not established.
The normal Plateau sample path returns silence without advancing its state
when a delay cursor or length is invalid; the processor must be reinitialized
before reuse. This does not validate arbitrary corrupt state, the pristine
shortcut under memory corruption, or concurrent firmware access.
Its low-cut and high-cut controls adjust tank damping; the input filter
cutoffs remain fixed. A 20 Hz DC blocker and the input filters precede the
predelay, as in the supplied core. Their histories are cleared on freeze entry
and frozen input is discarded. The filters use this port's one-pole implementation,
so matching the Multistomp control mapping does not establish audio parity.
Damping edits ramp their tank-filter coefficients over 64 samples; the
transition and its target execution time have not been qualified on hardware.
The tank output uses the Multistomp core's 0.5 gain after its DC blocker;
this port's output soft clipper and filters differ from the pedal's wrapper.
Predelay edits crossfade over 64 frames; changes during freeze take effect
without retaining frozen input. The transition has not been auditioned or
timed on the physical device.
The density display reports the target after rounding to a whole number of
taps. The requested setting is retained. Density edits that leave the rounded
tap count unchanged do not regenerate the filter. The audible bank may still
be transitioning to the target. A prior maximum-density diagnostic ran below
modeled audio cadence; the full range has not been qualified on hardware.
The bank builder normalizes up to eight taps per channel per frame so the
maximum-density filter can be ready between 90 ms fades at a 100 ms renewal
interval. Host tests check renewal timing and identical generated tap values;
the added per-frame work has not been timed on the device.

## Build & Test

The standalone DSP tests can be run with:

```bash
cargo test --offline --locked -p l6dsp-tests
```

The `sd-dual-efx-emulator` smoke case builds and loads the dynamic patch,
exercises all eight encoder mappings for both DSP banks, verifies the resulting
parameter values, captures the dialog text, and checks that it times out back
to the recorder screen. It also checks that reconstructed input and output
samples are nonzero and finite while the heap stays above its threshold. It
does not compare the emitted waveform to a reference, prove callback exclusion
across every firmware path, or qualify target timing and hardware behavior.
