#if canImport(CoreMIDI)
import Foundation
import Testing
@testable import L6Kit

/// Runs against real firmware when the emulator is up with `--usb`, exposing
/// its CoreMIDI ports. Enable with `L6_LIVE=1 swift test --filter Live`.
@MainActor
@Suite(.enabled(if: ProcessInfo.processInfo.environment["L6_LIVE"] == "1"), .serialized)
struct LiveEmulatorTests {
    @Test func connectsAndRoundTripsASetting() async throws {
        let connector = try CoreMIDIConnector(clientName: "L6Kit live test")
        let device = L6Device(connector: connector)
        let session = Task { await device.run() }
        defer { session.cancel() }

        for _ in 0..<300 where device.connection != .connected {
            try await Task.sleep(for: .milliseconds(100))
        }
        try #require(device.connection == .connected)
        #expect(device.identity?.model == .l6max)
        #expect(device.controlChangeMap.count == L6maxLayout.controlChangeSlotCount)
        print("LIVE identity:", device.identity?.version ?? "?", "card:", device.sdCard, "cc:", device.controlChangeMap.prefix(8))

        // Stay connected past the firmware's five second keep-alive window.
        try await Task.sleep(for: .seconds(7))
        #expect(device.connection == .connected)

        let channel = (device.midiChannel + 5) % 16
        device.midiChannel = channel
        device.pads[1].level = 30
        device.effects[3].setValue(777, at: 0)
        device[auxPost: 1, channel: 4] = false
        try await Task.sleep(for: .milliseconds(800))
        #expect(device.issue == nil)

        // A second, independent read of the device confirms it stored them.
        session.cancel()
        await session.value
        let transport = try #require(await connector.makeTransport())
        let link = DeviceLink(transport: transport)
        try await link.start()
        _ = try await link.identify()
        let settings = try await link.globalSettings()
        #expect(Int(settings.midiChannel) == channel)
        #expect(settings.padLevel[1] == 30)
        #expect(settings.effectValues[6] == 777)
        #expect(settings.auxPost[1][4] == false)
        #expect(try await link.resetControlChangeMap() == SimulatedDevice.defaultControlChangeMap)
        await link.stop()
    }
}
#endif
