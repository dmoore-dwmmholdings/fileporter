import { useState } from 'react';
import { Floor, Headline } from './Shell';
import { appBridge } from '../lib/bridge';
import type { AppSnapshotViewModel, BackendAppSnapshot } from '../types/view-models';

type Retention = 0 | 7 | 30 | 90;
const RETENTIONS: Retention[] = [7, 30, 90, 0];

export function ConfigView({ snapshot, onSnapshot }: { snapshot: AppSnapshotViewModel; onSnapshot: (snapshot: BackendAppSnapshot) => void }) {
  const [deviceName, setDeviceName] = useState(snapshot.localDeviceName);
  const [receiveDirectory, setReceiveDirectory] = useState(snapshot.receiveDirectory ?? '');
  const [listenAddress, setListenAddress] = useState(snapshot.settings.preferredListenAddress);
  const [historyRetentionDays, setHistoryRetentionDays] = useState<Retention>(snapshot.settings.historyRetentionDays as Retention);
  const [receivingEnabled, setReceivingEnabled] = useState(snapshot.lifecycle.receivingEnabled);
  const [automaticDeviceTrust, setAutomaticDeviceTrust] = useState(snapshot.settings.automaticDeviceTrust);
  const [launchAtLogin, setLaunchAtLogin] = useState(snapshot.launchAtLogin);
  const [notificationsEnabled, setNotificationsEnabled] = useState(snapshot.notificationsEnabled);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  const trimmed = listenAddress.trim();
  const validListenAddress = /.+:\d{1,5}$/.test(trimmed) && Number(trimmed.slice(trimmed.lastIndexOf(':') + 1)) <= 65535;

  const dirty = deviceName !== snapshot.localDeviceName
    || receiveDirectory !== (snapshot.receiveDirectory ?? '')
    || listenAddress !== snapshot.settings.preferredListenAddress
    || historyRetentionDays !== snapshot.settings.historyRetentionDays
    || receivingEnabled !== snapshot.lifecycle.receivingEnabled
    || automaticDeviceTrust !== snapshot.settings.automaticDeviceTrust
    || launchAtLogin !== snapshot.launchAtLogin
    || notificationsEnabled !== snapshot.notificationsEnabled;

  function discard() {
    setDeviceName(snapshot.localDeviceName);
    setReceiveDirectory(snapshot.receiveDirectory ?? '');
    setListenAddress(snapshot.settings.preferredListenAddress);
    setHistoryRetentionDays(snapshot.settings.historyRetentionDays as Retention);
    setReceivingEnabled(snapshot.lifecycle.receivingEnabled);
    setAutomaticDeviceTrust(snapshot.settings.automaticDeviceTrust);
    setLaunchAtLogin(snapshot.launchAtLogin);
    setNotificationsEnabled(snapshot.notificationsEnabled);
    setError(null); setStatus(null);
  }

  async function choose() {
    setError(null);
    try { const next = await appBridge.chooseReceiveDirectory(); if (next) setReceiveDirectory(next); }
    catch { setError('Picker failed'); }
  }

  async function apply() {
    if (!validListenAddress) { setError('Invalid address'); return; }
    setSaving(true); setError(null); setStatus(null);
    try {
      onSnapshot(await appBridge.updateSettings({
        deviceName: deviceName.trim(), receiveDirectory, receivingEnabled, listenAddress: trimmed,
        launchAtLogin, notificationsEnabled, automaticDeviceTrust, historyRetentionDays
      }));
      setStatus('Applied');
    } catch { setError('Apply failed'); }
    finally { setSaving(false); }
  }

  async function viewLogs() {
    setError(null);
    try { await appBridge.viewLogs(); setStatus('Opened'); }
    catch { setError('Could not open logs'); }
  }

  const toggles: Array<{ key: string; label: string; value: boolean; set: (value: boolean) => void }> = [
    { key: 'receive', label: 'Accept transports', value: receivingEnabled, set: setReceivingEnabled },
    { key: 'trust', label: 'Link pads automatically', value: automaticDeviceTrust, set: setAutomaticDeviceTrust },
    { key: 'launch', label: 'Start at sign-in', value: launchAtLogin, set: setLaunchAtLogin },
    { key: 'notify', label: 'Notify on arrival', value: notificationsEnabled, set: setNotificationsEnabled }
  ];

  return (
    <>
      <Floor variant="faint" />
      <div className="q-body center scrolls">
        <Headline title="This pad" id="config-heading" />

        <div className="cfg-grid fade">
          <div className="cfg-col">
            <label className="cfg-field">
              <span className="lbl">Name other pads see</span>
              <input className="field" style={{ fontSize: 15 }} aria-label="Name other pads see" value={deviceName} maxLength={48} onChange={(event) => setDeviceName(event.target.value)} />
              {snapshot.localDeviceId
                ? <span className="hint mono" style={{ color: 'var(--acc)' }}>{shortId(snapshot.localDeviceId)}</span>
                : null}
            </label>

            <div className="cfg-field">
              <label className="lbl" htmlFor="receive-directory">Where arrivals land</label>
              <span className="entry">
                <input id="receive-directory" className="field mono" style={{ fontSize: 13.5 }} value={receiveDirectory} readOnly />
                <button type="button" className="chip" disabled={saving} onClick={() => { void choose(); }}>Choose</button>
              </span>
            </div>

            <label className="cfg-field">
              <span className="lbl">Preferred listen address</span>
              <input
                className="field mono"
                style={{ fontSize: 13.5 }}
                value={listenAddress}
                aria-label="Preferred listen address"
                aria-invalid={!validListenAddress}
                placeholder="0.0.0.0:48721"
                onChange={(event) => setListenAddress(event.target.value)}
              />
            </label>

            <div className="cfg-field">
              <span className="lbl" id="retention-label">Keep the log for</span>
              <div className="cfg-retention" role="group" aria-labelledby="retention-label">
                {RETENTIONS.map((days) => (
                  <button
                    key={days}
                    type="button"
                    className={historyRetentionDays === days ? 'ret on' : 'ret'}
                    aria-pressed={historyRetentionDays === days}
                    onClick={() => setHistoryRetentionDays(days)}
                  >
                    {days === 0 ? 'Forever' : `${days} days`}
                  </button>
                ))}
              </div>
            </div>
          </div>

          <div className="cfg-toggles">
            {toggles.map((toggle) => (
              <button key={toggle.key} type="button" className="tog" role="switch" aria-label={toggle.label} aria-checked={toggle.value} onClick={() => toggle.set(!toggle.value)}>
                <span className={toggle.value ? 'sw on' : 'sw'} aria-hidden="true"><i /></span>
                <span className="tog-label">
                  <span>{toggle.label}</span>
                </span>
              </button>
            ))}
          </div>
        </div>

        <Diagnostics snapshot={snapshot} />
      </div>

      <div className="q-foot">
        <span className="foot-detail">
          {snapshot.network.listening
            ? <>Listening on <span className="mono">{snapshot.network.boundEndpoint ?? snapshot.network.preferredListenAddress}</span></>
            : 'Not listening'}
        </span>
        <span className="foot-detail">{snapshot.network.mdnsState || 'Beacon state unknown'}</span>
        <span className="foot-detail">{snapshot.trustedDevices.length} pad{snapshot.trustedDevices.length === 1 ? '' : 's'} linked</span>
        {snapshot.about.logsAvailable && <button type="button" className="chip foot-detail" onClick={() => { void viewLogs(); }}>Logs</button>}
        <div className="q-spacer" />
        {error
          ? <span className="err" role="alert">{error}</span>
          : <span style={{ fontSize: 12.5, color: dirty ? 'var(--hold)' : 'var(--dimmer)' }} role="status">
            {dirty ? 'Unsaved' : status ?? ''}
          </span>}
        <button type="button" className="chip" disabled={saving || !dirty} onClick={discard}>Discard</button>
        <button
          type="button"
          className={dirty ? 'chip solid' : 'chip'}
          disabled={saving || !dirty || !deviceName.trim() || !receiveDirectory || !validListenAddress}
          onClick={() => { void apply(); }}
        >
          {saving ? 'Applying…' : 'Apply'}
        </button>
      </div>
    </>
  );
}

/** Identities are long base32 strings; the board shows two readable groups. */
function shortId(id: string): string {
  const upper = id.toUpperCase();
  return upper.length <= 9 ? upper : `${upper.slice(0, 4)} · ${upper.slice(-4)}`;
}

function Diagnostics({ snapshot }: { snapshot: AppSnapshotViewModel }) {
  return (
    <div className="diag-group fade">
      <details className="diag">
        <summary>Network diagnostics</summary>
        <dl>
          <dt>Listener</dt><dd>{snapshot.network.listening ? 'Listening' : 'Stopped'}</dd>
          <dt>Bound endpoint</dt><dd>{snapshot.network.boundEndpoint ?? '—'}</dd>
          <dt>Preferred endpoint</dt><dd>{snapshot.network.preferredListenAddress}</dd>
          <dt>mDNS</dt><dd>{snapshot.network.mdnsState || '—'}</dd>
          <dt>Interfaces</dt><dd>{snapshot.network.localInterfaceSummaries.join(', ') || '—'}</dd>
          <dt>Trusted online endpoints</dt><dd>{snapshot.network.trustedOnlineEndpoints.join(', ') || '—'}</dd>
          <dt>Recent stable errors</dt><dd>{snapshot.network.recentErrorCodes.join(', ') || '—'}</dd>
        </dl>
      </details>
      <details className="diag">
        <summary>About Fileporter</summary>
        <p>
          Version {snapshot.about.appVersion} · Protocol {snapshot.about.protocolVersion} · Database migration {snapshot.about.databaseMigrationVersion} · Staging {snapshot.about.ownedStagingBytes.toLocaleString()} bytes
        </p>
      </details>
    </div>
  );
}
