# Dual-EFX: Velvet Noise Decorrelator & Plateau Reverb

**Disabled:** The firmware hooks and memory bindings are unsafe and have not
been qualified on hardware. Both patch builders reject this patch until those
issues are fixed and verified. The descriptions and estimates below describe
the intended design; they do not establish implemented behavior or safety.

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
   Expands beyond the stock 2-parameter limitation using all 8 physical channel encoders and the L6max OLED display for live parameter feedback.

## Model Support

Targeted exclusively for **LiveTrak L6max** firmware revision 1.10 (`980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461`).

## CPU & Memory Budget

The original CPU estimates and 160 KiB memory figure were not measured and
must not be used for target qualification. Cascaded sends retain a separate
audio history while sharing Velvet's filter state. The processor's memory
ownership, stack use and worst-case execution time remain unproven
on the L6max. The `0x83e00000` state address is unverified on hardware.

## Source Layout

- `src/math.rs`: Fast, standalone math functions (`sin`, `cos`, `sqrt`, `pow10`, `exp2`, `clamp`).
- `src/velvet.rs`: Velvet Noise Decorrelator core DSP engine.
- `src/plateau.rs`: Valley Plateau Dattorro plate reverb core DSP engine.
- `src/routing.rs`: Routing matrix and `DualEfxProcessor` audio engine.
  Weighted routing can use a caller-owned 768-byte workspace to avoid its
  block-sized temporary buses on the stack; simultaneous callers need separate
  workspaces. Firmware memory ownership remains unproven.
- `src/parameters.rs`: Parameter edits and copies of applied values.
- `src/admission.rs`: Standalone callback admission model; not connected to firmware.
- `src/parameter_view.rs`: Validation and formatting of copied display values.
- `src/firmware.rs`: Hardware and firmware bindings for L6max 1.10.
- `src/trampolines.s`, `src/trampolines.rs`: Displaced instruction veneers.
- `src/payload.rs`: Main payload entrypoints (`efx_dispatch_hook`, `knob_changed`).
- `linker/link.ld`: Payload placement at `PAYLOAD_BASE` (`0x80108000`) and `.state` at `0x83e00000`.
- `hooks.tsv`: Hook table guarding `0x8000277c`.

The standalone inspector parameter model maps encoders 1–8 as follows:

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
off. The model and value formatter are host-tested; firmware event handling,
display presentation, bank selection and routing controls are not connected.
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

Both patch builders intentionally reject `dual-efx`. The stock emulator's
`audio-path` case checks callback and device observations; it does not test
this effect or qualify its audio behavior. See the
[safety status](../../docs/dual-efx-safety-status.md) for the remaining work.
