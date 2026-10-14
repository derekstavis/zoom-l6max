# Zoomie

[Documentation index](README.md)

Zoomie is a SwiftUI editor for the ZOOM L6max on iPhone, iPad, and Mac. It
moved to its own repository, `derekstavis/zoom-l6-control`. It speaks the SysEx
protocol mapped in [the editor MIDI protocol notes](firmware/protocols/editor-midi.md).

To use this emulator instead of hardware on a Mac, start it with `--usb` and wait
for the recorder screen. The app finds its `for L6 Editor Port (Emulator)`
endpoints by name. Nothing in the editor has been run against hardware.
