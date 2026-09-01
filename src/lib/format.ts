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
