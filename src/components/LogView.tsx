import { useState } from 'react';
import { Floor, Headline } from './Shell';
import { appBridge } from '../lib/bridge';
import { formatBytes, formatPeer, formatWhen } from '../lib/format';
import type {
  AppSnapshotViewModel, BackendAppSnapshot, BatchState, HistoryItemViewModel,
  HistoryTopLevelItemViewModel, TransferBatchViewModel, TrustedDeviceViewModel
} from '../types/view-models';

/** The design's plain-language reading of a batch state — no machine shorthand. */
const STATE_WORD: Record<BatchState, string> = {
  queued: 'Queued', waiting: 'Held', preparing: 'Preparing', sending: 'Sending', receiving: 'Receiving',
  verifying: 'Verifying', complete: 'Verified', partial: 'Partial', paused: 'Paused', cancelled: 'Cancelled', failed: 'Failed'
};
const WARN_STATES: BatchState[] = ['waiting', 'queued', 'paused', 'partial'];
const BAD_STATES: BatchState[] = ['failed', 'cancelled'];
const IN_FLIGHT: BatchState[] = ['preparing', 'sending', 'receiving', 'verifying'];

export function LogView({ snapshot, onSnapshot }: { snapshot: AppSnapshotViewModel; onSnapshot: (snapshot: BackendAppSnapshot) => void }) {
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const moved = snapshot.history.reduce((total, entry) => total + entry.items.reduce((sum, item) => sum + (item.size || 0), 0), 0);
  const count = snapshot.history.length;
  const retention = snapshot.settings.historyRetentionDays;
  const failedCount = snapshot.history.filter((entry) => BAD_STATES.includes(entry.state)).length;
  const inFlight = snapshot.transfers.filter((batch) => IN_FLIGHT.includes(batch.state));

  async function exportLogs() {
    setError(null); setStatus(null);
    try { const destination = await appBridge.exportLogs(); setStatus(destination ? 'Exported redacted diagnostics.' : 'Log export was cancelled.'); }
    catch { setError('Fileporter could not export its logs.'); }
  }

  return (
    <>
      <Floor variant="faint" />
      <div className="q-body scrolls" style={{ gap: 22, paddingTop: 20 }}>
        <Headline
          title={count > 0
            ? `${count} transport${count === 1 ? '' : 's'}.`
            : inFlight.length > 0 ? `${inFlight.length} in flight.` : 'Nothing yet.'}
          sub={count > 0
            ? `${formatBytes(moved)} moved${retention ? ` in the last ${retention} days` : ''}. Every one verified end to end.`
            : inFlight.length > 0
              ? 'Nothing has finished yet. Everything that lands is verified end to end.'
              : 'What you send and what arrives will be listed here, newest first.'}
          id="log-heading"
        />

        {inFlight.map((batch) => (
          <Flight key={batch.id} batch={batch} onSnapshot={onSnapshot} onError={setError} />
        ))}

        <div className="records fade">
          {snapshot.history.map((entry) => (
            <Record key={entry.id} entry={entry} trusted={snapshot.trustedDevices} onSnapshot={onSnapshot} />
          ))}
        </div>
      </div>

      <div className="q-foot">
        <span className="foot-detail">{retention ? `Kept ${retention} days` : 'Kept forever'}</span>
        <span className="foot-detail">Staging <span className="mono">{formatBytes(snapshot.about.ownedStagingBytes) || '0 B'}</span></span>
        <span>{failedCount === 0 ? 'Nothing has failed verification' : `${failedCount} did not verify`}</span>
        {status && <span role="status" style={{ color: 'var(--acc)' }}>{status}</span>}
        {error && <span role="alert" style={{ color: 'var(--danger)' }}>{error}</span>}
        <div className="q-spacer" />
        <button type="button" className="chip" onClick={() => { void exportLogs(); }}>Export log</button>
      </div>
    </>
  );
}

function Flight({ batch, onSnapshot, onError }: {
  batch: TransferBatchViewModel;
  onSnapshot: (snapshot: BackendAppSnapshot) => void;
  onError: (message: string | null) => void;
}) {
  const [busy, setBusy] = useState(false);
  const outbound = batch.state !== 'receiving';
  const rate = batch.targets.map((target) => target.rateLabel).find(Boolean);

  async function abort() {
    setBusy(true); onError(null);
    try { onSnapshot(await appBridge.cancelBatch(batch.id)); }
    catch { onError('Fileporter could not abort that transport.'); }
    finally { setBusy(false); }
  }

  return (
    <div className="flight fade">
      <div className="flight-head">
        <span className="flight-vec">{outbound ? 'OUTBOUND' : 'INBOUND'}</span>
        <span className="flight-name">{batch.label}</span>
        <span className="flight-meta">{STATE_WORD[batch.state]}</span>
        <div className="q-spacer" />
        {rate && <span className="flight-rate">{rate}</span>}
        <button type="button" className="chip-mini" disabled={busy} onClick={() => { void abort(); }}>ABORT</button>
      </div>
      <div className="flight-bar" role="progressbar" aria-valuenow={Math.round(batch.progress)} aria-valuemin={0} aria-valuemax={100} aria-label={`${batch.label} progress`}>
        <span style={{ width: `${Math.max(2, Math.min(100, batch.progress))}%` }} />
      </div>
      {batch.targets.length > 0 && (
        <div className="flight-targets">
          {batch.targets.map((target) => (
            <span key={target.id}>{target.deviceName} <span className="mono">{Math.round(target.progress)}%</span></span>
          ))}
        </div>
      )}
    </div>
  );
}

type NativeAction = 'reveal' | 'copy' | 'move';

function Record({ entry, trusted, onSnapshot }: {
  entry: HistoryItemViewModel;
  trusted: TrustedDeviceViewModel[];
  onSnapshot: (snapshot: BackendAppSnapshot) => void;
}) {
  const [open, setOpen] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const incoming = entry.direction === 'incoming';
  const openable = incoming && entry.state === 'complete' && entry.items.length > 0;
  const size = entry.items.reduce((total, item) => total + (item.size || 0), 0);
  const stateClass = BAD_STATES.includes(entry.state) ? 'state bad' : WARN_STATES.includes(entry.state) ? 'state hold' : 'state';

  async function retry() {
    setBusy(true); setRetryError(null);
    try { onSnapshot(await appBridge.retryBatch(entry.id)); }
    catch { setRetryError('Fileporter could not run that transport again.'); }
    finally { setBusy(false); }
  }

  return (
    <div>
      <div className="row record-line">
        <button
          className="record"
          type="button"
          aria-expanded={openable ? open : undefined}
          disabled={!openable}
          onClick={() => openable && setOpen((value) => !value)}
        >
          <span className="caret" aria-hidden="true">{openable ? '›' : ''}</span>
          <span className={incoming ? 'vec in' : 'vec'}>{incoming ? 'Received' : 'Sent'}</span>
          <span className="payload" title={entry.summary}>{entry.summary}</span>
          <span className="pad">{formatPeer(entry.peerName, trusted)}</span>
          <span className="size">{size > 0 ? formatBytes(size) : `${entry.items.length} item${entry.items.length === 1 ? '' : 's'}`}</span>
          <span className={stateClass}>{STATE_WORD[entry.state]}</span>
          <span className="when">{formatWhen(entry.timeLabel)}</span>
        </button>
        {entry.state === 'failed' && (
          <span className="act record-retry">
            <button type="button" className="chip-mini" disabled={busy} onClick={() => { void retry(); }} aria-label={`Run ${entry.summary} again`}>RETRY</button>
          </span>
        )}
      </div>
      {retryError && <p className="det-note bad" role="alert">{retryError}</p>}
      {openable && open && <Detail entry={entry} />}
    </div>
  );
}

function Detail({ entry }: { entry: HistoryItemViewModel }) {
  const items = entry.items;
  const [note, setNote] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);

  // The whole batch in one gesture, for an arrival of more items than anyone
  // wants to act on a row at a time.
  async function actAll(action: NativeAction) {
    setBusy(true); setFailed(false); setNote(null);
    try {
      if (action === 'reveal') await appBridge.revealCompletedBatch(entry.id);
      else if (action === 'copy') await appBridge.copyCompletedBatch(entry.id);
      else await appBridge.moveCompletedBatch(entry.id);
      setNote(action === 'reveal' ? 'Revealed everything that arrived.' : action === 'copy' ? 'Everything that arrived is on the clipboard.' : 'Everything that arrived is staged to move.');
    } catch {
      setFailed(true);
      setNote(`Fileporter could not ${action} the arrived files.`);
    } finally { setBusy(false); }
  }

  async function act(action: NativeAction, item: HistoryTopLevelItemViewModel) {
    setBusy(true); setFailed(false); setNote(null);
    try {
      if (action === 'reveal') await appBridge.revealItem(item.itemId);
      else if (action === 'copy') await appBridge.copyItem(item.itemId);
      else await appBridge.moveItem(item.itemId);
      setNote(action === 'reveal' ? `Revealed ${item.displayName}.` : action === 'copy' ? `${item.displayName} is on the clipboard.` : `${item.displayName} is staged to move.`);
    } catch {
      setFailed(true);
      setNote(`Fileporter could not ${action} ${item.displayName}.`);
    } finally { setBusy(false); }
  }

  return (
    <div className="det">
      {items.length > 1 && (
        <div className="det-all">
          <span className="hint">All {items.length} items</span>
          <span className="act det-actions">
            <button type="button" className="chip-mini" disabled={busy} onClick={() => { void actAll('reveal'); }}>REVEAL ALL</button>
            <button type="button" className="chip-mini" disabled={busy} onClick={() => { void actAll('copy'); }}>COPY ALL</button>
            <button type="button" className="chip-mini" disabled={busy} onClick={() => { void actAll('move'); }}>MOVE ALL</button>
          </span>
        </div>
      )}
      {items.map((item) => (
        <div className="det-item" key={item.itemId}>
          <span className="name" title={item.destinationLabel ? `Saved to ${item.destinationLabel}` : item.displayName}>{item.displayName}</span>
          <span className="size">{item.kind === 'directory' ? `${item.size} items` : formatBytes(item.size)}</span>
          {item.available ? (
            <span className="act det-actions">
              <button type="button" className="chip-mini" disabled={busy} onClick={() => { void act('reveal', item); }} aria-label={`Reveal ${item.displayName}`}>REVEAL</button>
              <button type="button" className="chip-mini" disabled={busy} onClick={() => { void act('copy', item); }} aria-label={`Copy ${item.displayName}`}>COPY</button>
              <button type="button" className="chip-mini" disabled={busy} onClick={() => { void act('move', item); }} aria-label={`Move ${item.displayName}`}>MOVE</button>
            </span>
          ) : (
            <span className="det-actions hint">Moved or deleted</span>
          )}
        </div>
      ))}
      <p className={failed ? 'det-note bad' : 'det-note'} role="status">
        {note ?? 'Move stages the system clipboard — paste in your file manager to finish it.'}
      </p>
    </div>
  );
}
