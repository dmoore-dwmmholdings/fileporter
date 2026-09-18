import SwiftUI

/// 02 · Pads. Linked pads, pads nearby, and patterns held for dark ones.
struct PadsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var error: String?
    @State private var endpoint = ""
    @State private var adding = false
    @State private var renaming: Pad?
    @State private var alias = ""
    @FocusState private var endpointFocused: Bool

    var body: some View {
        if let snapshot = model.snapshot {
            content(snapshot)
        }
    }

    private func content(_ snapshot: AppSnapshot) -> some View {
        let automatic = snapshot.settings.automaticDeviceTrust
        let held = snapshot.queuedBatches.filter { $0.waitingForAvailable || $0.state == "queued" || $0.state == "waiting" }
        return ZStack {
            Floor()
            ScrollView {
                VStack(spacing: 28) {
                    Headline(title: "Pads")
                        .padding(.horizontal, 12)
                        .padding(.top, 18)

                    tiles(snapshot.pads)

                    if !snapshot.nearbyDevices.isEmpty || !held.isEmpty {
                        VStack(spacing: 2) {
                            ForEach(snapshot.nearbyDevices) { device in
                                NearbyRow(device: device, automatic: automatic, onError: { error = $0 })
                            }
                            ForEach(held) { batch in
                                HeldRow(batch: batch, pads: snapshot.pads, onError: { error = $0 })
                            }
                        }
                    }

                    if let error {
                        Text(error)
                            .font(Theme.sans(13))
                            .foregroundStyle(Theme.danger)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }

                    addPad
                }
                .padding(.horizontal, 22)
                .padding(.bottom, 32)
            }
            // Content that already fits must not rubber-band: the bounce
            // reads as a scroll and flips the minimised tab bar back open.
            .scrollBounceBehavior(.basedOnSize)
            .scrollDismissesKeyboard(.interactively)
            .safeAreaInset(edge: .top, spacing: 0) {
                ScreenHeader().background(Theme.background.opacity(0.001))
            }
        }
        .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $alias)
            Button("Cancel", role: .cancel) {}
            Button("Save") { rename() }
        }
    }

    private func tiles(_ pads: [Pad]) -> some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 18), GridItem(.flexible(), spacing: 18)], spacing: 26) {
            if pads.isEmpty {
                VStack(spacing: 5) {
                    PadArtView(art: .tile, timing: .tile, dark: true).frame(height: 84)
                    Text("No pads").font(Theme.sans(17)).foregroundStyle(Theme.dim).padding(.top, 6)
                }
                .multilineTextAlignment(.center)
                .gridCellColumns(2)
            } else {
                ForEach(pads) { pad in
                    PadTile(pad: pad) {
                        alias = pad.name
                        renaming = pad
                    }
                }
            }
        }
    }

    private var addPad: some View {
        VStack(alignment: .leading, spacing: 8) {
            FieldLabel(text: "Add by address")
            HStack(alignment: .lastTextBaseline, spacing: 12) {
                TextField("192.168.1.24:48721", text: $endpoint)
                    .font(Theme.mono(15))
                    .foregroundStyle(Theme.ink)
                    .keyboardType(.numbersAndPunctuation)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.go)
                    .onSubmit(add)
                    .underlinedField()
                    .accessibilityLabel("Add a pad by address")
                Button(adding ? "Adding…" : "Add", action: add)
                    .font(Theme.sans(14, weight: .medium))
                    .buttonStyle(.glass)
                    .disabled(adding)
            }
        }
    }

    private func add() {
        let trimmed = endpoint.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            error = "Enter an address"
            return
        }
        adding = true
        error = nil
        Task {
            defer { adding = false }
            do {
                try await model.startPairing(endpoint: trimmed)
                endpoint = ""
            } catch {
                self.error = "No pad at that address"
            }
        }
    }

    private func rename() {
        guard let pad = renaming else { return }
        let next = alias.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !next.isEmpty, next.count <= 128 else {
            error = "Name must be 1–128 characters"
            return
        }
        Task {
            do {
                error = nil
                try await model.renamePad(pad.id, to: next)
            } catch {
                self.error = "Rename failed"
            }
        }
    }
}

private struct PadTile: View {
    var pad: Pad
    var onRename: () -> Void

    var body: some View {
        VStack(spacing: 4) {
            PadArtView(art: .tile, timing: .tile, dark: !pad.online)
                .frame(height: 78)
            Text(pad.name)
                .font(Theme.sans(17))
                .tracking(-0.2)
                .foregroundStyle(Theme.ink)
                .lineLimit(1)
                .padding(.top, 6)
            Text(stateLine)
                .font(Theme.sans(13))
                .foregroundStyle(pad.online ? Theme.accent : Theme.dim)
                .lineLimit(2)
                .multilineTextAlignment(.center)
            Text(pad.fingerprintShort)
                .font(Theme.mono(11))
                .foregroundStyle(Theme.dimmer)
                .padding(.top, 2)
            Button("Rename", action: onRename)
                .buttonStyle(.mini)
                .padding(.top, 6)
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .contain)
    }

    private var stateLine: String {
        if pad.online { return "Linked" }
        guard let seen = pad.lastSeenAt else { return "Dark" }
        return "Dark · \(Format.when(String(seen)))"
    }
}

private struct NearbyRow: View {
    var device: NearbyDevice
    var automatic: Bool
    var onError: (String?) -> Void
    @Environment(AppModel.self) private var model
    @State private var busy = false

    var body: some View {
        HStack(spacing: 12) {
            Beacon()
            VStack(alignment: .leading, spacing: 2) {
                Text(device.displayName).font(Theme.sans(15)).foregroundStyle(Theme.ink)
                Text(device.endpoint).font(Theme.mono(12)).foregroundStyle(Theme.dim)
            }
            Spacer(minLength: 8)
            if automatic {
                Text("proving…").font(Theme.sans(13)).foregroundStyle(Theme.accent)
            } else {
                Button(busy ? "Linking" : "Link") {
                    busy = true
                    onError(nil)
                    Task {
                        defer { busy = false }
                        do { try await model.startPairing(discovered: device.deviceId) }
                        catch { onError("\(device.displayName) unreachable") }
                    }
                }
                .buttonStyle(.mini)
                .disabled(busy)
            }
        }
        .padding(.vertical, 11)
    }
}

private struct HeldRow: View {
    var batch: QueuedBatch
    var pads: [Pad]
    var onError: (String?) -> Void
    @Environment(AppModel.self) private var model
    @State private var busy = false

    var body: some View {
        let target = pads.first { batch.targetDeviceIds.contains($0.id) }
        HStack(spacing: 12) {
            Beacon(idle: true)
            Text(Format.plural(batch.itemCount, "item")).font(Theme.mono(13)).foregroundStyle(Theme.soft)
            Spacer(minLength: 8)
            Text("held · \(target?.name ?? "dark pad")").font(Theme.sans(13)).foregroundStyle(Theme.dim)
            Button("Discard") {
                busy = true
                onError(nil)
                Task {
                    defer { busy = false }
                    do { try await model.cancelBatch(batch.id) }
                    catch { onError("Discard failed") }
                }
            }
            .buttonStyle(.miniDanger)
            .disabled(busy)
        }
        .padding(.vertical, 11)
    }
}

/// Matching-code confirmation, shown whenever a pairing waits on this pad.
struct PairingSheet: View {
    var pairing: PendingPairing
    @Environment(AppModel.self) private var model
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Confirm \(pairing.remoteName)")
                .font(Theme.sans(26, weight: .light))
                .foregroundStyle(Theme.ink)
            // Comparing the code on both pads is the whole check; it stays.
            Text("Same code on both pads?")
                .font(Theme.sans(15, weight: .light))
                .foregroundStyle(Theme.dim)
            if let code = pairing.sasCode {
                Text(code.enumerated().map { index, character in index == 3 ? " \(character)" : String(character) }.joined())
                    .font(Theme.mono(44, weight: .medium))
                    .tracking(6)
                    .foregroundStyle(Theme.accent)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
                    .accessibilityLabel("Security code \(code.map(String.init).joined(separator: " "))")
            } else {
                Text("No code yet")
                    .font(Theme.sans(14))
                    .foregroundStyle(Theme.hold)
            }
            Hint(text: "Other pad · \(pairing.remoteConfirmed ? "confirmed" : "waiting")")
            if let error {
                Text(error).font(Theme.sans(13)).foregroundStyle(Theme.danger)
            }
            Spacer(minLength: 0)
            GlassEffectContainer(spacing: 12) {
                HStack(spacing: 12) {
                    Button { respond(false) } label: {
                        Text("Reject").frame(maxWidth: .infinity).padding(.vertical, 6)
                    }
                    .buttonStyle(.glass)
                    Button { respond(true) } label: {
                        Text(busy ? "Confirming…" : "Confirm link").frame(maxWidth: .infinity).padding(.vertical, 6)
                    }
                    .buttonStyle(.glassProminent)
                    .disabled(pairing.sasCode == nil)
                }
                .font(Theme.sans(16, weight: .medium))
                .disabled(busy)
            }
        }
        .padding(24)
        .presentationDetents([.medium])
        .presentationBackground(Theme.background.opacity(0.6))
        .interactiveDismissDisabled()
    }

    private func respond(_ accept: Bool) {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                if accept { try await model.confirmPairing(pairing.id) } else { try await model.rejectPairing(pairing.id) }
            } catch {
                self.error = "Failed"
            }
        }
    }
}
