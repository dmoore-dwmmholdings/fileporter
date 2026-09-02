import { CircleDot, Link, ShieldCheck, Wifi, WifiOff } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { appBridge } from '../lib/bridge';
import type { AppSnapshotViewModel, NearbyDeviceViewModel, PendingPairing, TrustedDeviceViewModel } from '../types/view-models';

export function DevicesView({ snapshot }: { snapshot: AppSnapshotViewModel }) {
  const automatic = snapshot.settings.automaticDeviceTrust;
  return <section className="tab-panel" aria-labelledby="devices-heading"><div className="panel-heading"><div><h1 id="devices-heading">Devices</h1><p>{automatic ? 'Fileporter finds and securely remembers other Fileporter computers on this private network.' : 'New devices require matching-code confirmation before they become trusted.'}</p></div></div><NearbyDevices devices={snapshot.nearbyDevices} automatic={automatic} /><ManualPairing automatic={automatic} />{!automatic && snapshot.pendingPairings.map((pairing) => <PairingDialog key={pairing.id} pairing={pairing} />)}<DeviceList devices={snapshot.trustedDevices} /></section>;
}

function NearbyDevices({ devices, automatic }: { devices: NearbyDeviceViewModel[]; automatic: boolean }) {
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  async function pair(deviceId: string) { setBusyId(deviceId); setError(null); try { await appBridge.startPairingDiscovered(deviceId); } catch { setError('That nearby device is no longer available. Refresh the list or use its private address.'); } finally { setBusyId(null); } }
  return <section className="device-group" aria-labelledby="nearby-heading"><h2 id="nearby-heading">Nearby devices</h2>{devices.length ? devices.map((device) => <article className="device-row" key={device.deviceId}><span className="device-icon"><Wifi size={18} /></span><div><strong>{device.displayName}</strong><span>{device.endpoint} · protocol {device.protocolVersion}</span></div>{automatic ? <span className="muted" role="status">Connecting securely…</span> : <button type="button" disabled={busyId === device.deviceId} onClick={() => { void pair(device.deviceId); }}>{busyId === device.deviceId ? 'Starting…' : 'Pair'}</button>}</article>) : <p className="muted">{automatic ? 'Looking for Fileporter devices on this private network…' : 'No unpaired nearby devices yet. You can still add one by its private address.'}</p>}{error && <p className="form-error" role="alert">{error}</p>}</section>;
}

function ManualPairing({ automatic }: { automatic: boolean }) {
  const [endpoint, setEndpoint] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function start() { if (!endpoint.trim()) { setError('Enter the private endpoint supplied by your other device.'); return; } setBusy(true); setError(null); try { await appBridge.startPairingAtEndpoint(endpoint.trim()); setEndpoint(''); } catch { setError('Fileporter could not connect at that endpoint. Check the address and try again.'); } finally { setBusy(false); } }
  return <section className="address-form" aria-labelledby="pair-device-heading"><h2 id="pair-device-heading"><Link size={16} aria-hidden="true" /> Connect by address</h2><p className="muted">Use this fallback when your network blocks discovery. Enter a private-network address and port.{automatic ? ' Trust is recorded automatically after identity proof.' : ''}</p><label htmlFor="pair-endpoint">Private endpoint</label><div className="endpoint-row"><input id="pair-endpoint" value={endpoint} onChange={(event) => setEndpoint(event.target.value)} placeholder="192.168.1.24:48721" autoComplete="off" /><button type="button" disabled={busy} onClick={() => { void start(); }}>{busy ? 'Connecting…' : 'Connect'}</button></div>{error && <p className="form-error" role="alert">{error}</p>}</section>;
}

function DeviceList({ devices }: { devices: TrustedDeviceViewModel[] }) {
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [alias, setAlias] = useState('');
  async function rename(deviceId: string) { if (!alias.trim() || Array.from(alias.trim()).length > 128) { setError('Choose a local device name up to 128 characters.'); return; } try { setError(null); await appBridge.renameTrustedDevice(deviceId, alias.trim()); setEditing(null); } catch { setError('Fileporter could not rename this device.'); } }
  return <section className="device-group"><h2>Trusted devices</h2>{devices.length ? devices.map((device) => <article className="device-row" key={device.id}><span className="device-icon"><ShieldCheck size={18} /></span><div><strong>{device.name}</strong><span className={device.state === 'online' ? 'presence-online' : 'presence-offline'}>{device.state === 'online' ? <Wifi size={13} aria-hidden="true" /> : <WifiOff size={13} aria-hidden="true" />}{device.state === 'online' ? 'Online and ready to receive' : device.lastSeenAt === null ? 'Offline' : 'Offline — seen previously'}</span></div>{editing === device.id ? <span className="endpoint-row"><input aria-label={`Local name for ${device.name}`} value={alias} maxLength={128} onChange={(event) => setAlias(event.target.value)} /><button type="button" onClick={() => { void rename(device.id); }}>Save name</button></span> : <button type="button" onClick={() => { setEditing(device.id); setAlias(device.name); }}>Rename locally</button>}</article>) : <p className="muted">Trusted devices will appear here automatically when Fileporter finds them.</p>}{error && <p className="form-error" role="alert">{error}</p>}</section>;
}

function PairingDialog({ pairing }: { pairing: PendingPairing }) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const rejectRef = useRef<HTMLButtonElement>(null);
  const canConfirm = Boolean(pairing.sasCode);
  useEffect(() => { (canConfirm ? confirmRef : rejectRef).current?.focus(); }, [canConfirm]);
  async function respond(accept: boolean) { setBusy(true); setError(null); try { if (accept) await appBridge.confirmPairing(pairing.id); else await appBridge.rejectPairing(pairing.id); } catch { setError('The pairing confirmation could not be saved.'); } finally { setBusy(false); } }
  return <div className="modal-backdrop" role="presentation"><section className="pairing-modal" role="dialog" aria-modal="true" aria-labelledby={`pairing-${pairing.id}`} aria-describedby={`pairing-help-${pairing.id}`}><CircleDot size={22} aria-hidden="true" /><h2 id={`pairing-${pairing.id}`}>Confirm {pairing.remoteName}</h2><p id={`pairing-help-${pairing.id}`}>Compare this code with the one shown on {pairing.remoteName}. Confirm only when both devices show the same code.</p>{pairing.sasCode ? <output className="pair-code" aria-label={`Security code ${pairing.sasCode}`}>{pairing.sasCode}</output> : <p className="form-error" role="status">Waiting for a matching security code. Confirmation is unavailable until it appears.</p>}<p className="pairing-state">Other device: {pairing.remoteConfirmed ? 'confirmed' : 'waiting for confirmation'}</p>{error && <p className="form-error" role="alert">{error}</p>}<div className="modal-actions"><button ref={rejectRef} type="button" className="quiet-danger" disabled={busy} onClick={() => { void respond(false); }}>Reject pairing</button><button ref={confirmRef} type="button" className="primary-action" disabled={busy || !canConfirm} onClick={() => { void respond(true); }}>{busy ? 'Confirming...' : 'Confirm pairing'}</button></div></section></div>;
}
