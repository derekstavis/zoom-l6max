import L6Kit
import SwiftUI

struct MIDIView: View {
    let device: L6Device

    var body: some View {
        Form {
            MIDISettingsSection(device: device)
            Section {
                NavigationLink("MIDI CC Mapping") {
                    ControlChangeMapView(device: device)
                }
            } footer: {
                Text("Choose which control change number moves each mixer parameter when Mixer Control via MIDI is on.")
            }
        }
        .formStyle(.grouped)
        .navigationTitle("MIDI")
    }
}

struct MIDISettingsSection: View {
    @Bindable var device: L6Device

    var body: some View {
        Section {
            Toggle("Mixer Control via MIDI", isOn: $device.mixerControlViaMIDI)
            Picker("MIDI Channel", selection: $device.midiChannel) {
                ForEach(0..<16, id: \.self) { channel in
                    Text("CH \(channel + 1)", comment: "MIDI channel. The variable is the channel number.").tag(channel)
                }
            }
            Picker("MIDI Out Mode", selection: $device.midiOutMode) {
                ForEach(MIDIOutMode.allCases) { mode in
                    Text(mode.name).tag(mode)
                }
            }
            Picker("MIDI Clock Source", selection: $device.midiClockSource) {
                ForEach(MIDIClockSource.allCases) { source in
                    Text(source.name).tag(source)
                }
            }
        }
    }
}

/// One entry of the control change map being edited.
@Observable
final class ControlChangeSlot: Identifiable {
    let id: Int
    var number: UInt8 = 0

    init(id: Int) {
        self.id = id
    }
}

/// A working copy of the control change map, sent to the device in one piece.
///
/// Each slot is its own observable object so that changing one number
/// re-evaluates one row rather than all of them.
@Observable
final class ControlChangeDraft {
    let slots = (0..<L6maxLayout.controlChangeSlotCount).map { ControlChangeSlot(id: $0) }

    var values: [UInt8] {
        get { slots.map(\.number) }
        set { for (slot, number) in zip(slots, newValue) { slot.number = number } }
    }
}

struct ControlChangeMapView: View {
    let device: L6Device
    @State private var draft = ControlChangeDraft()
    @State private var isConfirmingReset = false

    var body: some View {
        List {
            ForEach(Array(L6maxLayout.controlChangeChannelRows.enumerated()), id: \.element) { row, title in
                Section {
                    ForEach(0..<L6maxLayout.channelCount, id: \.self) { channel in
                        ControlChangeSlotRow(
                            slot: draft.slots[row * L6maxLayout.channelCount + channel],
                            title: .localized("CH \(channel + 1)")
                        )
                    }
                } header: {
                    Text(verbatim: title)
                }
            }
            Section("Other") {
                ForEach(Array(L6maxLayout.controlChangeGlobalSlots.enumerated()), id: \.element) { index, title in
                    ControlChangeSlotRow(
                        slot: draft.slots[L6maxLayout.controlChangeChannelRows.count * L6maxLayout.channelCount + index],
                        title: .verbatim(title)
                    )
                }
            }
        }
        .navigationTitle("MIDI CC Mapping")
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                ApplyControlChangeMapButton(draft: draft, device: device)
            }
            ToolbarItem(placement: .destructiveAction) {
                Button("Default MIDI Settings", role: .destructive) {
                    isConfirmingReset = true
                }
            }
        }
        .confirmationDialog("Restore the default MIDI CC mapping?", isPresented: $isConfirmingReset, titleVisibility: .visible) {
            Button("Restore Defaults", role: .destructive) {
                Task { await device.resetControlChangeMap() }
            }
        }
        .onChange(of: device.controlChangeMap, initial: true) { _, map in
            draft.values = map
        }
    }
}

/// Reads every slot to know whether anything changed, so it lives apart from the list.
struct ApplyControlChangeMapButton: View {
    let draft: ControlChangeDraft
    let device: L6Device

    var body: some View {
        Button("Apply") {
            Task { await device.applyControlChangeMap(draft.values) }
        }
        .disabled(draft.values == device.controlChangeMap || device.isBusy)
    }
}

struct ControlChangeSlotRow: View {
    @Bindable var slot: ControlChangeSlot
    let title: OptionTitle

    var body: some View {
        LabeledContent {
            LongListPicker(title: "Control Change", selection: $slot.number, options: ControlChangeOptions.all)
        } label: {
            OptionTitleText(title: title)
        }
    }
}

enum ControlChangeOptions {
    static let all: [LongListOption<UInt8>] =
        [LongListOption(value: 0, title: .localized("Not Mapped"))]
        + (UInt8(1)...119).filter(L6maxLayout.isAssignableControlChange).map {
            LongListOption(value: $0, title: .localized("CC#\(Int($0))"))
        }
}
