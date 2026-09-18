import SwiftUI

/// 04 · Config. Changes apply together and are checked before anything is saved.
struct ConfigScreen: View {
    @Environment(AppModel.self) private var model
    @State private var draft = Draft()
    @State private var loadedRevision: UInt64?
    @State private var saving = false
    @State private var error: String?
    @State private var status: String?

    private static let retentions = [7, 30, 90, 0]

    struct Draft: Equatable {
        var deviceName = ""
        var listenAddress = ""
        var retention = 30
        var receiving = true
        var automaticTrust = true
        var notifications = true

        init() {}

        init(_ snapshot: AppSnapshot) {
            deviceName = snapshot.localDeviceName
            listenAddress = snapshot.settings.preferredListenAddress
            retention = snapshot.settings.historyRetentionDays
            receiving = snapshot.lifecycle.receivingEnabled
            automaticTrust = snapshot.settings.automaticDeviceTrust
            notifications = snapshot.settings.notificationsEnabled
        }
    }

    var body: some View {
        if let snapshot = model.snapshot {
            content(snapshot)
                .onAppear { if loadedRevision == nil { reset(to: snapshot) } }
        }
    }

    private func content(_ snapshot: AppSnapshot) -> some View {
        let saved = Draft(snapshot)
        let dirty = draft != saved
        let validAddress = Self.validListenAddress(draft.listenAddress)
        return ZStack {
            Floor(variant: .faint)
            ScrollView {
                VStack(alignment: .leading, spacing: 26) {
                    Headline(title: "This pad")
                        .padding(.top, 18)

                    VStack(alignment: .leading, spacing: 8) {
                        FieldLabel(text: "Name other pads see")
                        TextField("This iPhone", text: $draft.deviceName)
                            .font(Theme.sans(17))
                            .foregroundStyle(Theme.ink)
                            .submitLabel(.done)
                            .underlinedField(invalid: draft.deviceName.trimmingCharacters(in: .whitespaces).isEmpty)
                            .onChange(of: draft.deviceName) { _, name in
                                if name.count > 48 { draft.deviceName = String(name.prefix(48)) }
                            }
                        if !snapshot.pairing.localDeviceId.isEmpty {
                            Text(Format.shortId(snapshot.pairing.localDeviceId))
                                .font(Theme.mono(12))
                                .foregroundStyle(Theme.accent)
                        }
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        FieldLabel(text: "Arrivals")
                        HStack(alignment: .firstTextBaseline, spacing: 12) {
                            Text("Files › On My iPhone › Fileporter")
                                .font(Theme.mono(13.5))
                                .foregroundStyle(Theme.value)
                                .fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                            Button("Open") { model.showInFiles(model.receivedFolder) }
                                .font(Theme.sans(14, weight: .medium))
                                .buttonStyle(.glass)
                        }
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        FieldLabel(text: "Listen address")
                        TextField("0.0.0.0:48721", text: $draft.listenAddress)
                            .font(Theme.mono(15))
                            .foregroundStyle(Theme.ink)
                            .keyboardType(.numbersAndPunctuation)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .underlinedField(invalid: !validAddress)
                    }

                    VStack(alignment: .leading, spacing: 10) {
                        FieldLabel(text: "Keep log")
                        GlassEffectContainer(spacing: 6) {
                            HStack(spacing: 6) {
                                ForEach(Self.retentions, id: \.self) { days in
                                    let on = draft.retention == days
                                    Button {
                                        withAnimation(.snappy(duration: 0.2)) { draft.retention = days }
                                    } label: {
                                        Text(days == 0 ? "Forever" : "\(days) days")
                                            .font(Theme.sans(14, weight: on ? .medium : .regular))
                                            .foregroundStyle(on ? Theme.onAccent : Theme.nav)
                                            .frame(maxWidth: .infinity)
                                            .padding(.vertical, 9)
                                            .contentShape(Capsule())
                                    }
                                    .buttonStyle(.plain)
                                    .glassChip(selected: on)
                                    .accessibilityAddTraits(on ? .isSelected : [])
                                }
                            }
                        }
                    }

                    VStack(spacing: 0) {
                        QuietToggle(label: "Accept transports", isOn: $draft.receiving)
                        QuietToggle(label: "Link pads automatically", isOn: $draft.automaticTrust)
                        QuietToggle(label: "Notify on arrival", isOn: $draft.notifications)
                    }

                    Diagnostics(snapshot: snapshot)
                }
                .padding(.horizontal, 22)
                // Only what the bar covers when it is showing. A fixed inset
                // left dead space that fought the tab bar's minimise gesture:
                // the height changed as it collapsed, which scrolled the view,
                // which expanded it again.
                .padding(.bottom, barVisible(dirty: dirty) ? 84 : 12)
            }
            // Content that already fits must not rubber-band: the bounce
            // reads as a scroll and flips the minimised tab bar back open.
            .scrollBounceBehavior(.basedOnSize)
            .scrollDismissesKeyboard(.interactively)
            .safeAreaInset(edge: .top, spacing: 0) { ScreenHeader() }
            .overlay(alignment: .bottom) {
                if barVisible(dirty: dirty) {
                    applyBar(snapshot: snapshot, dirty: dirty, validAddress: validAddress)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .animation(.snappy, value: barVisible(dirty: dirty))
        }
        .onChange(of: snapshot.revision) { _, _ in
            // Keep the draft in step with the core unless the user is mid-edit.
            if !dirty || saving { reset(to: snapshot) }
        }
    }

    /// The bar is for acting and for what just happened; there is nothing to
    /// say about settings that are simply in use.
    private func barVisible(dirty: Bool) -> Bool { dirty || error != nil || status != nil }

    private func applyBar(snapshot: AppSnapshot, dirty: Bool, validAddress: Bool) -> some View {
        HStack(spacing: 10) {
            Group {
                if let error {
                    Text(error).foregroundStyle(Theme.danger)
                } else if dirty {
                    Text("Unsaved").foregroundStyle(Theme.hold)
                } else if let status {
                    Text(status).foregroundStyle(Theme.dimmer)
                }
            }
            .font(Theme.sans(12.5))
            .lineLimit(2)
            Spacer(minLength: 4)
            if dirty {
                Button("Discard") { reset(to: snapshot) }
                    .buttonStyle(.glass)
                    .disabled(saving)
                Button(saving ? "Applying…" : "Apply") { apply(snapshot: snapshot) }
                    .buttonStyle(.glassProminent)
                    .disabled(saving || draft.deviceName.trimmingCharacters(in: .whitespaces).isEmpty || !validAddress)
            }
        }
        .font(Theme.sans(14, weight: .medium))
        .padding(.horizontal, 18)
        .padding(.vertical, 10)
        .frame(minHeight: 56)
        .glassEffect(.regular, in: .capsule)
        .padding(.horizontal, 12)
        .padding(.bottom, 6)
        .animation(.snappy, value: dirty)
    }

    private func reset(to snapshot: AppSnapshot) {
        draft = Draft(snapshot)
        loadedRevision = snapshot.revision
        error = nil
    }

    private func apply(snapshot: AppSnapshot) {
        let saved = Draft(snapshot)
        var patch = AppModel.SettingsPatch()
        let name = draft.deviceName.trimmingCharacters(in: .whitespaces)
        if name != saved.deviceName { patch.deviceName = name }
        if draft.listenAddress != saved.listenAddress { patch.listenAddress = draft.listenAddress.trimmingCharacters(in: .whitespaces) }
        if draft.retention != saved.retention { patch.historyRetentionDays = draft.retention }
        if draft.receiving != saved.receiving { patch.receivingEnabled = draft.receiving }
        if draft.automaticTrust != saved.automaticTrust { patch.automaticDeviceTrust = draft.automaticTrust }
        if draft.notifications != saved.notifications { patch.notificationsEnabled = draft.notifications }
        saving = true
        error = nil
        status = nil
        Task {
            defer { saving = false }
            do {
                try await model.updateSettings(patch)
                status = "Applied"
                if let next = model.snapshot { reset(to: next) }
                try? await Task.sleep(for: .seconds(2))
                status = nil
            } catch {
                self.error = (error as? CoreError)?.field == "listenAddress" ? "Invalid address" : "Apply failed"
            }
        }
    }

    static func validListenAddress(_ value: String) -> Bool {
        let trimmed = value.trimmingCharacters(in: .whitespaces)
        guard let colon = trimmed.lastIndex(of: ":"), colon != trimmed.startIndex,
            let port = Int(trimmed[trimmed.index(after: colon)...]), (0...65535).contains(port)
        else { return false }
        return true
    }
}

private struct Diagnostics: View {
    var snapshot: AppSnapshot

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            DisclosureGroup {
                VStack(alignment: .leading, spacing: 10) {
                    row("Listener", snapshot.network.listening ? "Listening" : "Stopped")
                    row("Bound endpoint", snapshot.network.boundEndpoint ?? "—")
                    row("Preferred endpoint", snapshot.network.preferredListenAddress)
                    row("mDNS", snapshot.network.mdnsState.isEmpty ? "—" : snapshot.network.mdnsState)
                    row("Interfaces", snapshot.network.localInterfaceSummaries.joined(separator: ", ").ifEmpty("—"))
                    row("Trusted online endpoints", snapshot.network.trustedOnlineEndpoints.joined(separator: ", ").ifEmpty("—"))
                    row("Recent stable errors", snapshot.network.recentErrorCodes.joined(separator: ", ").ifEmpty("—"))
                }
                .padding(.vertical, 8)
            } label: {
                Text("Diagnostics").font(Theme.sans(14)).foregroundStyle(Theme.dim)
            }
            DisclosureGroup {
                Text("\(snapshot.about.appVersion) · protocol \(snapshot.about.protocolVersion) · db \(snapshot.about.databaseMigrationVersion)")
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.value)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.vertical, 8)
            } label: {
                Text("About").font(Theme.sans(14)).foregroundStyle(Theme.dim)
            }
        }
        .tint(Theme.dimmer)
    }

    private func row(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label).font(Theme.sans(12)).foregroundStyle(Theme.dim)
            Text(value).font(Theme.mono(12)).foregroundStyle(Theme.value).textSelection(.enabled)
        }
    }
}
