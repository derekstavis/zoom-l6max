/// SysEx framing shared by every editor message: `F0 52 00 00 <function> ... F7`.
enum Frame {
    static let start: UInt8 = 0xF0
    static let end: UInt8 = 0xF7
    static let manufacturer: UInt8 = 0x52

    static let acknowledgment: UInt8 = 0x00
    static let globalSettings: UInt8 = 0x2A
    static let globalSettingsRequest: UInt8 = 0x2B
    static let parameterChange: UInt8 = 0x31
    static let patchData: UInt8 = 0x45
    static let patchDataRequest: UInt8 = 0x46
    static let recorder: UInt8 = 0x67

    static func wrap(_ function: UInt8, _ data: [UInt8] = []) -> [UInt8] {
        [start, manufacturer, 0, 0, function] + data.map { $0 & 0x7F } + [end]
    }
}

/// A setting whose parameter change carries exactly one value byte.
public enum SimpleSetting: UInt8, Sendable, Equatable, CaseIterable {
    case batteryType = 0x01
    case autoPowerOff = 0x02
    case mixerControlViaMIDI = 0x03
    case recorderMode = 0x04
    case sdCardReaderMode = 0x09
    case midiOutMode = 0x0C
    case midiChannel = 0x0D
    case usbMixMinus = 0x15
    case midiClockSource = 0x16
    case usbAudioInterfaceMode = 0x18
    case monitorPoint = 0x19
    case subOutPoint = 0x1A

    /// Acknowledgment code the device sends when it accepts the change.
    var acknowledgment: UInt8 {
        switch self {
        case .recorderMode: 0x04
        case .sdCardReaderMode: 0x09
        default: 0x00
        }
    }
}

/// Parameter change, function `31`. Both directions share one layout.
public enum Parameter: Sendable, Equatable {
    case dateTime(DeviceDateTime)
    case setting(SimpleSetting, UInt8)
    /// `file == nil` clears the assignment.
    case padFile(pad: Int, file: PadFile?)
    case padPlayMode(pad: Int, PadPlayMode)
    case padLevel(pad: Int, Int)
    case padPlay(pad: Int, Bool)
    case padMIDINote(pad: Int, note: UInt8?)
    case padClockSync(pad: Int, Bool)
    case factoryReset
    case keepAlive
    case controlChangeMap([UInt8])
    case error(DeviceErrorCode)
    case dialog(UInt8, DialogAnswer)
    case effect(type: Int, index: Int, value: Int)
    case auxSendPoint(channel: Int, aux: Int, post: Bool)

    var type: UInt8 {
        switch self {
        case .dateTime: 0x00
        case .setting(let setting, _): setting.rawValue
        case .padFile: 0x05
        case .padPlayMode: 0x06
        case .padLevel: 0x07
        case .padPlay: 0x08
        case .factoryReset: 0x0A
        case .keepAlive: 0x0B
        case .controlChangeMap: 0x0E
        case .padMIDINote: 0x0F
        case .error: 0x10
        case .dialog: 0x12
        case .effect: 0x13
        case .auxSendPoint: 0x14
        case .padClockSync: 0x17
        }
    }

    /// Changes with the same key overwrite each other, so only the newest needs sending.
    var coalescingKey: [UInt8] {
        switch self {
        case .padFile(let pad, _), .padPlayMode(let pad, _), .padLevel(let pad, _), .padPlay(let pad, _),
             .padMIDINote(let pad, _), .padClockSync(let pad, _):
            [type, UInt8(pad)]
        case .effect(let effect, let index, _): [type, UInt8(effect), UInt8(index)]
        case .auxSendPoint(let channel, let aux, _): [type, UInt8(channel), UInt8(aux)]
        case .dialog(let dialog, _): [type, dialog]
        default: [type]
        }
    }

    /// Acknowledgment code the device sends when it accepts this change.
    var acknowledgment: UInt8 {
        switch self {
        case .setting(let setting, _): setting.acknowledgment
        case .padFile: 0x05
        case .padPlay: 0x08
        case .keepAlive: 0x0B
        case .dialog: 0x12
        default: 0x00
        }
    }

    var arguments: [UInt8] {
        switch self {
        case .dateTime(let value):
            return [value.year - 2000, value.month, value.day, value.hour, value.minute, value.second].map { UInt8(clamping: $0) }
        case .setting(_, let value):
            return [value]
        case .padFile(let pad, let file):
            guard let file else { return [UInt8(pad), 0x7F, 0x7F, 0, 0] }
            let name = SevenBit.packName(file.name)
            return [UInt8(pad), UInt8((file.index >> 7) & 0x7F), UInt8(file.index & 0x7F)] + SevenBit.split14(name.count) + name
        case .padPlayMode(let pad, let mode):
            return [UInt8(pad), mode.rawValue]
        case .padLevel(let pad, let level):
            return [UInt8(pad), UInt8(clamping: level)]
        case .padPlay(let pad, let isOn):
            return [UInt8(pad), isOn ? 1 : 0]
        case .padMIDINote(let pad, let note):
            return [UInt8(pad), note ?? 0, note == nil ? 1 : 0]
        case .padClockSync(let pad, let isOn):
            return [UInt8(pad), isOn ? 1 : 0]
        case .factoryReset, .keepAlive:
            return []
        case .controlChangeMap(let map):
            return map
        case .error(let code):
            return [code.rawValue]
        case .dialog(let dialog, let answer):
            return [dialog, answer.rawValue]
        case .effect(let type, let index, let value):
            return [UInt8(type), UInt8(index)] + SevenBit.split14(value)
        case .auxSendPoint(let channel, let aux, let post):
            return [UInt8(channel), UInt8(aux), post ? 1 : 0]
        }
    }

    init?(type: UInt8, arguments a: ArraySlice<UInt8>) {
        let a = Array(a)
        func has(_ count: Int) -> Bool { a.count >= count }
        switch type {
        case 0x00:
            guard has(6) else { return nil }
            self = .dateTime(DeviceDateTime(year: 2000 + Int(a[0]), month: Int(a[1]), day: Int(a[2]), hour: Int(a[3]), minute: Int(a[4]), second: Int(a[5])))
        case 0x05:
            guard has(5) else { return nil }
            let index = Int(a[1]) << 7 | Int(a[2])
            let length = SevenBit.join14(low: a[3], high: a[4])
            guard length > 0, a.count >= 5 + length else {
                self = .padFile(pad: Int(a[0]), file: nil)
                return
            }
            self = .padFile(pad: Int(a[0]), file: PadFile(index: index, name: SevenBit.unpackName(Array(a[5..<5 + length]))))
        case 0x06:
            guard has(2), let mode = PadPlayMode(rawValue: a[1]) else { return nil }
            self = .padPlayMode(pad: Int(a[0]), mode)
        case 0x07:
            guard has(2) else { return nil }
            self = .padLevel(pad: Int(a[0]), Int(a[1]))
        case 0x08:
            guard has(2) else { return nil }
            self = .padPlay(pad: Int(a[0]), a[1] != 0)
        case 0x0A:
            self = .factoryReset
        case 0x0B:
            self = .keepAlive
        case 0x0E:
            self = .controlChangeMap(a)
        case 0x0F:
            guard has(3) else { return nil }
            self = .padMIDINote(pad: Int(a[0]), note: a[2] != 0 ? nil : a[1])
        case 0x10:
            guard has(1) else { return nil }
            self = .error(DeviceErrorCode(rawValue: a[0]))
        case 0x12:
            guard has(2), let answer = DialogAnswer(rawValue: a[1]) else { return nil }
            self = .dialog(a[0], answer)
        case 0x13:
            guard has(4) else { return nil }
            self = .effect(type: Int(a[0]), index: Int(a[1]), value: SevenBit.join14(low: a[2], high: a[3]))
        case 0x14:
            guard has(3) else { return nil }
            self = .auxSendPoint(channel: Int(a[0]), aux: Int(a[1]), post: a[2] != 0)
        case 0x17:
            guard has(2) else { return nil }
            self = .padClockSync(pad: Int(a[0]), a[1] != 0)
        default:
            guard has(1), let setting = SimpleSetting(rawValue: type) else { return nil }
            self = .setting(setting, a[0])
        }
    }
}

/// Patch data, function `45`, as sent by the device.
public enum PatchData: Sendable, Equatable {
    case fileCount(pad: Int, count: Int)
    case fileName(pad: Int, file: PadFile)
    /// `file == nil` when nothing is assigned.
    case assignedFile(pad: Int, file: PadFile?)
    case padState(pad: Int, isPlaying: Bool)
    case defaultControlChangeMap([UInt8])
    case sdInserted(Bool)
    case effect(type: Int, index: Int, value: Int)
    case auxSendPoint(channel: Int, aux: Int, post: Bool)

    init?(kind: UInt8, data d: ArraySlice<UInt8>) {
        let d = Array(d)
        func file() -> (pad: Int, file: PadFile?)? {
            guard d.count >= 5 else { return nil }
            let index = Int(d[1]) << 7 | Int(d[2])
            let length = SevenBit.join14(low: d[3], high: d[4])
            guard index != 0x3FFF, length > 0, d.count >= 5 + length else { return (Int(d[0]), nil) }
            return (Int(d[0]), PadFile(index: index, name: SevenBit.unpackName(Array(d[5..<5 + length]))))
        }
        switch kind {
        case 0x00:
            guard d.count >= 3 else { return nil }
            self = .fileCount(pad: Int(d[0]), count: Int(d[1]) << 7 | Int(d[2]))
        case 0x01:
            guard let parsed = file(), let file = parsed.file else { return nil }
            self = .fileName(pad: parsed.pad, file: file)
        case 0x02:
            guard let parsed = file() else { return nil }
            self = .assignedFile(pad: parsed.pad, file: parsed.file)
        case 0x03:
            guard d.count >= 2 else { return nil }
            self = .padState(pad: Int(d[0]), isPlaying: d[1] != 0)
        case 0x04:
            self = .defaultControlChangeMap(d)
        case 0x06:
            guard d.count >= 1 else { return nil }
            self = .sdInserted(d[0] != 0)
        case 0x07:
            guard d.count >= 4 else { return nil }
            self = .effect(type: Int(d[0]), index: Int(d[1]), value: SevenBit.join14(low: d[2], high: d[3]))
        case 0x08:
            guard d.count >= 3 else { return nil }
            self = .auxSendPoint(channel: Int(d[0]), aux: Int(d[1]), post: d[2] != 0)
        default:
            return nil
        }
    }
}

/// Everything the device can send on the editor port.
public enum DeviceMessage: Sendable, Equatable {
    case identity(Identity)
    /// Identity reply from something that is not an L6 family device.
    case foreignIdentity
    case acknowledgment(UInt8)
    case globalSettings(GlobalSettings)
    case parameter(Parameter)
    case patchData(PatchData)
    case sdCard(SDCardInfo)
    case unknown([UInt8])

    /// Acknowledgment code that announces a following error parameter.
    static let failureAcknowledgment: UInt8 = 0x40

    public init(_ bytes: [UInt8]) {
        self = DeviceMessage.parse(bytes) ?? .unknown(bytes)
    }

    private static func parse(_ b: [UInt8]) -> DeviceMessage? {
        guard b.count >= 6, b.first == Frame.start, b.last == Frame.end else { return nil }
        if b[1] == 0x7E {
            guard b[3] == 0x06, b[4] == 0x02 else { return nil }
            guard b.count == 15, b[5] == Frame.manufacturer, b[6] == 0x72, b[7] == 0,
                  let model = DeviceModel(rawValue: b[8]), b[9] == 0
            else { return .foreignIdentity }
            return .identity(Identity(model: model, rawVersion: String(decoding: b[10..<14], as: UTF8.self)))
        }
        guard b[1] == Frame.manufacturer, b[2] == 0, b[3] == 0 else { return nil }
        let body = b[5..<(b.count - 1)]
        switch b[4] {
        case Frame.acknowledgment:
            return body.first.map { .acknowledgment($0) }
        case Frame.globalSettings:
            return GlobalSettings(message: b).map { .globalSettings($0) }
        case Frame.parameterChange:
            guard let type = body.first else { return nil }
            return Parameter(type: type, arguments: body.dropFirst()).map { .parameter($0) }
        case Frame.patchData:
            guard let kind = body.first else { return nil }
            return PatchData(kind: kind, data: body.dropFirst()).map { .patchData($0) }
        case Frame.recorder:
            guard body.first == 0x00, body.count >= 25 else { return nil }
            let d = Array(body.dropFirst())
            // The L6 form omits the seconds byte.
            let hasSeconds = d.count >= 25
            let time = Int(d[1]) * 100 * 3600 + Int(d[2]) * 3600 + Int(d[3]) * 60 + (hasSeconds ? Int(d[4]) : 0)
            let counts = d[(hasSeconds ? 5 : 4)...]
            return .sdCard(SDCardInfo(
                state: SDCardState(rawValue: d[0]) ?? .invalid,
                remainingSeconds: time,
                usedBytes: SevenBit.join64(counts.prefix(10)),
                capacityBytes: SevenBit.join64(counts.dropFirst(10).prefix(10))
            ))
        default:
            return nil
        }
    }
}

/// Messages the host sends.
public enum HostMessage {
    public static let identityRequest: [UInt8] = [0xF0, 0x7E, 0x00, 0x06, 0x01, 0xF7]
    public static let globalSettingsRequest = Frame.wrap(Frame.globalSettingsRequest)
    public static let sdCardRequest = Frame.wrap(Frame.recorder, [0x01])
    public static let defaultControlChangeMapRequest = Frame.wrap(Frame.patchDataRequest, [0x04])

    public static func parameter(_ parameter: Parameter) -> [UInt8] {
        Frame.wrap(Frame.parameterChange, [parameter.type] + parameter.arguments)
    }

    /// Four version components, each 14 bits.
    public static func appVersion(_ components: [Int]) -> [UInt8] {
        Frame.wrap(Frame.patchData, [0x05] + (0..<4).flatMap { SevenBit.split14($0 < components.count ? components[$0] : 0) })
    }

    public static func fileCountRequest(pad: Int) -> [UInt8] {
        Frame.wrap(Frame.patchDataRequest, [0x00, UInt8(pad)])
    }

    public static func fileNameRequest(pad: Int, index: Int) -> [UInt8] {
        Frame.wrap(Frame.patchDataRequest, [0x01, UInt8(pad), UInt8((index >> 7) & 0x7F), UInt8(index & 0x7F)])
    }

    public static func assignedFileRequest(pad: Int) -> [UInt8] {
        Frame.wrap(Frame.patchDataRequest, [0x02, UInt8(pad)])
    }

    public static func padStateRequest(pad: Int) -> [UInt8] {
        Frame.wrap(Frame.patchDataRequest, [0x03, UInt8(pad)])
    }
}
