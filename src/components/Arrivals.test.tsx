import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import App from '../App';
import { appBridge } from '../lib/bridge';
import type { BackendAppSnapshot, HistoryItemViewModel } from '../types/view-models';

const base: BackendAppSnapshot = {
  revision: 4,
  lifecycle: { windowVisible: true, receivingEnabled: true, listening: true, receiving: true, shuttingDown: false },
  settings: { deviceName: 'Desk', receiveDirectory: '/Users/d/Fileporter', onboardingComplete: true, launchAtLogin: true, notificationsEnabled: true, receivingEnabled: true, automaticDeviceTrust: true, preferredListenAddress: '0.0.0.0:0', preferredListenPort: 0, historyRetentionDays: 30 },
  managersStarted: true, localDeviceName: 'Desk', devices: [], nearbyDevices: [], transfers: [], history: [], queuedBatches: [],
  pairing: { localDeviceId: 'local', pendingPairings: [], trustedDevices: [] },
  network: { listening: true, boundEndpoint: '0.0.0.0:5000', preferredListenAddress: '0.0.0.0:0', trustedOnlineEndpoints: [], mdnsState: 'advertising', localInterfaceSummaries: [], recentErrorCodes: [] },
  about: { appVersion: '0.1.2', protocolVersion: 1, logsAvailable: true, databaseMigrationVersion: 14, ownedStagingBytes: 0 }
};

const arrival = (overrides: Partial<HistoryItemViewModel['items'][number]> = {}): HistoryItemViewModel => ({
  id: 'batch-1', direction: 'incoming', peerName: 'peer-1', summary: '1 item', timeLabel: '1788234685', state: 'completed' as never,
  // The backend serialises the persisted state, so fixtures use its words.
  items: [{ itemId: 'item-1', displayName: 'holiday.png', kind: 'file', size: 2411724, state: 'completed' as never, available: true, ...overrides }]
});

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(appBridge, 'listenForNativeDrop').mockResolvedValue(() => undefined);
  vi.spyOn(appBridge, 'listenForSnapshotChanges').mockResolvedValue(() => undefined);
});

it('shows what arrived with its name, type and size', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({
    ...base,
    history: [arrival()],
    pairing: { ...base.pairing, trustedDevices: [{ deviceId: 'peer-1', name: 'DWMM Gaming', alias: null, pairedAt: 1, lastSeenAt: 2, certificateFingerprintShort: 'AB', autoSend: true, endpoint: null }] }
  });
  render(<App />);
  expect(await screen.findByText('holiday.png')).toBeVisible();
  // Type and size read as a file manager writes them, not as a raw byte count.
  expect(screen.getByText(/PNG image · 2\.4 MB · from DWMM Gaming/)).toBeVisible();
});

it('copies the arrived file to the system clipboard by its durable id', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history: [arrival()] });
  const copy = vi.spyOn(appBridge, 'copyItem').mockResolvedValue();
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: /^Copy$/ }));
  await waitFor(() => expect(copy).toHaveBeenCalledWith('item-1'));
  expect(await screen.findByRole('status')).toHaveTextContent(/Copied/);
});

it('cuts the arrived file so the file manager completes the move', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history: [arrival()] });
  const cut = vi.spyOn(appBridge, 'moveItem').mockResolvedValue();
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: /^Cut$/ }));
  await waitFor(() => expect(cut).toHaveBeenCalledWith('item-1'));
  expect(await screen.findByRole('status')).toHaveTextContent(/move/i);
});

it('reports a clipboard failure instead of implying the file is ready to paste', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history: [arrival()] });
  vi.spyOn(appBridge, 'copyItem').mockRejectedValue(new Error('clipboard unavailable'));
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: /^Copy$/ }));
  expect(await screen.findByRole('alert')).toHaveTextContent('could not be copied');
});

it('offers no clipboard actions for an output that is no longer on disk', async () => {
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history: [arrival({ available: false })] });
  render(<App />);
  expect(await screen.findByText('Moved or deleted')).toBeVisible();
  expect(screen.queryByRole('button', { name: /^Copy$/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /^Cut$/ })).not.toBeInTheDocument();
});

it('does not offer actions for an incoming item that has not finished', async () => {
  const pending = arrival({ state: 'receiving' as never });
  vi.spyOn(appBridge, 'getAppSnapshot').mockResolvedValue({ ...base, history: [pending] });
  render(<App />);
  await screen.findByText('Send files, simply.');
  expect(screen.queryByText('holiday.png')).not.toBeInTheDocument();
});
