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
    catch { setError('Fileporter could not open the folder picker.'); }
  }

  async function apply() {
    if (!validListenAddress) { setError('Use a loopback or private address with a port up to 65535.'); return; }
    setSaving(true); setError(null); setStatus(null);
    try {
      onSnapshot(await appBridge.updateSettings({
        deviceName: deviceName.trim(), receiveDirectory, receivingEnabled, listenAddress: trimmed,
        launchAtLogin, notificationsEnabled, automaticDeviceTrust, historyRetentionDays
      }));
      setStatus('Applied.');
    } catch { setError('Fileporter could not apply these changes. The last saved settings are still in use.'); }
    finally { setSaving(false); }
  }

  async function viewLogs() {
    setError(null);
    try { await appBridge.viewLogs(); setStatus('Opened the log folder.'); }
    catch { setError('Fileporter could not open its logs.'); }
  }

  const toggles: Array<{ key: string; label: string; note: string; value: boolean; set: (value: boolean) => void }> = [
    { key: 'receive', label: 'Accept inbound transports', note: 'Turn off and this pad stays visible but reassembles nothing.', value: receivingEnabled, set: setReceivingEnabled },
    { key: 'trust', label: 'Link authenticated pads automatically', note: 'Off requires a matching code on both pads before a link is kept.', value: automaticDeviceTrust, set: setAutomaticDeviceTrust },
    { key: 'launch', label: 'Bring the pad online at sign-in', note: 'Fileporter runs in the tray so transports land while the window is closed.', value: launchAtLogin, set: setLaunchAtLogin },
    { key: 'notify', label: 'Notify me when something arrives', note: 'A system notification naming what arrived and from which pad.', value: notificationsEnabled, set: setNotificationsEnabled }
  ];

  return (
    <>
      <Floor variant="faint" />
      <div className="q-body center scrolls">
        <Headline title="This pad." sub="Changes apply together. The folder and the address are checked before anything is saved." id="config-heading" />

        <div className="cfg-grid fade">
          <div className="cfg-col">
            <label className="cfg-field">
              <span className="lbl">Name other pads see</span>
              <input className="field" style={{ fontSize: 15 }} aria-label="Name other pads see" value={deviceName} maxLength={48} onChange={(event) => setDeviceName(event.target.value)} />
              <span className="hint">
                {snapshot.localDeviceId
                  ? <>Identity <span className="mono" style={{ color: 'var(--acc)' }}>{shortId(snapshot.localDeviceId)}</span>, pinned by your other pads.</>
                  : 'This is what your other pads will call it.'}
              </span>
            </label>

            <label className="cfg-field">
              <span className="lbl">Where arrivals land</span>
              <span className="entry">
                <input className="field mono" style={{ fontSize: 13.5 }} value={receiveDirectory} readOnly aria-label="Where arrivals land" />
                <button type="button" className="chip" disabled={saving} onClick={() => { void choose(); }}>Choose</button>
              </span>
              <span className="hint">Nothing is ever overwritten. A name that already exists lands beside it, numbered.</span>
            </label>

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
              <span className="hint">Loopback or private ranges only. Port 0 lets the system choose.</span>
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
                  <span className="hint">{toggle.note}</span>
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
            {dirty ? 'Not applied yet' : status ?? 'Everything here is in use'}
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
    <div className="diag fade">
      <details className="diag">
        <summary>Network diagnostics</summary>
        <dl>
          <dt>Listener</dt><dd>{snapshot.network.listening ? 'Listening' : 'Stopped'}</dd>
          <dt>Bound endpoint</dt><dd>{snapshot.network.boundEndpoint ?? 'Not currently bound'}</dd>
          <dt>Preferred endpoint</dt><dd>{snapshot.network.preferredListenAddress}</dd>
          <dt>mDNS</dt><dd>{snapshot.network.mdnsState || 'State unavailable.'}</dd>
          <dt>Interfaces</dt><dd>{snapshot.network.localInterfaceSummaries.length ? snapshot.network.localInterfaceSummaries.join(', ') : 'No active local interfaces reported.'}</dd>
          <dt>Trusted online endpoints</dt><dd>{snapshot.network.trustedOnlineEndpoints.length ? snapshot.network.trustedOnlineEndpoints.join(', ') : 'No trusted peer is currently online.'}</dd>
          <dt>Recent stable errors</dt><dd>{snapshot.network.recentErrorCodes.length ? snapshot.network.recentErrorCodes.join(', ') : 'None reported.'}</dd>
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
