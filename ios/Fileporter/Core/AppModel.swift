import Observation
import SwiftUI
import UIKit
import UserNotifications

/// The app's single source of truth. It owns the shared Rust core, mirrors its
/// snapshot, and translates iPhone lifecycle into the core's suspend/resume.
@Observable
final class AppModel {
    static let shared = AppModel()

    enum LoadState: Equatable {
        case loading
        case ready
        case failed(String)
    }

    struct Notice: Identifiable, Equatable {
        let id = UUID()
        var message: String
        var batchId: String?
        var bad = false
    }

    private(set) var snapshot: AppSnapshot?
    private(set) var loadState: LoadState = .loading
    /// Pads the next payload goes to. Follows whoever is online until the user
    /// picks for themselves, as the desktop does.
    var selectedPadIds: Set<String> = []
    private(set) var stagedFiles: [URL] = []
    var notice: Notice?

    private var recipientsChosen = false
    private var refreshing = false
    private var refreshPending = false
    private var started = false
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid
    private var notifiedHistoryIds: Set<String> = []
    private var isForeground = true

    let outbox = Outbox()

    private init() {}

    // MARK: Startup

    func start() async {
        guard !started else { return }
        started = true
        loadState = .loading
        do {
            let fileManager = FileManager.default
            let support = try fileManager.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
                .appending(path: "Fileporter", directoryHint: .isDirectory)
            let received = URL.documentsDirectory.appending(path: "Received", directoryHint: .isDirectory)
            try await CoreBridge.start(dataDirectory: support, receiveDirectory: received) { _, _ in
                Task { @MainActor in AppModel.shared.refresh() }
            }
            try await reload()
            loadState = .ready
            // Events carry every state change; this only keeps relative times
            // ("last echo 2 min ago") honest while the app is on screen.
            Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(10))
                    guard let self else { return }
                    if self.isForeground { self.refresh() }
                }
            }
        } catch {
            started = false
            loadState = .failed((error as? CoreError)?.message ?? error.localizedDescription)
        }
    }

    func retryStart() async {
        await start()
    }

    // MARK: Snapshot

    /// Coalesces bursts of core change signals into one snapshot read at a time.
    func refresh() {
        guard started else { return }
        if refreshing {
            refreshPending = true
            return
        }
        refreshing = true
        Task {
            defer {
                refreshing = false
                if refreshPending {
                    refreshPending = false
                    refresh()
                }
            }
            try? await reload()
        }
    }

    private func reload() async throws {
        apply(try await CoreBridge.call("snapshot", as: AppSnapshot.self))
    }

    private func apply(_ next: AppSnapshot) {
        if let current = snapshot, next.revision < current.revision { return }
        let previous = snapshot
        snapshot = next
        if !recipientsChosen {
            selectedPadIds = Set(next.pads.filter(\.online).map(\.id))
        } else {
            selectedPadIds.formIntersection(next.pads.map(\.id))
        }
        outbox.release(keeping: next)
        announceArrivals(previous: previous, next: next)
        UIApplication.shared.isIdleTimerDisabled = !next.activeTransfers.isEmpty
        finishBackgroundWorkIfIdle()
        if !stagedFiles.isEmpty, !selectedPadIds.isEmpty {
            let files = stagedFiles
            stagedFiles = []
            Task { await send(files) }
        }
    }

    private func perform(_ command: String, _ input: [String: String] = [:]) async throws {
        try await CoreBridge.perform(command, input)
        try await reload()
    }

    private func performReturningSnapshot(_ command: String, _ input: some Encodable & Sendable) async throws {
        apply(try await CoreBridge.call(command, input, as: AppSnapshot.self))
    }

    // MARK: Onboarding and settings

    nonisolated struct OnboardingInput: Encodable, Sendable {
        var deviceName: String
        var notificationsEnabled: Bool
        var automaticDeviceTrust: Bool
    }

    func completeOnboarding(deviceName: String, notificationsEnabled: Bool) async throws {
        try await performReturningSnapshot(
            "completeOnboarding",
            OnboardingInput(deviceName: deviceName, notificationsEnabled: notificationsEnabled, automaticDeviceTrust: true))
        if notificationsEnabled {
            _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
        }
    }

    nonisolated struct SettingsPatch: Encodable, Sendable, Equatable {
        var deviceName: String?
        var receivingEnabled: Bool?
        var listenAddress: String?
        var notificationsEnabled: Bool?
        var automaticDeviceTrust: Bool?
        var historyRetentionDays: Int?
    }

    func updateSettings(_ patch: SettingsPatch) async throws {
        try await performReturningSnapshot("updateSettings", patch)
        if patch.notificationsEnabled == true {
            _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
        }
    }

    // MARK: Sending

    func togglePad(_ id: String) {
        recipientsChosen = true
        if selectedPadIds.contains(id) {
            selectedPadIds.remove(id)
        } else {
            selectedPadIds.insert(id)
        }
    }

    private nonisolated struct EnqueueInput: Encodable, Sendable {
        var paths: [String]
        var targetDeviceIds: [String]
        var queueOffline: Bool
    }

    /// Sends items already copied into the outbox. With no pad picked they wait
    /// on the deck and go the moment one is.
    func send(_ files: [URL]) async {
        guard !files.isEmpty else { return }
        guard let snapshot else { return }
        let targets = snapshot.pads.map(\.id).filter(selectedPadIds.contains)
        guard !targets.isEmpty else {
            stagedFiles = files
            notice = Notice(message: "Pick a pad")
            return
        }
        let online = Set(snapshot.pads.filter(\.online).map(\.id))
        let queueOffline = targets.contains { !online.contains($0) }
        do {
            let queued = try await CoreBridge.call(
                "enqueuePaths",
                EnqueueInput(paths: files.map(\.path), targetDeviceIds: targets, queueOffline: queueOffline),
                as: QueuedBatchReply.self)
            outbox.assign(files, toBatch: queued.id)
            notice = Notice(
                message: "\(queueOffline ? "Held" : "Sending") · \(Format.plural(queued.itemCount, "item"))",
                batchId: queued.id)
            try? await reload()
        } catch {
            outbox.discard(files)
            notice = Notice(message: "Send failed", bad: true)
        }
    }

    func clearStaged() {
        outbox.discard(stagedFiles)
        stagedFiles = []
    }

    func cancelBatch(_ id: String) async throws {
        try await performReturningSnapshot("cancelBatch", ["batchId": id])
    }

    func retryBatch(_ id: String) async throws {
        try await performReturningSnapshot("retryBatch", ["batchId": id])
    }

    // MARK: Pads

    func startPairing(endpoint: String) async throws {
        try await perform("startPairingAtEndpoint", ["endpoint": endpoint])
    }

    func startPairing(discovered deviceId: String) async throws {
        try await perform("startPairingDiscovered", ["deviceId": deviceId])
    }

    func renamePad(_ id: String, to alias: String) async throws {
        try await perform("renameTrustedDevice", ["deviceId": id, "alias": alias])
    }

    func confirmPairing(_ id: String) async throws {
        try await perform("confirmPairing", ["pairingId": id])
    }

    func rejectPairing(_ id: String) async throws {
        try await perform("rejectPairing", ["pairingId": id])
    }

    // MARK: Arrivals

    /// Resolves completed arrivals to files. The core only ever returns
    /// finished incoming outputs that still exist.
    func files(forItem id: String) async throws -> [URL] {
        try await CoreBridge.call("itemPaths", ["itemId": id], as: [String].self).map { URL(filePath: $0) }
    }

    func files(forBatch id: String) async throws -> [URL] {
        try await CoreBridge.call("batchPaths", ["batchId": id], as: [String].self).map { URL(filePath: $0) }
    }

    var receivedFolder: URL { URL.documentsDirectory.appending(path: "Received", directoryHint: .isDirectory) }

    /// Opens the Files app at a location inside this app's documents.
    func showInFiles(_ url: URL) {
        var components = URLComponents()
        components.scheme = "shareddocuments"
        components.path = url.path
        if let target = components.url { UIApplication.shared.open(target) }
    }

    private func announceArrivals(previous: AppSnapshot?, next: AppSnapshot) {
        let finished = next.history.filter { $0.incoming && $0.state == .complete }
        guard let previous else {
            notifiedHistoryIds = Set(finished.map(\.id))
            return
        }
        let known = Set(previous.history.filter { $0.incoming && $0.state == .complete }.map(\.id)).union(notifiedHistoryIds)
        let fresh = finished.filter { !known.contains($0.id) }
        notifiedHistoryIds.formUnion(fresh.map(\.id))
        guard !fresh.isEmpty, next.settings.notificationsEnabled, !isForeground else { return }
        for entry in fresh {
            // Privacy-safe, as on the desktop: no peer names or paths.
            let content = UNMutableNotificationContent()
            content.title = "Fileporter"
            content.body = "Received \(Format.plural(entry.items.count, "item"))."
            content.sound = .default
            UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: entry.id, content: content, trigger: nil))
        }
    }

    // MARK: Lifecycle

    func scenePhaseChanged(to phase: ScenePhase) {
        guard started, snapshot?.settings.onboardingComplete == true else { return }
        switch phase {
        case .active:
            isForeground = true
            endBackgroundTask()
            Task {
                try? await performReturningSnapshot("resume", [String: String]())
            }
        case .background:
            isForeground = false
            // A transport in flight gets the time iOS allows to finish; the
            // core suspends at a durable checkpoint either way.
            if snapshot?.activeTransfers.isEmpty == false {
                beginBackgroundTask()
            } else {
                suspendCore()
            }
        default:
            break
        }
    }

    private func beginBackgroundTask() {
        guard backgroundTask == .invalid else { return }
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Finish transport") { [weak self] in
            self?.suspendCore()
        }
    }

    private func finishBackgroundWorkIfIdle() {
        guard !isForeground, backgroundTask != .invalid, snapshot?.activeTransfers.isEmpty == true else { return }
        suspendCore()
    }

    private func suspendCore() {
        Task {
            try? await CoreBridge.perform("suspend")
            endBackgroundTask()
        }
    }

    private func endBackgroundTask() {
        guard backgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(backgroundTask)
        backgroundTask = .invalid
    }
}
