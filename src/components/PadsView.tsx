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
        <Headline
          title="Your pads."
          sub={automatic
            ? 'Each one proves its own identity before a link is kept. A pad that goes dark holds your pattern until it wakes.'
            : 'Each new pad needs a matching code on both sides before a link is kept. A pad that goes dark holds your pattern until it wakes.'}
          id="pads-heading"
        />

        <div className="pad-grid fade">
          {snapshot.trustedDevices.length
            ? snapshot.trustedDevices.map((device) => <Tile key={device.id} device={device} onError={setError} />)
            : (
              // An unlit pad says "none yet" in the same object language the rest
              // of the screen speaks, rather than dropping to a line of prose.
              <div className="tile dark" style={{ gridColumn: '1 / -1', maxWidth: 240, margin: '0 auto' }}>
                <PadTile />
                <strong style={{ color: 'var(--dim)' }}>No pad yet</strong>
                <span className="state">Open Fileporter on another computer</span>
                <span className="fp">on this network</span>
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
  const linked = device.state === 'online';

  async function rename() {
    const next = alias.trim();
    if (!next || Array.from(next).length > 128) { onError('Choose a local name of up to 128 characters.'); return; }
    try { onError(null); await appBridge.renameTrustedDevice(device.id, next); setEditing(false); }
    catch { onError(`Fileporter could not rename ${device.name}.`); }
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
          ? device.autoSend ? 'Linked · beam ready' : 'Linked'
          : device.lastSeenAt ? `Dark · last echo ${formatWhen(String(device.lastSeenAt))}` : 'Dark · never seen'}
      </span>
      <span className="fp">{device.certificateFingerprintShort}</span>
      {!editing && (
        <span className="act" style={{ marginTop: 6 }}>
          <button type="button" className="chip-mini" onClick={() => { setAlias(device.name); setEditing(true); }}>RENAME</button>
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
    catch { onError(`${device.displayName} is no longer reachable. Try its address instead.`); }
    finally { setBusy(false); }
  }
  return (
    <div className="row pad-row">
      <span className="beacon" aria-hidden="true" />
      <span className="who">{device.displayName}</span>
      <span className="addr">{device.endpoint}</span>
      <div className="q-spacer" />
      {automatic
        ? <span className="proving" role="status">proving identity…</span>
        : <span className="act"><button type="button" className="chip-mini" disabled={busy} onClick={() => { void link(); }}>{busy ? 'LINKING' : 'LINK'}</button></span>}
    </div>
  );
}

function PendingRow({ pairing }: { pairing: PendingPairing }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const rejectRef = useRef<HTMLButtonElement>(null);
  const canConfirm = Boolean(pairing.sasCode);
  useEffect(() => { (canConfirm ? confirmRef : rejectRef).current?.focus(); }, [canConfirm]);

  async function respond(accept: boolean) {
    setBusy(true); setError(null);
    try { if (accept) await appBridge.confirmPairing(pairing.id); else await appBridge.rejectPairing(pairing.id); }
    catch { setError('That confirmation could not be saved.'); }
    finally { setBusy(false); }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section className="pairing-modal" role="dialog" aria-modal="true" aria-labelledby={`pairing-${pairing.id}`} aria-describedby={`pairing-help-${pairing.id}`}>
        <h2 id={`pairing-${pairing.id}`}>Confirm {pairing.remoteName}</h2>
        <p id={`pairing-help-${pairing.id}`}>
          Compare this code with the one on {pairing.remoteName}. Confirm only when both pads show the same code.
        </p>
        {pairing.sasCode
          ? <output className="pair-code" aria-label={`Security code ${pairing.sasCode}`}>{pairing.sasCode}</output>
          : <p className="err" role="status">Waiting for a matching code. Confirmation is unavailable until it appears.</p>}
        <p className="hint">Other pad: {pairing.remoteConfirmed ? 'confirmed' : 'waiting for confirmation'}</p>
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
    catch { onError('Fileporter could not discard that held pattern.'); }
    finally { setBusy(false); }
  }
  return (
    <div className="row pad-row">
      <span className="beacon idle" aria-hidden="true" />
      <span className="addr" style={{ color: 'var(--soft)', fontSize: 13 }}>{batch.itemCount} item{batch.itemCount === 1 ? '' : 's'}</span>
      <div className="q-spacer" />
      <span className="held">held for {target?.name ?? 'a dark pad'}</span>
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
    if (!endpoint.trim()) { onError('Enter the private address shown on your other pad.'); return; }
    setBusy(true); onError(null);
    try { await appBridge.startPairingAtEndpoint(endpoint.trim()); setEndpoint(''); }
    catch { onError('Fileporter could not reach a pad at that address.'); }
    finally { setBusy(false); }
  }
  return (
    <div className="q-foot align-end">
      <label className="add-pad">
        <span className="lbl">Add a pad by address</span>
        <span className="entry">
          <input
            className="field mono"
            value={endpoint}
            placeholder="192.168.1.24:48721"
            autoComplete="off"
            onChange={(event) => setEndpoint(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void add(); } }}
          />
          <button type="button" className="chip" disabled={busy} onClick={() => { void add(); }}>{busy ? 'Adding…' : 'Add'}</button>
        </span>
      </label>
      <div className="q-spacer" />
      <span className="foot-detail">A revoked pad stays revoked · it cannot quietly relink</span>
    </div>
  );
}
