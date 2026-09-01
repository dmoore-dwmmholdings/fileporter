// Exact values emitted by AppState::snapshot in the Rust backend.
export type DeviceState = 'online' | 'offline';
export type BatchState = 'queued' | 'waiting' | 'preparing' | 'sending' | 'verifying' | 'complete' | 'partial' | 'paused' | 'failed';
export type HistoryDirection = 'incoming' | 'outgoing';

// Mirrors the serde JSON emitted by src-tauri/src/state.rs and identity.rs.
export interface LifecycleSnapshot { windowVisible: boolean; receivingEnabled: boolean; listening: boolean; receiving: boolean; boundEndpoint?: string; shuttingDown: boolean; }
export interface SettingsSnapshot { deviceName: string; receiveDirectory: string | null; onboardingComplete: boolean; launchAtLogin: boolean; notificationsEnabled: boolean; receivingEnabled: boolean; automaticDeviceTrust: boolean; preferredListenAddress: string; preferredListenPort: number; historyRetentionDays: number; }
export interface NetworkDiagnostics { listening: boolean; boundEndpoint: string | null; preferredListenAddress: string; trustedOnlineEndpoints: string[]; mdnsState: string; localInterfaceSummaries: string[]; recentErrorCodes: string[]; }
export interface AboutSnapshot { appVersion: string; protocolVersion: number; logsAvailable: boolean; databaseMigrationVersion: number; ownedStagingBytes: number; }
export interface BackendDeviceViewModel { id: string; name: string; state: DeviceState; lastSeenAt?: number; }
export interface TransferTargetViewModel { id: string; deviceName: string; state: BatchState; progress: number; rateLabel?: string; }
export interface TransferBatchViewModel { id: string; label: string; state: BatchState; progress: number; targets: TransferTargetViewModel[]; }
export interface NearbyDeviceViewModel { deviceId: string; displayName: string; endpoint: string; certificateFingerprint: string; protocolVersion: number; capabilities: string[]; }
export interface HistoryTopLevelItemViewModel { itemId: string; displayName: string; kind: string; size: number; state: BatchState; available: boolean; destinationLabel?: string; }
export interface HistoryItemViewModel { id: string; direction: HistoryDirection; peerName: string; summary: string; timeLabel: string; state: BatchState; items: HistoryTopLevelItemViewModel[]; }
export interface QueuedBatch { id: string; itemCount: number; targetDeviceIds: string[]; state: string; waitingForAvailable: boolean; }
export interface PendingPairing { id: string; deviceId: string; remoteName: string; certificateFingerprint: string; expiresAt: number; localConfirmed: boolean; remoteConfirmed: boolean; sasCode: string | null; }
export interface TrustedDevice { deviceId: string; name: string; alias: string | null; pairedAt: number; lastSeenAt: number | null; certificateFingerprintShort: string; autoSend: boolean; endpoint: string | null; }
export interface PairingSnapshot { localDeviceId: string; pendingPairings: PendingPairing[]; trustedDevices: TrustedDevice[]; }
export interface BackendAppSnapshot {
  revision: number; lifecycle: LifecycleSnapshot; settings: SettingsSnapshot; managersStarted: boolean; localDeviceName: string;
  devices: BackendDeviceViewModel[]; nearbyDevices: NearbyDeviceViewModel[]; transfers: TransferBatchViewModel[]; history: HistoryItemViewModel[]; queuedBatches: QueuedBatch[]; pairing: PairingSnapshot; network: NetworkDiagnostics; about: AboutSnapshot;
}
export interface TrustedDeviceViewModel { id: string; name: string; lastSeenAt: number | null; state: DeviceState; certificateFingerprintShort: string; autoSend: boolean; endpoint: string | null; }
export interface AppSnapshotViewModel {
  revision: number; localDeviceName: string; onboardingComplete: boolean; receiveDirectory: string | null; launchAtLogin: boolean; notificationsEnabled: boolean;
  lifecycle: LifecycleSnapshot; settings: SettingsSnapshot; network: NetworkDiagnostics; about: AboutSnapshot; devices: BackendDeviceViewModel[]; nearbyDevices: NearbyDeviceViewModel[]; trustedDevices: TrustedDeviceViewModel[]; pendingPairings: PendingPairing[]; transfers: TransferBatchViewModel[]; history: HistoryItemViewModel[]; queuedBatches: QueuedBatch[];
}
export const emptySnapshot: AppSnapshotViewModel = {
  revision: 0, localDeviceName: 'This device', onboardingComplete: false, receiveDirectory: null, launchAtLogin: true, notificationsEnabled: true,
  lifecycle: { windowVisible: false, receivingEnabled: true, listening: false, receiving: false, shuttingDown: false }, settings: { deviceName: '', receiveDirectory: null, onboardingComplete: false, launchAtLogin: true, notificationsEnabled: true, receivingEnabled: true, automaticDeviceTrust: true, preferredListenAddress: '127.0.0.1:0', preferredListenPort: 0, historyRetentionDays: 30 }, network: { listening: false, boundEndpoint: null, preferredListenAddress: '127.0.0.1:0', trustedOnlineEndpoints: [], mdnsState: 'unknown', localInterfaceSummaries: [], recentErrorCodes: [] }, about: { appVersion: '0.1.0', protocolVersion: 1, logsAvailable: false, databaseMigrationVersion: 0, ownedStagingBytes: 0 }, devices: [], nearbyDevices: [], trustedDevices: [], pendingPairings: [], transfers: [], history: [], queuedBatches: []
};
export function toViewModel(snapshot: BackendAppSnapshot): AppSnapshotViewModel {
  return {
    revision: snapshot.revision, localDeviceName: snapshot.localDeviceName || snapshot.settings.deviceName || 'This device', onboardingComplete: snapshot.settings.onboardingComplete,
    receiveDirectory: snapshot.settings.receiveDirectory, launchAtLogin: snapshot.settings.launchAtLogin, notificationsEnabled: snapshot.settings.notificationsEnabled,
    lifecycle: snapshot.lifecycle, settings: snapshot.settings, network: snapshot.network, about: snapshot.about, devices: snapshot.devices, nearbyDevices: snapshot.nearbyDevices,
    trustedDevices: snapshot.pairing.trustedDevices.map((device) => ({ id: device.deviceId, name: device.alias ?? device.name, lastSeenAt: device.lastSeenAt, certificateFingerprintShort: device.certificateFingerprintShort, autoSend: device.autoSend, endpoint: device.endpoint, state: snapshot.devices.find((presence) => presence.id === device.deviceId)?.state ?? 'offline' })),
    pendingPairings: snapshot.pairing.pendingPairings, transfers: snapshot.transfers, history: snapshot.history, queuedBatches: snapshot.queuedBatches
  };
}
