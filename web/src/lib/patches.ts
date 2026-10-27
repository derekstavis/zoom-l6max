export interface PatchInfo {
  id: string
  models: readonly ('L6' | 'L6max')[]
  title: string
  description: string
  readmeUrl: string
  /** Optional demo clip filename under `public/demos/`, e.g. recorded from the emulator. */
  demoGif?: string
  /** Optional plain-language instructions for triggering the patch from outside gear (e.g. a MIDI command). */
  requestCommand?: string
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
    demoGif: 'knob-dialog.gif',
  },
  {
    id: 'din-mixer-control',
    models: ['L6max'],
    title: 'Mixer control over MIDI jack',
    description:
      "Makes the mixer's documented \"Mixer Control via MIDI\" setting work over the physical TRS MIDI IN jack, not just USB — the stock firmware never implements that direction despite the setting existing.",
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/din-mixer-control/README.md',
    demoGif: 'din-mixer-control.gif',
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
