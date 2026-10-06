/// Device families that answer the identity request with ZOOM's `52 72 00` header.
public enum DeviceModel: UInt8, Sendable, Equatable, CaseIterable {
    case l6 = 0x0B
    case l6max = 0x0D
}

public struct Identity: Sendable, Equatable {
    public var model: DeviceModel
    /// Four ASCII digits, for example `0110` for system 1.10.
    public var rawVersion: String

    public init(model: DeviceModel, rawVersion: String) {
        self.model = model
        self.rawVersion = rawVersion
    }

    /// `0110` becomes `1.10`.
    public var version: String {
        guard rawVersion.count == 4, rawVersion.allSatisfy(\.isNumber) else { return rawVersion }
        let major = rawVersion.prefix(2).drop { $0 == "0" }
        return "\(major.isEmpty ? "0" : String(major)).\(rawVersion.suffix(2))"
    }
}

public enum BatteryType: UInt8, Sendable, Equatable, CaseIterable, Identifiable {
    case alkaline, nickelMetalHydride, lithium
    public var id: UInt8 { rawValue }
}

public enum RecorderMode: UInt8, Sendable, Equatable, CaseIterable, Identifiable {
    case multiTrack, master
    public var id: UInt8 { rawValue }
}

/// Wire value 1 selects Stereo Mix; 0 selects Multi Track.
public enum AudioInterfaceMode: UInt8, Sendable, Equatable, CaseIterable, Identifiable {
    case multiTrack, stereoMix
    public var id: UInt8 { rawValue }
}

public enum MIDIClockSource: UInt8, Sendable, Equatable, CaseIterable, Identifiable {
    case automatic, midiIn, usbMIDI
    public var id: UInt8 { rawValue }
}

public enum MIDIOutMode: UInt8, Sendable, Equatable, CaseIterable, Identifiable {
    case out, thru
    public var id: UInt8 { rawValue }
}

/// Tap point for the monitor and sub outputs.
public enum OutputPoint: UInt8, Sendable, Equatable, CaseIterable, Identifiable {
    case preMasterFader, preMasterFaderWithCompressor, postMasterFader
    public var id: UInt8 { rawValue }
}

public enum PadPlayMode: UInt8, Sendable, Equatable, CaseIterable, Identifiable {
    case oneShot, loop, hold
    public var id: UInt8 { rawValue }
}

public enum SDCardState: UInt8, Sendable, Equatable {
    case absent, ready, invalid
}

public struct SDCardInfo: Sendable, Equatable {
    public var state: SDCardState
    public var remainingSeconds: Int
    public var usedBytes: UInt64
    public var capacityBytes: UInt64

    public init(state: SDCardState = .absent, remainingSeconds: Int = 0, usedBytes: UInt64 = 0, capacityBytes: UInt64 = 0) {
        self.state = state
        self.remainingSeconds = remainingSeconds
        self.usedBytes = usedBytes
        self.capacityBytes = capacityBytes
    }
}

/// An audio file in a pad's `SOUND_PAD/PADn` folder.
public struct PadFile: Sendable, Equatable, Hashable, Identifiable {
    public var index: Int
    public var name: String
    public var id: Int { index }

    public init(index: Int, name: String) {
        self.index = index
        self.name = name
    }
}

public struct DeviceDateTime: Sendable, Equatable {
    public var year: Int
    public var month: Int
    public var day: Int
    public var hour: Int
    public var minute: Int
    public var second: Int

    public init(year: Int, month: Int, day: Int, hour: Int, minute: Int, second: Int) {
        self.year = year
        self.month = month
        self.day = day
        self.hour = hour
        self.minute = minute
        self.second = second
    }
}

/// Codes the device sends in parameter change type `10` after a refused request.
public struct DeviceErrorCode: RawRepresentable, Sendable, Equatable, Hashable {
    public var rawValue: UInt8
    public init(rawValue: UInt8) { self.rawValue = rawValue }

    public static let fileTransferWhileRecording = DeviceErrorCode(rawValue: 0x00)
    public static let resetWhileRecording = DeviceErrorCode(rawValue: 0x02)
    public static let resetNeedsConfirmation = DeviceErrorCode(rawValue: 0x03)
    public static let appVersionOutOfRange = DeviceErrorCode(rawValue: 0x04)
    public static let appVersionTooOld = DeviceErrorCode(rawValue: 0x05)
    public static let fileNeedsResampling = DeviceErrorCode(rawValue: 0x06)
    public static let fileSampleRate = DeviceErrorCode(rawValue: 0x07)
    public static let fileBitDepth = DeviceErrorCode(rawValue: 0x08)
    public static let fileChannelCount = DeviceErrorCode(rawValue: 0x09)
    public static let recorderModeWhileRecording = DeviceErrorCode(rawValue: 0x0A)
    public static let refused = DeviceErrorCode(rawValue: 0x0B)
    public static let padBusy = DeviceErrorCode(rawValue: 0x0C)
}

public enum DialogAnswer: UInt8, Sendable, Equatable {
    case no, yes, other
}

/// Fixed shape of the L6max mixer as the editor protocol exposes it.
public enum L6maxLayout {
    public static let padCount = 4
    public static let channelCount = 8
    public static let auxCount = 2
    public static let controlChangeSlotCount = 94
    public static let padLevelRange = 0...59
    /// Pad level 49 is 0 dB; 0 is silence; each step is 1 dB.
    public static let padLevelUnity = 49

    /// Row titles of the control change grid; each has one slot per channel.
    public static let controlChangeChannelRows = [
        "EQ HI LEVEL", "EQ MID FREQ", "EQ MID LEVEL", "EQ LO LEVEL", "SUB MIX SEND", "AUX1 SEND",
        "AUX2 SEND", "EFX SEND", "PAN", "LEVEL", "MUTE",
    ]
    /// The last six slots are not per channel.
    public static let controlChangeGlobalSlots = [
        "CH 5 MONO x2", "CH 6 MONO x2", "USB 1/2", "USB 3/4", "EFX TYPE", "COMPRESSOR",
    ]

    /// Control change numbers the device refuses and stores as unassigned.
    public static func isAssignableControlChange(_ number: UInt8) -> Bool {
        number != 0 && number <= 0x77 && number != 0x20 && !(0x60...0x65).contains(number)
    }

    public struct EffectParameterSpec: Sendable, Equatable {
        public var name: String
        public var range: ClosedRange<Int>
    }

    public struct EffectSpec: Sendable, Equatable {
        public var name: String
        public var parameters: [EffectParameterSpec]
    }

    public static let effects: [EffectSpec] = [
        EffectSpec(name: "Hall", parameters: [.init(name: "DECAY", range: 0...100), .init(name: "TONE", range: 0...100)]),
        EffectSpec(name: "Room", parameters: [.init(name: "DECAY", range: 0...100), .init(name: "TONE", range: 0...100)]),
        EffectSpec(name: "Spring", parameters: [.init(name: "DWELL", range: 0...100), .init(name: "TONE", range: 0...100)]),
        EffectSpec(name: "Delay", parameters: [.init(name: "TIME", range: 10...2000), .init(name: "FEEDBACK", range: 0...100)]),
        EffectSpec(name: "Echo", parameters: [.init(name: "TIME", range: 10...2000), .init(name: "REPEAT", range: 0...100)]),
    ]
}
