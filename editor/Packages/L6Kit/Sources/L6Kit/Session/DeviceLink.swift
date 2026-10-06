/// Request and reply plumbing for one open editor port.
///
/// The device handles one request at a time and most replies carry no request
/// identifier, so requests are serialized and each waits for its own reply.
public actor DeviceLink {
    private struct Waiter {
        let id: Int
        let match: @Sendable (DeviceMessage) -> (any Sendable)?
        var continuation: CheckedContinuation<any Sendable, any Error>?
        var result: Result<any Sendable, any Error>?
        var failureAnnounced = false
    }

    private let transport: any L6Transport
    private let eventContinuation: AsyncStream<DeviceMessage>.Continuation
    private var reader: Task<Void, Never>?
    private var waiter: Waiter?
    private var nextWaiterID = 0
    private var isBusy = false
    private var queue: [CheckedContinuation<Void, Never>] = []
    private var isClosed = false

    /// Messages the device sent that no request was waiting for. Finishes on disconnect.
    public nonisolated let events: AsyncStream<DeviceMessage>

    public init(transport: any L6Transport) {
        self.transport = transport
        (events, eventContinuation) = AsyncStream<DeviceMessage>.makeStream()
    }

    public func start() async throws {
        let stream = try await transport.open()
        reader = Task { [weak self] in
            for await bytes in stream {
                await self?.receive(DeviceMessage(bytes))
            }
            await self?.transportClosed()
        }
    }

    public func stop() async {
        reader?.cancel()
        await transport.close()
        transportClosed()
    }

    // MARK: Requests

    public func identify() async throws -> Identity {
        try await request(HostMessage.identityRequest, retries: 4) { message -> Result<Identity, LinkError>? in
            switch message {
            case .identity(let identity): .success(identity)
            case .foreignIdentity: .failure(.notAnL6)
            default: nil
            }
        }.get()
    }

    public func globalSettings() async throws -> GlobalSettings {
        try await request(HostMessage.globalSettingsRequest, retries: 2) {
            if case .globalSettings(let settings) = $0 { settings } else { nil }
        }
    }

    public func sendAppVersion(_ components: [Int]) async throws {
        try await request(HostMessage.appVersion(components)) { $0 == .acknowledgment(0x11) ? true : nil }
    }

    public func sdCard() async throws -> SDCardInfo {
        try await request(HostMessage.sdCardRequest, retries: 1) {
            if case .sdCard(let info) = $0 { info } else { nil }
        }
    }

    public func fileCount(pad: Int) async throws -> Int {
        try await request(HostMessage.fileCountRequest(pad: pad), retries: 1) {
            if case .patchData(.fileCount(pad, let count)) = $0 { count } else { nil }
        }
    }

    public func fileName(pad: Int, index: Int) async throws -> PadFile {
        try await request(HostMessage.fileNameRequest(pad: pad, index: index), retries: 1) {
            if case .patchData(.fileName(pad, let file)) = $0 { file } else { nil }
        }
    }

    public func assignedFile(pad: Int) async throws -> PadFile? {
        try await request(HostMessage.assignedFileRequest(pad: pad), retries: 1) { message -> PadFile?? in
            if case .patchData(.assignedFile(pad, let file)) = message { .some(file) } else { nil }
        }
    }

    public func padIsPlaying(pad: Int) async throws -> Bool {
        try await request(HostMessage.padStateRequest(pad: pad), retries: 1) {
            if case .patchData(.padState(pad, let isPlaying)) = $0 { isPlaying } else { nil }
        }
    }

    /// Restores the default control change map on the device and returns it.
    public func resetControlChangeMap() async throws -> [UInt8] {
        try await request(HostMessage.defaultControlChangeMapRequest, retries: 1) {
            if case .patchData(.defaultControlChangeMap(let map)) = $0 { map } else { nil }
        }
    }

    /// Sends a parameter change and waits for its acknowledgment.
    /// - Parameter alsoAccepting: further acknowledgment codes that count as success.
    public func set(_ parameter: Parameter, timeout: Duration = .seconds(1), alsoAccepting: Set<UInt8> = []) async throws {
        let accepted = alsoAccepting.union([parameter.acknowledgment])
        try await request(HostMessage.parameter(parameter), timeout: timeout) { message -> Bool? in
            if case .acknowledgment(let code) = message, accepted.contains(code) { true } else { nil }
        }
    }

    /// Sends without waiting; used where the device does not always reply.
    public func post(_ parameter: Parameter) async throws {
        await lock()
        defer { unlock() }
        guard !isClosed else { throw LinkError.disconnected }
        try await transport.send(HostMessage.parameter(parameter))
    }

    // MARK: Plumbing

    @discardableResult
    private func request<T: Sendable>(
        _ bytes: [UInt8],
        timeout: Duration = .seconds(1),
        retries: Int = 0,
        _ match: @escaping @Sendable (DeviceMessage) -> T?
    ) async throws -> T {
        await lock()
        defer { unlock() }
        var attempt = 0
        while true {
            guard !isClosed else { throw LinkError.disconnected }
            let id = install { match($0) }
            do {
                try await transport.send(bytes)
                scheduleTimeout(id, after: timeout)
                guard let value = try await wait(id) as? T else { throw LinkError.disconnected }
                return value
            } catch LinkError.timeout where attempt < retries {
                attempt += 1
            } catch {
                if waiter?.id == id { waiter = nil }
                throw error
            }
        }
    }

    private func install(_ match: @escaping @Sendable (DeviceMessage) -> (any Sendable)?) -> Int {
        nextWaiterID += 1
        waiter = Waiter(id: nextWaiterID, match: match)
        return nextWaiterID
    }

    private func scheduleTimeout(_ id: Int, after timeout: Duration) {
        Task { [weak self] in
            try? await Task.sleep(for: timeout)
            await self?.resolve(id, .failure(LinkError.timeout))
        }
    }

    private func wait(_ id: Int) async throws -> any Sendable {
        guard waiter?.id == id else { throw LinkError.disconnected }
        if let result = waiter?.result {
            waiter = nil
            return try result.get()
        }
        return try await withCheckedThrowingContinuation { waiter?.continuation = $0 }
    }

    private func resolve(_ id: Int, _ result: Result<any Sendable, any Error>) {
        guard waiter?.id == id, waiter?.result == nil else { return }
        if let continuation = waiter?.continuation {
            waiter = nil
            continuation.resume(with: result)
        } else {
            waiter?.result = result
        }
    }

    private func receive(_ message: DeviceMessage) {
        if let current = waiter, current.result == nil {
            if current.failureAnnounced, case .parameter(.error(let code)) = message {
                resolve(current.id, .failure(LinkError.device(code)))
                return
            }
            if message == .acknowledgment(DeviceMessage.failureAcknowledgment) {
                waiter?.failureAnnounced = true
                return
            }
            if let value = current.match(message) {
                resolve(current.id, .success(value))
                return
            }
        }
        eventContinuation.yield(message)
    }

    private func transportClosed() {
        guard !isClosed else { return }
        isClosed = true
        if let id = waiter?.id { resolve(id, .failure(LinkError.disconnected)) }
        eventContinuation.finish()
    }

    private func lock() async {
        if isBusy {
            await withCheckedContinuation { queue.append($0) }
        } else {
            isBusy = true
        }
    }

    private func unlock() {
        if queue.isEmpty {
            isBusy = false
        } else {
            queue.removeFirst().resume()
        }
    }
}
