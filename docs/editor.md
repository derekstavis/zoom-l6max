# Zoomie

[Documentation index](README.md)

A SwiftUI editor for the ZOOM L6max on iPhone, iPad, and Mac. It speaks the
SysEx protocol mapped in [the editor MIDI protocol notes](firmware/protocols/editor-midi.md)
over the device's `for L6 Editor Port`.

It covers the settings the original editor exposes: sound pad files, play mode,
level, MIDI note and clock sync; monitor and sub out points; AUX send points;
effect parameters; USB audio mode and mix minus; MIDI channel, out mode, clock
source and CC mapping; battery type, auto power off, recorder mode, SD card
status, clock, file transfer mode, and reset.

## Layout

- `Packages/L6Kit` holds everything that is not user interface: message
  encoding, the CoreMIDI transport, the request/reply session, an observable
  device model, and a simulated L6max.
- `Zoomie` is the app target. Views take the model objects from L6Kit.
- `project.yml` is the [XcodeGen](https://github.com/yonaskolb/XcodeGen) spec
  for `Zoomie.xcodeproj`. Run `xcodegen generate` after changing it.

The app targets iOS 27 and macOS 27 and builds with Xcode 27.

## Running

Open `Zoomie.xcodeproj` and run the `Zoomie` scheme. Set a development
team to run on an iPhone or iPad.

Without a device, choose **Try the Demo**, or launch with `-demo`. The demo
uses the simulated L6max. `-tab pads|mixer|midi|device` picks the first tab.

To use the emulator instead of hardware on a Mac, start it with `--usb` and
wait for the recorder screen. The app finds its
`for L6 Editor Port (Emulator)` endpoints by name.

## Tests

```sh
cd Packages/L6Kit
swift test
```

The suite checks the codec against bytes captured from stock firmware 1.10 and
drives the session and model against the simulated device.

With the emulator running and exposing its MIDI ports, this also runs the
session against real firmware through CoreMIDI:

```sh
L6_LIVE=1 swift test --filter Live
```

## Limits

- Only the L6max layout is implemented. An L6 is detected and reported as
  unsupported.
- Nothing has been run against hardware. Behavior was checked against stock
  firmware in the emulator, without an SD card.
- Pad file lists, assignment, and the resample question follow the firmware
  code but were exercised only against the simulated device. Which resample
  answer overwrites and which creates a copy is an assumption.
- After entering file transfer mode the device ends the session. The app
  reconnects and offers to leave the mode; that path is untested on firmware.
