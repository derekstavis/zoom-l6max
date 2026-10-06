#if DEBUG && os(macOS)
import AppKit
import L6Kit
import SwiftUI

/// Debug aid: `-resizeBenchmark <screen>` shows one screen against the simulated
/// device, resizes the window through a fixed series of sizes, prints how long
/// each layout-and-draw pass took, and quits.
enum ResizeBenchmark {
    static var screen: String? {
        let arguments = ProcessInfo.processInfo.arguments
        guard let flag = arguments.firstIndex(of: "-resizeBenchmark"), arguments.indices.contains(flag + 1) else { return nil }
        return arguments[flag + 1]
    }

    static func run(screen: String) async {
        try? await Task.sleep(for: .seconds(3))
        guard let window = NSApp.windows.first(where: { $0.contentView != nil && $0.isVisible }) ?? NSApp.windows.first else {
            print("BENCH \(screen) no window")
            exit(1)
        }
        let clock = ContinuousClock()
        var samples: [Double] = []
        let origin = window.frame.origin
        for step in 0..<120 {
            let width = 620 + Double((step * 37) % 700)
            let height = 520 + Double((step * 23) % 320)
            let start = clock.now
            window.setFrame(NSRect(x: origin.x, y: origin.y, width: width, height: height), display: true)
            window.contentView?.layoutSubtreeIfNeeded()
            window.displayIfNeeded()
            CATransaction.flush()
            let elapsed = clock.now - start
            samples.append(Double(elapsed.components.attoseconds) / 1e15 + Double(elapsed.components.seconds) * 1000)
            await Task.yield()
        }
        samples.sort()
        let median = samples[samples.count / 2]
        let p90 = samples[samples.count * 9 / 10]
        print(String(format: "BENCH %@ median %.2f ms  p90 %.2f ms  max %.2f ms", screen, median, p90, samples.last ?? 0))
        exit(0)
    }
}

/// Shows the screen named on the command line, including ones normally reached by navigation.
struct BenchmarkRoot: View {
    let device: L6Device
    let screen: String

    var body: some View {
        Group {
            if device.connection != .connected {
                ProgressView()
            } else {
                NavigationStack {
                    switch screen {
                    case "effects": EffectParametersView(effects: device.effects)
                    case "cc": ControlChangeMapView(device: device)
                    case "aux": AuxSendPointView(device: device)
                    case "mixer": MixerView(device: device)
                    case "midi": MIDIView(device: device)
                    case "device": DeviceView(device: device, isDemo: true, setDemo: { _ in })
                    default: SoundPadsView(pads: device.pads, device: device)
                    }
                }
            }
        }
        .task { await device.run() }
        .task { await ResizeBenchmark.run(screen: screen) }
    }
}
#endif
