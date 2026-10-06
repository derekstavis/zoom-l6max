import Testing
@testable import L6Kit

/// Global setting dump captured from stock firmware 1.10 with fresh settings.
let capturedDump: [UInt8] = [
    0xf0, 0x52, 0x00, 0x00, 0x2a, 0x2e, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x31, 0x31, 0x31, 0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x5d, 0x5e, 0x5f, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6d, 0x6e, 0x71, 0x72, 0x75, 0x77, 0x3c, 0x3e, 0x40, 0x41, 0x00, 0x00, 0x00, 0x00, 0x28, 0x00, 0x32, 0x00, 0x32, 0x00, 0x32, 0x00, 0x5a, 0x00, 0x1e, 0x00, 0x79, 0x03, 0x23, 0x00, 0x79, 0x03, 0x32, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf7,
]

@Suite struct SevenBitTests {
    @Test func packingRoundTrips() {
        let bytes: [UInt8] = [0x80, 0x01, 0xFF, 0x00, 0x7F, 0x81, 0x10, 0xAA, 0x55]
        let packed = SevenBit.pack(bytes)
        #expect(packed.count == 11)
        #expect(packed.allSatisfy { $0 < 0x80 })
        #expect(SevenBit.unpack(packed) == bytes)
    }

    @Test func headerCarriesHighBitsMostSignificantFirst() {
        #expect(SevenBit.pack([0x80, 0x00, 0x80]) == [0x50, 0x00, 0x00, 0x00])
    }

    @Test func namesRoundTrip() {
        let name = "Küche 01 ☕.wav"
        #expect(SevenBit.unpackName(SevenBit.packName(name)) == name)
    }

    @Test func wideIntegers() {
        #expect(SevenBit.join14(low: 0x79, high: 0x03) == 505)
        #expect(SevenBit.split14(174) == [0x2E, 0x01])
        #expect(SevenBit.join64(SevenBit.split64(32_000_000_000)[...]) == 32_000_000_000)
    }
}

@Suite struct MessageTests {
    @Test func parsesCapturedDump() throws {
        guard case .globalSettings(let settings) = DeviceMessage(capturedDump) else {
            Issue.record("dump did not parse")
            return
        }
        #expect(settings.autoPowerOff == 1)
        #expect(settings.padPlayMode == [1, 1, 1, 1])
        #expect(settings.padLevel == [0x31, 0x31, 0x31, 0x31])
        #expect(settings.controlChangeMap == SimulatedDevice.defaultControlChangeMap)
        #expect(settings.padMIDINote == [0x3C, 0x3E, 0x40, 0x41])
        #expect(settings.effectValues == [40, 50, 50, 50, 90, 30, 505, 35, 505, 50])
        #expect(settings.monitorPoint == 1)
        #expect(settings.auxPost == [[Bool]](repeating: [Bool](repeating: true, count: 8), count: 2))
        #expect(settings.message == capturedDump)
    }

    @Test func parsesIdentity() {
        let reply: [UInt8] = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x52, 0x72, 0x00, 0x0D, 0x00, 0x30, 0x31, 0x31, 0x30, 0xF7]
        #expect(DeviceMessage(reply) == .identity(Identity(model: .l6max, rawVersion: "0110")))
        #expect(Identity(model: .l6max, rawVersion: "0110").version == "1.10")
        let other: [UInt8] = [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x41, 0x00, 0x00, 0x01, 0x00, 0x30, 0x31, 0x31, 0x30, 0xF7]
        #expect(DeviceMessage(other) == .foreignIdentity)
    }

    @Test func buildsHostMessagesAsObservedOnTheWire() {
        #expect(HostMessage.globalSettingsRequest == [0xF0, 0x52, 0, 0, 0x2B, 0xF7])
        #expect(HostMessage.appVersion([2, 0, 0, 39]) == [0xF0, 0x52, 0, 0, 0x45, 0x05, 2, 0, 0, 0, 0, 0, 0x27, 0, 0xF7])
        #expect(HostMessage.sdCardRequest == [0xF0, 0x52, 0, 0, 0x67, 0x01, 0xF7])
        #expect(HostMessage.parameter(.keepAlive) == [0xF0, 0x52, 0, 0, 0x31, 0x0B, 0xF7])
        #expect(HostMessage.parameter(.setting(.midiChannel, 5)) == [0xF0, 0x52, 0, 0, 0x31, 0x0D, 0x05, 0xF7])
        let time = DeviceDateTime(year: 2026, month: 10, day: 5, hour: 12, minute: 34, second: 56)
        #expect(HostMessage.parameter(.dateTime(time)) == [0xF0, 0x52, 0, 0, 0x31, 0x00, 0x1A, 0x0A, 0x05, 0x0C, 0x22, 0x38, 0xF7])
        #expect(HostMessage.parameter(.padFile(pad: 2, file: nil)) == [0xF0, 0x52, 0, 0, 0x31, 0x05, 2, 0x7F, 0x7F, 0, 0, 0xF7])
        #expect(HostMessage.parameter(.effect(type: 3, index: 0, value: 505)) == [0xF0, 0x52, 0, 0, 0x31, 0x13, 3, 0, 0x79, 0x03, 0xF7])
    }

    @Test func parsesCapturedReplies() {
        #expect(DeviceMessage([0xF0, 0x52, 0, 0, 0x00, 0x11, 0xF7]) == .acknowledgment(0x11))
        #expect(DeviceMessage([0xF0, 0x52, 0, 0, 0x45, 0x00, 0x01, 0x00, 0x00, 0xF7]) == .patchData(.fileCount(pad: 1, count: 0)))
        #expect(DeviceMessage([0xF0, 0x52, 0, 0, 0x45, 0x02, 0x03, 0x7F, 0x7F, 0x00, 0x00, 0xF7]) == .patchData(.assignedFile(pad: 3, file: nil)))
        #expect(DeviceMessage([0xF0, 0x52, 0, 0, 0x45, 0x03, 0x02, 0x00, 0xF7]) == .patchData(.padState(pad: 2, isPlaying: false)))
        let noCard = [0xF0, 0x52, 0, 0, 0x67, 0x00] + [UInt8](repeating: 0, count: 25) + [0xF7]
        #expect(DeviceMessage(noCard) == .sdCard(SDCardInfo()))
        #expect(DeviceMessage([0xF0, 0x52, 0, 0, 0x31, 0x10, 0x03, 0xF7]) == .parameter(.error(.resetNeedsConfirmation)))
    }

    @Test func parametersRoundTrip() {
        let samples: [Parameter] = [
            .padFile(pad: 1, file: PadFile(index: 130, name: "Long file name with ünïcode.wav")),
            .padMIDINote(pad: 0, note: nil), .padMIDINote(pad: 3, note: 60), .padClockSync(pad: 2, true),
            .padPlayMode(pad: 1, .hold), .padLevel(pad: 0, 59), .dialog(6, .other),
            .auxSendPoint(channel: 7, aux: 1, post: false), .setting(.subOutPoint, 2),
            .controlChangeMap(SimulatedDevice.defaultControlChangeMap),
        ]
        for parameter in samples {
            #expect(DeviceMessage(HostMessage.parameter(parameter)) == .parameter(parameter))
        }
    }

    @Test func assemblerSplitsAndJoinsFragments() {
        var assembler = SysExAssembler()
        #expect(assembler.push([0x90, 0x40, 0xF0, 0x52, 0x00]).isEmpty)
        #expect(assembler.push([0xF8, 0x00, 0x2B]).isEmpty)
        #expect(assembler.push([0xF7, 0xF0, 0x7E, 0xF7]) == [[0xF0, 0x52, 0x00, 0x00, 0x2B, 0xF7], [0xF0, 0x7E, 0xF7]])
    }

    #if canImport(CoreMIDI)
    @Test func universalPacketsRoundTrip() {
        for message in [HostMessage.identityRequest, HostMessage.globalSettingsRequest, capturedDump, [0xF0, 0xF7]] {
            let words = UniversalPacket.words(forSysEx: message)
            #expect(words.count % 2 == 0)
            #expect(UniversalPacket.sysExBytes(from: words) == message)
        }
        #expect(UniversalPacket.words(forSysEx: HostMessage.identityRequest) == [0x3004_7E00, 0x0601_0000])
    }

    @Test func recognizesTheEditorPort() {
        #expect(CoreMIDIConnector.isEditorPort(names: ["for L6 Editor Port (Emulator)"], input: true))
        #expect(CoreMIDIConnector.isEditorPort(names: ["for L6 Editor Port", "L6max"], input: false))
        #expect(CoreMIDIConnector.isEditorPort(names: ["MIDIIN3 (L6max)"], input: true))
        #expect(!CoreMIDIConnector.isEditorPort(names: ["L6max Mixer Control Port (Emulator)"], input: true))
        #expect(!CoreMIDIConnector.isEditorPort(names: ["Some Editor"], input: true))
    }
    #endif
}
