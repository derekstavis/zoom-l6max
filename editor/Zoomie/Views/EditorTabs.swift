import L6Kit
import SwiftUI

enum EditorTab: String, Hashable {
    case pads, mixer, midi, device

    /// `-tab mixer` on the command line opens that tab, for screenshots and UI checks.
    static var launchTab: EditorTab {
        let arguments = ProcessInfo.processInfo.arguments
        guard let flag = arguments.firstIndex(of: "-tab"), arguments.indices.contains(flag + 1) else { return .pads }
        return EditorTab(rawValue: arguments[flag + 1]) ?? .pads
    }
}

struct EditorTabs: View {
    let device: L6Device
    let isDemo: Bool
    let setDemo: (Bool) -> Void
    @State private var selection = EditorTab.launchTab

    var body: some View {
        TabView(selection: $selection) {
            Tab("Sound Pads", systemImage: "square.grid.2x2", value: EditorTab.pads) {
                NavigationStack {
                    SoundPadsView(pads: device.pads, device: device)
                }
            }
            Tab("Mixer", systemImage: "slider.vertical.3", value: EditorTab.mixer) {
                NavigationStack {
                    MixerView(device: device)
                }
            }
            Tab("MIDI", systemImage: "pianokeys", value: EditorTab.midi) {
                NavigationStack {
                    MIDIView(device: device)
                }
            }
            Tab("Device", systemImage: "gearshape", value: EditorTab.device) {
                NavigationStack {
                    DeviceView(device: device, isDemo: isDemo, setDemo: setDemo)
                }
            }
        }
        .tabViewStyle(.sidebarAdaptable)
    }
}

#Preview {
    @Previewable @State var model = AppModel.preview
    RootView(model: model)
}

extension AppModel {
    /// A model already switched to the simulated device.
    static var preview: AppModel {
        let model = AppModel()
        model.setDemo(true)
        return model
    }
}
