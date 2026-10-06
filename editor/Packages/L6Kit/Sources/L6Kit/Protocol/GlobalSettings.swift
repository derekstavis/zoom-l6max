/// The L6max global setting dump, function `2A`.
public struct GlobalSettings: Sendable, Equatable {
    public var batteryType: UInt8 = 0
    public var autoPowerOff: UInt8 = 1
    public var mixerControlViaMIDI: UInt8 = 0
    public var recorderMode: UInt8 = 0
    public var usbAudioInterfaceMode: UInt8 = 0
    public var usbMixMinus: UInt8 = 0
    public var midiClockSource: UInt8 = 0
    public var padClockSync: [Bool] = Array(repeating: false, count: L6maxLayout.padCount)
    public var padPlayMode: [UInt8] = Array(repeating: 1, count: L6maxLayout.padCount)
    public var padLevel: [UInt8] = Array(repeating: UInt8(L6maxLayout.padLevelUnity), count: L6maxLayout.padCount)
    public var padIsPlaying: [Bool] = Array(repeating: false, count: L6maxLayout.padCount)
    public var sdCardReaderMode: UInt8 = 0
    public var midiOutMode: UInt8 = 0
    public var midiChannel: UInt8 = 0
    /// One control change number per mixer parameter; 0 means not mapped.
    public var controlChangeMap: [UInt8] = Array(repeating: 0, count: L6maxLayout.controlChangeSlotCount)
    /// `nil` means the pad does not respond to a note.
    public var padMIDINote: [UInt8?] = [0x3C, 0x3E, 0x40, 0x41]
    /// Two values per effect type, in type order.
    public var effectValues: [Int] = [40, 50, 50, 50, 90, 30, 505, 35, 505, 50]
    public var monitorPoint: UInt8 = 1
    public var subOutPoint: UInt8 = 1
    /// `auxPost[aux][channel]`.
    public var auxPost: [[Bool]] = Array(repeating: Array(repeating: true, count: L6maxLayout.channelCount), count: L6maxLayout.auxCount)

    public init() {}

    static let messageLength = 182
    static let payloadLength = 174

    init?(message b: [UInt8]) {
        guard b.count >= Self.messageLength, b[4] == Frame.globalSettings,
              SevenBit.join14(low: b[5], high: b[6]) >= Self.payloadLength
        else { return nil }
        batteryType = b[7]
        autoPowerOff = b[8]
        mixerControlViaMIDI = b[9]
        recorderMode = b[10]
        usbAudioInterfaceMode = b[11]
        usbMixMinus = b[12]
        midiClockSource = b[13]
        padClockSync = b[14..<18].map { $0 != 0 }
        padPlayMode = Array(b[18..<22])
        padLevel = Array(b[22..<26])
        padIsPlaying = b[26..<30].map { $0 != 0 }
        sdCardReaderMode = b[30]
        midiOutMode = b[31]
        midiChannel = b[32]
        controlChangeMap = Array(b[33..<127])
        padMIDINote = (0..<4).map { b[131 + $0] != 0 ? nil : b[127 + $0] }
        effectValues = (0..<10).map { SevenBit.join14(low: b[135 + 2 * $0], high: b[136 + 2 * $0]) }
        monitorPoint = b[155]
        subOutPoint = b[156]
        auxPost = (0..<L6maxLayout.auxCount).map { aux in
            (0..<L6maxLayout.channelCount).map { b[157 + aux * L6maxLayout.channelCount + $0] != 0 }
        }
    }

    /// The device-side encoding, used by the simulated device and tests.
    public var message: [UInt8] {
        var data: [UInt8] = SevenBit.split14(Self.payloadLength)
        data += [batteryType, autoPowerOff, mixerControlViaMIDI, recorderMode, usbAudioInterfaceMode, usbMixMinus, midiClockSource]
        data += padClockSync.map { $0 ? 1 : 0 }
        data += padPlayMode
        data += padLevel
        data += padIsPlaying.map { $0 ? 1 : 0 }
        data += [sdCardReaderMode, midiOutMode, midiChannel]
        data += controlChangeMap
        data += padMIDINote.map { $0 ?? 0 }
        data += padMIDINote.map { $0 == nil ? 1 : 0 }
        data += effectValues.flatMap { SevenBit.split14($0) }
        data += [monitorPoint, subOutPoint]
        data += auxPost.flatMap { $0.map { $0 ? 1 : 0 } }
        data += Array(repeating: 0, count: 8)
        return Frame.wrap(Frame.globalSettings, data)
    }
}
