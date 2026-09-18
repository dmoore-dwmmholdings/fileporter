import Foundation

// Mirrors the serde JSON emitted by AppState::snapshot in src-tauri/src/state.rs
// and identity.rs — the same contract src/types/view-models.ts decodes.

nonisolated struct AppSnapshot: Decodable, Equatable, Sendable {
    var revision: UInt64
    var lifecycle: Lifecycle
    var settings: SettingsSnapshot
    var localDeviceName: String
    var devices: [DevicePresence]
    var nearbyDevices: [NearbyDevice]
    var transfers: [TransferBatch]
    var history: [HistoryEntry]
    var queuedBatches: [QueuedBatch]
    var pairing: PairingSnapshot
    var network: NetworkDiagnostics
    var about: About
}

nonisolated struct Lifecycle: Decodable, Equatable, Sendable {
    var receivingEnabled: Bool
    var listening: Bool
    var receiving: Bool
    var boundEndpoint: String?
    var shuttingDown: Bool
}

nonisolated struct SettingsSnapshot: Decodable, Equatable, Sendable {
    var deviceName: String
    var receiveDirectory: String?
    var onboardingComplete: Bool
    var notificationsEnabled: Bool
    var automaticDeviceTrust: Bool
    var receivingEnabled: Bool
    var preferredListenAddress: String
    var preferredListenPort: Int
    var historyRetentionDays: Int
}

nonisolated struct NetworkDiagnostics: Decodable, Equatable, Sendable {
    var listening: Bool
    var boundEndpoint: String?
    var preferredListenAddress: String
    var trustedOnlineEndpoints: [String]
    var mdnsState: String
    var localInterfaceSummaries: [String]
    var recentErrorCodes: [String]
}

nonisolated struct About: Decodable, Equatable, Sendable {
    var appVersion: String
    var protocolVersion: Int
    var databaseMigrationVersion: Int
    var ownedStagingBytes: Int64
}

nonisolated struct DevicePresence: Decodable, Equatable, Sendable {
    var id: String
    var name: String
    var state: String
    var lastSeenAt: Int64?
}

nonisolated struct NearbyDevice: Decodable, Equatable, Identifiable, Sendable {
    var deviceId: String
    var displayName: String
    var endpoint: String
    var certificateFingerprint: String
    var id: String { deviceId }
}

/// The backend serialises the state it persisted ("completed", "receiving");
/// normalise once at the boundary, exactly as the desktop view models do.
nonisolated enum BatchState: String, Sendable {
    case queued, waiting, preparing, sending, receiving, verifying, complete, partial, paused, cancelled, failed

    init(raw: String) {
        self = raw == "completed" ? .complete : BatchState(rawValue: raw) ?? .failed
    }

    /// The design's plain-language reading of a batch state.
    var word: String {
        switch self {
        case .queued: "Queued"
        case .waiting: "Held"
        case .preparing: "Preparing"
        case .sending: "Sending"
        case .receiving: "Receiving"
        case .verifying: "Verifying"
        case .complete: "Verified"
        case .partial: "Partial"
        case .paused: "Paused"
        case .cancelled: "Cancelled"
        case .failed: "Failed"
        }
    }

    var inFlight: Bool { [.preparing, .sending, .receiving, .verifying].contains(self) }
    var isWarning: Bool { [.waiting, .queued, .paused, .partial].contains(self) }
    var isBad: Bool { [.failed, .cancelled].contains(self) }
}

nonisolated struct TransferBatch: Decodable, Equatable, Identifiable, Sendable {
    var id: String
    var label: String
    var rawState: String
    var progress: Int
    var targets: [TransferTarget]
    var state: BatchState { BatchState(raw: rawState) }

    enum CodingKeys: String, CodingKey {
        case id, label, progress, targets
        case rawState = "state"
    }
}

nonisolated struct TransferTarget: Decodable, Equatable, Identifiable, Sendable {
    var id: String
    var deviceName: String
    var rawState: String
    var progress: Int
    var rateLabel: String?

    enum CodingKeys: String, CodingKey {
        case id, deviceName, progress, rateLabel
        case rawState = "state"
    }
}

nonisolated struct HistoryEntry: Decodable, Equatable, Identifiable, Sendable {
    var id: String
    var direction: String
    var peerName: String
    var summary: String
    var timeLabel: String
    var rawState: String
    var items: [HistoryItem]
    var state: BatchState { BatchState(raw: rawState) }
    var incoming: Bool { direction == "incoming" }

    enum CodingKeys: String, CodingKey {
        case id, direction, peerName, summary, timeLabel, items
        case rawState = "state"
    }
}

nonisolated struct HistoryItem: Decodable, Equatable, Identifiable, Sendable {
    var itemId: String
    var displayName: String
    var kind: String
    /// Bytes: for a folder, everything inside it.
    var size: Int64
    /// Files inside a folder; absent for a file.
    var itemCount: Int64?
    var rawState: String
    var available: Bool
    var destinationLabel: String?
    var id: String { itemId }
    var state: BatchState { BatchState(raw: rawState) }

    enum CodingKeys: String, CodingKey {
        case itemId, displayName, kind, size, itemCount, available, destinationLabel
        case rawState = "state"
    }
}

nonisolated struct QueuedBatch: Decodable, Equatable, Identifiable, Sendable {
    var id: String
    var itemCount: Int
    var targetDeviceIds: [String]
    var state: String
    var waitingForAvailable: Bool
}

nonisolated struct PendingPairing: Decodable, Equatable, Identifiable, Sendable {
    var id: String
    var deviceId: String
    var remoteName: String
    var certificateFingerprint: String
    var expiresAt: Int64
    var localConfirmed: Bool
    var remoteConfirmed: Bool
    var sasCode: String?
}

nonisolated struct TrustedDevice: Decodable, Equatable, Sendable {
    var deviceId: String
    var name: String
    var alias: String?
    var pairedAt: Int64
    var lastSeenAt: Int64?
    var certificateFingerprintShort: String
    var autoSend: Bool
    var endpoint: String?
}

nonisolated struct PairingSnapshot: Decodable, Equatable, Sendable {
    var localDeviceId: String
    var pendingPairings: [PendingPairing]
    var trustedDevices: [TrustedDevice]
}

nonisolated struct QueuedBatchReply: Decodable, Sendable {
    var id: String
    var itemCount: Int
}

/// A linked pad as every screen presents it: its local name and whether it is
/// reachable right now.
nonisolated struct Pad: Identifiable, Equatable, Sendable {
    var id: String
    var name: String
    var online: Bool
    var lastSeenAt: Int64?
    var fingerprintShort: String
    var autoSend: Bool
    var endpoint: String?
}

nonisolated extension AppSnapshot {
    var pads: [Pad] {
        pairing.trustedDevices.map { device in
            Pad(
                id: device.deviceId,
                name: device.alias ?? device.name,
                online: devices.first { $0.id == device.deviceId }?.state == "online",
                lastSeenAt: device.lastSeenAt,
                fingerprintShort: device.certificateFingerprintShort,
                autoSend: device.autoSend,
                endpoint: device.endpoint
            )
        }
    }

    var activeTransfers: [TransferBatch] { transfers.filter { $0.state.inFlight } }

    var listening: Bool { lifecycle.listening || network.listening }
}
