import L6Kit
import SwiftUI

@main
struct ZoomieApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            #if DEBUG && os(macOS)
            if let screen = ResizeBenchmark.screen {
                BenchmarkRoot(device: model.device, screen: screen)
            } else {
                RootView(model: model)
            }
            #else
            RootView(model: model)
            #endif
        }
        #if os(macOS)
        .defaultSize(width: 980, height: 720)
        #endif
    }
}

/// Owns the device the app is editing: the real one, or a simulated one in demo mode.
@Observable
final class AppModel {
    static let clockSyncKey = "syncsClockOnConnect"

    private(set) var device: L6Device
    private(set) var isDemo: Bool

    init() {
        let arguments = ProcessInfo.processInfo.arguments
        let isDemo = arguments.contains("-demo") || arguments.contains("-resizeBenchmark")
        self.isDemo = isDemo
        device = Self.makeDevice(isDemo: isDemo)
    }

    func setDemo(_ isDemo: Bool) {
        guard isDemo != self.isDemo else { return }
        self.isDemo = isDemo
        device = Self.makeDevice(isDemo: isDemo)
    }

    private static func makeDevice(isDemo: Bool) -> L6Device {
        let connector: any DeviceConnector =
            if isDemo { SimulatedDevice() } else { (try? CoreMIDIConnector()) ?? UnavailableConnector() }
        let device = L6Device(connector: connector)
        device.syncsClockOnConnect = UserDefaults.standard.object(forKey: clockSyncKey) as? Bool ?? true
        return device
    }
}

/// Stands in when the MIDI system cannot be reached; no device is ever found.
private nonisolated struct UnavailableConnector: DeviceConnector {
    func makeTransport() async -> (any L6Transport)? { nil }
    func changes() -> AsyncStream<Void> { AsyncStream { _ in } }
}
