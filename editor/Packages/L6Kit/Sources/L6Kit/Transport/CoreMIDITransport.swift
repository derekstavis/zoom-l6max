#if canImport(CoreMIDI)
import CoreMIDI
import Foundation
import Synchronization

/// Universal MIDI Packet conversion for 7-bit SysEx (message type 3).
enum UniversalPacket {
    /// Words for one SysEx message given with its `F0` and `F7`.
    static func words(forSysEx message: [UInt8], group: UInt32 = 0) -> [UInt32] {
        let payload = Array(message.dropFirst().dropLast())
        let chunkCount = max(1, (payload.count + 5) / 6)
        var words: [UInt32] = []
        words.reserveCapacity(chunkCount * 2)
        for chunk in 0..<chunkCount {
            let bytes = Array(payload[(chunk * 6)..<min(chunk * 6 + 6, payload.count)])
            let status: UInt32 = chunkCount == 1 ? 0 : chunk == 0 ? 1 : chunk == chunkCount - 1 ? 3 : 2
            func byte(_ index: Int) -> UInt32 { index < bytes.count ? UInt32(bytes[index]) : 0 }
            words.append(0x3 << 28 | group << 24 | status << 20 | UInt32(bytes.count) << 16 | byte(0) << 8 | byte(1))
            words.append(byte(2) << 24 | byte(3) << 16 | byte(4) << 8 | byte(5))
        }
        return words
    }

    /// MIDI 1.0 bytes of every SysEx packet in `words`, with `F0`/`F7` restored.
    static func sysExBytes(from words: [UInt32]) -> [UInt8] {
        var out: [UInt8] = []
        var index = 0
        while index < words.count {
            let word = words[index]
            let type = word >> 28
            let length = switch type {
            case 0x0, 0x1, 0x2, 0x6, 0x7: 1
            case 0x3, 0x4, 0x8, 0x9, 0xA: 2
            case 0xB, 0xC: 3
            default: 4
            }
            if type == 0x3, index + 1 < words.count {
                let status = (word >> 20) & 0xF
                let count = Int((word >> 16) & 0xF)
                let next = words[index + 1]
                let bytes = [word >> 8, word, next >> 24, next >> 16, next >> 8, next].map { UInt8($0 & 0x7F) }
                if status == 0 || status == 1 { out.append(Frame.start) }
                out.append(contentsOf: bytes.prefix(min(count, 6)))
                if status == 0 || status == 3 { out.append(Frame.end) }
            }
            index += length
        }
        return out
    }
}

/// Finds the L6 editor port among CoreMIDI endpoints.
public final class CoreMIDIConnector: DeviceConnector {
    private final class Notifier: Sendable {
        let continuations = Mutex<[UUID: AsyncStream<Void>.Continuation]>([:])

        func notify() {
            let current = continuations.withLock { Array($0.values) }
            for continuation in current { continuation.yield() }
        }
    }

    private let client: MIDIClientRef
    private let notifier = Notifier()

    public init(clientName: String = "Zoomie") throws {
        var client = MIDIClientRef()
        let notifier = notifier
        let status = MIDIClientCreateWithBlock(clientName as CFString, &client) { _ in
            notifier.notify()
        }
        guard status == noErr else { throw LinkError.portUnavailable }
        self.client = client
    }

    deinit {
        MIDIClientDispose(client)
    }

    public func changes() -> AsyncStream<Void> {
        let id = UUID()
        let notifier = notifier
        let (stream, continuation) = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingNewest(1))
        notifier.continuations.withLock { $0[id] = continuation }
        continuation.onTermination = { _ in
            notifier.continuations.withLock { $0[id] = nil }
        }
        return stream
    }

    public func makeTransport() async -> (any L6Transport)? {
        guard let source = Self.endpoint(input: true), let destination = Self.endpoint(input: false) else { return nil }
        return CoreMIDITransport(client: client, source: source, destination: destination, changes: changes())
    }

    /// The first endpoint whose names mention `L6` and the editor port.
    static func endpoint(input: Bool) -> MIDIEndpointRef? {
        let count = input ? MIDIGetNumberOfSources() : MIDIGetNumberOfDestinations()
        for index in 0..<count {
            let endpoint = input ? MIDIGetSource(index) : MIDIGetDestination(index)
            if isEditorPort(names: names(of: endpoint), input: input) { return endpoint }
        }
        return nil
    }

    static func isEditorPort(names: [String], input: Bool) -> Bool {
        let joined = names.joined(separator: " ")
        return joined.contains("L6") && (joined.contains("Editor") || joined.contains(input ? "MIDIIN3" : "MIDIOUT3"))
    }

    static func contains(_ endpoint: MIDIEndpointRef, input: Bool) -> Bool {
        let count = input ? MIDIGetNumberOfSources() : MIDIGetNumberOfDestinations()
        return (0..<count).contains { (input ? MIDIGetSource($0) : MIDIGetDestination($0)) == endpoint }
    }

    private static func names(of endpoint: MIDIEndpointRef) -> [String] {
        var objects: [MIDIObjectRef] = [endpoint]
        var entity = MIDIEntityRef()
        if MIDIEndpointGetEntity(endpoint, &entity) == noErr {
            objects.append(entity)
            var device = MIDIDeviceRef()
            if MIDIEntityGetDevice(entity, &device) == noErr { objects.append(device) }
        }
        return objects.flatMap { object in
            [kMIDIPropertyName, kMIDIPropertyDisplayName, kMIDIPropertyModel].compactMap { property in
                var value: Unmanaged<CFString>?
                guard MIDIObjectGetStringProperty(object, property, &value) == noErr else { return nil }
                return value?.takeRetainedValue() as String?
            }
        }
    }
}

final class CoreMIDITransport: L6Transport {
    private struct State {
        var input = MIDIPortRef()
        var output = MIDIPortRef()
        var assembler = SysExAssembler()
        var continuation: AsyncStream<[UInt8]>.Continuation?
        var monitor: Task<Void, Never>?
    }

    private final class Shared: Sendable {
        let state = Mutex(State())
    }

    private let client: MIDIClientRef
    private let source: MIDIEndpointRef
    private let destination: MIDIEndpointRef
    private let changes: AsyncStream<Void>
    private let shared = Shared()

    init(client: MIDIClientRef, source: MIDIEndpointRef, destination: MIDIEndpointRef, changes: AsyncStream<Void>) {
        self.client = client
        self.source = source
        self.destination = destination
        self.changes = changes
    }

    func open() async throws -> AsyncStream<[UInt8]> {
        let (stream, continuation) = AsyncStream<[UInt8]>.makeStream()
        let shared = shared
        var input = MIDIPortRef()
        var output = MIDIPortRef()
        let created = MIDIInputPortCreateWithProtocol(client, "Zoomie Input" as CFString, ._1_0, &input) { list, _ in
            var words: [UInt32] = []
            for packet in list.unsafeSequence() {
                let count = Int(packet.pointee.wordCount)
                withUnsafeBytes(of: packet.pointee.words) { raw in
                    words.append(contentsOf: raw.bindMemory(to: UInt32.self).prefix(count))
                }
            }
            let bytes = UniversalPacket.sysExBytes(from: words)
            let messages = shared.state.withLock { $0.assembler.push(bytes) }
            for message in messages { continuation.yield(message) }
        }
        guard created == noErr,
              MIDIOutputPortCreate(client, "Zoomie Output" as CFString, &output) == noErr,
              MIDIPortConnectSource(input, source, nil) == noErr
        else {
            if input != 0 { MIDIPortDispose(input) }
            if output != 0 { MIDIPortDispose(output) }
            throw LinkError.portUnavailable
        }
        // The stream ends when the device's endpoints disappear.
        let source = source
        let changes = changes
        let monitor = Task {
            for await _ in changes where !CoreMIDIConnector.contains(source, input: true) {
                continuation.finish()
                break
            }
        }
        shared.state.withLock {
            $0.input = input
            $0.output = output
            $0.continuation = continuation
            $0.monitor = monitor
        }
        return stream
    }

    func send(_ message: [UInt8]) async throws {
        let output = shared.state.withLock { $0.output }
        guard output != 0 else { throw LinkError.disconnected }
        let words = UniversalPacket.words(forSysEx: message)
        // A packet holds at most 64 words; SysEx packets are word pairs.
        let packetStarts = Array(stride(from: 0, to: words.count, by: 64))
        let byteCount = MemoryLayout<MIDIEventList>.size + packetStarts.count * MemoryLayout<MIDIEventPacket>.size
        let buffer = UnsafeMutableRawPointer.allocate(byteCount: byteCount, alignment: MemoryLayout<MIDIEventList>.alignment)
        defer { buffer.deallocate() }
        let list = buffer.bindMemory(to: MIDIEventList.self, capacity: 1)
        var packet = MIDIEventListInit(list, ._1_0)
        for start in packetStarts {
            let slice = Array(words[start..<min(start + 64, words.count)])
            packet = slice.withUnsafeBufferPointer { MIDIEventListAdd(list, byteCount, packet, 0, slice.count, $0.baseAddress!) }
        }
        guard MIDISendEventList(output, destination, list) == noErr else { throw LinkError.disconnected }
    }

    func close() async {
        let state = shared.state.withLock { state in
            defer { state = State() }
            return state
        }
        state.monitor?.cancel()
        state.continuation?.finish()
        if state.input != 0 {
            MIDIPortDisconnectSource(state.input, source)
            MIDIPortDispose(state.input)
        }
        if state.output != 0 { MIDIPortDispose(state.output) }
    }
}
#endif
