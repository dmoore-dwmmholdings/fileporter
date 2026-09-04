import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Header, type Screen } from './components/Shell';
import { TransportView, type PickChoice } from './components/TransportView';
import { PadsView } from './components/PadsView';
import { LogView } from './components/LogView';
import { ConfigView } from './components/ConfigView';
import { OnboardingView } from './components/OnboardingView';
import { appBridge } from './lib/bridge';
import { emptySnapshot, toViewModel, type AppSnapshotViewModel, type BackendAppSnapshot } from './types/view-models';

export interface FileporterAppProps { initialSnapshot?: AppSnapshotViewModel; }

const ACTIVE_STATES = ['preparing', 'sending', 'verifying', 'receiving'];

export default function App({ initialSnapshot = emptySnapshot }: FileporterAppProps) {
  const [snapshot, setSnapshot] = useState(initialSnapshot);
  const [loadState, setLoadState] = useState<'loading' | 'ready' | 'error'>(initialSnapshot.revision > 0 ? 'ready' : 'loading');
  const [screen, setScreen] = useState<Screen>('transport');
  const revisionRef = useRef(initialSnapshot.revision);
  const [selectedDeviceIds, setSelectedDeviceIds] = useState<string[]>(() =>
    initialSnapshot.devices.filter((device) => device.state === 'online').map((device) => device.id));
  const [notice, setNotice] = useState<{ message: string; batchId?: string; bad?: boolean } | null>(null);
  const [stagedPaths, setStagedPaths] = useState<string[]>([]);
  const cancelNoticeRef = useRef<HTMLButtonElement>(null);
  const selectedIdsRef = useRef(selectedDeviceIds);
  // Recipients follow whoever is online until the user picks for themselves.
  // Capturing them once meant a launch that finished before discovery did left
  // the selection permanently empty, and every drop asked for a device instead
  // of sending.
  const recipientsChosenRef = useRef(false);
  selectedIdsRef.current = selectedDeviceIds;
  // Read at the moment a batch is submitted, so a snapshot that lands between
  // the drop and the enqueue cannot change whether it is queued.
  const onlineIdsRef = useRef<string[]>([]);
  onlineIdsRef.current = snapshot.devices.filter((device) => device.state === 'online').map((device) => device.id);

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
      setScreen('config');
      window.setTimeout(() => document.querySelector<HTMLButtonElement>('.nav.on')?.focus(), 0);
    }).then((stop) => { unlisten = stop; }).catch(() => undefined);
    return () => unlisten?.();
  }, []);

  const submitPaths = useCallback(async (paths: string[], targetDeviceIds = [...selectedIdsRef.current]) => {
    if (targetDeviceIds.length === 0) {
      setStagedPaths(paths);
      setNotice({ message: 'Pick a pad for these — a dark one holds them until it wakes.' });
      return;
    }
    // A dark pad is a legitimate destination: the batch is queued rather than
    // refused, and goes the moment that pad comes back.
    const onlineIds = new Set(onlineIdsRef.current);
    const queueOffline = targetDeviceIds.some((id) => !onlineIds.has(id));
    try {
      const queued = await appBridge.enqueuePaths(paths, targetDeviceIds, queueOffline);
      setStagedPaths([]);
      setNotice({
        message: queueOffline
          ? `Holding ${queued.itemCount} item${queued.itemCount === 1 ? '' : 's'} until that pad wakes.`
          : `Preparing ${queued.itemCount} item${queued.itemCount === 1 ? '' : 's'} for transport.`,
        batchId: queued.id
      });
    } catch {
      setNotice({ message: 'Fileporter could not start that transport. Your files remain where they are.', bad: true });
    }
  }, []);

  const selectPaths = useCallback(async (choice: PickChoice) => {
    try {
      const paths = choice === 'files' ? await appBridge.chooseFiles() : await appBridge.chooseDirectory();
      if (paths.length) await submitPaths(paths);
    } catch {
      setNotice({ message: 'The picker could not open. Try again, or drag items onto the pad.', bad: true });
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

  // Staged items go the moment a pad is picked, so the deck never holds a
  // payload the user already told Fileporter where to send.
  useEffect(() => {
    if (!stagedPaths.length || !selectedDeviceIds.length) return;
    const paths = stagedPaths;
    setStagedPaths([]);
    void submitPaths(paths, [...selectedDeviceIds]);
  }, [selectedDeviceIds, stagedPaths, submitPaths]);

  const toggleDevice = useCallback((id: string) => {
    recipientsChosenRef.current = true;
    setSelectedDeviceIds((selected) => selected.includes(id) ? selected.filter((value) => value !== id) : [...selected, id]);
  }, []);

  async function cancelQueuedNotice() {
    if (!notice?.batchId) return;
    const batchId = notice.batchId;
    setNotice(null);
    try { await appBridge.cancelBatch(batchId); }
    catch { setNotice({ message: 'Fileporter could not cancel that transport.', bad: true }); }
  }

  const online = useMemo(() => snapshot.devices.filter((device) => device.state === 'online').length, [snapshot]);
  const listening = snapshot.lifecycle.listening || snapshot.network.listening;
  const active = snapshot.transfers.filter((batch) => ACTIVE_STATES.includes(batch.state));
  const rate = active.map((batch) => batch.targets.map((target) => target.rateLabel).find(Boolean)).find(Boolean);

  const linkedCount = snapshot.trustedDevices.length;
  const statusLine = active.length
    ? rate ?? `${active.length} in flight`
    : linkedCount === 0 ? 'no pads linked' : `${online} of ${linkedCount} linked`;
  const statusSummary = `${listening ? (snapshot.lifecycle.receivingEnabled ? 'Listening and receiving' : 'Listening') : 'Offline'}; `
    + `${online} of ${linkedCount} linked pad${linkedCount === 1 ? '' : 's'} online; `
    + `${active.length} transport${active.length === 1 ? '' : 's'} in flight.`;

  if (loadState === 'loading') return <main className="q"><div className="q-shell-message" aria-live="polite">Waking the pad…</div></main>;
  if (loadState === 'error') {
    return (
      <main className="q">
        <div className="q-shell-message" role="alert">
          <strong>Fileporter couldn’t load.</strong>
          <button type="button" className="chip" onClick={() => { void hydrate(); }}>Try again</button>
        </div>
      </main>
    );
  }
  if (!snapshot.onboardingComplete) {
    return <OnboardingView onComplete={(next) => { revisionRef.current = next.revision; setSnapshot(toViewModel(next)); setScreen('transport'); }} />;
  }

  return (
    <main className="q">
      <Header
        screen={screen}
        onNavigate={setScreen}
        deviceName={snapshot.localDeviceName}
        statusLine={statusLine}
        statusSummary={statusSummary}
        online={listening}
      />

      {screen === 'transport' && (
        <TransportView
          snapshot={snapshot}
          selectedDeviceIds={selectedDeviceIds}
          onToggleDevice={toggleDevice}
          onPick={(choice) => { void selectPaths(choice); }}
          stagedPaths={stagedPaths}
        />
      )}
      {screen === 'pads' && <PadsView snapshot={snapshot} />}
      {screen === 'log' && <LogView snapshot={snapshot} onSnapshot={applySnapshot} />}
      {screen === 'config' && <ConfigView snapshot={snapshot} onSnapshot={applySnapshot} />}

      {notice && (
        <div className={notice.bad ? 'notice bad' : 'notice'} role="status" aria-live="polite">
          <span>{notice.message}</span>
          {notice.batchId && <button ref={cancelNoticeRef} type="button" className="chip-mini" onClick={() => { void cancelQueuedNotice(); }}>CANCEL</button>}
        </div>
      )}
    </main>
  );
}
