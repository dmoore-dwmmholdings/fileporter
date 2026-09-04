import type { ReactNode } from 'react';

export type Screen = 'transport' | 'pads' | 'log' | 'config';

/** The pad-and-beam glyph the design uses as the mark on every screen. */
export function BrandMark({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 20 20" fill="none" stroke="var(--acc)" strokeWidth="1.2" aria-hidden="true">
      <ellipse cx="10" cy="15" rx="8" ry="3" />
      <path d="M10 12V2m0 0L6.5 5.5M10 2l3.5 3.5" />
    </svg>
  );
}

const TABS: Array<{ id: Screen; label: string }> = [
  { id: 'transport', label: 'Transport' },
  { id: 'pads', label: 'Pads' },
  { id: 'log', label: 'Log' },
  { id: 'config', label: 'Config' }
];

interface HeaderProps {
  screen: Screen;
  onNavigate: (screen: Screen) => void;
  deviceName: string;
  /** The short right-hand reading: "2 of 3 linked", "84.2 MB/s", "in sync". */
  statusLine: string;
  /** Full sentence for assistive technology, which the short line abbreviates. */
  statusSummary: string;
  online: boolean;
}

export function Header({ screen, onNavigate, deviceName, statusLine, statusSummary, online }: HeaderProps) {
  return (
    <header className="q-header">
      <span className="q-brand">
        <BrandMark />
        <span>Fileporter</span>
      </span>
      <nav className="q-nav" aria-label="Main navigation">
        {TABS.map((tab) => (
          <button
            key={tab.id}
            type="button"
            className={screen === tab.id ? 'nav on' : 'nav'}
            aria-current={screen === tab.id ? 'page' : undefined}
            onClick={() => onNavigate(tab.id)}
          >
            {tab.label}
          </button>
        ))}
      </nav>
      <div className="q-spacer" />
      <span className="q-status">
        {deviceName}
        <span className="rule" aria-hidden="true" />
        <span className={online ? 'q-dot' : 'q-dot off'} aria-hidden="true" />
        <span aria-hidden="true">{statusLine}</span>
        <span className="sr-only">{statusSummary}</span>
      </span>
    </header>
  );
}

/** The floor glow behind every screen. */
export function Floor({ variant }: { variant?: 'tall' | 'faint' }) {
  const cls = variant === 'tall' ? 'q-floor floor-tall' : variant === 'faint' ? 'q-floor floor-faint' : 'q-floor';
  return <span className={cls} aria-hidden="true" />;
}

export function Headline({ title, sub, id }: { title: string; sub?: ReactNode; id?: string }) {
  return (
    <div className="q-headline fade">
      <h1 id={id}>{title}</h1>
      {sub ? <p>{sub}</p> : null}
    </div>
  );
}
