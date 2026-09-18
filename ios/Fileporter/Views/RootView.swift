import QuickLook
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model
    @State private var presenter = ArrivalPresenter()
    @State private var tab: Screen = .transport
    /// Onboarding stays on screen for a beat after it succeeds so the pad can
    /// light before the tabs replace it.
    @State private var onboardingShown = false

    enum Screen: Hashable { case transport, pads, log, config }

    /// The accessory reads as pads when idle and as a transfer when one is
    /// running; tapping it goes wherever it is pointing.
    private func active(_ snapshot: AppSnapshot) -> Bool { !snapshot.activeTransfers.isEmpty }

    var body: some View {
        ZStack {
            Theme.background.ignoresSafeArea()
            switch model.loadState {
            case .loading:
                Text("Loading…")
                    .font(Theme.sans(15, weight: .light))
                    .foregroundStyle(Theme.dim)
            case .failed(let message):
                VStack(spacing: 16) {
                    Text("Couldn’t load").font(Theme.sans(22, weight: .light)).foregroundStyle(Theme.ink)
                    Text(message).font(Theme.sans(14)).foregroundStyle(Theme.dim).multilineTextAlignment(.center)
                    Button("Try again") { Task { await model.retryStart() } }
                        .buttonStyle(.glass)
                }
                .padding(32)
            case .ready:
                if let snapshot = model.snapshot, snapshot.settings.onboardingComplete, !onboardingShown {
                    tabs(snapshot)
                } else {
                    OnboardingScreen()
                        .onChange(of: model.snapshot?.settings.onboardingComplete) { _, complete in
                            guard complete == true else { return }
                            onboardingShown = true
                            Task {
                                try? await Task.sleep(for: .seconds(1.4))
                                withAnimation(.easeInOut(duration: 0.4)) { onboardingShown = false }
                            }
                        }
                }
            }
        }
        .environment(presenter)
        .preferredColorScheme(.dark)
        .tint(Theme.accent)
        .sheet(item: $presenter.shareItems) { items in
            ShareSheet(items: items.urls).presentationDetents([.medium, .large])
        }
        .quickLookPreview($presenter.previewURL)
        .alert("Unavailable", isPresented: Binding(get: { presenter.error != nil }, set: { if !$0 { presenter.error = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(presenter.error ?? "")
        }
    }

    private func tabs(_ snapshot: AppSnapshot) -> some View {
        TabView(selection: $tab) {
            Tab("Transport", systemImage: "arrow.up.circle", value: .transport) {
                TransportScreen()
            }
            Tab("Pads", systemImage: "circle.hexagongrid", value: .pads) {
                PadsScreen()
            }
            .badge(snapshot.pairing.pendingPairings.count)
            Tab("Log", systemImage: "list.bullet", value: .log) {
                LogScreen()
            }
            Tab("Config", systemImage: "slider.horizontal.3", value: .config) {
                ConfigScreen()
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        .tabViewBottomAccessory {
            StatusAccessory(snapshot: snapshot) { tab = active(snapshot) ? .log : .pads }
        }
        .sheet(item: Binding(
            get: { snapshot.pairing.pendingPairings.first { !$0.localConfirmed } },
            set: { _ in }
        )) { pairing in
            PairingSheet(pairing: pairing)
        }
    }
}

/// The glass accessory above the tabs: this pad's live reading, or what is on
/// the beam right now.
private struct StatusAccessory: View {
    var snapshot: AppSnapshot
    var onOpen: () -> Void
    @Environment(\.tabViewBottomAccessoryPlacement) private var placement

    var body: some View {
        let active = snapshot.activeTransfers
        let pads = snapshot.pads
        let online = pads.filter(\.online).count
        Button(action: onOpen) {
            HStack(spacing: 10) {
                if let batch = active.first {
                    Image(systemName: batch.state == .receiving ? "arrow.down" : "arrow.up")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Theme.accent)
                    Text(batch.label)
                        .font(Theme.mono(13))
                        .foregroundStyle(Theme.ink)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 6)
                    if placement != .inline, let rate = batch.targets.compactMap(\.rateLabel).first {
                        Text(rate).font(Theme.mono(12)).foregroundStyle(Theme.accent)
                    }
                    Text("\(batch.progress)%")
                        .font(Theme.mono(13))
                        .foregroundStyle(Theme.value)
                        .contentTransition(.numericText())
                } else {
                    StatusDot(on: snapshot.listening)
                    Text(snapshot.listening ? "Listening" : "Offline")
                        .font(Theme.sans(13.5))
                        .foregroundStyle(Theme.soft)
                    Spacer(minLength: 6)
                    Text(pads.isEmpty ? "no pads linked" : "\(online) of \(pads.count) linked")
                        .font(Theme.sans(13.5))
                        .foregroundStyle(Theme.dim)
                }
            }
            .padding(.horizontal, 16)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .animation(.snappy, value: active.first?.progress)
        .accessibilityLabel(active.isEmpty
            ? "\(snapshot.listening ? "Listening" : "Offline"), \(online) of \(pads.count) linked pads online"
            : "\(active.count) transport\(active.count == 1 ? "" : "s") in flight")
    }
}
