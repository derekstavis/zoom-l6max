import L6Kit
import SwiftUI

struct SoundPadsView: View {
    let pads: [SoundPad]
    let device: L6Device

    var body: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 300, maximum: 520), spacing: 16)], spacing: 16) {
                ForEach(pads) { pad in
                    SoundPadCard(pad: pad)
                }
            }
            .padding()
        }
        .safeAreaInset(edge: .top) {
            CardWarning(device: device)
        }
        .navigationTitle("Sound Pads")
    }
}

/// Reads the card state itself, so card updates do not touch the pad grid.
struct CardWarning: View {
    let device: L6Device

    var body: some View {
        let state = device.sdCard.state
        if state != .ready {
            Label(state == .absent ? "No SD Card" : "Invalid SD Card", systemImage: "sdcard")
                .font(.callout)
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(.thinMaterial, in: Capsule())
        }
    }
}

struct SoundPadCard: View {
    let pad: SoundPad

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            PadTrigger(pad: pad)
            PadFileRow(pad: pad)
            PadSettings(pad: pad)
        }
        .padding()
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 16))
    }
}

/// The pad itself: press to play, like the button on the device.
struct PadTrigger: View {
    let pad: SoundPad

    var body: some View {
        Button {
            // Presses are sent as they happen by the button style.
        } label: {
            Text("PAD \(pad.id + 1)", comment: "Label on a sound pad button. The variable is the pad number.")
                .font(.title2.bold())
                .frame(maxWidth: .infinity, minHeight: 72)
        }
        .buttonStyle(PadButtonStyle(isPlaying: pad.isPlaying, onPress: pad.press, onRelease: pad.release))
        .disabled(pad.assignedFile == nil)
        .accessibilityValue(pad.isPlaying ? Text("Playing") : Text("Stopped"))
        .accessibilityAction {
            pad.press()
            pad.release()
        }
    }
}

struct PadButtonStyle: ButtonStyle {
    let isPlaying: Bool
    let onPress: () -> Void
    let onRelease: () -> Void

    func makeBody(configuration: Configuration) -> some View {
        PadButtonBody(configuration: configuration, isPlaying: isPlaying, onPress: onPress, onRelease: onRelease)
    }
}

private struct PadButtonBody: View {
    let configuration: ButtonStyleConfiguration
    let isPlaying: Bool
    let onPress: () -> Void
    let onRelease: () -> Void
    @Environment(\.isEnabled) private var isEnabled

    var body: some View {
        configuration.label
            .foregroundStyle(isPlaying ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
            .background(
                isPlaying ? AnyShapeStyle(.tint) : AnyShapeStyle(.fill.tertiary),
                in: RoundedRectangle(cornerRadius: 12)
            )
            .opacity(isEnabled ? 1 : 0.5)
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(.snappy(duration: 0.12), value: configuration.isPressed)
            .onChange(of: configuration.isPressed) { _, isPressed in
                if isPressed { onPress() } else { onRelease() }
            }
    }
}

struct PadFileRow: View {
    @Bindable var pad: SoundPad

    var body: some View {
        if pad.files.isEmpty {
            Text(
                "No files for this pad. Store audio files in the SOUND_PAD/PAD\(pad.id + 1) folder on the SD card.",
                comment: "Shown when a sound pad has no audio files. The variable is the pad number."
            )
            .font(.callout)
            .foregroundStyle(.secondary)
        } else {
            LabeledContent("File") {
                Picker("File", selection: $pad.assignedFileIndex) {
                    Text("None assigned").tag(Int?.none)
                    ForEach(pad.files) { file in
                        Text(verbatim: file.name).tag(Int?.some(file.index))
                    }
                }
                .labelsHidden()
            }
        }
    }
}

struct PadSettings: View {
    @Bindable var pad: SoundPad

    var body: some View {
        Picker("Play Mode", selection: $pad.playMode) {
            ForEach(PadPlayMode.allCases) { mode in
                Text(mode.name).tag(mode)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()

        PadLevelRow(pad: pad)

        LabeledContent("MIDI Note") {
            LongListPicker(title: "MIDI Note", selection: $pad.midiNote, options: MIDINoteName.options)
        }

        Toggle("MIDI Clock Sync", isOn: $pad.clockSync)
    }
}

struct PadLevelRow: View {
    @Bindable var pad: SoundPad

    var body: some View {
        LabeledContent {
            Slider(
                value: $pad.levelPosition,
                in: Double(L6maxLayout.padLevelRange.lowerBound)...Double(L6maxLayout.padLevelRange.upperBound)
            ) {
                Text("Level")
            }
            .labelsHidden()
        } label: {
            PadLevelLabel(level: pad.level)
        }
    }
}

struct PadLevelLabel: View {
    let level: Int

    var body: some View {
        VStack(alignment: .leading) {
            Text("Level")
            if level == 0 {
                Text(verbatim: "−∞ dB")
            } else {
                Text("\(level - L6maxLayout.padLevelUnity, format: .number.sign(strategy: .always(includingZero: false))) dB", comment: "Sound pad level in decibels.")
            }
        }
        .monospacedDigit()
        .frame(minWidth: 64, alignment: .leading)
    }
}
