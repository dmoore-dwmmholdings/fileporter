import { describe, expect, it } from 'vitest';
import { formatBytes, formatKind, formatPeer, formatWhen } from './format';
import type { TrustedDeviceViewModel } from '../types/view-models';

const at = (iso: string) => new Date(iso).getTime();

describe('formatWhen', () => {
  const now = at('2026-08-31T22:30:00');

  it('renders a unix-second stamp relatively inside the first hour', () => {
    expect(formatWhen(String(Math.floor(at('2026-08-31T22:29:40') / 1000)), now)).toBe('Just now');
    expect(formatWhen(String(Math.floor(at('2026-08-31T22:12:00') / 1000)), now)).toBe('18 min ago');
  });

  it('renders an older same-day stamp as a clock time, not an epoch', () => {
    const label = formatWhen(String(Math.floor(at('2026-08-31T09:05:00') / 1000)), now);
    expect(label).not.toMatch(/^\d{9,}$/);
    expect(label).toMatch(/9[:.]05/);
  });

  it('carries the date once the stamp is not from today', () => {
    expect(formatWhen(String(Math.floor(at('2026-08-12T09:05:00') / 1000)), now)).toMatch(/Aug/);
  });

  it('passes through a value the backend already formatted', () => {
    expect(formatWhen('Now', now)).toBe('Now');
    expect(formatWhen('', now)).toBe('');
  });
});

describe('formatPeer', () => {
  const trusted = [{ id: 'AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJKKKKLLLLMMMM', name: 'DWMM Gaming' }] as TrustedDeviceViewModel[];

  it('resolves a device id to the name the user gave it', () => {
    expect(formatPeer('AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJKKKKLLLLMMMM', trusted)).toBe('DWMM Gaming');
  });

  it('shortens an unresolvable identity instead of printing all 52 characters', () => {
    const label = formatPeer('ZZZZBBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJKKKKLLLLMMMM', trusted);
    expect(label).toBe('ZZZZBB…MMMM');
  });

  it('leaves an already-readable name alone', () => {
    expect(formatPeer('Laptop', trusted)).toBe('Laptop');
    expect(formatPeer('Laptop', [])).toBe('Laptop');
  });
});

describe('formatBytes', () => {
  it('reads like a file manager rather than a byte count', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(2411724)).toBe('2.4 MB');
    expect(formatBytes(8555707)).toBe('8.6 MB');
    expect(formatBytes(45_000_000)).toBe('45 MB');
    expect(formatBytes(3_200_000_000)).toBe('3.2 GB');
  });

  it('gives nothing rather than nonsense for a size it cannot use', () => {
    expect(formatBytes(-1)).toBe('');
    expect(formatBytes(Number.NaN)).toBe('');
  });
});

describe('formatKind', () => {
  it('names the common types a person actually sends', () => {
    expect(formatKind('holiday.PNG', 'file')).toBe('PNG image');
    expect(formatKind('contract.pdf', 'file')).toBe('PDF document');
    expect(formatKind('Fileporter_0.1.0_x64-setup.exe', 'file')).toBe('Windows installer');
  });

  it('falls back to the extension, and never mislabels a folder', () => {
    expect(formatKind('capture.rawthing', 'file')).toBe('RAWTHING file');
    expect(formatKind('Photos', 'directory')).toBe('Folder');
    expect(formatKind('LICENSE', 'file')).toBe('File');
    expect(formatKind('trailing.', 'file')).toBe('File');
    expect(formatKind('.gitignore', 'file')).toBe('File');
  });
});
