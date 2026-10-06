import Testing
@testable import L6Kit

@Suite struct LinkTests {
    private func connectedLink(_ device: SimulatedDevice = SimulatedDevice()) async throws -> DeviceLink {
        let link = DeviceLink(transport: device)
        try await link.start()
        #expect(try await link.identify() == Identity(model: .l6max, rawVersion: "0110"))
        _ = try await link.globalSettings()
        return link
    }

    @Test func ignoresCommandsBeforeIdentity() async throws {
        let link = DeviceLink(transport: SimulatedDevice())
        try await link.start()
        await #expect(throws: LinkError.timeout) {
            try await link.set(.keepAlive, timeout: .milliseconds(50))
        }
    }

    @Test func readsFilesAndCard() async throws {
        let link = try await connectedLink()
        #expect(try await link.fileCount(pad: 0) == 3)
        #expect(try await link.fileName(pad: 0, index: 1) == PadFile(index: 1, name: "Air Horn.wav"))
        #expect(try await link.assignedFile(pad: 1) == PadFile(index: 1, name: "Rimshot.wav"))
        #expect(try await link.assignedFile(pad: 3) == nil)
        #expect(try await link.sdCard().capacityBytes == 32_000_000_000)
        #expect(try await link.sdCard().remainingSeconds == 5 * 3600 + 42 * 60 + 7)
    }

    @Test func reportsDeviceErrors() async throws {
        let link = try await connectedLink()
        await #expect(throws: LinkError.device(.resetNeedsConfirmation)) {
            try await link.set(.factoryReset)
        }
        await #expect(throws: LinkError.device(.appVersionTooOld)) {
            try await link.sendAppVersion([0, 0, 0, 0])
        }
        try await link.sendAppVersion([2, 0, 0, 39])
    }

    @Test func serializesConcurrentRequests() async throws {
        let link = try await connectedLink()
        try await withThrowingTaskGroup(of: Void.self) { group in
            for channel in 0..<16 {
                group.addTask { try await link.set(.setting(.midiChannel, UInt8(channel))) }
                group.addTask { _ = try await link.fileCount(pad: channel % 4) }
            }
            try await group.waitForAll()
        }
    }

    @Test func endsEventsWhenThePortCloses() async throws {
        let device = SimulatedDevice()
        let link = try await connectedLink(device)
        await device.close()
        for await _ in link.events {}
        await #expect(throws: LinkError.disconnected) { try await link.set(.keepAlive) }
    }
}

@MainActor
@Suite struct DeviceModelTests {
    private func connected(_ simulated: SimulatedDevice = SimulatedDevice()) async throws -> (L6Device, Task<Void, Never>) {
        let device = L6Device(connector: simulated)
        device.syncsClockOnConnect = false
        let session = Task { await device.run() }
        try await wait { device.connection == .connected }
        return (device, session)
    }

    private func wait(_ condition: @MainActor () -> Bool) async throws {
        for _ in 0..<400 where !condition() {
            try await Task.sleep(for: .milliseconds(5))
        }
        #expect(condition())
    }

    @Test func loadsDeviceState() async throws {
        let (device, session) = try await connected()
        defer { session.cancel() }
        #expect(device.identity?.version == "1.10")
        #expect(device.autoPowerOff)
        #expect(device.controlChangeMap == SimulatedDevice.defaultControlChangeMap)
        #expect(device.pads[0].files.map(\.name) == ["Kick 808.wav", "Air Horn.wav", "Applause.wav"])
        #expect(device.pads[0].assignedFile?.name == "Kick 808.wav")
        #expect(device.pads[3].assignedFile == nil)
        #expect(device.effects[3].values == [505, 35])
        #expect(device[auxPost: 1, channel: 7])
        #expect(device.sdCard.state == .ready)
    }

    @Test func sendsChangesAndSurvivesReconnect() async throws {
        let simulated = SimulatedDevice()
        let (device, session) = try await connected(simulated)
        device.midiChannel = 9
        device.batteryType = .lithium
        device[auxPost: 0, channel: 2] = false
        device.pads[1].level = 12
        device.effects[4].setValue(1234, at: 0)
        try await Task.sleep(for: .milliseconds(100))
        session.cancel()
        await session.value

        let (again, second) = try await connected(simulated)
        defer { second.cancel() }
        #expect(again.midiChannel == 9)
        #expect(again.batteryType == .lithium)
        #expect(!again[auxPost: 0, channel: 2])
        #expect(again.pads[1].level == 12)
        #expect(again.effects[4].values[0] == 1234)
    }

    @Test func movesANoteBetweenPads() async throws {
        let (device, session) = try await connected()
        defer { session.cancel() }
        device.pads[2].midiNote = 0x3C
        #expect(device.pads[0].midiNote == nil)
        #expect(device.pads[2].midiNote == 0x3C)
    }

    @Test func assignsFilesAndAsksAboutResampling() async throws {
        let (device, session) = try await connected()
        defer { session.cancel() }
        device.pads[2].assign(device.pads[2].files[0])
        try await wait { device.pads[2].assignedFile?.name == "Jingle Intro.wav" }

        device.pads[0].assign(device.pads[0].files[2])
        try await wait { device.resamplePrompt != nil }
        #expect(device.pads[0].assignedFile?.index == 0)
        let prompt = try #require(device.resamplePrompt)
        await device.answerResample(prompt, .yes)
        #expect(device.pads[0].assignedFile?.name == "Applause.wav")
    }

    @Test func resetsAfterConfirmation() async throws {
        let (device, session) = try await connected()
        defer { session.cancel() }
        device.midiChannel = 3
        try await Task.sleep(for: .milliseconds(50))
        await device.resetAllSettings()
        #expect(device.midiChannel == 0)
        #expect(device.issue == nil)
    }

    @Test func playsPads() async throws {
        let (device, session) = try await connected()
        defer { session.cancel() }
        device.pads[0].press()
        try await wait { device.pads[0].isPlaying }
        device.pads[0].press()
        try await wait { !device.pads[0].isPlaying }
    }

    @Test func followsChangesMadeOnTheDevice() async throws {
        let simulated = SimulatedDevice()
        let (device, session) = try await connected(simulated)
        defer { session.cancel() }
        simulated.emit(HostMessage.parameter(.setting(.recorderMode, 1)))
        try await wait { device.recorderMode == .master }
    }
}
