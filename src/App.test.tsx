import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import App from './App';
import { appBridge } from './lib/bridge';
import type { BackendAppSnapshot, TrustedDevice } from './types/view-models';

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

function trusted(id: string, name: string, extra: Partial<TrustedDevice> = {}): TrustedDevice {
  return { deviceId: id, name, alias: null, pairedAt: 1, lastSeenAt: 1, certificateFingerprintShort: 'ABCD', autoSend: true, endpoint: '192.168.1.4:48721', ...extra };
}

/** A snapshot whose pad is both linked and currently online. */
function withPad(id: string, name: string, online = true): BackendAppSnapshot {
  return {
    ...readySnapshot,
    devices: online ? [{ id, name, state: 'online' as const }] : [],
    pairing: { ...readySnapshot.pairing, trustedDevices: [trusted(id, name)] }
  };
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(appBridge, 'listenForNativeDrop').mockResolvedValue(() => undefined);
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockResolvedValue(() => undefined);
});

it('hydrates the screen from the Tauri snapshot', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  expect(await screen.findByText('Desk')).toBeVisible();
  expect(screen.getByRole('heading', { name: 'Drop anything.' })).toBeVisible();
});

it('shows an actionable error when snapshot loading fails', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockRejectedValue(new Error('offline'));
  render(<App />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Fileporter couldn’t load.');
  expect(screen.getByRole('button', { name: 'Try again' })).toBeVisible();
});

it('sends to a pad that only comes online after launch, without asking first', async () => {
  // The app almost always finishes loading before discovery finds anyone, so a
  // recipient set captured once at startup is empty forever and every drop is
  // met with "pick a pad" instead of a transfer.
  let receiveSnapshot: ((snapshot: BackendAppSnapshot) => void) | undefined;
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockImplementation(async (handler) => { receiveSnapshot = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['peer-1'], state: 'queued', waitingForAvailable: false });
  render(<App />);
  expect(await screen.findByText('No pad linked')).toBeVisible();

  await act(async () => {
    receiveSnapshot?.({
      ...readySnapshot, revision: 3,
      devices: [{ id: 'peer-1', name: 'DWMM Gaming', state: 'online', lastSeenAt: 10 }],
      pairing: { ...readySnapshot.pairing, trustedDevices: [trusted('peer-1', 'DWMM Gaming')] }
    });
  });
  expect(await screen.findByRole('button', { name: 'DWMM Gaming' })).toBeVisible();

  fireEvent.click(screen.getByRole('button', { name: 'Send files or folders' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Browse files' }));

  await waitFor(() => expect(enqueue).toHaveBeenCalledWith(['C:\\report.pdf'], ['peer-1'], false));
});

it('keeps a pad the user deselected even as snapshots arrive', async () => {
  let receiveSnapshot: ((snapshot: BackendAppSnapshot) => void) | undefined;
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockImplementation(async (handler) => { receiveSnapshot = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(withPad('peer-1', 'DWMM Gaming'));
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'DWMM Gaming' }));

  await act(async () => {
    receiveSnapshot?.({ ...withPad('peer-1', 'DWMM Gaming'), revision: 4 });
  });

  expect(screen.getByRole('button', { name: 'DWMM Gaming' })).toHaveAttribute('aria-pressed', 'false');
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

it('opens Config from the tray navigation event', async () => {
  let navigate: ((destination: 'settings') => void) | undefined;
  vi.spyOn(appBridge, 'listenForNavigation').mockImplementation(async (handler) => { navigate = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  await screen.findByText('Desk');
  act(() => { navigate?.('settings'); });
  expect(await screen.findByRole('heading', { name: 'This pad.' })).toBeVisible();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Config' })).toHaveFocus());
});

it('validates first run, chooses a folder, and completes setup', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(onboardingSnapshot);
  const choose = vi.spyOn(appBridge, 'chooseDirectory').mockResolvedValue(['C:\\Incoming']);
  const complete = vi.spyOn(appBridge, 'completeOnboarding').mockResolvedValue({ ...readySnapshot, revision: 3, localDeviceName: 'Studio Mac', settings: { ...readySnapshot.settings, deviceName: 'Studio Mac', receiveDirectory: 'C:\\Incoming' } });
  render(<App />);
  expect(await screen.findByRole('heading', { name: 'Set up this pad.' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Bring this pad online' })).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Name this pad'), { target: { value: 'Studio Mac' } });
  fireEvent.click(screen.getByRole('button', { name: 'Choose' }));
  await waitFor(() => expect(choose).toHaveBeenCalledOnce());
  expect(await screen.findByDisplayValue('C:\\Incoming')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Bring this pad online' }));
  await waitFor(() => expect(complete).toHaveBeenCalledWith({ deviceName: 'Studio Mac', receiveDirectory: 'C:\\Incoming', launchAtLogin: true, notificationsEnabled: true, automaticDeviceTrust: true }));
  expect(await screen.findByText('Studio Mac')).toBeVisible();
});

it('offers separate file and folder pickers from the pad', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(withPad('laptop', 'Laptop'));
  const chooseFiles = vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  const chooseDirectory = vi.spyOn(appBridge, 'chooseDirectory').mockResolvedValue(['C:\\Photos']);
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'preparing', waitingForAvailable: false });
  render(<App />);
  expect(await screen.findByRole('button', { name: 'Laptop' })).toBeVisible();

  fireEvent.click(screen.getByRole('button', { name: 'Send files or folders' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
  await waitFor(() => expect(chooseFiles).toHaveBeenCalledOnce());
  expect(enqueue).toHaveBeenCalledWith(['C:\\report.pdf'], ['laptop'], false);

  fireEvent.click(screen.getByRole('button', { name: 'Send files or folders' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse folder' }));
  await waitFor(() => expect(chooseDirectory).toHaveBeenCalledOnce());
});

it('keeps the stage armed until nested drag leaves have all completed', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  const { container } = render(<App />);
  await screen.findByText('Desk');
  const stage = container.querySelector('.q-stage-area')!;
  fireEvent.dragEnter(stage);
  fireEvent.dragEnter(stage);
  expect(stage).toHaveClass('drag');
  fireEvent.dragLeave(stage);
  expect(stage).toHaveClass('drag');
  fireEvent.dragLeave(stage);
  expect(stage).not.toHaveClass('drag');
});

it('enqueues a native drop once with the pads selected when it arrived', async () => {
  let receiveDrop: ((payload: { paths: string[] }) => void) | undefined;
  vi.spyOn(appBridge, 'listenForNativeDrop').mockImplementation(async (handler) => { receiveDrop = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(withPad('laptop', 'Laptop'));
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'preparing', waitingForAvailable: false });
  render(<App />);
  expect(await screen.findByRole('button', { name: 'Laptop' })).toBeVisible();
  await waitFor(() => expect(receiveDrop).toBeTypeOf('function'));
  receiveDrop?.({ paths: ['C:\\drop\\photo.jpg'] });
  // Changing the selection afterwards must not redirect a drop already in flight.
  fireEvent.click(screen.getByRole('button', { name: 'Laptop' }));
  await waitFor(() => expect(enqueue).toHaveBeenCalledOnce());
  expect(enqueue).toHaveBeenCalledWith(['C:\\drop\\photo.jpg'], ['laptop'], false);
});

it('queues a drop aimed at a dark pad rather than refusing it', async () => {
  // A linked pad that is currently dark is still a destination: the batch waits
  // for it. Losing this path would silently strand everything dropped offline.
  let receiveDrop: ((payload: { paths: string[] }) => void) | undefined;
  vi.spyOn(appBridge, 'listenForNativeDrop').mockImplementation(async (handler) => { receiveDrop = handler; return () => undefined; });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(withPad('laptop', 'Laptop', false));
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'queued', waitingForAvailable: true });
  render(<App />);
  const chip = await screen.findByRole('button', { name: /Laptop/ });
  expect(chip).toHaveAttribute('aria-pressed', 'false');

  await waitFor(() => expect(receiveDrop).toBeTypeOf('function'));
  act(() => { receiveDrop?.({ paths: ['C:\\drop\\photo.jpg'] }); });
  // Nothing is selected, so the payload stages on the deck and waits.
  expect(await screen.findByText(/a dark one holds them until it wakes/)).toBeVisible();
  expect(enqueue).not.toHaveBeenCalled();

  fireEvent.click(chip);
  await waitFor(() => expect(enqueue).toHaveBeenCalledWith(['C:\\drop\\photo.jpg'], ['laptop'], true));
  expect(await screen.findByText(/Holding 1 item until that pad wakes/)).toBeVisible();
});

it('stages picker paths when no pad is selected', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  const chooseFiles = vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  const enqueue = vi.spyOn(appBridge, 'enqueuePaths');
  render(<App />);
  await screen.findByText('Desk');
  fireEvent.click(screen.getByRole('button', { name: 'Send files or folders' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
  await waitFor(() => expect(chooseFiles).toHaveBeenCalledOnce());
  expect(await screen.findByText(/Pick a pad for these/)).toBeVisible();
  expect(enqueue).not.toHaveBeenCalled();
  // The staged payload is visible on the deck rather than silently held.
  expect(screen.getByText('report.pdf')).toBeVisible();
});

it('cancels the notice transfer using its returned batch ID', async () => {
  const snapshot = withPad('laptop', 'Laptop');
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
  vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
  vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'stored-batch-id', itemCount: 1, targetDeviceIds: ['laptop'], state: 'queued', waitingForAvailable: false });
  const cancelBatch = vi.spyOn(appBridge, 'cancelBatch').mockResolvedValue({ ...snapshot, revision: 3 });
  render(<App />);
  await screen.findByRole('button', { name: 'Laptop' });
  fireEvent.click(screen.getByRole('button', { name: 'Send files or folders' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
  fireEvent.click(await screen.findByRole('button', { name: 'CANCEL' }));
  await waitFor(() => expect(cancelBatch).toHaveBeenCalledWith('stored-batch-id'));
});

it('announces a queued transfer, focuses its cancellation, and expires it after five seconds', async () => {
  vi.useFakeTimers();
  try {
    vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(withPad('laptop', 'Laptop'));
    vi.spyOn(appBridge, 'chooseFiles').mockResolvedValue(['C:\\report.pdf']);
    vi.spyOn(appBridge, 'enqueuePaths').mockResolvedValue({ id: 'batch-1', itemCount: 1, targetDeviceIds: ['laptop'], state: 'queued', waitingForAvailable: false });
    render(<App />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    fireEvent.click(screen.getByRole('button', { name: 'Send files or folders' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Browse files' }));
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(screen.getByRole('button', { name: 'CANCEL' })).toHaveFocus();
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(screen.queryByRole('button', { name: 'CANCEL' })).not.toBeInTheDocument();
  } finally { vi.useRealTimers(); }
});

it('applies every editable Config field through the supported patch DTO', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  vi.spyOn(appBridge, 'chooseReceiveDirectory').mockResolvedValue('C:\\Incoming');
  const update = vi.spyOn(appBridge, 'updateSettings').mockResolvedValue({ ...readySnapshot, revision: 3 });
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Config' }));
  fireEvent.change(screen.getByLabelText('Name other pads see'), { target: { value: 'Studio' } });
  fireEvent.click(screen.getByRole('button', { name: 'Choose' }));
  await waitFor(() => expect(screen.getByDisplayValue('C:\\Incoming')).toBeVisible());
  fireEvent.click(screen.getByRole('switch', { name: 'Accept inbound transports' }));
  fireEvent.click(screen.getByRole('switch', { name: 'Bring the pad online at sign-in' }));
  fireEvent.click(screen.getByRole('switch', { name: 'Notify me when something arrives' }));
  fireEvent.click(screen.getByRole('switch', { name: 'Link authenticated pads automatically' }));
  fireEvent.click(screen.getByRole('button', { name: '90 days' }));
  fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
  await waitFor(() => expect(update).toHaveBeenCalledWith({
    deviceName: 'Studio', receiveDirectory: 'C:\\Incoming', receivingEnabled: false, listenAddress: '127.0.0.1:48721',
    launchAtLogin: false, notificationsEnabled: false, automaticDeviceTrust: false, historyRetentionDays: 90
  }));
});

it('keeps Apply inert until something actually changed, and restores on Discard', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Config' }));
  expect(screen.getByRole('button', { name: 'Apply' })).toBeDisabled();
  expect(screen.getByText('Everything here is in use')).toBeVisible();

  fireEvent.change(screen.getByLabelText('Name other pads see'), { target: { value: 'Studio' } });
  expect(screen.getByRole('button', { name: 'Apply' })).toBeEnabled();
  expect(screen.getByText('Not applied yet')).toBeVisible();

  fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
  expect(screen.getByLabelText('Name other pads see')).toHaveValue('Desk');
  expect(screen.getByRole('button', { name: 'Apply' })).toBeDisabled();
});

it('keeps Apply disabled for an invalid preferred listen address', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(readySnapshot);
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Config' }));
  fireEvent.change(screen.getByLabelText('Preferred listen address'), { target: { value: 'not-an-endpoint' } });
  expect(screen.getByRole('button', { name: 'Apply' })).toBeDisabled();
  expect(screen.getByLabelText('Preferred listen address')).toHaveAttribute('aria-invalid', 'true');
});

it('renders safe expanded diagnostics and about actions from the snapshot', async () => {
  const snapshot = {
    ...readySnapshot,
    network: { ...readySnapshot.network, boundEndpoint: '192.168.1.8:48721', trustedOnlineEndpoints: ['192.168.1.9:48721'], mdnsState: 'advertising', localInterfaceSummaries: ['Wi-Fi: 192.168.1.8'], recentErrorCodes: ['network_timeout'] },
    about: { ...readySnapshot.about, databaseMigrationVersion: 8, ownedStagingBytes: 1024 }
  };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(snapshot);
  const viewLogs = vi.spyOn(appBridge, 'viewLogs').mockResolvedValue();
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Config' }));
  fireEvent.click(screen.getByText('Network diagnostics'));
  expect(screen.getByText('192.168.1.9:48721')).toBeVisible();
  expect(screen.getByText('Wi-Fi: 192.168.1.8')).toBeVisible();
  expect(screen.getByText('network_timeout')).toBeVisible();
  fireEvent.click(screen.getByText('About Fileporter'));
  expect(screen.getByText(/Version 0.1.0/)).toBeVisible();
  expect(screen.getByText(/Database migration 8/)).toBeVisible();
  expect(screen.getByText(/Staging 1,024 bytes/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Logs' }));
  await waitFor(() => expect(viewLogs).toHaveBeenCalledWith());
});

it('reads header state from real snapshot counts', async () => {
  const offline = { ...readySnapshot, lifecycle: { ...readySnapshot.lifecycle, listening: false }, network: { ...readySnapshot.network, listening: false } };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(offline);
  const view = render(<App />);
  expect(await screen.findByText(/Offline; 0 of 0 linked pads online; 0 transports in flight/)).toBeVisible();
  expect(screen.getByText('no pads linked')).toBeVisible();
  view.unmount();

  const busy: BackendAppSnapshot = {
    ...withPad('laptop', 'Laptop'),
    transfers: [{ id: 'active', label: 'Report', state: 'sending', progress: 50, targets: [{ id: 't', deviceName: 'Laptop', state: 'sending', progress: 50, rateLabel: '12 MB/s' }] }]
  };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(busy);
  render(<App />);
  expect(await screen.findByText(/Listening and receiving; 1 of 1 linked pad online; 1 transport in flight/)).toBeVisible();
  // A live rate is the more useful reading while something is moving.
  expect(screen.getByText('12 MB/s')).toBeVisible();
});
