export interface PatchInfo {
  id: string
  models: readonly ('L6' | 'L6max')[]
  title: string
  description: string
  readmeUrl: string
  /** Optional demo image filename under `public/demos/`, e.g. recorded from the emulator. */
  demoImage?: string
  /** Optional plain-language instructions for triggering the patch from outside gear (e.g. a MIDI command). */
  requestCommand?: string
}

export interface LoadablePatchInfo {
  id: string
  models: readonly ('L6' | 'L6max')[]
  title: string
  description: string
  /** Filename hosted under `public/loadable-patches/<model>/`. */
  fileName: string
  loaderAbi: number
  readmeUrl: string
}

// Descriptions summarize each patch's own README in the main repo — see the
// linked file for the full write-up, including limitations and how it was
// verified.
export const PATCHES: PatchInfo[] = [
  {
    id: 'knob-dialog',
    models: ['L6max'],
    title: 'Show knob values on display',
    description:
      'Shows a popup with the actual value — dB, Hz, percent, or pan position — when you turn a channel knob. The stock firmware changes the value but never displays it.',
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/knob-dialog/README.md',
    demoImage: 'knob-dialog.webp',
  },
  {
    id: 'din-mixer-control',
    models: ['L6max'],
    title: 'Mixer control over MIDI jack',
    description:
      "Makes the mixer's documented \"Mixer Control via MIDI\" setting work over the physical TRS MIDI IN jack, not just USB — the stock firmware never implements that direction despite the setting existing.",
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/din-mixer-control/README.md',
    demoImage: 'din-mixer-control.webp',
  },
  {
    id: 'midi-sync',
    models: ['L6', 'L6max'],
    title: 'Read mixer values over MIDI',
    description:
      "Lets a MIDI controller (for example a motorized-fader surface or the editor app) request the mixer's entire current state. Pairs with mixer control above: without it, a motorized surface starts out of sync until you touch each fader. The stock firmware only ever announces changes made after the controller connects.",
    requestCommand: 'Send Control Change 127 with a value of 64 or higher, on the mixer’s configured MIDI channel.',
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/midi-sync/README.md',
  },
]

/** Dynamic modules shipped by the site. The persistent Patch loader must already
 * be installed on the device before these files can be used. */
export const LOADABLE_PATCHES: LoadablePatchInfo[] = [
  {
    id: 'midi-sync',
    models: ['L6', 'L6max'],
    title: 'Read mixer values over MIDI',
    description:
      'The same MIDI sync feature as above, packaged as an SD card patch. It loads when the card is mounted and clears from RAM when the mixer restarts.',
    fileName: 'MIDI.BIN',
    loaderAbi: 2,
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/patch-loader/README.md',
  },
  {
    id: 'knob-dialog',
    models: ['L6max'],
    title: 'Show knob values on display',
    description:
      'The same knob-value dialog as above, packaged as an SD card patch. It loads when the card is mounted and clears from RAM when the mixer restarts.',
    fileName: 'KNOB.BIN',
    loaderAbi: 2,
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/patch-loader/README.md',
  },
]
