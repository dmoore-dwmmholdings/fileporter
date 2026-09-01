import { ArrowLeft, FolderOpen, MonitorUp, Plus, Settings, Wifi, WifiOff } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MagicSendControl, type MagicChoice } from './components/MagicSendControl';
import { StatusPill } from './components/StatusPill';
import { DevicesView } from './components/DevicesView';
import { ActivityView } from './components/ActivityView';
import { appBridge } from './lib/bridge';
import { formatPeer, formatWhen } from './lib/format';
import { emptySnapshot, toViewModel, type AppSnapshotViewModel, type BackendAppSnapshot } from './types/view-models';

export interface FileporterAppProps { initialSnapshot?: AppSnapshotViewModel; }

export default function App({ initialSnapshot = emptySnapshot }: FileporterAppProps) {
  const [snapshot, setSnapshot] = useState(initialSnapshot);
  const [loadState, setLoadState] = useState<'loading' | 'ready' | 'error'>(initialSnapshot.revision > 0 ? 'ready' : 'loading');
  const [screen, setScreen] = useState<'home' | 'devices' | 'activity' | 'settings'>('home');
  const revisionRef = useRef(initialSnapshot.revision);
  const [selectedDeviceIds, setSelectedDeviceIds] = useState<string[]>(() =>
    initialSnapshot.devices.filter((device) => device.state === 'online').map((device) => device.id));
  const [isDropActive, setDropActive] = useState(false);
  const [notice, setNotice] = useState<{ message: string; batchId?: string } | null>(null);
  const [stagedPaths, setStagedPaths] = useState<string[]>([]);
  const [stagedTargetIds, setStagedTargetIds] = useState<string[]>([]);
  const cancelNoticeRef = useRef<HTMLButtonElement>(null);
  const selectedIdsRef = useRef(selectedDeviceIds);
  const dragDepthRef = useRef(0);
  // Recipients follow whoever is online until the user picks for themselves.
  // Capturing them once meant a launch that finished before discovery did left
  // the selection permanently empty, and every drop asked for a device instead
  // of sending.
  const recipientsChosenRef = useRef(false);
  selectedIdsRef.current = selectedDeviceIds;
  const connectedDevices = useMemo(() => snapshot.devices.filter((device) => device.state === 'online'), [snapshot]);
  const offlineTrustedDevices = useMemo(() => snapshot.trustedDevices.filter((device) => device.state === 'offline'), [snapshot]);

  useEffect(() => {
    if (!notice?.batchId) return;
    cancelNoticeRef.current?.focus();
    const timer = window.setTimeout(() => setNotice(null), 5000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const applySnapshot = useCallback((incoming: BackendAppSnapshot) => {
    if (incoming.revision <= revisionRef.current) return;
    revisionRef.current = incoming.revision;
    setSnapshot(toViewModel(incoming));
    if (!recipientsChosenRef.current) {
      setSelectedDeviceIds((incoming.devices ?? []).filter((device) => device.state === 'online').map((device) => device.id));
    }
    setLoadState('ready');
  }, []);

  const hydrate = useCallback(async () => {
    setLoadState('loading');
    try {
      const incoming = await appBridge.getAppSnapshot();
      if (incoming.revision >= revisionRef.current) {
        revisionRef.current = incoming.revision;
        setSnapshot(toViewModel(incoming));
        if (!recipientsChosenRef.current) {
          setSelectedDeviceIds((incoming.devices ?? []).filter((device) => device.state === 'online').map((device) => device.id));
        }
      }
      setLoadState('ready');
    } catch {
      setLoadState('error');
    }
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let mounted = true;
    appBridge.listenForSnapshotChanges((incoming) => applySnapshot(incoming))
      .then((stop) => { unlisten = stop; if (mounted) void hydrate(); })
      .catch(() => { if (mounted) void hydrate(); });
    return () => { mounted = false; unlisten?.(); };
  }, [applySnapshot, hydrate]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    appBridge.listenForNavigation((destination) => {
      if (destination !== 'settings') return;
      setScreen('settings');
      window.setTimeout(() => document.querySelector<HTMLButtonElement>('.back-button')?.focus(), 0);
    }).then((stop) => { unlisten = stop; }).catch(() => undefined);
    return () => unlisten?.();
  }, []);

  const submitPaths = useCallback(async (paths: string[], targetDeviceIds = [...selectedIdsRef.current]) => {
    if (targetDeviceIds.length === 0) {
      setStagedPaths(paths);
      setStagedTargetIds([]);
      setNotice({ message: 'Choose a trusted device to queue these items for delivery.' });
      return;
    }
    try {
      const queued = await appBridge.enqueuePaths(paths, targetDeviceIds);
      setNotice({ message: `Preparing ${queued.itemCount} item${queued.itemCount === 1 ? '' : 's'} for transfer.`, batchId: queued.id });
    } catch {
      setNotice({ message: 'Fileporter could not start this transfer. Your files remain where they are.' });
    }
  }, []);

  const selectPaths = useCallback(async (choice: MagicChoice) => {
    try {
      const paths = choice === 'files' ? await appBridge.chooseFiles() : await appBridge.chooseDirectory();
      if (paths.length) await submitPaths(paths);
    } catch {
      setNotice({ message: 'The picker could not open. Try again, or drag items here.' });
    }
  }, [submitPaths]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    appBridge.listenForNativeDrop(({ paths }) => {
      // Native drag-drop arrives asynchronously. Capture the intended recipients
      // at the event boundary so a later recipient change cannot redirect it.
      const targetDeviceIds = [...selectedIdsRef.current];
      void submitPaths(paths, targetDeviceIds);
    }).then((stop) => { unlisten = stop; }).catch(() => undefined);
    return () => unlisten?.();
  }, [submitPaths]);

  function toggleDevice(id: string) {
    recipientsChosenRef.current = true;
    setSelectedDeviceIds((selected) => selected.includes(id) ? selected.filter((value) => value !== id) : [...selected, id]);
  }

  function toggleStagedTarget(id: string) {
    setStagedTargetIds((selected) => selected.includes(id) ? selected.filter((value) => value !== id) : [...selected, id]);
  }

  async function sendStaged() {
    if (!stagedPaths.length || !stagedTargetIds.length) return;
    try {
      const queued = await appBridge.enqueuePaths(stagedPaths, stagedTargetIds, true);
      setStagedPaths([]); setStagedTargetIds([]);
      setNotice({ message: `Queued ${queued.itemCount} item${queued.itemCount === 1 ? '' : 's'} for delivery when your device is available.`, batchId: queued.id });
    } catch { setNotice({ message: 'Fileporter could not queue these items. Your files remain where they are.' }); }
  }

  async function cancelQueuedNotice() {
    if (!notice?.batchId) return;
    const batchId = notice.batchId;
    setNotice(null);
    try { await appBridge.cancelBatch(batchId); } catch { setNotice({ message: 'Fileporter could not cancel that queued transfer.' }); }
  }

  if (loadState === 'loading') return <main className="loading-shell" aria-live="polite">Loading Fileporter…</main>;
  if (loadState === 'error') return <main className="loading-shell" role="alert"><strong>Fileporter couldn’t load.</strong><button type="button" onClick={() => { void hydrate(); }}>Try again</button></main>;
  if (!snapshot.onboardingComplete) return <Onboarding onComplete={(next) => { revisionRef.current = next.revision; setSnapshot(toViewModel(next)); setScreen('home'); }} />;
  if (screen === 'settings') return <SettingsView snapshot={snapshot} onBack={() => setScreen('home')} onSnapshot={applySnapshot} />;
  if (screen === 'devices') return <AppFrame snapshot={snapshot} screen={screen} onNavigate={setScreen}><DevicesView snapshot={snapshot} /></AppFrame>;
  if (screen === 'activity') return <AppFrame snapshot={snapshot} screen={screen} onNavigate={setScreen}><ActivityView snapshot={snapshot} onSnapshot={applySnapshot} /></AppFrame>;

  return (
    <AppFrame snapshot={snapshot} screen={screen} onNavigate={setScreen}><div
      onDragEnter={() => { dragDepthRef.current += 1; setDropActive(true); }}
      onDragLeave={() => { dragDepthRef.current = Math.max(0, dragDepthRef.current - 1); if (dragDepthRef.current === 0) setDropActive(false); }}
    >

      <section className="recipient-strip" aria-label="Recipients">
        <span className="eyebrow">SEND TO</span>
        {connectedDevices.length > 0 ? (
          <>
            <button type="button" className={selectedDeviceIds.length === connectedDevices.length ? 'recipient active' : 'recipient'} onClick={() => { recipientsChosenRef.current = true; setSelectedDeviceIds(selectedDeviceIds.length === connectedDevices.length ? [] : connectedDevices.map((device) => device.id)); }}>All online ({connectedDevices.length})</button>
            {connectedDevices.map((device) => <button key={device.id} type="button" className={selectedDeviceIds.includes(device.id) ? 'recipient active' : 'recipient'} onClick={() => toggleDevice(device.id)}>{device.name}</button>)}
          </>
        ) : <span className="offline-copy"><WifiOff size={15} aria-hidden="true" /> No devices online</span>}
      </section>

      <section className={`drop-surface ${isDropActive ? 'drop-active' : ''}`} aria-label="Send files or folders" onDragOver={(event) => event.preventDefault()} onDrop={(event) => { event.preventDefault(); dragDepthRef.current = 0; setDropActive(false); }}>
        <div className="drop-icon"><Plus size={28} aria-hidden="true" /></div>
        <h1>Send files, simply.</h1>
        <p>Drop files or folders anywhere here{selectedDeviceIds.length ? ` to send to ${selectedDeviceIds.length} device${selectedDeviceIds.length === 1 ? '' : 's'}.` : '.'}</p>
        <MagicSendControl disabled={false} onChoose={(choice) => { void selectPaths(choice); }} />
        <p className="drop-hint">or drag and drop from your file browser</p>
      </section>

      {notice && <div className="notice" role="status" aria-live="polite"><span>{notice.message}</span>{notice.batchId && <button ref={cancelNoticeRef} type="button" onClick={() => { void cancelQueuedNotice(); }}>Cancel</button>}</div>}
      {stagedPaths.length > 0 && <section className="empty-targets" aria-labelledby="staged-send-heading"><strong id="staged-send-heading">{stagedPaths.length} item{stagedPaths.length === 1 ? '' : 's'} ready to send</strong><span>Choose a trusted device. Items will send automatically when it is available.</span>{offlineTrustedDevices.length ? <div className="staged-recipients" aria-label="Trusted offline recipients">{offlineTrustedDevices.map((device) => <button key={device.id} type="button" className={stagedTargetIds.includes(device.id) ? 'recipient active' : 'recipient'} aria-pressed={stagedTargetIds.includes(device.id)} onClick={() => toggleStagedTarget(device.id)}>{device.name}</button>)}</div> : <span className="muted">Fileporter is looking for your other computers.</span>}<div><button type="button" onClick={() => setScreen('devices')}>View devices</button><button type="button" disabled={!stagedTargetIds.length} onClick={() => { void sendStaged(); }}>Send when available</button><button type="button" className="text-button" onClick={() => { setStagedPaths([]); setStagedTargetIds([]); }}>Clear</button></div></section>}
      {selectedDeviceIds.length === 0 && stagedPaths.length === 0 && <section className="empty-targets" aria-live="polite"><strong>Looking for Fileporter devices…</strong><span>Other computers on this private network will appear automatically.</span></section>}

      {snapshot.transfers.length > 0 && <section className="activity-section"><h2>Sending</h2>{snapshot.transfers.map((batch) => <article className="transfer-card" key={batch.id}><div><strong>{batch.label}</strong><StatusPill state={batch.state} /></div><progress value={batch.progress} max="100">{batch.progress}%</progress>{batch.targets.map((target) => <div className="target-row" key={target.id}><span>{target.deviceName}</span><span>{target.rateLabel ?? ''}</span><StatusPill state={target.state} /></div>)}</article>)}</section>}
      <section className="activity-section"><h2>Recent activity</h2>{snapshot.history.length ? snapshot.history.map((item) => <article className="history-row" key={item.id}><span>{item.direction === 'incoming' ? 'Received from' : 'Sent to'} {formatPeer(item.peerName, snapshot.trustedDevices)}</span><span>{item.summary}</span><span>{formatWhen(item.timeLabel)}</span><StatusPill state={item.state} /></article>) : <p className="muted">Transfers you send and receive will appear here.</p>}</section>
    </div></AppFrame>
  );
}
function AppFrame({ snapshot, screen, onNavigate, children }: { snapshot: AppSnapshotViewModel; screen: 'home' | 'devices' | 'activity' | 'settings'; onNavigate: (screen: 'home' | 'devices' | 'activity' | 'settings') => void; children: React.ReactNode }) {
  const onlineDevices = snapshot.trustedDevices.filter((device) => device.state === 'online').length;
  const activeTransfers = snapshot.transfers.filter((batch) => ['preparing', 'sending', 'verifying'].includes(batch.state)).length;
  const waitingTransfers = snapshot.transfers.filter((batch) => ['queued', 'waiting'].includes(batch.state)).length + snapshot.queuedBatches.filter((batch) => batch.waitingForAvailable || ['queued', 'waiting'].includes(batch.state)).length;
  const isListening = snapshot.lifecycle.listening || snapshot.network.listening;
  const summary = `${isListening ? (snapshot.lifecycle.receivingEnabled ? 'Listening and receiving' : 'Listening') : 'Offline'}; ${onlineDevices} trusted device${onlineDevices === 1 ? '' : 's'} online; ${waitingTransfers} queued or waiting; ${activeTransfers} active transfer${activeTransfers === 1 ? '' : 's'}.`;
  return <main className="app-shell"><header className="app-header"><div className="brand"><span className="brand-mark" aria-hidden="true"><MonitorUp size={18} /></span><span>Fileporter</span></div><nav className="app-nav" aria-label="Main navigation"><button type="button" className={screen === 'home' ? 'nav-active' : ''} onClick={() => onNavigate('home')}>Send</button><button type="button" className={screen === 'devices' ? 'nav-active' : ''} onClick={() => onNavigate('devices')}>Devices</button><button type="button" className={screen === 'activity' ? 'nav-active' : ''} onClick={() => onNavigate('activity')}>Activity</button></nav><div className={`device-status ${isListening ? 'status-online' : 'status-offline'}`} aria-label={summary}><span aria-hidden="true">{isListening ? <Wifi size={16} /> : <WifiOff size={16} />}</span><span>{isListening ? 'Listening' : 'Offline'}</span><span aria-hidden="true"> · </span><span>{onlineDevices} online</span><span className="sr-only"> {snapshot.localDeviceName}</span></div><button className="icon-button" type="button" aria-label="Open settings" onClick={() => onNavigate('settings')}><Settings size={19} /></button></header>{children}</main>;
}

interface OnboardingProps { onComplete: (snapshot: BackendAppSnapshot) => void; }

function Onboarding({ onComplete }: OnboardingProps) {
  const [deviceName, setDeviceName] = useState('');
  const [directory, setDirectory] = useState<string | null>(null);
  const [launchAtLogin, setLaunchAtLogin] = useState(true);
  const [notificationsEnabled, setNotificationsEnabled] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const validName = deviceName.trim().length >= 1 && Array.from(deviceName.trim()).length <= 48;

  async function chooseDirectory() {
    setError(null);
    try { const paths = await appBridge.chooseDirectory(); setDirectory(paths[0] ?? null); } catch { setError('The receive-folder picker could not open. Please try again.'); }
  }
  async function complete() {
    if (!validName || !directory) { setError('Enter a device name and choose a receive folder to continue.'); return; }
    setSaving(true); setError(null);
    try {
      const next = await appBridge.completeOnboarding({ deviceName: deviceName.trim(), receiveDirectory: directory, launchAtLogin, notificationsEnabled, automaticDeviceTrust: true });
      onComplete(next);
    } catch { setError('Fileporter could not save setup. Your folder has not been enabled for receiving.'); }
    finally { setSaving(false); }
  }
  return <main className="onboarding-shell"><section className="onboarding-card"><div className="brand"><span className="brand-mark"><MonitorUp size={18} /></span>Fileporter</div><h1>Send files directly to your other computers on this network.</h1><p className="muted">Fileporter computers discover and securely remember each other automatically. On macOS, allow local-network access. On Windows, allow the firewall prompt on Private networks.</p><label>Device name<input value={deviceName} maxLength={48} onChange={(event) => setDeviceName(event.target.value)} placeholder="This computer" autoFocus /></label><label>Receive folder<span className="field-row"><input value={directory ?? ''} readOnly placeholder="Choose where received files go" /><button type="button" onClick={() => { void chooseDirectory(); }}><FolderOpen size={16} /> Choose</button></span></label><label className="check-row"><input type="checkbox" checked={launchAtLogin} onChange={(event) => setLaunchAtLogin(event.target.checked)} /> Launch Fileporter when I sign in</label><label className="check-row"><input type="checkbox" checked={notificationsEnabled} onChange={(event) => setNotificationsEnabled(event.target.checked)} /> Notify me when files arrive</label>{error && <p className="form-error" role="alert">{error}</p>}<button className="primary-action" type="button" disabled={!validName || !directory || saving} onClick={() => { void complete(); }}>{saving ? 'Saving…' : 'Finish setup'}</button></section></main>;
}

interface SettingsViewProps { snapshot: AppSnapshotViewModel; onBack: () => void; onSnapshot: (snapshot: BackendAppSnapshot) => void; }

function SettingsView({ snapshot, onBack, onSnapshot }: SettingsViewProps) {
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [deviceName, setDeviceName] = useState(snapshot.localDeviceName);
  const [receiveDirectory, setReceiveDirectory] = useState(snapshot.receiveDirectory ?? '');
  const [receivingEnabled, setReceivingEnabled] = useState(snapshot.lifecycle.receivingEnabled);
  const [listenAddress, setListenAddress] = useState(snapshot.settings.preferredListenAddress);
  const [launchAtLogin, setLaunchAtLogin] = useState(snapshot.launchAtLogin);
  const [notificationsEnabled, setNotificationsEnabled] = useState(snapshot.notificationsEnabled);
  const [automaticDeviceTrust, setAutomaticDeviceTrust] = useState(snapshot.settings.automaticDeviceTrust);
  const [historyRetentionDays, setHistoryRetentionDays] = useState<0 | 7 | 30 | 90>(snapshot.settings.historyRetentionDays as 0 | 7 | 30 | 90);
  const validListenAddress = /.+:\d{1,5}$/.test(listenAddress.trim()) && Number(listenAddress.trim().slice(listenAddress.trim().lastIndexOf(':') + 1)) <= 65535;
  const restore = () => { setDeviceName(snapshot.localDeviceName); setReceiveDirectory(snapshot.receiveDirectory ?? ''); setReceivingEnabled(snapshot.lifecycle.receivingEnabled); setListenAddress(snapshot.settings.preferredListenAddress); setLaunchAtLogin(snapshot.launchAtLogin); setNotificationsEnabled(snapshot.notificationsEnabled); setAutomaticDeviceTrust(snapshot.settings.automaticDeviceTrust); setHistoryRetentionDays(snapshot.settings.historyRetentionDays as 0 | 7 | 30 | 90); setError(null); setStatus(null); };
  async function changeDirectory() {
    setError(null);
    try {
      const next = await appBridge.chooseReceiveDirectory(); if (next) setReceiveDirectory(next);
    } catch { setError('Fileporter could not open the receive-folder picker.'); }
  }
  async function save() { if (!validListenAddress) { setError('Use a loopback or private listen address with a port between 0 and 65535.'); return; } setSaving(true); setError(null); setStatus(null); try { const next = await appBridge.updateSettings({ deviceName: deviceName.trim(), receiveDirectory, receivingEnabled, listenAddress: listenAddress.trim(), launchAtLogin, notificationsEnabled, automaticDeviceTrust, historyRetentionDays }); onSnapshot(next); setStatus('Settings saved.'); } catch { setError('Fileporter could not save your settings. The previous saved settings are still in use.'); } finally { setSaving(false); } }
  async function viewLogs() { setError(null); try { await appBridge.viewLogs(); setStatus('Opened Fileporter logs in your file manager.'); } catch { setError('Fileporter could not open its logs.'); } }
  async function exportLogs() { setError(null); try { const destination = await appBridge.exportLogs(); setStatus(destination ? 'Exported redacted diagnostics to the folder you selected.' : 'Log export was cancelled.'); } catch { setError('Fileporter could not export its logs.'); } }
  return (
    <main className="app-shell">
      <header className="app-header"><button className="back-button" type="button" onClick={onBack}><ArrowLeft size={18} /> Back</button><div className="brand">Settings</div></header>
      <section className="settings-card">
        <h1>Settings</h1><p>Changes are saved together. Fileporter validates the receive folder and listen address before applying them.</p>
        <label>Device name<input value={deviceName} maxLength={48} onChange={(event) => setDeviceName(event.target.value)} /></label>
        <label>Receive folder<span className="field-row"><input value={receiveDirectory} readOnly aria-label="Receive folder" /><button type="button" disabled={saving} onClick={() => { void changeDirectory(); }}><FolderOpen size={16} /> Choose</button></span></label>
        <label className="check-row"><input type="checkbox" checked={receivingEnabled} onChange={(event) => setReceivingEnabled(event.target.checked)} /> Accept incoming transfers</label>
        <label className="check-row"><input type="checkbox" checked={automaticDeviceTrust} onChange={(event) => setAutomaticDeviceTrust(event.target.checked)} /> Automatically trust authenticated Fileporter devices on this private network</label>
        <p className="muted">Turn this off to require matching-code confirmation before trusting a new device.</p>
        <label>Preferred listen address<input aria-invalid={!validListenAddress} value={listenAddress} onChange={(event) => setListenAddress(event.target.value)} placeholder="127.0.0.1:48721" /></label>
        <label>History retention<select aria-label="History retention" value={historyRetentionDays} onChange={(event) => setHistoryRetentionDays(Number(event.target.value) as 0 | 7 | 30 | 90)}><option value={7}>7 days</option><option value={30}>30 days</option><option value={90}>90 days</option><option value={0}>Forever</option></select></label>
        <label className="check-row"><input type="checkbox" checked={launchAtLogin} onChange={(event) => setLaunchAtLogin(event.target.checked)} /> Launch Fileporter when I sign in</label>
        <label className="check-row"><input type="checkbox" checked={notificationsEnabled} onChange={(event) => setNotificationsEnabled(event.target.checked)} /> Notify me when files arrive</label>
        <details className="settings-details"><summary>Network diagnostics</summary><dl>
          <dt>Listener</dt><dd>{snapshot.network.listening ? 'Listening' : 'Stopped'}</dd><dt>Bound endpoint</dt><dd>{snapshot.network.boundEndpoint ?? 'Not currently bound'}</dd><dt>Preferred endpoint</dt><dd>{snapshot.network.preferredListenAddress}</dd><dt>mDNS</dt><dd>{snapshot.network.mdnsState || 'State unavailable.'}</dd><dt>Interfaces</dt><dd>{snapshot.network.localInterfaceSummaries.length ? snapshot.network.localInterfaceSummaries.join(', ') : 'No active local interfaces reported.'}</dd><dt>Trusted online endpoints</dt><dd>{snapshot.network.trustedOnlineEndpoints.length ? snapshot.network.trustedOnlineEndpoints.join(', ') : 'No trusted peer is currently online.'}</dd><dt>Recent stable errors</dt><dd>{snapshot.network.recentErrorCodes.length ? snapshot.network.recentErrorCodes.join(', ') : 'None reported.'}</dd>
        </dl></details>
        <details className="settings-details"><summary>Trusted-device details</summary>{snapshot.trustedDevices.length ? snapshot.trustedDevices.map((device) => <p key={device.id}><strong>{device.name}</strong> · {device.state} · {device.certificateFingerprintShort} · {device.autoSend ? 'auto-send eligible' : 'not auto-send eligible'}{device.endpoint ? ` · ${device.endpoint}` : ''}</p>) : <p>No trusted devices yet.</p>}</details>
        <details className="settings-details"><summary>About Fileporter</summary><p>Version {snapshot.about.appVersion} · Protocol {snapshot.about.protocolVersion} · Database migration {snapshot.about.databaseMigrationVersion} · Staging {snapshot.about.ownedStagingBytes.toLocaleString()} bytes</p><div className="field-row">{snapshot.about.logsAvailable && <button type="button" onClick={() => { void viewLogs(); }}>View logs</button>}<button type="button" onClick={() => { void exportLogs(); }}>Export logs</button></div></details>
        {status && <p className="muted" role="status">{status}</p>}{error && <p className="form-error" role="alert">{error}</p>}
        <div className="field-row"><button type="button" disabled={saving} onClick={restore}>Discard changes</button><button className="primary-action" type="button" disabled={saving || !deviceName.trim() || !receiveDirectory || !validListenAddress} onClick={() => { void save(); }}>{saving ? 'Saving…' : 'Save settings'}</button></div>
      </section>
    </main>
  );
}
