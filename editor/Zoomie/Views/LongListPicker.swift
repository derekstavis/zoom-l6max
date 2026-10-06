import SwiftUI

/// Text for an option: either a device legend shown as is, or a localized phrase.
enum OptionTitle: Equatable {
    case verbatim(String)
    case localized(LocalizedStringResource)
}

struct OptionTitleText: View {
    let title: OptionTitle

    var body: some View {
        switch title {
        case .verbatim(let text): Text(verbatim: text)
        case .localized(let resource): Text(resource)
        }
    }
}

struct LongListOption<Value: Hashable>: Equatable, Identifiable {
    let value: Value
    let title: OptionTitle
    var id: Value { value }
}

/// A picker for lists of a hundred or more options.
///
/// A menu picker builds every option up front and keeps them alive, which is
/// costly when many such pickers are on screen. This one shows only the current
/// choice and builds the list while it is open.
struct LongListPicker<Value: Hashable>: View {
    let title: LocalizedStringResource
    @Binding var selection: Value
    let options: [LongListOption<Value>]
    @State private var isPresented = false

    var body: some View {
        Button {
            isPresented = true
        } label: {
            HStack(spacing: 4) {
                if let current = options.first(where: { $0.value == selection }) {
                    OptionTitleText(title: current.title)
                }
                Image(systemName: "chevron.up.chevron.down")
                    .imageScale(.small)
            }
        }
        .buttonStyle(.borderless)
        .accessibilityLabel(Text(title))
        .popover(isPresented: $isPresented) {
            LongListPickerList(title: title, selection: $selection, options: options, isPresented: $isPresented)
        }
    }
}

private struct LongListPickerList<Value: Hashable>: View {
    let title: LocalizedStringResource
    @Binding var selection: Value
    let options: [LongListOption<Value>]
    @Binding var isPresented: Bool

    var body: some View {
        ScrollViewReader { proxy in
            List(options) { option in
                Button {
                    selection = option.value
                    isPresented = false
                } label: {
                    HStack {
                        OptionTitleText(title: option.title)
                        Spacer()
                        if option.value == selection {
                            Image(systemName: "checkmark")
                                .foregroundStyle(.tint)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(option.value == selection ? .isSelected : [])
            }
            .onAppear {
                proxy.scrollTo(selection, anchor: .center)
            }
        }
        .frame(minWidth: 260, minHeight: 380)
        .presentationDetents([.medium, .large])
        .accessibilityLabel(Text(title))
    }
}
