/// A byte pipe to the device's editor port that carries whole SysEx messages.
public protocol L6Transport: Sendable {
    /// Opens the port. The stream yields one complete message per element,
    /// `F0` through `F7`, and finishes when the port goes away.
    func open() async throws -> AsyncStream<[UInt8]>
    func send(_ message: [UInt8]) async throws
    func close() async
}

/// Finds the editor port and reports when the set of ports may have changed.
public protocol DeviceConnector: Sendable {
    func makeTransport() async -> (any L6Transport)?
    func changes() -> AsyncStream<Void>
}

public enum LinkError: Error, Equatable, Sendable {
    case portUnavailable
    case timeout
    case disconnected
    /// The device refused a request and reported this code.
    case device(DeviceErrorCode)
    case notAnL6
    case unsupportedModel(DeviceModel)
}

/// Collects MIDI byte fragments into whole SysEx messages.
struct SysExAssembler {
    private var pending: [UInt8] = []
    private var isCollecting = false

    /// Feeds bytes that already include `F0` and `F7`.
    mutating func push(_ bytes: some Sequence<UInt8>) -> [[UInt8]] {
        var complete: [[UInt8]] = []
        for byte in bytes {
            if byte == Frame.start {
                pending = [byte]
                isCollecting = true
            } else if isCollecting {
                if byte == Frame.end {
                    pending.append(byte)
                    complete.append(pending)
                    pending = []
                    isCollecting = false
                } else if byte < 0x80 {
                    pending.append(byte)
                } else if byte < 0xF8 {
                    // Another status byte aborts the message; real-time bytes pass through.
                    pending = []
                    isCollecting = false
                }
            }
        }
        return complete
    }
}
