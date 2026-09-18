import Foundation
import UniformTypeIdentifiers

/// Picked files are copied into app storage before they are queued: iOS only
/// lends a picked file for as long as the picker's security scope lasts, and a
/// held batch may not leave for hours. Each batch's copies are removed once the
/// core no longer needs them.
@MainActor
final class Outbox {
    private let root: URL
    private let defaults = UserDefaults.standard
    private let assignmentsKey = "outbox.batchFolders"

    init() {
        root = (try? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true))
            .map { $0.appending(path: "Outbox", directoryHint: .isDirectory) }
            ?? URL.temporaryDirectory.appending(path: "Outbox", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var excluded = root
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? excluded.setResourceValues(values)
    }

    /// Copies files or folders the picker lent us into a fresh outbox folder.
    func adopt(_ urls: [URL]) throws -> [URL] {
        let folder = try makeFolder()
        return try urls.map { source in
            let scoped = source.startAccessingSecurityScopedResource()
            defer { if scoped { source.stopAccessingSecurityScopedResource() } }
            let destination = uniqueDestination(in: folder, named: source.lastPathComponent)
            try FileManager.default.copyItem(at: source, to: destination)
            return destination
        }
    }

    /// Moves a file iOS handed over temporarily (from Photos) into the outbox.
    func adoptTemporary(_ url: URL, preferredName: String) throws -> URL {
        let folder = try makeFolder()
        let destination = uniqueDestination(in: folder, named: preferredName)
        try FileManager.default.copyItem(at: url, to: destination)
        return destination
    }

    func assign(_ files: [URL], toBatch batchId: String) {
        var assignments = defaults.dictionary(forKey: assignmentsKey) as? [String: [String]] ?? [:]
        assignments[batchId] = Array(Set(files.map { $0.deletingLastPathComponent().lastPathComponent }))
        defaults.set(assignments, forKey: assignmentsKey)
    }

    func discard(_ files: [URL]) {
        for folder in Set(files.map { $0.deletingLastPathComponent() }) where folder.deletingLastPathComponent() == root {
            try? FileManager.default.removeItem(at: folder)
        }
    }

    /// Removes copies whose batch has finished, was cancelled, or is gone from
    /// history. Failed batches keep theirs so they can be retried.
    func release(keeping snapshot: AppSnapshot) {
        var assignments = defaults.dictionary(forKey: assignmentsKey) as? [String: [String]] ?? [:]
        guard !assignments.isEmpty else { return }
        let history = Dictionary(snapshot.history.map { ($0.id, $0.state) }, uniquingKeysWith: { first, _ in first })
        let live = Set(snapshot.queuedBatches.map(\.id)).union(snapshot.transfers.filter { !$0.state.isBad && $0.state != .complete }.map(\.id))
        var changed = false
        for (batchId, folders) in assignments where !live.contains(batchId) {
            let state = history[batchId]
            guard state == nil || state == .complete || state == .cancelled else { continue }
            for folder in folders {
                try? FileManager.default.removeItem(at: root.appending(path: folder, directoryHint: .isDirectory))
            }
            assignments[batchId] = nil
            changed = true
        }
        if changed { defaults.set(assignments, forKey: assignmentsKey) }
    }

    private func makeFolder() throws -> URL {
        let folder = root.appending(path: UUID().uuidString, directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    private func uniqueDestination(in folder: URL, named name: String) -> URL {
        let safe = name.isEmpty ? "Untitled" : name
        var candidate = folder.appending(path: safe)
        let stem = (safe as NSString).deletingPathExtension
        let ext = (safe as NSString).pathExtension
        var index = 2
        while FileManager.default.fileExists(atPath: candidate.path) {
            candidate = folder.appending(path: ext.isEmpty ? "\(stem) \(index)" : "\(stem) \(index).\(ext)")
            index += 1
        }
        return candidate
    }
}
