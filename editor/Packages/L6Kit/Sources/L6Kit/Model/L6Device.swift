import Foundation
import Observation

public enum ConnectionFailure: Sendable, Equatable {
    /// Something answered on the port, but it is not an L6.
    case notAnL6
    case unsupportedModel(DeviceModel)
    case appVersionRejected
}

public enum ConnectionState: Sendable, Equatable {
    case searching
    case connecting
    case connected
    case failed(ConnectionFailure)
}

/// A problem the user should hear about.
public struct DeviceIssue: Identifiable, Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        case device(DeviceErrorCode)
        case noReply
    }

    public let id = UUID()
    public var kind: Kind
}

/// The device asks whether a pad file may be resampled to 48 kHz.
public struct ResamplePrompt: Identifiable, Sendable, Equatable {
    public let id = UUID()
    public var pad: Int
    public var file: PadFile
}

/// Live state of one L6max, kept in step with the device over its editor port.
@MainActor
@Observable
public final class L6Device {
    // MARK: Connection

    public private(set) var connection: ConnectionState = .searching
    public private(set) var identity: Identity?
    public private(set) var sdCard = SDCardInfo()
    public private(set) var isFileTransferMode = false
    public private(set) var isBusy = false
    public private(set) var lastClockSync: Date?
    public var issue: DeviceIssue?
    public var resamplePrompt: ResamplePrompt?

    // MARK: Settings

    public var batteryType: BatteryType = .alkaline {
        didSet { push(.batteryType, batteryType.rawValue, oldValue, batteryType) { $0.batteryType = $1 } }
    }
    /// On powers the device off after 10 hours.
    public var autoPowerOff = true {
        didSet { push(.autoPowerOff, autoPowerOff ? 1 : 0, oldValue, autoPowerOff) { $0.autoPowerOff = $1 } }
    }
    public var mixerControlViaMIDI = false {
        didSet { push(.mixerControlViaMIDI, mixerControlViaMIDI ? 1 : 0, oldValue, mixerControlViaMIDI) { $0.mixerControlViaMIDI = $1 } }
    }
    public var recorderMode: RecorderMode = .multiTrack {
        didSet { push(.recorderMode, recorderMode.rawValue, oldValue, recorderMode) { $0.recorderMode = $1 } }
    }
    /// Changing this restarts the device's USB connection.
    public var audioInterfaceMode: AudioInterfaceMode = .multiTrack {
        didSet { push(.usbAudioInterfaceMode, audioInterfaceMode.rawValue, oldValue, audioInterfaceMode) { $0.audioInterfaceMode = $1 } }
    }
    public var usbMixMinus = false {
        didSet { push(.usbMixMinus, usbMixMinus ? 1 : 0, oldValue, usbMixMinus) { $0.usbMixMinus = $1 } }
    }
    public var midiClockSource: MIDIClockSource = .automatic {
        didSet { push(.midiClockSource, midiClockSource.rawValue, oldValue, midiClockSource) { $0.midiClockSource = $1 } }
    }
    public var midiOutMode: MIDIOutMode = .out {
        didSet { push(.midiOutMode, midiOutMode.rawValue, oldValue, midiOutMode) { $0.midiOutMode = $1 } }
    }
    /// Zero based; channel 1 is 0.
    public var midiChannel = 0 {
        didSet { push(.midiChannel, UInt8(clamping: midiChannel), oldValue, midiChannel) { $0.midiChannel = $1 } }
    }
    public var monitorPoint: OutputPoint = .preMasterFaderWithCompressor {
        didSet { push(.monitorPoint, monitorPoint.rawValue, oldValue, monitorPoint) { $0.monitorPoint = $1 } }
    }
    public var subOutPoint: OutputPoint = .preMasterFaderWithCompressor {
        didSet { push(.subOutPoint, subOutPoint.rawValue, oldValue, subOutPoint) { $0.subOutPoint = $1 } }
    }

    /// One control change number per mixer parameter; 0 means not mapped.
    public private(set) var controlChangeMap = Array(repeating: UInt8(0), count: L6maxLayout.controlChangeSlotCount)
    private var auxPost = Array(repeating: Array(repeating: true, count: L6maxLayout.channelCount), count: L6maxLayout.auxCount)

    public let pads: [SoundPad]
    public let effects: [Effect]

    /// Whether the clock is set from this computer each time the device connects.
    public var syncsClockOnConnect = true

    // MARK: Private

    @ObservationIgnored private let connector: any DeviceConnector
    @ObservationIgnored private var link: DeviceLink?
    @ObservationIgnored private var isApplyingRemote = false
    @ObservationIgnored private var restart: AsyncStream<Void>.Continuation?
    @ObservationIgnored private var inFlight: Set<[UInt8]> = []
    @ObservationIgnored private var queued: [[UInt8]: (parameter: Parameter, revert: @MainActor (L6Device) -> Void)] = [:]

    /// The version this app reports; the device refuses versions it considers too old.
    static let reportedAppVersion = [2, 0, 0, 39]

    public init(connector: any DeviceConnector) {
        self.connector = connector
        pads = (0..<L6maxLayout.padCount).map { SoundPad(id: $0) }
        effects = L6maxLayout.effects.enumerated().map { Effect(id: $0.offset, spec: $0.element) }
        for pad in pads { pad.device = self }
        for effect in effects { effect.device = self }
    }

    /// Whether AUX `aux` takes channel `channel` after its fader.
    public subscript(auxPost aux: Int, channel channel: Int) -> Bool {
        get { auxPost[aux][channel] }
        set {
            let old = auxPost[aux][channel]
            guard old != newValue else { return }
            auxPost[aux][channel] = newValue
            send(.auxSendPoint(channel: channel, aux: aux, post: newValue)) { $0.auxPost[aux][channel] = old }
        }
    }

    // MARK: Session

    /// Finds the device, keeps the session alive, and reconnects until cancelled.
    public func run() async {
        while !Task.isCancelled {
            guard let transport = await connector.makeTransport() else {
                connection = .searching
                await waitForPortChange()
                continue
            }
            if connection == .searching { connection = .connecting }
            let link = DeviceLink(transport: transport)
            var failure: ConnectionFailure?
            do {
                try await runSession(link)
            } catch LinkError.notAnL6 {
                failure = .notAnL6
            } catch LinkError.unsupportedModel(let model) {
                failure = .unsupportedModel(model)
            } catch LinkError.device(let code) where code == .appVersionTooOld || code == .appVersionOutOfRange {
                failure = .appVersionRejected
            } catch {}
            self.link = nil
            restart = nil
            inFlight = []
            queued = [:]
            await link.stop()
            for pad in pads { pad.isPlaying = false }
            if Task.isCancelled { break }
            if let failure {
                connection = .failed(failure)
                await waitForPortChange()
            } else {
                connection = .searching
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
    }

    private func runSession(_ link: DeviceLink) async throws {
        try await link.start()
        let identity = try await link.identify()
        guard identity.model == .l6max else { throw LinkError.unsupportedModel(identity.model) }
        let settings = try await link.globalSettings()
        try await link.sendAppVersion(Self.reportedAppVersion)
        let card = try await link.sdCard()
        self.link = link
        self.identity = identity
        apply(settings)
        sdCard = card
        try await refreshPads(link)
        if syncsClockOnConnect {
            try await link.set(.dateTime(.now))
            lastClockSync = .now
        }
        connection = .connected

        let (restarts, restart) = AsyncStream<Void>.makeStream()
        self.restart = restart
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask {
                // The device drops the session after five seconds without one.
                while true {
                    try await Task.sleep(for: .seconds(1))
                    try await link.set(.keepAlive, timeout: .seconds(2))
                }
            }
            group.addTask {
                for await message in link.events { await self.handle(message) }
                throw LinkError.disconnected
            }
            group.addTask {
                for await _ in restarts { break }
                throw LinkError.disconnected
            }
            defer { group.cancelAll() }
            try await group.next()
        }
    }

    private func waitForPortChange() async {
        let changes = connector.changes()
        await withTaskGroup(of: Void.self) { group in
            group.addTask { for await _ in changes { break } }
            group.addTask { try? await Task.sleep(for: .seconds(2)) }
            await group.next()
            group.cancelAll()
        }
    }

    private func refreshPads(_ link: DeviceLink) async throws {
        for pad in pads { try await refresh(pad, link) }
    }

    private func refresh(_ pad: SoundPad, _ link: DeviceLink) async throws {
        let count = try await link.fileCount(pad: pad.id)
        var files: [PadFile] = []
        for index in 0..<count {
            files.append(try await link.fileName(pad: pad.id, index: index))
        }
        let assigned = try await link.assignedFile(pad: pad.id)
        pad.files = files
        pad.assignedFile = assigned
    }

    // MARK: Commands

    /// Sets the device clock from this computer.
    public func syncClock() async {
        await perform { link in
            try await link.set(.dateTime(.now))
            self.lastClockSync = .now
        }
    }

    public func resetAllSettings() async {
        await perform { link in
            do {
                try await link.set(.factoryReset)
            } catch LinkError.device(.resetNeedsConfirmation) {
                try await link.set(.dialog(DeviceErrorCode.resetNeedsConfirmation.rawValue, .yes), timeout: .seconds(10))
            }
            self.apply(try await link.globalSettings())
            try await self.refreshPads(link)
        }
    }

    /// In file transfer mode the device is a card reader and ends the editor session.
    public func setFileTransferMode(_ isOn: Bool) async {
        await perform { link in
            try await link.set(.setting(.sdCardReaderMode, isOn ? 1 : 0), timeout: .seconds(5))
            self.isFileTransferMode = isOn
            if isOn {
                // The device forgets the session; start over once it has switched.
                try? await Task.sleep(for: .seconds(1))
                self.restart?.yield()
            } else {
                self.sdCard = try await link.sdCard()
                try await self.refreshPads(link)
            }
        }
    }

    public func applyControlChangeMap(_ map: [UInt8]) async {
        guard map.count == L6maxLayout.controlChangeSlotCount, map != controlChangeMap else { return }
        await perform { link in
            try await link.set(.controlChangeMap(map))
            self.controlChangeMap = map.map { L6maxLayout.isAssignableControlChange($0) ? $0 : 0 }
        }
    }

    /// Restores the factory control change map on the device.
    public func resetControlChangeMap() async {
        await perform { link in
            self.controlChangeMap = try await link.resetControlChangeMap()
        }
    }

    /// Answers the pending resample question.
    public func answerResample(_ prompt: ResamplePrompt, _ answer: DialogAnswer) async {
        await perform { link in
            // Resampling can take a while; the device confirms when it is done.
            try await link.set(
                .dialog(DeviceErrorCode.fileNeedsResampling.rawValue, answer),
                timeout: .seconds(120),
                alsoAccepting: [0x05]
            )
            try await self.refresh(self.pads[prompt.pad], link)
        }
    }

    // MARK: Sending

    private func perform(_ body: (DeviceLink) async throws -> Void) async {
        guard let link, connection == .connected else { return }
        isBusy = true
        defer { isBusy = false }
        do {
            try await body(link)
        } catch {
            report(error)
        }
    }

    private func push<Value: Equatable & Sendable>(
        _ setting: SimpleSetting, _ raw: UInt8, _ old: Value, _ new: Value,
        restore: @escaping @MainActor (L6Device, Value) -> Void
    ) {
        guard old != new else { return }
        send(.setting(setting, raw)) { restore($0, old) }
    }

    /// Sends a change the user made and undoes it locally if the device refuses.
    ///
    /// While a change to the same parameter is still on its way, only the
    /// newest value is kept, so dragging a slider does not queue every step.
    func send(_ parameter: Parameter, revert: @escaping @MainActor (L6Device) -> Void) {
        guard !isApplyingRemote else { return }
        guard let link, connection == .connected else {
            remote { revert(self) }
            return
        }
        let key = parameter.coalescingKey
        if inFlight.contains(key) {
            queued[key] = (parameter, revert)
            return
        }
        inFlight.insert(key)
        Task {
            var current = (parameter: parameter, revert: revert)
            while true {
                do {
                    try await link.set(current.parameter)
                } catch {
                    self.queued[key] = nil
                    self.remote { current.revert(self) }
                    self.report(error)
                    break
                }
                guard let next = self.queued.removeValue(forKey: key) else { break }
                current = next
            }
            self.inFlight.remove(key)
        }
    }

    /// Runs a model update that must not be echoed back to the device.
    func remote(_ update: () -> Void) {
        let wasApplying = isApplyingRemote
        isApplyingRemote = true
        update()
        isApplyingRemote = wasApplying
    }

    func withLink(_ body: @escaping @MainActor (DeviceLink) async throws -> Void) {
        guard let link, connection == .connected else { return }
        Task {
            do {
                try await body(link)
            } catch {
                self.report(error)
            }
        }
    }

    func report(_ error: any Error) {
        switch error {
        case LinkError.device(let code): issue = DeviceIssue(kind: .device(code))
        case LinkError.timeout: issue = DeviceIssue(kind: .noReply)
        default: break
        }
    }

    // MARK: Receiving

    private func apply(_ settings: GlobalSettings) {
        remote {
            batteryType = BatteryType(rawValue: settings.batteryType) ?? .alkaline
            autoPowerOff = settings.autoPowerOff != 0
            mixerControlViaMIDI = settings.mixerControlViaMIDI != 0
            recorderMode = RecorderMode(rawValue: settings.recorderMode) ?? .multiTrack
            audioInterfaceMode = AudioInterfaceMode(rawValue: settings.usbAudioInterfaceMode) ?? .multiTrack
            usbMixMinus = settings.usbMixMinus != 0
            midiClockSource = MIDIClockSource(rawValue: settings.midiClockSource) ?? .automatic
            midiOutMode = MIDIOutMode(rawValue: settings.midiOutMode) ?? .out
            midiChannel = Int(settings.midiChannel)
            monitorPoint = OutputPoint(rawValue: settings.monitorPoint) ?? .preMasterFaderWithCompressor
            subOutPoint = OutputPoint(rawValue: settings.subOutPoint) ?? .preMasterFaderWithCompressor
            isFileTransferMode = settings.sdCardReaderMode != 0
            controlChangeMap = settings.controlChangeMap
            auxPost = settings.auxPost
            for pad in pads {
                pad.clockSync = settings.padClockSync[pad.id]
                pad.playMode = PadPlayMode(rawValue: settings.padPlayMode[pad.id]) ?? .oneShot
                pad.level = Int(settings.padLevel[pad.id])
                pad.isPlaying = settings.padIsPlaying[pad.id]
                pad.midiNote = settings.padMIDINote[pad.id]
            }
            for effect in effects {
                effect.values = Array(settings.effectValues[(effect.id * 2)..<(effect.id * 2 + 2)])
            }
        }
    }

    private func handle(_ message: DeviceMessage) {
        switch message {
        case .globalSettings(let settings):
            apply(settings)
        case .sdCard(let info):
            sdCard = info
        case .parameter(let parameter):
            remote { apply(parameter) }
        case .patchData(let data):
            remote { apply(data) }
        case .identity, .foreignIdentity, .acknowledgment, .unknown:
            break
        }
    }

    private func pad(_ index: Int) -> SoundPad? {
        pads.indices.contains(index) ? pads[index] : nil
    }

    private func apply(_ parameter: Parameter) {
        switch parameter {
        case .setting(let setting, let value):
            switch setting {
            case .batteryType: batteryType = BatteryType(rawValue: value) ?? batteryType
            case .autoPowerOff: autoPowerOff = value != 0
            case .mixerControlViaMIDI: mixerControlViaMIDI = value != 0
            case .recorderMode: recorderMode = RecorderMode(rawValue: value) ?? recorderMode
            case .sdCardReaderMode: isFileTransferMode = value != 0
            case .midiOutMode: midiOutMode = MIDIOutMode(rawValue: value) ?? midiOutMode
            case .midiChannel: midiChannel = Int(value)
            case .usbMixMinus: usbMixMinus = value != 0
            case .midiClockSource: midiClockSource = MIDIClockSource(rawValue: value) ?? midiClockSource
            case .usbAudioInterfaceMode: audioInterfaceMode = AudioInterfaceMode(rawValue: value) ?? audioInterfaceMode
            case .monitorPoint: monitorPoint = OutputPoint(rawValue: value) ?? monitorPoint
            case .subOutPoint: subOutPoint = OutputPoint(rawValue: value) ?? subOutPoint
            }
        case .padFile(let index, let file): pad(index)?.assignedFile = file
        case .padPlayMode(let index, let mode): pad(index)?.playMode = mode
        case .padLevel(let index, let level): pad(index)?.level = level
        case .padPlay(let index, let isOn): pad(index)?.isPlaying = isOn
        case .padMIDINote(let index, let note): pad(index)?.midiNote = note
        case .padClockSync(let index, let isOn): pad(index)?.clockSync = isOn
        case .controlChangeMap(let map) where map.count == L6maxLayout.controlChangeSlotCount: controlChangeMap = map
        case .effect(let type, let index, let value): setEffect(type, index, value)
        case .auxSendPoint(let channel, let aux, let post): setAuxPost(aux, channel, post)
        case .error(let code): issue = DeviceIssue(kind: .device(code))
        case .dateTime, .factoryReset, .keepAlive, .controlChangeMap, .dialog: break
        }
    }

    private func apply(_ data: PatchData) {
        switch data {
        case .padState(let index, let isPlaying): pad(index)?.isPlaying = isPlaying
        case .assignedFile(let index, let file): pad(index)?.assignedFile = file
        case .effect(let type, let index, let value): setEffect(type, index, value)
        case .auxSendPoint(let channel, let aux, let post): setAuxPost(aux, channel, post)
        case .defaultControlChangeMap(let map) where map.count == L6maxLayout.controlChangeSlotCount: controlChangeMap = map
        case .sdInserted:
            // The card changed; ask again for its size and each pad's files.
            withLink { link in
                self.sdCard = try await link.sdCard()
                try await self.refreshPads(link)
            }
        case .fileCount, .fileName, .defaultControlChangeMap: break
        }
    }

    private func setEffect(_ type: Int, _ index: Int, _ value: Int) {
        guard effects.indices.contains(type), (0..<2).contains(index) else { return }
        effects[type].values[index] = value
    }

    private func setAuxPost(_ aux: Int, _ channel: Int, _ post: Bool) {
        guard auxPost.indices.contains(aux), auxPost[aux].indices.contains(channel) else { return }
        auxPost[aux][channel] = post
    }
}

extension DeviceDateTime {
    /// The current wall clock time in the user's calendar and time zone.
    public static var now: DeviceDateTime {
        let parts = Calendar(identifier: .gregorian).dateComponents([.year, .month, .day, .hour, .minute, .second], from: Date())
        return DeviceDateTime(
            year: parts.year ?? 2000, month: parts.month ?? 1, day: parts.day ?? 1,
            hour: parts.hour ?? 0, minute: parts.minute ?? 0, second: parts.second ?? 0
        )
    }
}

/// One of the four sound pads.
@MainActor
@Observable
public final class SoundPad: Identifiable {
    /// Zero based.
    public nonisolated let id: Int
    /// Audio files found in this pad's folder on the SD card.
    public internal(set) var files: [PadFile] = []
    public internal(set) var assignedFile: PadFile?
    public internal(set) var isPlaying = false

    public var playMode: PadPlayMode = .oneShot {
        didSet {
            let old = oldValue
            guard old != playMode else { return }
            device?.send(.padPlayMode(pad: id, playMode)) { $0.pads[self.id].playMode = old }
        }
    }
    /// 0 is silent, 49 is 0 dB, 59 is +10 dB.
    public var level = L6maxLayout.padLevelUnity {
        didSet {
            let old = oldValue
            guard old != level else { return }
            device?.send(.padLevel(pad: id, level)) { $0.pads[self.id].level = old }
        }
    }
    /// `nil` when no note triggers the pad.
    public var midiNote: UInt8? {
        didSet {
            let old = oldValue
            guard old != midiNote, let device else { return }
            // A note belongs to one pad; the device clears it from the others.
            let conflicts = midiNote.map { note in device.pads.filter { $0.id != id && $0.midiNote == note } } ?? []
            device.send(.padMIDINote(pad: id, note: midiNote)) { device in
                device.pads[self.id].midiNote = old
                for pad in conflicts { pad.midiNote = self.midiNote }
            }
            device.remote { for pad in conflicts { pad.midiNote = nil } }
        }
    }
    public var clockSync = false {
        didSet {
            let old = oldValue
            guard old != clockSync else { return }
            device?.send(.padClockSync(pad: id, clockSync)) { $0.pads[self.id].clockSync = old }
        }
    }

    @ObservationIgnored weak var device: L6Device?

    init(id: Int) {
        self.id = id
    }

    /// Assigns a file from `files`, or clears the pad with `nil`.
    public func assign(_ file: PadFile?) {
        guard file != assignedFile, let device else { return }
        device.withLink { link in
            do {
                try await link.set(.padFile(pad: self.id, file: file), timeout: .seconds(5))
                self.assignedFile = file
            } catch LinkError.device(.fileNeedsResampling) {
                if let file { device.resamplePrompt = ResamplePrompt(pad: self.id, file: file) }
            }
        }
    }

    /// The pad was pressed. In hold mode, call `release()` when it is let go.
    public func press() {
        device?.withLink { try await $0.post(.padPlay(pad: self.id, true)) }
    }

    public func release() {
        guard playMode == .hold else { return }
        device?.withLink { try await $0.post(.padPlay(pad: self.id, false)) }
    }
}

/// One of the send effect types and its two parameters.
@MainActor
@Observable
public final class Effect: Identifiable {
    public nonisolated let id: Int
    public nonisolated let spec: L6maxLayout.EffectSpec
    public internal(set) var values: [Int]

    @ObservationIgnored weak var device: L6Device?

    init(id: Int, spec: L6maxLayout.EffectSpec) {
        self.id = id
        self.spec = spec
        values = spec.parameters.map(\.range.lowerBound)
    }

    public func setValue(_ value: Int, at index: Int) {
        guard values.indices.contains(index) else { return }
        let old = values[index]
        let new = min(max(value, spec.parameters[index].range.lowerBound), spec.parameters[index].range.upperBound)
        guard old != new else { return }
        values[index] = new
        device?.send(.effect(type: id, index: index, value: new)) { $0.effects[self.id].values[index] = old }
    }
}
