import Synchronization

/// An in-memory L6max that follows the firmware's editor session rules.
/// It backs previews, tests, and the app's demo mode.
public final class SimulatedDevice: L6Transport, DeviceConnector {
    private struct State {
        var session = 0
        var settings = GlobalSettings()
        var files: [[String]]
        var assigned: [Int?] = [0, 1, nil, nil]
        var card: SDCardInfo
        var pendingAssignment: (pad: Int, index: Int)?
        var continuation: AsyncStream<[UInt8]>.Continuation?
    }

    private let state: Mutex<State>
    public static let defaultControlChangeMap: [UInt8] =
        Array(0x01...0x08) + Array(0x0B...0x12) + Array(0x15...0x1C) + Array(0x21...0x58)
        + [0x5D, 0x5E, 0x5F, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x6D, 0x6E, 0x71, 0x72, 0x75, 0x77]

    public init(
        files: [[String]] = [
            ["Kick 808.wav", "Air Horn.wav", "Applause.wav"],
            ["Snare Roll.wav", "Rimshot.wav"],
            ["Jingle Intro.wav"],
            [],
        ],
        card: SDCardInfo = SDCardInfo(state: .ready, remainingSeconds: 5 * 3600 + 42 * 60 + 7, usedBytes: 11_800_000_000, capacityBytes: 32_000_000_000)
    ) {
        var initial = State(files: files, card: card)
        initial.settings.controlChangeMap = Self.defaultControlChangeMap
        state = Mutex(initial)
    }

    // MARK: DeviceConnector

    public func makeTransport() async -> (any L6Transport)? { self }

    public func changes() -> AsyncStream<Void> {
        AsyncStream { _ in }
    }

    // MARK: L6Transport

    public func open() async throws -> AsyncStream<[UInt8]> {
        let (stream, continuation) = AsyncStream<[UInt8]>.makeStream()
        state.withLock {
            $0.session = 0
            $0.continuation = continuation
        }
        return stream
    }

    public func close() async {
        state.withLock {
            $0.continuation?.finish()
            $0.continuation = nil
        }
    }

    public func send(_ message: [UInt8]) async throws {
        let (replies, continuation) = state.withLock { state in
            (Self.handle(message, &state), state.continuation)
        }
        guard let continuation else { throw LinkError.disconnected }
        for reply in replies { continuation.yield(reply) }
    }

    /// Simulates a change made on the device's own panel.
    public func emit(_ message: [UInt8]) {
        state.withLock { $0.continuation }?.yield(message)
    }

    // MARK: Firmware behavior

    private static func acknowledgment(_ code: UInt8) -> [UInt8] {
        Frame.wrap(Frame.acknowledgment, [code])
    }

    private static func failure(_ code: DeviceErrorCode) -> [[UInt8]] {
        [acknowledgment(DeviceMessage.failureAcknowledgment), HostMessage.parameter(.error(code))]
    }

    private static func fileData(kind: UInt8, pad: Int, index: Int?, in state: State) -> [UInt8] {
        guard let index, state.files.indices.contains(pad), state.files[pad].indices.contains(index) else {
            return Frame.wrap(Frame.patchData, [kind, UInt8(pad), 0x7F, 0x7F, 0, 0])
        }
        let name = SevenBit.packName(state.files[pad][index])
        return Frame.wrap(Frame.patchData, [kind, UInt8(pad), UInt8(index >> 7), UInt8(index & 0x7F)] + SevenBit.split14(name.count) + name)
    }

    private static func cardMessage(_ card: SDCardInfo) -> [UInt8] {
        let hours = card.remainingSeconds / 3600
        let time = [hours / 100, hours % 100, card.remainingSeconds % 3600 / 60, card.remainingSeconds % 60].map { UInt8($0) }
        return Frame.wrap(Frame.recorder, [0x00, card.state.rawValue] + time + SevenBit.split64(card.usedBytes) + SevenBit.split64(card.capacityBytes))
    }

    private static func handle(_ b: [UInt8], _ state: inout State) -> [[UInt8]] {
        guard b.count >= 6, b.first == Frame.start, b.last == Frame.end else { return [] }
        if b[1] == 0x7E {
            guard b[3] == 0x06, b[4] == 0x01 else { return [] }
            if state.session == 0 { state.session = 1 }
            return [[0xF0, 0x7E, 0x00, 0x06, 0x02, 0x52, 0x72, 0x00, DeviceModel.l6max.rawValue, 0x00, 0x30, 0x31, 0x31, 0x30, 0xF7]]
        }
        guard b[1] == Frame.manufacturer else { return [] }
        let function = b[4]
        if state.session == 1 {
            guard function == Frame.globalSettingsRequest else { return [] }
            state.session = 2
        }
        guard state.session == 2 else { return [] }
        let body = Array(b[5..<(b.count - 1)])
        switch function {
        case Frame.globalSettingsRequest:
            return [state.settings.message]
        case Frame.recorder where body.first == 0x01:
            return [cardMessage(state.card)]
        case Frame.patchData where body.first == 0x05 && body.count >= 9:
            return body[1...].allSatisfy { $0 == 0 } ? failure(.appVersionTooOld) : [acknowledgment(0x11)]
        case Frame.patchDataRequest:
            guard let kind = body.first else { return [] }
            let pad = body.count > 1 ? Int(body[1]) : 0
            guard kind == 0x04 || (0..<L6maxLayout.padCount).contains(pad) else { return [] }
            switch kind {
            case 0x00:
                let count = state.files[pad].count
                return [Frame.wrap(Frame.patchData, [0x00, UInt8(pad), UInt8(count >> 7), UInt8(count & 0x7F)])]
            case 0x01 where body.count >= 4:
                return [fileData(kind: 0x01, pad: pad, index: Int(body[2]) << 7 | Int(body[3]), in: state)]
            case 0x02:
                return [fileData(kind: 0x02, pad: pad, index: state.assigned[pad], in: state)]
            case 0x03:
                return [Frame.wrap(Frame.patchData, [0x03, UInt8(pad), state.settings.padIsPlaying[pad] ? 1 : 0])]
            case 0x04:
                state.settings.controlChangeMap = defaultControlChangeMap
                return [Frame.wrap(Frame.patchData, [0x04] + defaultControlChangeMap)]
            default:
                return []
            }
        case Frame.parameterChange:
            guard let type = body.first, let parameter = Parameter(type: type, arguments: body.dropFirst()) else { return [] }
            return apply(parameter, &state)
        default:
            return []
        }
    }

    private static func apply(_ parameter: Parameter, _ state: inout State) -> [[UInt8]] {
        func pads(_ pad: Int) -> Bool { (0..<L6maxLayout.padCount).contains(pad) }
        let ok = [acknowledgment(parameter.acknowledgment)]
        switch parameter {
        case .dateTime, .keepAlive:
            return ok
        case .setting(let setting, let value):
            switch setting {
            case .batteryType: state.settings.batteryType = value
            case .autoPowerOff: state.settings.autoPowerOff = value
            case .mixerControlViaMIDI: state.settings.mixerControlViaMIDI = value
            case .recorderMode:
                state.settings.recorderMode = value
                return ok + [cardMessage(state.card)]
            case .sdCardReaderMode: state.settings.sdCardReaderMode = value
            case .midiOutMode: state.settings.midiOutMode = value
            case .midiChannel: state.settings.midiChannel = value
            case .usbMixMinus: state.settings.usbMixMinus = value
            case .midiClockSource: state.settings.midiClockSource = value
            case .usbAudioInterfaceMode: state.settings.usbAudioInterfaceMode = value
            case .monitorPoint: state.settings.monitorPoint = value
            case .subOutPoint: state.settings.subOutPoint = value
            }
            return ok
        case .padFile(let pad, let file):
            guard pads(pad) else { return [] }
            guard let file else {
                state.assigned[pad] = nil
                return ok
            }
            guard state.files[pad].indices.contains(file.index) else { return failure(.refused) }
            // The demo's last file of pad 1 needs resampling, to exercise that dialog.
            if pad == 0, file.index == state.files[pad].count - 1, state.files[pad].count > 1 {
                state.pendingAssignment = (pad, file.index)
                return failure(.fileNeedsResampling)
            }
            state.assigned[pad] = file.index
            return ok
        case .padPlayMode(let pad, let mode):
            guard pads(pad) else { return [] }
            state.settings.padPlayMode[pad] = mode.rawValue
            return ok
        case .padLevel(let pad, let level):
            guard pads(pad), L6maxLayout.padLevelRange.contains(level) else { return [] }
            state.settings.padLevel[pad] = UInt8(level)
            return ok
        case .padPlay(let pad, let isOn):
            guard pads(pad), state.assigned[pad] != nil else { return [] }
            let mode = PadPlayMode(rawValue: state.settings.padPlayMode[pad]) ?? .oneShot
            let playing = mode == .hold ? isOn : (isOn ? !state.settings.padIsPlaying[pad] : state.settings.padIsPlaying[pad])
            state.settings.padIsPlaying[pad] = playing
            return [Frame.wrap(Frame.patchData, [0x03, UInt8(pad), playing ? 1 : 0])]
        case .padMIDINote(let pad, let note):
            guard pads(pad) else { return [] }
            if let note {
                for other in 0..<L6maxLayout.padCount where state.settings.padMIDINote[other] == note {
                    state.settings.padMIDINote[other] = nil
                }
            }
            state.settings.padMIDINote[pad] = note
            return ok
        case .padClockSync(let pad, let isOn):
            guard pads(pad) else { return [] }
            state.settings.padClockSync[pad] = isOn
            return ok
        case .factoryReset:
            return failure(.resetNeedsConfirmation)
        case .controlChangeMap(let map):
            guard map.count == L6maxLayout.controlChangeSlotCount else { return [] }
            state.settings.controlChangeMap = map.map { L6maxLayout.isAssignableControlChange($0) ? $0 : 0 }
            return ok
        case .dialog(let dialog, let answer):
            if dialog == DeviceErrorCode.resetNeedsConfirmation.rawValue, answer == .yes {
                var fresh = GlobalSettings()
                fresh.controlChangeMap = defaultControlChangeMap
                state.settings = fresh
                return ok + [cardMessage(state.card)]
            }
            if dialog == DeviceErrorCode.fileNeedsResampling.rawValue, answer != .no, let pending = state.pendingAssignment {
                state.pendingAssignment = nil
                state.assigned[pending.pad] = pending.index
                return [acknowledgment(0x05)]
            }
            state.pendingAssignment = nil
            return ok
        case .effect(let type, let index, let value):
            guard (0..<5).contains(type), (0..<2).contains(index) else { return [] }
            state.settings.effectValues[type * 2 + index] = value
            return ok
        case .auxSendPoint(let channel, let aux, let post):
            guard (0..<L6maxLayout.channelCount).contains(channel), (0..<L6maxLayout.auxCount).contains(aux) else { return [] }
            state.settings.auxPost[aux][channel] = post
            return ok
        case .error:
            return []
        }
    }
}
