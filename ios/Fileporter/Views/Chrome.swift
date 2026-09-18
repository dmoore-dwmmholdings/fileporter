import QuickLook
import SwiftUI
import UIKit

/// The brand mark, from the branding kit's symbol.
struct BrandMark: View {
    var height: CGFloat = 18

    var body: some View {
        Image("BrandMark")
            .resizable()
            .scaledToFit()
            .frame(height: height)
            .accessibilityHidden(true)
    }
}

/// The header every screen shares: the mark, and this pad's own reading.
struct ScreenHeader: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 9) {
            BrandMark()
            Text("Fileporter")
                .font(Theme.sans(14, weight: .medium))
                .foregroundStyle(Theme.ink)
            Spacer(minLength: 12)
            if let snapshot = model.snapshot {
                HStack(spacing: 8) {
                    StatusDot(on: snapshot.listening)
                    Text(snapshot.localDeviceName)
                        .font(Theme.sans(13))
                        .foregroundStyle(Theme.dim)
                        .lineLimit(1)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .glassEffect(.regular, in: .capsule)
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(snapshot.localDeviceName), \(snapshot.listening ? "listening" : "offline")")
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 6)
        .padding(.bottom, 4)
    }
}

/// Wraps chips onto as many centred lines as they need.
struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = rows(for: subviews, width: proposal.width ?? .infinity)
        let height = rows.map(\.height).reduce(0, +) + CGFloat(max(0, rows.count - 1)) * spacing
        let width = rows.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in rows(for: subviews, width: bounds.width) {
            var x = bounds.minX + (bounds.width - row.width) / 2
            for index in row.indices {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: y + (row.height - size.height) / 2), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private struct Row {
        var indices: [Int] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func rows(for subviews: Subviews, width: CGFloat) -> [Row] {
        var rows: [Row] = [Row()]
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            let needed = rows[rows.count - 1].indices.isEmpty ? size.width : rows[rows.count - 1].width + spacing + size.width
            if needed > width, !rows[rows.count - 1].indices.isEmpty {
                rows.append(Row())
            }
            var row = rows[rows.count - 1]
            row.width = row.indices.isEmpty ? size.width : row.width + spacing + size.width
            row.height = max(row.height, size.height)
            row.indices.append(index)
            rows[rows.count - 1] = row
        }
        return rows.filter { !$0.indices.isEmpty }
    }
}

/// What the arrival actions present: a share sheet or a Quick Look preview.
@Observable
final class ArrivalPresenter {
    var shareItems: ShareItems?
    var previewURL: URL?
    var error: String?

    struct ShareItems: Identifiable {
        let id = UUID()
        var urls: [URL]
    }

    func share(item: HistoryItem, model: AppModel) {
        Task {
            do { shareItems = ShareItems(urls: try await model.files(forItem: item.itemId)) }
            catch { self.error = "\(item.displayName) is gone" }
        }
    }

    func shareAll(entry: HistoryEntry, model: AppModel) {
        Task {
            do { shareItems = ShareItems(urls: try await model.files(forBatch: entry.id)) }
            catch { self.error = "Those files are gone" }
        }
    }

    func preview(item: HistoryItem, model: AppModel) {
        Task {
            do { previewURL = try await model.files(forItem: item.itemId).first }
            catch { self.error = "\(item.displayName) is gone" }
        }
    }

    func reveal(item: HistoryItem, model: AppModel) {
        Task {
            if let url = try? await model.files(forItem: item.itemId).first {
                model.showInFiles(url.deletingLastPathComponent())
            } else {
                model.showInFiles(model.receivedFolder)
            }
        }
    }
}

/// The three things you want to do with something that arrived, in a glass menu.
struct ArrivalMenu: View {
    var item: HistoryItem
    @Environment(AppModel.self) private var model
    @Environment(ArrivalPresenter.self) private var presenter

    var body: some View {
        Menu {
            Button("Share", systemImage: "square.and.arrow.up") { presenter.share(item: item, model: model) }
            if item.kind != "directory" {
                Button("Quick Look", systemImage: "eye") { presenter.preview(item: item, model: model) }
            }
            Button("Show in Files", systemImage: "folder") { presenter.reveal(item: item, model: model) }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(Theme.soft)
                .frame(width: 34, height: 34)
                .contentShape(Circle())
        }
        .glassEffect(.regular.interactive(), in: .circle)
        .accessibilityLabel("Actions for \(item.displayName)")
    }
}

struct ShareSheet: UIViewControllerRepresentable {
    var items: [URL]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

/// A transient line at the bottom of a screen, with Cancel for a batch just queued.
struct NoticeBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let notice = model.notice {
            HStack(spacing: 12) {
                Text(notice.message)
                    .font(Theme.sans(13.5))
                    .foregroundStyle(notice.bad ? Theme.danger : Theme.soft)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
                if let batchId = notice.batchId {
                    Button("Cancel") {
                        model.notice = nil
                        Task {
                            do { try await model.cancelBatch(batchId) }
                            catch { model.notice = .init(message: "Cancel failed", bad: true) }
                        }
                    }
                    .font(Theme.sans(13.5, weight: .medium))
                    .buttonStyle(.glass)
                }
            }
            .padding(.leading, 18)
            .padding(.trailing, 8)
            .padding(.vertical, 8)
            .glassEffect(.regular, in: .capsule)
            .padding(.horizontal, 16)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .task(id: notice.id) {
                try? await Task.sleep(for: .seconds(5))
                if model.notice?.id == notice.id {
                    withAnimation { model.notice = nil }
                }
            }
        }
    }
}
