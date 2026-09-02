import type { TrustedDeviceViewModel } from '../types/view-models';

/**
 * The backend history view-model carries `timeLabel` as the batch's raw
 * `created_at` (unix seconds) and `peerName` as the raw device id. Both are
 * identifiers, not presentation, so they are resolved here where the viewer's
 * locale and timezone are actually known.
 */

const MINUTE = 60;
const HOUR = 60 * MINUTE;

/** Renders a unix-second stamp in the viewer's locale, newest ones relatively. */
export function formatWhen(value: string, now = Date.now()): string {
  const seconds = /^\d+$/.test(value.trim()) ? Number(value.trim()) : Number.NaN;
  const when = Number.isFinite(seconds) ? new Date(seconds * 1000) : new Date(value);
  // Anything the backend did not hand us as a stamp — a test fixture, a label
  // a future backend formats itself — is already presentation. Leave it alone.
  if (Number.isNaN(when.getTime())) return value;

  const elapsed = Math.round((now - when.getTime()) / 1000);
  if (elapsed >= 0 && elapsed < MINUTE) return 'Just now';
  if (elapsed >= 0 && elapsed < HOUR) {
    const minutes = Math.floor(elapsed / MINUTE);
    return `${minutes} min ago`;
  }

  const today = new Date(now);
  const sameDay = when.getFullYear() === today.getFullYear()
    && when.getMonth() === today.getMonth()
    && when.getDate() === today.getDate();
  const time = when.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  if (sameDay) return time;

  const date = when.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  return `${date}, ${time}`;
}

/**
 * Resolves a device id to the name the user gave it. A forgotten or never
 * trusted peer has no name to resolve, so it degrades to a short readable
 * stem rather than a full 52-character identity string.
 */
export function formatPeer(value: string, trusted: TrustedDeviceViewModel[] = []): string {
  const match = trusted.find((device) => device.id === value);
  if (match) return match.name;
  // Ids are long base32 identities; anything shorter is already a name.
  if (value.length <= 20) return value;
  return `${value.slice(0, 6)}…${value.slice(-4)}`;
}

/** Sizes read the way a file manager writes them, not as raw byte counts. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '';
  if (bytes < 1000) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  // One decimal below 10 keeps "1.4 MB" readable without "1.40 MB" noise.
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

const KNOWN_TYPES: Record<string, string> = {
  jpg: 'JPEG image', jpeg: 'JPEG image', png: 'PNG image', gif: 'GIF image',
  heic: 'HEIC image', webp: 'WebP image', svg: 'SVG image',
  pdf: 'PDF document', doc: 'Word document', docx: 'Word document',
  xls: 'Excel workbook', xlsx: 'Excel workbook', ppt: 'Presentation', pptx: 'Presentation',
  txt: 'Text', md: 'Markdown', rtf: 'Rich text', csv: 'CSV',
  zip: 'ZIP archive', tar: 'Archive', gz: 'Archive', rar: 'RAR archive', '7z': '7z archive',
  mp3: 'Audio', wav: 'Audio', flac: 'Audio', m4a: 'Audio',
  mp4: 'Video', mov: 'Video', mkv: 'Video', avi: 'Video',
  exe: 'Windows installer', msi: 'Windows installer', dmg: 'Disk image', pkg: 'Installer',
  app: 'Application', json: 'JSON', xml: 'XML', html: 'HTML',
};

/** A human label for what arrived, falling back to the bare extension. */
export function formatKind(displayName: string, kind: string): string {
  if (kind === 'directory') return 'Folder';
  const dot = displayName.lastIndexOf('.');
  if (dot <= 0 || dot === displayName.length - 1) return 'File';
  const extension = displayName.slice(dot + 1).toLowerCase();
  return KNOWN_TYPES[extension] ?? `${extension.toUpperCase()} file`;
}

/** macOS has no cut: Finder moves only on Option-Command-V after a copy. */
export function isMacLike(): boolean {
  if (typeof navigator === 'undefined') return false;
  return /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
}
