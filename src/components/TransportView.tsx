import { useEffect, useRef, useState } from 'react';
import { TransportPad } from './PadArt';
import { Floor } from './Shell';
import { appBridge } from '../lib/bridge';
import { formatBytes, formatPeer, isMacLike } from '../lib/format';
import type { AppSnapshotViewModel, HistoryTopLevelItemViewModel } from '../types/view-models';

/** What rests on the deck: at most three, the way the board draws it. */
const VISIBLE_PACKETS = 3;
/** Arrivals along the bottom are a glance, not the log. */
const VISIBLE_ARRIVALS = 2;

export type PickChoice = 'files' | 'folder';

const ACTIVE_STATES = ['preparing', 'sending', 'verifying'];

type Glyph = 'file' | 'folder' | 'image';

function FileGlyph({ kind }: { kind: Glyph }) {
  const stroke = { fill: 'none', stroke: 'var(--acc)', strokeWidth: 1.7, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const };
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" aria-hidden="true" {...stroke}>
      {kind === 'folder' && <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />}
      {kind === 'image' && <><rect x="3" y="4" width="18" height="16" rx="1" /><path d="m3 15 5-5 4 4 3-3 6 6" /></>}
      {kind === 'file' && <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5" />}
    </svg>
  );
}

function glyphFor(name: string, isDirectory: boolean): Glyph {
  if (isDirectory) return 'folder';
  const extension = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
  return /^(jpg|jpeg|png|gif|heic|webp|svg|bmp|tiff)$/.test(extension) ? 'image' : 'file';
}

interface Packet { key: string; name: string; size: string; glyph: Glyph; }

interface TransportViewProps {
  snapshot: AppSnapshotViewModel;
  selectedDeviceIds: string[];
  onToggleDevice: (id: string) => void;
  onPick: (choice: PickChoice) => void;
  stagedPaths: string[];
}

export function TransportView({ snapshot, selectedDeviceIds, onToggleDevice, onPick, stagedPaths }: TransportViewProps) {
  const [dragging, setDragging] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [landed, setLanded] = useState(false);
  const dragDepth = useRef(0);
  const menuRef = useRef<HTMLDivElement>(null);
  // Every linked pad is a destination, dark ones included — picking one queues
  // the payload until it wakes, which is what "holds your pattern" means.
  const pads = snapshot.trustedDevices;
  const active = snapshot.transfers.filter((batch) => ACTIVE_STATES.includes(batch.state));
  const sending = active.length > 0;

  // "Landed." is a moment, not a state the backend reports. Watch the number of
  // finished batches and show the ring when it goes up.
  const completedCount = snapshot.transfers.filter((batch) => batch.state === 'complete').length;
  const previousCompleted = useRef(completedCount);
  useEffect(() => {
    if (completedCount > previousCompleted.current) {
      previousCompleted.current = completedCount;
      setLanded(true);
      const timer = window.setTimeout(() => setLanded(false), 2600);
      return () => window.clearTimeout(timer);
    }
    previousCompleted.current = completedCount;
  }, [completedCount]);

  useEffect(() => {
    if (!menuOpen) return;
    menuRef.current?.querySelector<HTMLButtonElement>('button')?.focus();
    function onKey(event: KeyboardEvent) { if (event.key === 'Escape') setMenuOpen(false); }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [menuOpen]);

  const selected = pads.filter((device) => selectedDeviceIds.includes(device.id));
  const count = selected.length;
  const darkCount = selected.filter((device) => device.state !== 'online').length;
  const padWord = `${count} pad${count === 1 ? '' : 's'}`;
  const phase = sending ? 'go' : landed ? 'done' : 'idle';

  // A dark destination changes what actually happens — it waits — so the line
  // says so rather than implying an immediate transport.
  const held = darkCount > 0
    ? `, ${darkCount === count ? 'waiting' : `${darkCount} waiting`} until ${darkCount === 1 ? 'it wakes' : 'they wake'}`
    : '';

  const title = dragging && !sending ? 'Let go.' : sending ? 'Going.' : landed ? 'Landed.' : 'Drop anything.';
  const sub = dragging && !sending
    ? `Release and it goes to ${padWord}${held}`
    : sending
      ? `On the beam to ${padWord}`
      : landed
        ? 'Every pad returned a matching digest'
        : pads.length === 0
          ? 'No pad is linked yet — Fileporter is listening for one'
          : count === 0 ? 'Pick a pad below' : `It reassembles on ${padWord}${held}`;

  // The deck carries what is actually moving; failing that, what is staged and
  // waiting for a recipient. An idle deck stays empty.
  const packets: Packet[] = sending
    ? active.slice(0, VISIBLE_PACKETS).map((batch) => ({
      key: batch.id,
      name: batch.label,
      size: `${Math.round(batch.progress)}%`,
      glyph: glyphFor(batch.label, batch.label.endsWith('/'))
    }))
    : stagedPaths.slice(0, VISIBLE_PACKETS).map((path, index) => {
      const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
      return { key: `${path}-${index}`, name, size: 'staged', glyph: glyphFor(name, !/\.[^.]+$/.test(name)) };
    });

  const arrivals = snapshot.history
    .filter((batch) => batch.direction === 'incoming')
    .flatMap((batch) => batch.items
      .filter((item) => item.state === 'complete' && item.available)
      .map((item) => ({ item, from: formatPeer(batch.peerName, snapshot.trustedDevices) })))
    .slice(0, VISIBLE_ARRIVALS);

  return (
    <>
      <div
        className={`q-stage-area ph-${phase} ${dragging ? 'drag' : ''}`}
        onDragEnter={(event) => { event.preventDefault(); dragDepth.current += 1; setDragging(true); }}
        onDragLeave={() => { dragDepth.current = Math.max(0, dragDepth.current - 1); if (dragDepth.current === 0) setDragging(false); }}
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => { event.preventDefault(); dragDepth.current = 0; setDragging(false); }}
      >
        <Floor />

        <button
          className="q-stage"
          type="button"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          aria-label="Send files or folders"
          onClick={() => setMenuOpen((open) => !open)}
        >
          <span className="beam beam-o" aria-hidden="true" />
          <span className="beam beam-m" aria-hidden="true" />
          <span className="ring" aria-hidden="true" />
          <TransportPad className="stage-pad" />
        </button>

        <div className="stage-head">
          <div className="q-headline fade">
            <h1>{title}</h1>
            <p>{sub}</p>
          </div>
        </div>

        <div className="stage-chips" aria-label="Destination pads" role="group">
          {pads.length > 0
            ? pads.map((device) => {
              const picked = selectedDeviceIds.includes(device.id);
              const dark = device.state !== 'online';
              return (
                <button
                  key={device.id}
                  type="button"
                  className={picked ? 'pad-chip on' : dark ? 'pad-chip dark' : 'pad-chip'}
                  aria-pressed={picked}
                  onClick={() => onToggleDevice(device.id)}
                >
                  {device.name}
                  {dark && <span className="sr-only"> (dark — will wait until it wakes)</span>}
                </button>
              );
            })
            : <span className="pad-chip dark">No pad linked</span>}
        </div>

        {menuOpen && (
          <div className="stage-menu" ref={menuRef} role="menu" aria-label="Browse files or folders">
            <button type="button" role="menuitem" className="pad-chip" onClick={() => { setMenuOpen(false); onPick('files'); }}>Browse files</button>
            <button type="button" role="menuitem" className="pad-chip" onClick={() => { setMenuOpen(false); onPick('folder'); }}>Browse folder</button>
          </div>
        )}

        <div className="stage-deck" aria-hidden={packets.length === 0}>
          {packets.map((packet, index) => (
            <span className={`pkt p${index + 1}`} key={packet.key}>
              <FileGlyph kind={packet.glyph} />
              <span className="mono pkt-name">{packet.name}</span>
              <span className="mono pkt-size">{packet.size}</span>
            </span>
          ))}
        </div>
      </div>

      <div className="q-foot">
        {arrivals.map((arrival) => <ArrivalRow key={arrival.item.itemId} item={arrival.item} from={arrival.from} />)}
        <div className="q-spacer" />
        <span className="tagline">Encrypted · verified · never leaves this network</span>
      </div>
    </>
  );
}

/**
 * What just arrived, with the two things you actually want to do with it. Copy
 * and Cut put the real file on the system clipboard, so the paste happens in
 * the file manager exactly as it would for any other file.
 */
function ArrivalRow({ item, from }: { item: HistoryTopLevelItemViewModel; from: string }) {
  const [note, setNote] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);

  async function put(action: 'copy' | 'cut') {
    setBusy(true); setFailed(false); setNote(null);
    try {
      if (action === 'copy') await appBridge.copyItem(item.itemId);
      else await appBridge.moveItem(item.itemId);
      // Finder has no cut. A copied file moves only on Option-Command-V, so say
      // which paste finishes the job rather than implying a plain paste.
      setNote(action === 'copy'
        ? isMacLike() ? 'Copied — press ⌘V where you want it' : 'Copied — paste where you want it'
        : isMacLike() ? 'Ready to move — press ⌥⌘V where you want it' : 'Ready to move — paste where you want it');
    } catch {
      setFailed(true);
      setNote(action === 'copy' ? 'Could not copy that file' : 'Could not prepare that file');
    } finally { setBusy(false); }
  }

  return (
    <span className="row arrival-row">
      <span className="name" title={`${item.displayName} · from ${from}`}>{item.displayName}</span>
      <span className="size">{item.kind === 'directory' ? `${item.size} items` : formatBytes(item.size)}</span>
      <span className="act">
        <button type="button" className="chip-mini" disabled={busy} onClick={() => { void put('copy'); }} aria-label={`Copy ${item.displayName}`}>COPY</button>
        <button type="button" className="chip-mini" disabled={busy} onClick={() => { void put('cut'); }} aria-label={`Cut ${item.displayName}`}>CUT</button>
      </span>
      {note && <span className={failed ? 'hint warn' : 'arrival-note'} role="status">{note}</span>}
    </span>
  );
}
