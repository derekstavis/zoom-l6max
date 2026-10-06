import Foundation
import L6Kit
import SwiftUI

extension BatteryType {
    var name: LocalizedStringResource {
        switch self {
        case .alkaline: "Alkaline"
        case .nickelMetalHydride: "Ni-MH"
        case .lithium: "Lithium"
        }
    }
}

extension RecorderMode {
    var name: LocalizedStringResource {
        switch self {
        case .multiTrack: "Multi Track"
        case .master: "Master"
        }
    }
}

extension AudioInterfaceMode {
    var name: LocalizedStringResource {
        switch self {
        case .multiTrack: "Multi Track"
        case .stereoMix: "Stereo Mix"
        }
    }
}

extension MIDIClockSource {
    var name: LocalizedStringResource {
        switch self {
        case .automatic: "Auto"
        case .midiIn: "MIDI In"
        case .usbMIDI: "USB MIDI"
        }
    }
}

extension MIDIOutMode {
    var name: LocalizedStringResource {
        switch self {
        case .out: "Out"
        case .thru: "Thru"
        }
    }
}

extension OutputPoint {
    var name: LocalizedStringResource {
        switch self {
        case .preMasterFader: "Pre Master Fader"
        case .preMasterFaderWithCompressor: "Pre Master Fader + Comp"
        case .postMasterFader: "Post Master Fader"
        }
    }
}

extension PadPlayMode {
    var name: LocalizedStringResource {
        switch self {
        case .oneShot: "One-shot"
        case .loop: "Loop"
        case .hold: "Hold"
        }
    }
}

extension DeviceIssue {
    var title: LocalizedStringResource {
        switch kind {
        case .noReply: "No Reply"
        case .device(let code):
            switch code {
            case .fileTransferWhileRecording, .resetWhileRecording, .recorderModeWhileRecording, .padBusy: "Now Recording"
            case .appVersionTooOld, .appVersionOutOfRange: "App Version Too Old"
            case .fileSampleRate, .fileBitDepth, .fileChannelCount: "Invalid File"
            default: "Error"
            }
        }
    }

    var message: LocalizedStringResource {
        switch kind {
        case .noReply: "The L6max did not answer. The setting was not changed."
        case .device(let code):
            switch code {
            case .fileTransferWhileRecording: "Cannot enter File Transfer Mode because recording is in progress."
            case .resetWhileRecording: "Cannot reset all settings because recording is in progress."
            case .recorderModeWhileRecording: "Recorder Mode cannot be switched because recording is in progress."
            case .padBusy: "This sound pad cannot be changed right now."
            case .appVersionTooOld, .appVersionOutOfRange: "The L6max does not accept this app version."
            case .fileSampleRate: "Choose a 48 kHz file."
            case .fileBitDepth: "Choose a 16/24-bit or 32-bit float file."
            case .fileChannelCount: "Choose a mono or stereo file."
            default: "The L6max refused the change (code \(Int(code.rawValue)))."
            }
        }
    }
}

enum MIDINoteName {
    private static let names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

    /// `60` becomes `C3 (60)`, matching the octave numbering printed in the manual.
    static func label(_ note: UInt8) -> String {
        "\(names[Int(note) % 12])\(Int(note) / 12 - 2) (\(note))"
    }

    /// Every choice for a pad's note, starting with none.
    static let options: [LongListOption<UInt8?>] =
        [LongListOption(value: nil, title: .localized("Not Mapped"))]
        + (UInt8(0)...127).map { LongListOption(value: $0, title: .verbatim(label($0))) }
}

extension SoundPad {
    /// The pad's level as a slider position.
    var levelPosition: Double {
        get { Double(level) }
        set { level = Int(newValue.rounded()) }
    }

    /// Index of the assigned file among `files`; setting it asks the device to assign that file.
    var assignedFileIndex: Int? {
        get { assignedFile?.index }
        set { assign(newValue.flatMap { index in files.first { $0.index == index } }) }
    }
}

extension Effect {
    /// Parameter `index` as a slider position.
    subscript(position index: Int) -> Double {
        get { Double(values[index]) }
        set { setValue(Int(newValue.rounded()), at: index) }
    }
}

extension SDCardInfo {
    var remainingTime: Duration { .seconds(remainingSeconds) }
}
