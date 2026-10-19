export interface PatchInfo {
  id: string
  title: string
  description: string
  readmeUrl: string
}

// Descriptions summarize each patch's own README in the main repo — see the
// linked file for the full write-up, including limitations and how it was
// verified.
export const PATCHES: PatchInfo[] = [
  {
    id: 'knob-dialog',
    title: 'Channel knob value notifications',
    description:
      'Shows a popup with the actual value — dB, Hz, percent, or pan position — when you turn a channel knob. The stock firmware changes the value but never displays it.',
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/knob-dialog/README.md',
  },
  {
    id: 'midi-sync',
    title: 'MIDI state sync',
    description:
      "Lets a MIDI controller (for example a motorized-fader surface) request the mixer's entire current state. The stock firmware only ever announces changes made after the controller connects.",
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/midi-sync/README.md',
  },
  {
    id: 'din-mixer-control',
    title: 'DIN mixer control',
    description:
      "Makes the mixer's documented \"Mixer Control via MIDI\" setting work over the physical MIDI IN (DIN) jack, not just USB — the stock firmware never implements the DIN direction despite the setting existing.",
    readmeUrl:
      'https://github.com/derekstavis/zoom-l6max/blob/main/patches/din-mixer-control/README.md',
  },
]
