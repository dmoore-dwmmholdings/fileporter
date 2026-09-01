import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import App from './App';
import { appBridge } from './lib/bridge';
import type { BackendAppSnapshot } from './types/view-models';

const onboardingSnapshot: BackendAppSnapshot = {
  revision: 1,
  lifecycle: { windowVisible: true, receivingEnabled: true, listening: false, receiving: false, shuttingDown: false },
  settings: { deviceName: '', receiveDirectory: null, onboardingComplete: false, launchAtLogin: true, notificationsEnabled: true, receivingEnabled: true, automaticDeviceTrust: true, preferredListenAddress: '0.0.0.0:0', preferredListenPort: 0, historyRetentionDays: 30 },
  managersStarted: false, localDeviceName: '', devices: [], nearbyDevices: [], transfers: [], history: [], queuedBatches: [], pairing: { localDeviceId: 'local', pendingPairings: [], trustedDevices: [] }, network: { listening: false, boundEndpoint: null, preferredListenAddress: '0.0.0.0:0', trustedOnlineEndpoints: [], mdnsState: 'stopped', localInterfaceSummaries: [], recentErrorCodes: [] }, about: { appVersion: '0.1.0', protocolVersion: 1, logsAvailable: true, databaseMigrationVersion: 8, ownedStagingBytes: 0 }
};
const readySnapshot: BackendAppSnapshot = {
  revision: 2,
  lifecycle: { windowVisible: true, receivingEnabled: true, listening: false, receiving: false, shuttingDown: false },
  settings: { deviceName: 'Desk', receiveDirectory: 'C:\\Fileporter', onboardingComplete: true, launchAtLogin: true, notificationsEnabled: true, receivingEnabled: true, automaticDeviceTrust: true, preferredListenAddress: '127.0.0.1:48721', preferredListenPort: 48721, historyRetentionDays: 30 },
  managersStarted: true, localDeviceName: 'Desk', devices: [], nearbyDevices: [], transfers: [], history: [], queuedBatches: [], pairing: { localDeviceId: 'local', pendingPairings: [], trustedDevices: [] }, network: { listening: true, boundEndpoint: '127.0.0.1:48721', preferredListenAddress: '127.0.0.1:48721', trustedOnlineEndpoints: [], mdnsState: 'advertising', localInterfaceSummaries: ['Wi-Fi: 192.168.1.8'], recentErrorCodes: [] }, about: { appVersion: '0.1.0', protocolVersion: 1, logsAvailable: true, databaseMigrationVersion: 8, ownedStagingBytes: 0 }
};

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(appBridge, 'listenForNativeDrop').mockResolvedValue(() => undefined);
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockResolvedValue(() => undefined);
});

it('hydrates the screen from the Tauri snapshot', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  expect(await screen.findByText('Desk')).toBeVisible();
  expect(screen.getByText(/Transfers you send/)).toBeVisible();
});

it('shows an actionable error when snapshot loading fails', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockRejectedValue(new Error('offline'));
  render(<App />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Fileporter couldn’t load.');
});

it('sends to a device that only comes online after launch, without asking first', async () => {
  // The app almost always finishes loading before discovery finds anyone, so a
  // recipient set captured once at startup is empty forever and every drop is
  // met with "choose a device" instead of a transfer.
  let receiveSnapshot: ((snapshot: BackendAppSnapshot) => void) | undefined;
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockImplementation(async (handler) => { receiveSnapshot = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['peer-1'], state: 'queued', waitingForAvailable: false });
  render(<App />);
  expect(await screen.findByText('No devices online')).toBeVisible();

  await act(async () => {
    receiveSnapshot?.({ ...readySnapshot, revision: 3, devices: [{ id: 'peer-1', name: 'DWMM Gaming', state: 'online', lastSeenAt: 10 }] });
  });
  expect(await screen.findByRole('button', { name: 'DWMM Gaming' })).toBeVisible();

  fireEvent.click(screen.getByRole('button', { name: 'Send something' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: /Browse files/ }));

  await waitFor(() => expect(enqueue).toHaveBeenCalledWith(['C:\\report.pdf'], ['peer-1']));
});

it('keeps a recipient the user deselected even as snapshots arrive', async () => {
  let receiveSnapshot: ((snapshot: BackendAppSnapshot) => void) | undefined;
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockImplementation(async (handler) => { receiveSnapshot = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...readySnapshot, devices: [{ id: 'peer-1', name: 'DWMM Gaming', state: 'online', lastSeenAt: 10 }] });
  render(<App />);
  const chip = await screen.findByRole('button', { name: 'DWMM Gaming' });
  fireEvent.click(chip);

  await act(async () => {
    receiveSnapshot?.({ ...readySnapshot, revision: 4, devices: [{ id: 'peer-1', name: 'DWMM Gaming', state: 'online', lastSeenAt: 12 }] });
  });

  expect(screen.getByRole('button', { name: 'DWMM Gaming' }).className).not.toContain('active');
});

it('discards an older snapshot event after a newer snapshot', async () => {
  let receiveSnapshot: ((snapshot: BackendAppSnapshot) => void) | undefined;
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockImplementation(async (handler) => { receiveSnapshot = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  expect(await screen.findByText('Desk')).toBeVisible();
  receiveSnapshot?.({ ...readySnapshot, revision: 3, localDeviceName: 'Laptop', settings: { ...readySnapshot.settings, deviceName: 'Laptop' } });
  expect(await screen.findByText('Laptop')).toBeVisible();
  receiveSnapshot?.({ ...readySnapshot, revision: 2, localDeviceName: 'Old name', settings: { ...readySnapshot.settings, deviceName: 'Old name' } });
  await waitFor(() => expect(screen.queryByText('Old name')).not.toBeInTheDocument());
});

it('opens and focuses Settings from the tray navigation event', async () => {
  let navigate: ((destination: 'settings') => void) | undefined;
  vi.spyOn(appBridge, 'listenForNavigation').mockImplementation(async (handler) => { navigate = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  await screen.findByText('Desk');
  navigate?.('settings');
  expect(await screen.findByRole('heading', { name: 'Settings' })).toBeVisible();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Back' })).toHaveFocus());
});

it('validates onboarding, chooses a folder, and completes setup', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(onboardingSnapshot);
  const choose = vi.spyOn(appBridge, 'chooseDirectory').mockResolvedValue(['C:\\Incoming']);
  const complete = vi.spyOn(appBridge, 'completeOnboarding').mockResolvedValue({ ...readySnapshot, revision: 3, localDeviceName: 'Studio Mac', settings: { ...readySnapshot.settings, deviceName: 'Studio Mac', receiveDirectory: 'C:\\Incoming' } });
  render(<App />);
  expect(await screen.findByText(/Send files directly/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Finish setup' })).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Device name'), { target: { value: 'Studio Mac' } });
  fireEvent.click(screen.getByRole('button', { name: 'Choose' }));
  await waitFor(() => expect(choose).toHaveBeenCalledOnce());
  expect(await screen.findByDisplayValue('C:\\Incoming')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Finish setup' }));
  await waitFor(() => expect(complete).toHaveBeenCalledWith({ deviceName: 'Studio Mac', receiveDirectory: 'C:\\Incoming', launchAtLogin: true, notificationsEnabled: true, automaticDeviceTrust: true }));
  expect(await screen.findByText('Studio Mac')).toBeVisible();
});

it('offers separate file and folder pickers from the home magic control', async () => {
  const snapshot = { ...readySnapshot, devices: [{ id: 'laptop', name: 'Laptop', state: 'online' as const }] };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
  const chooseFiles = vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'preparing', waitingForAvailable: false });
  render(<App />);
  expect(await screen.findByText('Laptop')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Send something' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
  await waitFor(() => expect(chooseFiles).toHaveBeenCalledOnce());
  expect(enqueue).toHaveBeenCalledWith(['C:\\report.pdf'], ['laptop']);
});

it('keeps the drop surface active until nested drag leaves have all completed', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  const surface = await screen.findByLabelText('Send files or folders');
  fireEvent.dragEnter(surface);
  fireEvent.dragEnter(surface);
  expect(surface).toHaveClass('drop-active');
  fireEvent.dragLeave(surface);
  expect(surface).toHaveClass('drop-active');
  fireEvent.dragLeave(surface);
  expect(surface).not.toHaveClass('drop-active');
});

it('enqueues a native drop once with the recipients selected when it arrived', async () => {
  let receiveDrop: ((payload: { paths: string[] }) => void) | undefined;
  vi.spyOn(appBridge, 'listenForNativeDrop').mockImplementation(async (handler) => { receiveDrop = handler; return () => undefined; });
  const snapshot = { ...readySnapshot, devices: [{ id: 'laptop', name: 'Laptop', state: 'online' as const }] };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'preparing', waitingForAvailable: false });
  render(<App />);
  expect(await screen.findByText('Laptop')).toBeVisible();
  await waitFor(() => expect(receiveDrop).toBeTypeOf('function'));
  receiveDrop?.({ paths: ['C:\\drop\\photo.jpg'] });
  fireEvent.click(screen.getByRole('button', { name: 'Laptop' }));
  await waitFor(() => expect(enqueue).toHaveBeenCalledOnce());
  expect(enqueue).toHaveBeenCalledWith(['C:\\drop\\photo.jpg'], ['laptop']);
});

it('stages a native drop without a recipient and queues it for a chosen trusted offline device', async () => {
  let receiveDrop: ((payload: { paths: string[] }) => void) | undefined;
  vi.spyOn(appBridge, 'listenForNativeDrop').mockImplementation(async (handler) => { receiveDrop = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...readySnapshot, pairing: { ...readySnapshot.pairing, trustedDevices: [{ deviceId: 'laptop', name: 'Laptop', alias: null, pairedAt: 1, lastSeenAt: 1, certificateFingerprintShort: 'ABCD', autoSend: true, endpoint: '192.168.1.4:48721' }] } });
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'queued', waitingForAvailable: true });
  render(<App />);
  await screen.findByText('Recent activity');
  receiveDrop?.({ paths: ['C:\\drop\\photo.jpg'] });
  expect(await screen.findByText('1 item ready to send')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Laptop' }));
  fireEvent.click(screen.getByRole('button', { name: 'Send when available' }));
  await waitFor(() => expect(enqueue).toHaveBeenCalledWith(['C:\\drop\\photo.jpg'], ['laptop'], true));
});

it('stages picker paths when no recipient is selected', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  const chooseFiles = vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths');
  render(<App />);
  await screen.findByText('Recent activity');
  fireEvent.click(screen.getByRole('button', { name: 'Send something' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
  await waitFor(() => expect(chooseFiles).toHaveBeenCalledOnce());
  expect(await screen.findByText('1 item ready to send')).toBeVisible();
  expect(enqueue).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Send when available' })).toBeDisabled();
});

it('cancels the snackbar transfer using its returned batch ID', async () => {
  const snapshot = { ...readySnapshot, devices: [{ id: 'laptop', name: 'Laptop', state: 'online' as const }] };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
  vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'stored-batch-id', itemCount: 1, targetDeviceIds: ['laptop'], state: 'queued', waitingForAvailable: false });
  const cancelBatch = vi.spyOn(appBridge, 'cancelBatch').mockResolvedValue({ ...snapshot, revision: 3 });
  render(<App />);
  await screen.findByText('Laptop');
  fireEvent.click(screen.getByRole('button', { name: 'Send something' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(cancelBatch).toHaveBeenCalledWith('stored-batch-id'));
});

it('announces a queued transfer, focuses its cancellation action, and expires it after five seconds', async () => {
  vi.useFakeTimers();
  try {
    const snapshot = { ...readySnapshot, devices: [{ id: 'laptop', name: 'Laptop', state: 'online' as const }] };
    vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
    vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
    vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'queued', waitingForAvailable: false });
    render(<App />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    fireEvent.click(screen.getByRole('button', { name: 'Send something' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toHaveFocus();
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
  } finally { vi.useRealTimers(); }
});

it('saves every editable settings field through the supported patch DTO', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  vi.spyOn(appBridge, 'chooseReceiveDirectory').mockResolvedValue('C:\\Incoming');
  const update = vi.spyOn(appBridge, 'updateSettings').mockResolvedValue({ ...readySnapshot, revision: 3, lifecycle: { ...readySnapshot.lifecycle, receivingEnabled: false }, settings: { ...readySnapshot.settings, deviceName: 'Studio', receiveDirectory: 'C:\\Incoming', launchAtLogin: false, notificationsEnabled: false }, localDeviceName: 'Studio' });
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Open settings' }));
  fireEvent.change(screen.getByLabelText('Device name'), { target: { value: 'Studio' } });
  fireEvent.click(screen.getByRole('button', { name: 'Choose' }));
  await waitFor(() => expect(screen.getByDisplayValue('C:\\Incoming')).toBeVisible());
  fireEvent.click(screen.getByLabelText('Accept incoming transfers'));
  fireEvent.click(screen.getByLabelText('Launch Fileporter when I sign in'));
  fireEvent.click(screen.getByLabelText('Notify me when files arrive'));
  fireEvent.click(screen.getByLabelText('Automatically trust authenticated Fileporter devices on this private network'));
  fireEvent.click(screen.getByRole('button', { name: 'Save settings' }));
  await waitFor(() => expect(update).toHaveBeenCalledWith({ deviceName: 'Studio', receiveDirectory: 'C:\\Incoming', receivingEnabled: false, listenAddress: '127.0.0.1:48721', launchAtLogin: false, notificationsEnabled: false, automaticDeviceTrust: false, historyRetentionDays: 30 }));
});

it('shows snapshot-backed diagnostics and about actions without path input', async () => {
  const snapshot = { ...readySnapshot, network: { ...readySnapshot.network, listening: true, boundEndpoint: '192.168.1.8:48721', trustedOnlineEndpoints: ['192.168.1.9:48721'], mdnsState: 'advertising', localInterfaceSummaries: ['Wi-Fi: 192.168.1.8'], recentErrorCodes: ['network_timeout'] }, about: { ...readySnapshot.about, databaseMigrationVersion: 8, ownedStagingBytes: 1024 }, pairing: { ...readySnapshot.pairing, trustedDevices: [{ deviceId: 'laptop', name: 'Laptop', alias: null, pairedAt: 1, lastSeenAt: 2, certificateFingerprintShort: 'ABCD', autoSend: true, endpoint: '192.168.1.9:48721' }] } };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
  const viewLogs = vi.spyOn(appBridge, 'viewLogs').mockResolvedValue();
  const exportLogs = vi.spyOn(appBridge, 'exportLogs').mockResolvedValue('C:\\Exports');
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Open settings' }));
  fireEvent.click(screen.getByText('Network diagnostics'));
  expect(screen.getByText('192.168.1.9:48721')).toBeVisible();
  fireEvent.click(screen.getByText('About Fileporter'));
  expect(screen.getByText(/Version 0.1.0/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'View logs' }));
  await waitFor(() => expect(viewLogs).toHaveBeenCalledWith());
  fireEvent.click(screen.getByRole('button', { name: 'Export logs' }));
  await waitFor(() => expect(exportLogs).toHaveBeenCalledWith());
  expect(await screen.findByRole('status')).toHaveTextContent('Exported redacted diagnostics');
});

it('renders safe expanded diagnostics from the snapshot', async () => {
  const snapshot = { ...readySnapshot, network: { ...readySnapshot.network, mdnsState: 'advertising', localInterfaceSummaries: ['Wi-Fi: 192.168.1.8'], recentErrorCodes: ['network_timeout'] }, about: { ...readySnapshot.about, databaseMigrationVersion: 8, ownedStagingBytes: 1024 } };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Open settings' }));
  fireEvent.click(screen.getByText('Network diagnostics'));
  expect(screen.getByText('advertising')).toBeVisible();
  expect(screen.getByText('Wi-Fi: 192.168.1.8')).toBeVisible();
  expect(screen.getByText('network_timeout')).toBeVisible();
  fireEvent.click(screen.getByText('About Fileporter'));
  expect(screen.getByText(/Database migration 8/)).toBeVisible();
  expect(screen.getByText(/Staging 1,024 bytes/)).toBeVisible();
});

it('announces offline and listening header state from real snapshot counts', async () => {
  const offline = { ...readySnapshot, lifecycle: { ...readySnapshot.lifecycle, listening: false }, network: { ...readySnapshot.network, listening: false } };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(offline);
  const view = render(<App />);
  expect(await screen.findByLabelText(/Offline; 0 trusted devices online; 0 queued or waiting; 0 active transfers/)).toBeVisible();
  const listening = { ...readySnapshot, lifecycle: { ...readySnapshot.lifecycle, listening: true }, devices: [{ id: 'laptop', name: 'Laptop', state: 'online' as const }], pairing: { ...readySnapshot.pairing, trustedDevices: [{ deviceId: 'laptop', name: 'Laptop', alias: null, pairedAt: 1, lastSeenAt: 1, certificateFingerprintShort: 'ABCD', autoSend: true, endpoint: null }] }, transfers: [{ id: 'active', label: 'Report', state: 'sending' as const, progress: 50, targets: [] }, { id: 'wait', label: 'Photos', state: 'waiting' as const, progress: 0, targets: [] }], queuedBatches: [{ id: 'queue', itemCount: 1, targetDeviceIds: ['laptop'], state: 'queued', waitingForAvailable: true }] };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(listening);
  view.unmount();
  render(<App />);
  expect(await screen.findByLabelText(/Listening and receiving; 1 trusted device online; 2 queued or waiting; 1 active transfer/)).toBeVisible();
});

it('keeps settings save disabled for an invalid preferred listen address', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Open settings' }));
  fireEvent.change(screen.getByLabelText('Preferred listen address'), { target: { value: 'not-an-endpoint' } });
  expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
});
