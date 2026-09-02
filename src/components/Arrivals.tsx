import { Archive, Copy, File, FileText, Folder, Image, Music, Scissors, Video } from 'lucide-react';
import { useState } from 'react';
import { appBridge } from '../lib/bridge';
import { formatBytes, formatKind, formatPeer, isMacLike } from '../lib/format';
import type { AppSnapshotViewModel, HistoryTopLevelItemViewModel } from '../types/view-models';

/** Just-arrived is a glance, not a log; the Activity tab keeps the full list. */
const VISIBLE_ARRIVALS = 3;

interface Arrival {
  item: HistoryTopLevelItemViewModel;
  from: string;
  when: string;
}

function iconFor(name: string, kind: string) {
  if (kind === 'directory') return Folder;
  const extension = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
  if (/^(jpg|jpeg|png|gif|heic|webp|svg|bmp|tiff)$/.test(extension)) return Image;
  if (/^(mp3|wav|flac|m4a|aac|ogg)$/.test(extension)) return Music;
  if (/^(mp4|mov|mkv|avi|webm)$/.test(extension)) return Video;
  if (/^(zip|tar|gz|rar|7z|bz2)$/.test(extension)) return Archive;
  if (/^(txt|md|rtf|csv|json|xml|html|pdf|doc|docx)$/.test(extension)) return FileText;
  return File;
}

/**
 * What just landed, with the two things you actually want to do with it. Copy
 * and Cut put the real file on the system clipboard, so the paste happens in
 * the file manager exactly as it would for any other file.
 */
export function Arrivals({ snapshot }: { snapshot: AppSnapshotViewModel }) {
  const arrivals: Arrival[] = snapshot.history
    .filter((batch) => batch.direction === 'incoming')
    .flatMap((batch) =>
      batch.items
        .filter((item) => item.state === 'complete')
        .map((item) => ({
          item,
          from: formatPeer(batch.peerName, snapshot.trustedDevices),
          when: batch.timeLabel,
        })))
    .slice(0, VISIBLE_ARRIVALS);

  if (!arrivals.length) return null;

  return (
    <section className="arrivals" aria-label="Files that just arrived">
      <h2>Just arrived</h2>
      {arrivals.map((arrival) => (
        <ArrivalRow key={arrival.item.itemId} arrival={arrival} />
      ))}
    </section>
  );
}

function ArrivalRow({ arrival }: { arrival: Arrival }) {
  const { item, from } = arrival;
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const Icon = iconFor(item.displayName, item.kind);

  async function put(action: 'copy' | 'cut') {
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      if (action === 'copy') await appBridge.copyItem(item.itemId);
      else await appBridge.moveItem(item.itemId);
      // Finder has no cut. A copied file moves only on Option-Command-V, so
      // say which paste finishes the job rather than implying a plain paste.
      setStatus(
        action === 'copy'
          ? isMacLike() ? 'Copied — press ⌘V where you want it' : 'Copied — paste where you want it'
          : isMacLike() ? 'Ready to move — press ⌥⌘V where you want it' : 'Ready to move — paste where you want it');
    } catch {
      setError(
        action === 'copy'
          ? 'That file could not be copied to the clipboard.'
          : 'That file could not be prepared for moving.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <article className="arrival">
      <span className="arrival-icon" aria-hidden="true"><Icon size={20} /></span>
      <div className="arrival-detail">
        <strong>{item.displayName}</strong>
        <span className="muted">
          {formatKind(item.displayName, item.kind)}
          {item.size > 0 || item.kind !== 'directory' ? ` · ${formatBytes(item.size)}` : ''}
          {` · from ${from}`}
        </span>
        {status && <span className="arrival-status" role="status">{status}</span>}
        {error && <span className="form-error" role="alert">{error}</span>}
      </div>
      {item.available ? (
        <div className="arrival-actions">
          <button type="button" disabled={busy} onClick={() => { void put('copy'); }}>
            <Copy size={15} aria-hidden="true" /> Copy
          </button>
          <button type="button" disabled={busy} onClick={() => { void put('cut'); }}>
            <Scissors size={15} aria-hidden="true" /> Cut
          </button>
        </div>
      ) : (
        <span className="unavailable">Moved or deleted</span>
      )}
    </article>
  );
}
