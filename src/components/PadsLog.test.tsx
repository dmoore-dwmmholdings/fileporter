import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import App from '../App';
import { appBridge } from '../lib/bridge';
import type { BackendAppSnapshot, HistoryItemViewModel } from '../types/view-models';

const base: BackendAppSnapshot = {
  revision: 4,
  lifecycle: { windowVisible: true, receivingEnabled: true, listening: true, receiving: false, shuttingDown: false },
  settings: { deviceName: 'Desk', receiveDirectory: 'C:\\Incoming', onboardingComplete: true, launchAtLogin: true, notificationsEnabled: true, receivingEnabled: true, automaticDeviceTrust: false, preferredListenAddress: '127.0.0.1:48721', preferredListenPort: 48721, historyRetentionDays: 30 },
  managersStarted: true, localDeviceName: 'Desk', devices: [], nearbyDevices: [], transfers: [], history: [], queuedBatches: [],
  pairing: { localDeviceId: 'local', pendingPairings: [], trustedDevices: [] },
  network: { listening: true, boundEndpoint: '127.0.0.1:48721', preferredListenAddress: '127.0.0.1:48721', trustedOnlineEndpoints: [], mdnsState: 'advertising', localInterfaceSummaries: [], recentErrorCodes: [] },
  about: { appVersion: '0.1.0', protocolVersion: 1, logsAvailable: true, databaseMigrationVersion: 8, ownedStagingBytes: 0 }
};

const laptop = { deviceId: 'device-1', name: 'Laptop', alias: null, pairedAt: 1, lastSeenAt: null, certificateFingerprintShort: 'ABCD', autoSend: true, endpoint: null };

function received(items: HistoryItemViewModel['items']): HistoryItemViewModel {
  return { id: 'batch-1', direction: 'incoming', peerName: 'Laptop', summary: 'contracts/', timeLabel: 'Now', state: 'complete', items };
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(appBridge, 'listenForNativeDrop').mockResolvedValue(() => undefined);
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockResolvedValue(() => undefined);
});

async function openPads() {
  fireEvent.click(await screen.findByRole('button', { name: 'Pads' }));
}
async function openLog() {
  fireEvent.click(await screen.findByRole('button', { name: 'Log' }));
}

// ── Pads ──────────────────────────────────────────────────────────────

it('links a nearby pad by its stored device id when trust is not automatic', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({
    ...base,
    nearbyDevices: [{ deviceId: 'nearby-1', displayName: 'Studio Mac', endpoint: '192.168.1.8:48721', certificateFingerprint: 'fp', protocolVersion: 1, capabilities: [] }]
  });
  const pair = vi.spyOn(appBridge, 'startPairingDiscovered').mockResolvedValue({ id: 'pair-1', deviceId: 'nearby-1', remoteName: 'Studio Mac', certificateFingerprint: 'fp', expiresAt: 9, localConfirmed: false, remoteConfirmed: false, sasCode: null });
  render(<App />);
  await openPads();
  fireEvent.click(screen.getByRole('button', { name: 'LINK' }));
  await waitFor(() => expect(pair).toHaveBeenCalledWith('nearby-1'));
});

it('shows automatic discovery as progress, with no link button', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({
    ...base,
    settings: { ...base.settings, automaticDeviceTrust: true },
    nearbyDevices: [{ deviceId: 'nearby-1', displayName: 'Studio Mac', endpoint: '192.168.1.8:48721', certificateFingerprint: 'fp', protocolVersion: 1, capabilities: [] }]
  });
  render(<App />);
  await openPads();
  expect(await screen.findByRole('status')).toHaveTextContent('proving identity');
  expect(screen.queryByRole('button', { name: 'LINK' })).not.toBeInTheDocument();
});

it('confirms a pending link with its backend pairing id', async () => {
  const pairing = { id: 'pairing-1', deviceId: 'device-1', remoteName: 'Laptop', certificateFingerprint: 'abc', expiresAt: 99, localConfirmed: false, remoteConfirmed: false, sasCode: '042 007' };
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, pairing: { ...base.pairing, pendingPairings: [pairing] } });
  const confirm = vi.spyOn(appBridge, 'confirmPairing').mockResolvedValue({ pairing: { ...pairing, localConfirmed: true }, trustedDevice: null });
  render(<App />);
  await openPads();
  fireEvent.click(await screen.findByRole('button', { name: 'Confirm link' }));
  await waitFor(() => expect(confirm).toHaveBeenCalledWith('pairing-1'));
});

it('keeps confirmation disabled until the backend supplies a matching code', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({
    ...base,
    pairing: { ...base.pairing, pendingPairings: [{ id: 'pairing-1', deviceId: 'device-1', remoteName: 'Laptop', certificateFingerprint: 'abc', expiresAt: 99, localConfirmed: false, remoteConfirmed: false, sasCode: null }] }
  });
  render(<App />);
  await openPads();
  expect(await screen.findByRole('button', { name: 'Confirm link' })).toBeDisabled();
  expect(screen.getByRole('status')).toHaveTextContent('Waiting for a matching code');
});

it('adds a pad at the exact address typed, and reports failure accessibly', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue(base);
  const start = vi.spyOn(appBridge, 'startPairingAtEndpoint').mockRejectedValue(new Error('unreachable'));
  render(<App />);
  await openPads();
  fireEvent.change(screen.getByLabelText('Add a pad by address'), { target: { value: '192.168.1.24:48721' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add' }));
  await waitFor(() => expect(start).toHaveBeenCalledWith('192.168.1.24:48721'));
  expect(await screen.findByRole('alert')).toHaveTextContent('could not reach a pad at that address');
});

it('renames a linked pad locally by its stored device id', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, pairing: { ...base.pairing, trustedDevices: [laptop] } });
  const rename = vi.spyOn(appBridge, 'renameTrustedDevice').mockResolvedValue();
  render(<App />);
  await openPads();
  fireEvent.click(await screen.findByRole('button', { name: 'RENAME' }));
  fireEvent.change(screen.getByLabelText('Local name for Laptop'), { target: { value: 'Office laptop' } });
  fireEvent.click(screen.getByRole('button', { name: 'SAVE' }));
  await waitFor(() => expect(rename).toHaveBeenCalledWith('device-1', 'Office laptop'));
});

it('discards a pattern held for a dark pad by its batch id', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({
    ...base,
    pairing: { ...base.pairing, trustedDevices: [laptop] },
    queuedBatches: [{ id: 'queued-1', itemCount: 84, targetDeviceIds: ['device-1'], state: 'queued', waitingForAvailable: true }]
  });
  const cancel = vi.spyOn(appBridge, 'cancelBatch').mockResolvedValue({ ...base, revision: 5 });
  render(<App />);
  await openPads();
  expect(await screen.findByText('held for Laptop')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'DISCARD' }));
  await waitFor(() => expect(cancel).toHaveBeenCalledWith('queued-1'));
});

// ── Log ───────────────────────────────────────────────────────────────

it('aborts an in-flight transport with its batch id', async () => {
  const transfers = [{ id: 'active', label: 'Photo', state: 'sending' as const, progress: 30, targets: [] }];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, transfers });
  const cancel = vi.spyOn(appBridge, 'cancelBatch').mockResolvedValue({ ...base, revision: 5, transfers });
  render(<App />);
  await openLog();
  fireEvent.click(screen.getByRole('button', { name: 'ABORT' }));
  await waitFor(() => expect(cancel).toHaveBeenCalledWith('active'));
});

it('runs a failed transport again with its batch id', async () => {
  const history: HistoryItemViewModel[] = [{ id: 'failed-1', direction: 'outgoing', peerName: 'Laptop', summary: 'archive.zip', timeLabel: 'Now', state: 'failed', items: [] }];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  const retry = vi.spyOn(appBridge, 'retryBatch').mockResolvedValue({ ...base, revision: 6, history });
  render(<App />);
  await openLog();
  fireEvent.click(await screen.findByRole('button', { name: 'Run archive.zip again' }));
  await waitFor(() => expect(retry).toHaveBeenCalledWith('failed-1'));
});

it('uses stored item ids for per-item actions and marks what is gone', async () => {
  const history = [received([
    { itemId: 'item-1', displayName: 'photo.jpg', kind: 'file', size: 12, state: 'complete' as const, available: true },
    { itemId: 'item-2', displayName: 'gone.txt', kind: 'file', size: 3, state: 'complete' as const, available: false }
  ])];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  const move = vi.spyOn(appBridge, 'moveItem').mockResolvedValue();
  render(<App />);
  await openLog();
  fireEvent.click(await screen.findByRole('button', { name: /Received/ }));
  fireEvent.click(screen.getByRole('button', { name: 'Move photo.jpg' }));
  await waitFor(() => expect(move).toHaveBeenCalledWith('item-1'));
  expect(screen.getByText('gone.txt')).toBeVisible();
  expect(screen.getByText('Moved or deleted')).toBeVisible();
});

it('acts on a whole arrival with its batch id and reports the outcome accessibly', async () => {
  const history = [received([
    { itemId: 'item-1', displayName: 'a.pdf', kind: 'file', size: 12, state: 'complete' as const, available: true },
    { itemId: 'item-2', displayName: 'b.pdf', kind: 'file', size: 13, state: 'complete' as const, available: true }
  ])];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  const move = vi.spyOn(appBridge, 'moveCompletedBatch').mockResolvedValue();
  render(<App />);
  await openLog();
  fireEvent.click(await screen.findByRole('button', { name: /Received/ }));
  fireEvent.click(screen.getByRole('button', { name: 'MOVE ALL' }));
  await waitFor(() => expect(move).toHaveBeenCalledWith('batch-1'));
  expect(await screen.findByText(/staged to move/)).toBeVisible();
});

it('reports a clipboard failure instead of implying the files are ready', async () => {
  const history = [received([
    { itemId: 'item-1', displayName: 'a.pdf', kind: 'file', size: 12, state: 'complete' as const, available: true },
    { itemId: 'item-2', displayName: 'b.pdf', kind: 'file', size: 13, state: 'complete' as const, available: true }
  ])];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  vi.spyOn(appBridge, 'copyCompletedBatch').mockRejectedValue(new Error('clipboard unavailable'));
  render(<App />);
  await openLog();
  fireEvent.click(await screen.findByRole('button', { name: /Received/ }));
  fireEvent.click(screen.getByRole('button', { name: 'COPY ALL' }));
  expect(await screen.findByText(/could not copy the arrived files/)).toBeVisible();
});

it('does not open a record that has nothing landed to act on', async () => {
  const history: HistoryItemViewModel[] = [{ id: 'sent-1', direction: 'outgoing', peerName: 'Laptop', summary: 'report.pdf', timeLabel: 'Now', state: 'complete', items: [] }];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  render(<App />);
  await openLog();
  expect(await screen.findByRole('button', { name: /Sent/ })).toBeDisabled();
});

// ── Arrivals on Transport ─────────────────────────────────────────────

it('copies an arrived file to the clipboard by its durable id', async () => {
  const history = [received([{ itemId: 'item-1', displayName: 'photo.jpg', kind: 'file', size: 2048, state: 'complete' as const, available: true }])];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  const copy = vi.spyOn(appBridge, 'copyItem').mockResolvedValue();
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Copy photo.jpg' }));
  await waitFor(() => expect(copy).toHaveBeenCalledWith('item-1'));
  expect(await screen.findByRole('status')).toHaveTextContent(/Copied/);
});

it('cuts an arrived file so the file manager completes the move', async () => {
  const history = [received([{ itemId: 'item-1', displayName: 'photo.jpg', kind: 'file', size: 2048, state: 'complete' as const, available: true }])];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  const move = vi.spyOn(appBridge, 'moveItem').mockResolvedValue();
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Cut photo.jpg' }));
  await waitFor(() => expect(move).toHaveBeenCalledWith('item-1'));
  expect(await screen.findByRole('status')).toHaveTextContent(/Ready to move/);
});

it('reports a clipboard failure rather than implying the file is ready to paste', async () => {
  const history = [received([{ itemId: 'item-1', displayName: 'photo.jpg', kind: 'file', size: 2048, state: 'complete' as const, available: true }])];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  vi.spyOn(appBridge, 'copyItem').mockRejectedValue(new Error('clipboard unavailable'));
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Copy photo.jpg' }));
  expect(await screen.findByText('Could not copy that file')).toBeVisible();
});

it('offers no arrival actions for an output that is no longer on disk', async () => {
  const history = [received([{ itemId: 'item-1', displayName: 'gone.txt', kind: 'file', size: 10, state: 'complete' as const, available: false }])];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  render(<App />);
  await screen.findByRole('heading', { name: 'Drop anything.' });
  expect(screen.queryByRole('button', { name: /Copy gone.txt/ })).not.toBeInTheDocument();
});

it('does not show an incoming item that has not finished', async () => {
  const history: HistoryItemViewModel[] = [{
    id: 'batch-1', direction: 'incoming', peerName: 'Laptop', summary: 'big.iso', timeLabel: 'Now', state: 'receiving',
    items: [{ itemId: 'item-1', displayName: 'big.iso', kind: 'file', size: 10, state: 'receiving', available: false }]
  }];
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history });
  render(<App />);
  await screen.findByRole('heading', { name: 'Drop anything.' });
  expect(screen.queryByRole('button', { name: /Copy big.iso/ })).not.toBeInTheDocument();
});
