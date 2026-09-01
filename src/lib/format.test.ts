import { describe, expect, it } from 'vitest';
import { formatPeer, formatWhen } from './format';
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
