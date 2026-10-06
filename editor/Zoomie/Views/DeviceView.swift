import L6Kit
import SwiftUI

struct DeviceView: View {
    let device: L6Device
    let isDemo: Bool
    let setDemo: (Bool) -> Void

    var body: some View {
        Form {
            AboutSection(version: device.identity?.version, isDemo: isDemo, setDemo: setDemo)
            PowerSection(device: device)
            RecorderSection(device: device)
            CardSection(card: device.sdCard)
            ClockSection(device: device)
            MaintenanceSection(device: device)
        }
        .formStyle(.grouped)
        .navigationTitle("Device")
    }
}

struct AboutSection: View {
    let version: String?
    let isDemo: Bool
    let setDemo: (Bool) -> Void

    var body: some View {
        Section {
            LabeledContent("Model") {
                Text(verbatim: "L6max")
            }
            LabeledContent("Device Version") {
                Text(verbatim: version ?? "—")
            }
            if isDemo {
                Button("Exit Demo") { setDemo(false) }
            }
        } footer: {
            if isDemo {
                Text("This is a simulated L6max. Nothing is sent to a real device.")
            }
        }
    }
}

struct PowerSection: View {
    @Bindable var device: L6Device

    var body: some View {
        Section {
            Picker("Battery Type", selection: $device.batteryType) {
                ForEach(BatteryType.allCases) { type in
                    Text(type.name).tag(type)
                }
            }
            Toggle("Auto Power Off", isOn: $device.autoPowerOff)
        } header: {
            Text("Power")
        } footer: {
            Text("Auto Power Off turns the L6max off after 10 hours without use.")
        }
    }
}

struct RecorderSection: View {
    @Bindable var device: L6Device

    var body: some View {
        Section("Recorder") {
            Picker("Recorder Mode", selection: $device.recorderMode) {
                ForEach(RecorderMode.allCases) { mode in
                    Text(mode.name).tag(mode)
                }
            }
        }
    }
}

struct CardSection: View {
    let card: SDCardInfo

    var body: some View {
        Section("SD Card") {
            switch card.state {
            case .absent:
                Text("No SD Card").foregroundStyle(.secondary)
            case .invalid:
                Text("Invalid SD Card").foregroundStyle(.secondary)
            case .ready:
                LabeledContent("Used") {
                    Text(
                        "\(Int64(clamping: card.usedBytes), format: .byteCount(style: .file)) of \(Int64(clamping: card.capacityBytes), format: .byteCount(style: .file))",
                        comment: "SD card usage. The first variable is the space used, the second is the card's capacity."
                    )
                }
                LabeledContent("Remaining Recording Time") {
                    Text(card.remainingTime, format: .time(pattern: .hourMinuteSecond))
                        .monospacedDigit()
                }
            }
        }
    }
}

struct ClockSection: View {
    let device: L6Device
    @AppStorage(AppModel.clockSyncKey) private var syncsOnConnect = true

    var body: some View {
        Section {
            Toggle("Set When Connecting", isOn: $syncsOnConnect)
            Button("Set Date and Time Now") {
                Task { await device.syncClock() }
            }
            .disabled(device.isBusy)
        } header: {
            Text("Date and Time")
        } footer: {
            if let date = device.lastClockSync {
                Text("Set from this device at \(date, format: .dateTime.hour().minute().second()).", comment: "The variable is the time of day the clock was last set.")
            }
        }
        .onChange(of: syncsOnConnect) { _, isOn in
            device.syncsClockOnConnect = isOn
        }
    }
}

struct MaintenanceSection: View {
    let device: L6Device
    @State private var isConfirmingFileTransfer = false
    @State private var isConfirmingReset = false

    var body: some View {
        Section {
            Button("Enter File Transfer Mode…") {
                isConfirmingFileTransfer = true
            }
            .confirmationDialog("Enter File Transfer Mode?", isPresented: $isConfirmingFileTransfer, titleVisibility: .visible) {
                Button("Enter File Transfer Mode") {
                    Task { await device.setFileTransferMode(true) }
                }
            } message: {
                Text("The L6max stops mixing and its SD card appears as a drive.")
            }

            Button("Reset All Settings…", role: .destructive) {
                isConfirmingReset = true
            }
            .confirmationDialog("Reset all settings?", isPresented: $isConfirmingReset, titleVisibility: .visible) {
                Button("Reset All Settings", role: .destructive) {
                    Task { await device.resetAllSettings() }
                }
            }
        }
        .disabled(device.isBusy)
    }
}
