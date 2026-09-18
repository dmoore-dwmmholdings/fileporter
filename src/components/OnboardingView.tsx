import { useState } from 'react';
import { TransportPad } from './PadArt';
import { BrandMark, Floor } from './Shell';
import { appBridge } from '../lib/bridge';
import type { BackendAppSnapshot } from '../types/view-models';

/**
 * First run. The pad is dim until the settings are saved and the listener is
 * actually advertising — the art is a readout, not decoration.
 */
export function OnboardingView({ onComplete }: { onComplete: (snapshot: BackendAppSnapshot) => void }) {
  const [deviceName, setDeviceName] = useState('');
  const [directory, setDirectory] = useState<string | null>(null);
  const [launchAtLogin, setLaunchAtLogin] = useState(true);
  const [notificationsEnabled, setNotificationsEnabled] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [live, setLive] = useState(false);

  const validName = deviceName.trim().length >= 1 && Array.from(deviceName.trim()).length <= 48;
  const ready = validName && Boolean(directory);

  async function choose() {
    setError(null);
    try { const paths = await appBridge.chooseDirectory(); setDirectory(paths[0] ?? null); }
    catch { setError('Picker failed'); }
  }

  async function engage() {
    if (!ready || !directory) { setError('Name and folder required'); return; }
    setSaving(true); setError(null);
    try {
      const next = await appBridge.completeOnboarding({
        deviceName: deviceName.trim(), receiveDirectory: directory, launchAtLogin, notificationsEnabled, automaticDeviceTrust: true
      });
      // Let the pad light before the shell swaps, so bringing it online reads as
      // one motion rather than a screen change.
      setLive(true);
      window.setTimeout(() => onComplete(next), 620);
    } catch {
      setError('Setup failed');
      setSaving(false);
    }
  }

  const toggles = [
    { key: 'launch', label: 'Start at sign-in', value: launchAtLogin, set: setLaunchAtLogin },
    { key: 'notify', label: 'Notify on arrival', value: notificationsEnabled, set: setNotificationsEnabled }
  ];

  return (
    <main className={live ? 'q ob-ready' : 'q'}>
      <Floor variant="tall" />

      <header className="q-header">
        <span className="q-brand">
          <BrandMark />
          <span>Fileporter</span>
        </span>
        <div className="q-spacer" />
        <span className="q-status">
          <span className={live ? 'q-dot' : 'q-dot off'} aria-hidden="true" />
          {live ? 'online · advertising' : 'offline'}
        </span>
      </header>

      <div className="q-body scrolls" style={{ paddingTop: 22 }}>
        <div className="q-headline fade">
          <h1>{live ? 'Pad is live' : 'Set up this pad'}</h1>
        </div>

        <div className="ob-form fade">
          <label className="cfg-field">
            <span className="lbl">Name this pad</span>
            <input
              className="field"
              style={{ fontSize: 15 }}
              aria-label="Name this pad"
              value={deviceName}
              maxLength={48}
              placeholder="This computer"
              autoFocus
              onChange={(event) => setDeviceName(event.target.value)}
            />
          </label>

          <label className="cfg-field">
            <span className="lbl">Where arrivals land</span>
            <span className="entry">
              <input className="field mono" style={{ fontSize: 13.5 }} value={directory ?? ''} readOnly placeholder="Choose a folder" aria-label="Where arrivals land" />
              <button type="button" className="chip" onClick={() => { void choose(); }}>Choose</button>
            </span>
          </label>

          <div className="ob-toggles">
            {toggles.map((toggle) => (
              <button key={toggle.key} type="button" className="tog compact" role="switch" aria-label={toggle.label} aria-checked={toggle.value} onClick={() => toggle.set(!toggle.value)}>
                <span className={toggle.value ? 'sw on' : 'sw'} aria-hidden="true"><i /></span>
                <span style={{ fontSize: 13.5 }}>{toggle.label}</span>
              </button>
            ))}
          </div>

          {error && <p className="err" role="alert">{error}</p>}

          <button className="cta" type="button" disabled={!ready || saving} onClick={() => { void engage(); }}>
            {live ? 'Live' : saving ? 'Starting…' : 'Bring this pad online'}
          </button>
        </div>
      </div>

      <TransportPad className="ob-pad" />
    </main>
  );
}
