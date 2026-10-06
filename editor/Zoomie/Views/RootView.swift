import L6Kit
import SwiftUI

struct RootView: View {
    let model: AppModel

    var body: some View {
        DeviceScreen(device: model.device, isDemo: model.isDemo, setDemo: model.setDemo)
            // A new device object means a new session; the old one is cancelled.
            .task(id: ObjectIdentifier(model.device)) {
                await model.device.run()
            }
    }
}

/// Shows the editor once connected, and the reason it is not until then.
struct DeviceScreen: View {
    let device: L6Device
    let isDemo: Bool
    let setDemo: (Bool) -> Void

    var body: some View {
        Group {
            if device.connection != .connected {
                ConnectionStatusView(state: device.connection, isDemo: isDemo, setDemo: setDemo)
            } else if device.isFileTransferMode {
                FileTransferView(device: device)
            } else {
                EditorTabs(device: device, isDemo: isDemo, setDemo: setDemo)
            }
        }
        .deviceAlerts(device)
    }
}

struct ConnectionStatusView: View {
    let state: ConnectionState
    let isDemo: Bool
    let setDemo: (Bool) -> Void

    var body: some View {
        ContentUnavailableView {
            switch state {
            case .searching, .connected:
                Label("Connect Your L6max", systemImage: "cable.connector")
            case .connecting:
                Label("Connecting…", systemImage: "cable.connector")
            case .failed(.notAnL6):
                Label("Invalid Device", systemImage: "exclamationmark.triangle")
            case .failed(.unsupportedModel):
                Label("This Model Is Not Supported", systemImage: "exclamationmark.triangle")
            case .failed(.appVersionRejected):
                Label("App Version Too Old", systemImage: "exclamationmark.triangle")
            }
        } description: {
            switch state {
            case .searching, .connected:
                Text("Connect the L6max with a USB cable and turn it on. The app connects by itself.")
            case .connecting:
                ProgressView()
            case .failed(.notAnL6):
                Text("Check that the L6max is connected to this device with a USB cable.")
            case .failed(.unsupportedModel):
                Text("This app works with the L6max. The L6 uses a different settings layout.")
            case .failed(.appVersionRejected):
                Text("The L6max firmware expects a newer editor. Reconnect the USB cable or restart the L6max.")
            }
        } actions: {
            if isDemo {
                Button("Exit Demo") { setDemo(false) }
            } else {
                Button("Try the Demo") { setDemo(true) }
            }
        }
    }
}

/// The device is acting as a card reader; nothing else can be edited.
struct FileTransferView: View {
    let device: L6Device

    var body: some View {
        ContentUnavailableView {
            Label("File Transfer Mode", systemImage: "sdcard")
        } description: {
            Text("The SD card in the L6max is available to this device as a drive. Eject it before leaving this mode.")
        } actions: {
            Button("Exit File Transfer Mode") {
                Task { await device.setFileTransferMode(false) }
            }
            .buttonStyle(.borderedProminent)
            .disabled(device.isBusy)
        }
    }
}

/// Presents problems and questions that come from the device.
private struct DeviceAlerts: ViewModifier {
    @Bindable var device: L6Device

    func body(content: Content) -> some View {
        content
            // The title depends on the issue, so it is resolved here rather than taken from a literal.
            .alert(String(localized: device.issue?.title ?? "Error"), item: $device.issue) { _ in
                Button("OK") {}
            } message: { issue in
                Text(issue.message)
            }
            .confirmationDialog("Resample to 48 kHz", item: $device.resamplePrompt, titleVisibility: .visible) { prompt in
                Button("Overwrite") {
                    Task { await device.answerResample(prompt, .yes) }
                }
                Button("Create") {
                    Task { await device.answerResample(prompt, .other) }
                }
                Button("Cancel", role: .cancel) {
                    Task { await device.answerResample(prompt, .no) }
                }
            } message: { prompt in
                Text(
                    "“\(prompt.file.name)” must be resampled to use it. Do you want to overwrite the original file or create a new one?",
                    comment: "Question before assigning an audio file to a sound pad. The variable is the file name."
                )
            }
    }
}

extension View {
    func deviceAlerts(_ device: L6Device) -> some View {
        modifier(DeviceAlerts(device: device))
    }
}

#Preview("Searching") {
    ConnectionStatusView(state: .searching, isDemo: false, setDemo: { _ in })
}
