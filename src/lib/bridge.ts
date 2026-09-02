import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { BackendAppSnapshot, PendingPairing, QueuedBatch, TrustedDevice } from '../types/view-models';

export type NativeDropPayload = { paths: string[] };
export type SnapshotChangedPayload = BackendAppSnapshot;

export interface CompleteOnboardingInput {
  deviceName: string;
  receiveDirectory: string;
  launchAtLogin?: boolean;
  notificationsEnabled?: boolean;
  automaticDeviceTrust?: boolean;
}
export interface UpdateSettingsInput { deviceName?: string; receiveDirectory?: string; receivingEnabled?: boolean; listenAddress?: string; launchAtLogin?: boolean; notificationsEnabled?: boolean; automaticDeviceTrust?: boolean; historyRetentionDays?: 0 | 7 | 30 | 90; }
export interface RequestPairingInput { remoteName: string; publicKey: number[]; certificateFingerprint: string; }
export interface StartPairingAtEndpointInput { endpoint: string; }
export interface PairingConfirmation { pairing: PendingPairing; trustedDevice: TrustedDevice | null; }

export const appBridge = {
  getAppSnapshot: () => invoke<BackendAppSnapshot>('get_app_snapshot'),
  chooseFiles: () => invoke<string[]>('choose_files'),
  chooseDirectory: () => invoke<string[]>('choose_directory'),
  chooseReceiveDirectory: () => invoke<string | null>('choose_receive_directory'),
  viewLogs: () => invoke<void>('view_logs'),
  exportLogs: () => invoke<string | null>('export_logs'),
  completeOnboarding: (input: CompleteOnboardingInput) => invoke<BackendAppSnapshot>('complete_onboarding', { input }),
  updateSettings: (input: UpdateSettingsInput) => invoke<BackendAppSnapshot>('update_settings', { input }),
  requestPairing: (input: RequestPairingInput) => invoke<PendingPairing>('request_pairing', { input }),
  startPairingAtEndpoint: (endpoint: string) => invoke<PendingPairing>('start_pairing_at_endpoint', { input: { endpoint } }),
  startPairingDiscovered: (deviceId: string) => invoke<PendingPairing>('start_pairing_discovered', { input: { deviceId } }),
  renameTrustedDevice: (deviceId: string, alias: string) => invoke<void>('rename_trusted_device', { input: { deviceId, alias } }),
  confirmPairing: (pairingId: string) => invoke<PairingConfirmation>('confirm_pairing', { input: { pairingId } }),
  rejectPairing: (pairingId: string) => invoke<void>('reject_pairing', { input: { pairingId } }),
  enqueuePaths: (paths: string[], targetDeviceIds: string[], queueOffline = false) =>
    invoke<QueuedBatch>('enqueue_paths', { input: { paths, targetDeviceIds, queueOffline } }),
  cancelBatch: (batchId: string) => invoke<BackendAppSnapshot>('cancel_batch', { input: { batchId } }),
  retryBatch: (batchId: string) => invoke<BackendAppSnapshot>('retry_batch', { input: { batchId } }),
  // Native actions receive only durable IDs. The Rust side resolves and validates
  // completed outputs; the webview never supplies a filesystem path.
  revealCompletedBatch: (batchId: string) => invoke<void>('reveal_completed_batch', { input: { batchId } }),
  copyCompletedBatch: (batchId: string) => invoke<void>('copy_completed_batch', { input: { batchId } }),
  moveCompletedBatch: (batchId: string) => invoke<void>('move_completed_batch', { input: { batchId } }),
  revealItem: (itemId: string) => invoke<void>('reveal_item', { input: { itemId } }),
  copyItem: (itemId: string) => invoke<void>('copy_item', { input: { itemId } }),
  moveItem: (itemId: string) => invoke<void>('move_item', { input: { itemId } }),
  showMainWindow: () => invoke<void>('show_main_window'),
  quitApp: () => invoke<void>('quit_app'),
  listenForNativeDrop: (handler: (payload: NativeDropPayload) => void): Promise<UnlistenFn> =>
    listen<NativeDropPayload>('tauri://drag-drop', (event) => handler(event.payload)),
  listenForSnapshotChanges: (handler: (payload: SnapshotChangedPayload) => void): Promise<UnlistenFn> =>
    listen<SnapshotChangedPayload>('app://snapshot-changed', (event) => handler(event.payload)),
  listenForNavigation: (handler: (destination: 'settings') => void): Promise<UnlistenFn> =>
    listen<'settings'>('app://navigate', (event) => handler(event.payload))
};
