import { useEffect, useRef, useState } from 'react';
import { PadTile } from './PadArt';
import { Floor, Headline } from './Shell';
import { appBridge } from '../lib/bridge';
import { formatWhen } from '../lib/format';
import type { AppSnapshotViewModel, NearbyDeviceViewModel, PendingPairing, QueuedBatch, TrustedDeviceViewModel } from '../types/view-models';

export function PadsView({ snapshot }: { snapshot: AppSnapshotViewModel }) {
  const automatic = snapshot.settings.automaticDeviceTrust;
  const [error, setError] = useState<string | null>(null);
  const held = snapshot.queuedBatches.filter((batch) => batch.waitingForAvailable || batch.state === 'queued' || batch.state === 'waiting');

  return (
    <>
      <Floor />
      <div className="q-body center scrolls">
        <Headline title="Pads" id="pads-heading" />

        <div className="pad-grid fade">
          {snapshot.trustedDevices.length
            ? snapshot.trustedDevices.map((device) => <Tile key={device.id} device={device} onError={setError} />)
            : (
              // An unlit pad says "none yet" in the same object language the rest
              // of the screen speaks, rather than dropping to a line of prose.
              <div className="tile dark" style={{ gridColumn: '1 / -1', maxWidth: 240, margin: '0 auto' }}>
                <PadTile />
                <strong style={{ color: 'var(--dim)' }}>No pads</strong>
              </div>
            )}
        </div>

        <div className="pad-rows fade">
          {snapshot.nearbyDevices.map((device) => <NearbyRow key={device.deviceId} device={device} automatic={automatic} onError={setError} />)}
          {snapshot.pendingPairings.map((pairing) => <PendingRow key={pairing.id} pairing={pairing} />)}
          {held.map((batch) => <HeldRow key={batch.id} batch={batch} devices={snapshot.trustedDevices} onError={setError} />)}
          {error && <p className="err" role="alert" style={{ padding: '8px 0' }}>{error}</p>}
        </div>
      </div>

      <AddPad onError={setError} />
    </>
  );
}

function Tile({ device, onError }: { device: TrustedDeviceViewModel; onError: (message: string | null) => void }) {
  const [editing, setEditing] = useState(false);
  const [alias, setAlias] = useState(device.name);
  const renameRef = useRef<HTMLButtonElement>(null);
  const wasEditing = useRef(false);
  const linked = device.state === 'online';
  useEffect(() => {
    if (wasEditing.current && !editing) renameRef.current?.focus();
    wasEditing.current = editing;
  }, [editing]);

  async function rename() {
    const next = alias.trim();
    if (!next || Array.from(next).length > 128) { onError('Name must be 1–128 characters'); return; }
    try { onError(null); await appBridge.renameTrustedDevice(device.id, next); setEditing(false); }
    catch { onError('Rename failed'); }
  }

  return (
    <div className={linked ? 'tile row' : 'tile row dark'}>
      <PadTile />
      {editing ? (
        <span className="tile-rename">
          <input
            className="field mono"
            aria-label={`Local name for ${device.name}`}
            value={alias}
            maxLength={128}
            autoFocus
            onChange={(event) => setAlias(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void rename(); } if (event.key === 'Escape') setEditing(false); }}
          />
          <button type="button" className="chip-mini" onClick={() => { void rename(); }}>SAVE</button>
        </span>
      ) : (
        <strong>{device.name}</strong>
      )}
      <span className={linked ? 'state linked' : 'state'}>
        {linked
          ? 'Linked'
          : device.lastSeenAt ? `Dark · ${formatWhen(String(device.lastSeenAt))}` : 'Dark'}
      </span>
      <span className="fp">{device.certificateFingerprintShort}</span>
      {!editing && (
        <span className="act" style={{ marginTop: 6 }}>
          <button ref={renameRef} type="button" className="chip-mini" onClick={() => { setAlias(device.name); setEditing(true); }}>RENAME</button>
        </span>
      )}
    </div>
  );
}

function NearbyRow({ device, automatic, onError }: { device: NearbyDeviceViewModel; automatic: boolean; onError: (message: string | null) => void }) {
  const [busy, setBusy] = useState(false);
  async function link() {
    setBusy(true); onError(null);
    try { await appBridge.startPairingDiscovered(device.deviceId); }
    catch { onError(`${device.displayName} unreachable`); }
    finally { setBusy(false); }
  }
  return (
    <div className="row pad-row">
      <span className="beacon" aria-hidden="true" />
      <span className="who">{device.displayName}</span>
      <span className="addr">{device.endpoint}</span>
      <div className="q-spacer" />
      {automatic
        ? <span className="proving" role="status">proving…</span>
        : <span className="act"><button type="button" className="chip-mini" disabled={busy} onClick={() => { void link(); }}>{busy ? 'LINKING' : 'LINK'}</button></span>}
    </div>
  );
}

function PendingRow({ pairing }: { pairing: PendingPairing }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const rejectRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLElement>(null);
  const canConfirm = Boolean(pairing.sasCode);
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => { if (previous?.isConnected) previous.focus(); };
  }, []);
  useEffect(() => { (canConfirm ? confirmRef : rejectRef).current?.focus(); }, [canConfirm]);

  async function respond(accept: boolean) {
    setBusy(true); setError(null);
    try { if (accept) await appBridge.confirmPairing(pairing.id); else await appBridge.rejectPairing(pairing.id); }
    catch { setError('Failed'); }
    finally { setBusy(false); }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        ref={dialogRef} tabIndex={-1}
        className="pairing-modal" role="dialog" aria-modal="true" aria-labelledby={`pairing-${pairing.id}`} aria-describedby={`pairing-help-${pairing.id}`}
        onKeyDown={(event) => {
          if (event.key !== 'Tab') return;
          const buttons = [rejectRef.current, confirmRef.current].filter((button): button is HTMLButtonElement => Boolean(button && !button.disabled));
          const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
          event.preventDefault();
          if (!buttons.length) { dialogRef.current?.focus(); return; }
          const next = (index + (event.shiftKey ? buttons.length - 1 : 1)) % buttons.length;
          buttons[next]?.focus();
        }}
      >
        <h2 id={`pairing-${pairing.id}`}>Confirm {pairing.remoteName}</h2>
        {/* Comparing the code on both pads is the whole check; it stays. */}
        <p id={`pairing-help-${pairing.id}`}>Same code on both pads?</p>
        {pairing.sasCode
          ? <output className="pair-code" aria-label={`Security code ${pairing.sasCode}`}>{pairing.sasCode}</output>
          : <p className="err" role="status">No code yet</p>}
        <p className="hint">Other pad · {pairing.remoteConfirmed ? 'confirmed' : 'waiting'}</p>
        {error && <p className="err" role="alert">{error}</p>}
        <div className="modal-actions">
          <button ref={rejectRef} type="button" className="chip" disabled={busy} onClick={() => { void respond(false); }}>Reject</button>
          <button ref={confirmRef} type="button" className="chip solid" disabled={busy || !canConfirm} onClick={() => { void respond(true); }}>
            {busy ? 'Confirming…' : 'Confirm link'}
          </button>
        </div>
      </section>
    </div>
  );
}

function HeldRow({ batch, devices, onError }: { batch: QueuedBatch; devices: TrustedDeviceViewModel[]; onError: (message: string | null) => void }) {
  const [busy, setBusy] = useState(false);
  const target = devices.find((device) => batch.targetDeviceIds.includes(device.id));
  async function discard() {
    setBusy(true); onError(null);
    try { await appBridge.cancelBatch(batch.id); }
    catch { onError('Discard failed'); }
    finally { setBusy(false); }
  }
  return (
    <div className="row pad-row">
      <span className="beacon idle" aria-hidden="true" />
      <span className="addr" style={{ color: 'var(--soft)', fontSize: 13 }}>{batch.itemCount} item{batch.itemCount === 1 ? '' : 's'}</span>
      <div className="q-spacer" />
      <span className="held">held · {target?.name ?? 'dark pad'}</span>
      <span className="act">
        <button type="button" className="chip-mini danger" disabled={busy} onClick={() => { void discard(); }}>DISCARD</button>
      </span>
    </div>
  );
}

function AddPad({ onError }: { onError: (message: string | null) => void }) {
  const [endpoint, setEndpoint] = useState('');
  const [busy, setBusy] = useState(false);
  async function add() {
    if (!endpoint.trim()) { onError('Enter an address'); return; }
    setBusy(true); onError(null);
    try { await appBridge.startPairingAtEndpoint(endpoint.trim()); setEndpoint(''); }
    catch { onError('No pad at that address'); }
    finally { setBusy(false); }
  }
  return (
    <div className="q-foot align-end">
      <div className="add-pad">
        <label className="lbl" htmlFor="pad-endpoint">Add by address</label>
        <span className="entry">
          <input
            id="pad-endpoint"
            className="field mono"
            value={endpoint}
            placeholder="192.168.1.24:48721"
            autoComplete="off"
            onChange={(event) => setEndpoint(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void add(); } }}
          />
          <button type="button" className="chip" disabled={busy} onClick={() => { void add(); }}>{busy ? 'Adding…' : 'Add'}</button>
        </span>
      </div>
      <div className="q-spacer" />
    </div>
  );
}
