import SwiftUI

/// 03 · Log. What is in flight, and every transport that finished, newest first.
struct LogScreen: View {
    @Environment(AppModel.self) private var model
    @State private var error: String?

    var body: some View {
        if let snapshot = model.snapshot {
            content(snapshot)
        }
    }

    private func content(_ snapshot: AppSnapshot) -> some View {
        let count = snapshot.history.count
        let moved = snapshot.history.reduce(Int64(0)) { total, entry in total + entry.items.reduce(0) { $0 + max(0, $1.size) } }
        let retention = snapshot.settings.historyRetentionDays
        let failed = snapshot.history.filter { $0.state.isBad }.count
        let inFlight = snapshot.activeTransfers
        let title = count > 0 ? Format.plural(count, "transport") : inFlight.isEmpty ? "Nothing yet" : "\(inFlight.count) in flight"
        let sub = count > 0 ? Format.bytes(moved) : nil

        return ZStack {
            Floor(variant: .faint)
            ScrollView {
                VStack(spacing: 22) {
                    Headline(title: title, sub: sub)
                        .padding(.top, 18)
                        .padding(.horizontal, 12)

                    ForEach(inFlight) { batch in
                        Flight(batch: batch, onError: { error = $0 })
                    }

                    if let error {
                        Text(error).font(Theme.sans(13)).foregroundStyle(Theme.danger)
                    }

                    LazyVStack(spacing: 0) {
                        ForEach(snapshot.history) { entry in
                            Record(entry: entry, pads: snapshot.pads)
                            Rectangle().fill(Theme.line(0.1)).frame(height: 1)
                        }
                    }

                    VStack(spacing: 4) {
                        Hint(text: "\(retention > 0 ? "\(retention) days" : "Forever") · staging \(Format.bytes(snapshot.about.ownedStagingBytes))")
                        if failed > 0 {
                            Hint(text: "\(failed) failed", tone: Theme.danger)
                        }
                    }
                    .frame(maxWidth: .infinity)
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 32)
                .animation(.snappy, value: snapshot.history.map(\.id))
            }
            // Content that already fits must not rubber-band: the bounce
            // reads as a scroll and flips the minimised tab bar back open.
            .scrollBounceBehavior(.basedOnSize)
            .safeAreaInset(edge: .top, spacing: 0) { ScreenHeader() }
        }
    }
}

private struct Flight: View {
    var batch: TransferBatch
    var onError: (String?) -> Void
    @Environment(AppModel.self) private var model
    @State private var busy = false

    var body: some View {
        let outbound = batch.state != .receiving
        let rate = batch.targets.compactMap(\.rateLabel).first
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Text(outbound ? "OUTBOUND" : "INBOUND")
                    .font(Theme.mono(11))
                    .tracking(1.1)
                    .foregroundStyle(Theme.accent)
                Text(batch.state.word).font(Theme.mono(12)).foregroundStyle(Theme.dim)
                Spacer(minLength: 8)
                if let rate {
                    Text(rate).font(Theme.mono(12.5)).foregroundStyle(Theme.accent)
                }
                Button("Abort") {
                    busy = true
                    onError(nil)
                    Task {
                        defer { busy = false }
                        do { try await model.cancelBatch(batch.id) }
                        catch { onError("Abort failed") }
                    }
                }
                .buttonStyle(.mini)
                .disabled(busy)
            }
            Text(batch.label)
                .font(Theme.mono(15))
                .foregroundStyle(Theme.ink)
                .lineLimit(1)
                .truncationMode(.middle)
            ProgressView(value: Double(min(100, max(2, batch.progress))), total: 100)
                .progressViewStyle(BeamProgressStyle())
                .accessibilityLabel("\(batch.label) progress")
                .accessibilityValue("\(batch.progress) percent")
            if !batch.targets.isEmpty {
                FlowLayout(spacing: 18) {
                    ForEach(batch.targets) { target in
                        Text("\(Text(target.deviceName).foregroundStyle(Theme.dim)) \(Text("\(target.progress)%").font(Theme.mono(12)).foregroundStyle(Theme.value))")
                            .font(Theme.sans(12.5))
                    }
                }
            }
        }
        .padding(16)
        .glassEffect(.regular, in: .rect(cornerRadius: 20))
    }
}

/// The desktop's 2px flight bar, with light passing along it.
struct BeamProgressStyle: ProgressViewStyle {
    func makeBody(configuration: Configuration) -> some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Rectangle().fill(Theme.accent.opacity(0.14))
                Rectangle()
                    .fill(Theme.accent)
                    .frame(width: proxy.size.width * (configuration.fractionCompleted ?? 0))
                    .animation(.easeOut(duration: 0.4), value: configuration.fractionCompleted)
            }
        }
        .frame(height: 2)
    }
}

private struct Record: View {
    var entry: HistoryEntry
    var pads: [Pad]
    @Environment(AppModel.self) private var model
    @Environment(ArrivalPresenter.self) private var presenter
    @State private var open = false
    @State private var busy = false
    @State private var retryError: String?

    var body: some View {
        let openable = entry.incoming && entry.state == .complete && !entry.items.isEmpty
        let size = entry.items.reduce(Int64(0)) { $0 + max(0, $1.size) }
        let tone: Color = entry.state.isBad ? Theme.danger : entry.state.isWarning ? Theme.hold : Theme.dim
        VStack(alignment: .leading, spacing: 0) {
            Button {
                guard openable else { return }
                withAnimation(.snappy(duration: 0.25)) { open.toggle() }
            } label: {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(open ? Theme.accent : Theme.dimmer)
                        .rotationEffect(.degrees(open ? 90 : 0))
                        .opacity(openable ? 1 : 0)
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(entry.incoming ? "Received" : "Sent")
                                .font(Theme.sans(13))
                                .foregroundStyle(entry.incoming ? Theme.accent : Theme.dim)
                            Text(entry.summary)
                                .font(Theme.mono(14))
                                .foregroundStyle(Theme.ink)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer(minLength: 6)
                            Text(entry.state.word).font(Theme.sans(13)).foregroundStyle(tone)
                        }
                        HStack(spacing: 6) {
                            Text(Format.peer(entry.peerName, pads: pads)).foregroundStyle(Theme.value)
                            Text("·")
                            Text(size > 0 ? Format.bytes(size) : Format.plural(entry.items.count, "item")).font(Theme.mono(12))
                            Text("·")
                            Text(Format.when(entry.timeLabel))
                        }
                        .font(Theme.sans(12.5))
                        .foregroundStyle(Theme.dim)
                        .lineLimit(1)
                    }
                }
                .padding(.vertical, 12)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint(openable ? (open ? "Collapse" : "Show what arrived") : "")

            if entry.state == .failed {
                HStack {
                    if let retryError {
                        Text(retryError).font(Theme.sans(12.5)).foregroundStyle(Theme.danger)
                    }
                    Spacer()
                    Button("Retry") {
                        busy = true
                        retryError = nil
                        Task {
                            defer { busy = false }
                            do { try await model.retryBatch(entry.id) }
                            catch { retryError = "Retry failed" }
                        }
                    }
                    .buttonStyle(.mini)
                    .disabled(busy)
                }
                .padding(.bottom, 10)
            }

            if openable, open {
                VStack(alignment: .leading, spacing: 8) {
                    if entry.items.count > 1 {
                        HStack {
                            Hint(text: "All \(entry.items.count)")
                            Spacer()
                            Button("Share all") { presenter.shareAll(entry: entry, model: model) }
                                .buttonStyle(.mini)
                        }
                    }
                    ForEach(entry.items) { item in
                        HStack(spacing: 10) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(item.displayName)
                                    .font(Theme.mono(13))
                                    .foregroundStyle(Theme.soft)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                Text(item.available ? Format.size(of: item) : "Gone")
                                    .font(Theme.mono(11.5))
                                    .foregroundStyle(Theme.dim)
                            }
                            Spacer(minLength: 8)
                            if item.available {
                                ArrivalMenu(item: item)
                            }
                        }
                    }
                }
                .padding(.leading, 20)
                .padding(.bottom, 14)
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
    }
}
