import L6Kit
import SwiftUI

struct MixerView: View {
    let device: L6Device

    var body: some View {
        Form {
            OutputPointsSection(device: device)
            USBAudioSection(device: device)
            Section {
                NavigationLink("AUX Send Point") {
                    AuxSendPointView(device: device)
                }
                NavigationLink("Effect Parameters") {
                    EffectParametersView(effects: device.effects)
                }
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Mixer")
    }
}

struct OutputPointsSection: View {
    @Bindable var device: L6Device

    var body: some View {
        Section("Outputs") {
            Picker("Monitor Point", selection: $device.monitorPoint) {
                ForEach(OutputPoint.allCases) { point in
                    Text(point.name).tag(point)
                }
            }
            Picker("Sub Out Point", selection: $device.subOutPoint) {
                ForEach(OutputPoint.allCases) { point in
                    Text(point.name).tag(point)
                }
            }
        }
    }
}

struct USBAudioSection: View {
    @Bindable var device: L6Device

    var body: some View {
        Section {
            Picker("Audio Interface Mode", selection: $device.audioInterfaceMode) {
                ForEach(AudioInterfaceMode.allCases) { mode in
                    Text(mode.name).tag(mode)
                }
            }
            Toggle("USB Mix Minus", isOn: $device.usbMixMinus)
        } header: {
            Text("USB Audio")
        } footer: {
            Text("Changing the audio interface mode restarts the USB connection. The app reconnects after a moment.")
        }
    }
}

struct AuxSendPointView: View {
    let device: L6Device

    var body: some View {
        Form {
            ForEach(0..<L6maxLayout.auxCount, id: \.self) { aux in
                AuxSendPointSection(device: device, aux: aux)
            }
        }
        .formStyle(.grouped)
        .navigationTitle("AUX Send Point")
    }
}

struct AuxSendPointSection: View {
    @Bindable var device: L6Device
    let aux: Int

    var body: some View {
        Section {
            ForEach(0..<L6maxLayout.channelCount, id: \.self) { channel in
                Picker(selection: $device[auxPost: aux, channel: channel]) {
                    Text("Pre").tag(false)
                    Text("Post").tag(true)
                } label: {
                    Text("CH \(channel + 1)", comment: "Mixer channel. The variable is the channel number.")
                }
                .pickerStyle(.segmented)
            }
        } header: {
            Text("AUX \(aux + 1) Send", comment: "Section title. The variable is the AUX bus number.")
        }
    }
}

struct EffectParametersView: View {
    let effects: [Effect]

    var body: some View {
        Form {
            ForEach(effects) { effect in
                Section {
                    ForEach(effect.spec.parameters.indices, id: \.self) { index in
                        EffectParameterRow(effect: effect, index: index)
                    }
                } header: {
                    // Effect and parameter names are the legends printed on the device.
                    Text(verbatim: effect.spec.name)
                }
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Effect Parameters")
    }
}

struct EffectParameterRow: View {
    @Bindable var effect: Effect
    let index: Int

    var body: some View {
        let parameter = effect.spec.parameters[index]
        LabeledContent {
            Slider(
                value: $effect[position: index],
                in: Double(parameter.range.lowerBound)...Double(parameter.range.upperBound)
            ) {
                Text(verbatim: parameter.name)
            }
            .labelsHidden()
        } label: {
            VStack(alignment: .leading) {
                Text(verbatim: parameter.name)
                Text(effect.values[index], format: .number)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            .frame(minWidth: 84, alignment: .leading)
        }
    }
}
