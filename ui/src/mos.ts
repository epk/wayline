import type { Num } from './types.ts';

export type Band = 'excellent' | 'good' | 'fair' | 'poor';

export interface Score {
  el: number;
  r: number;
  mos: number;
  band: Band;
}

const finite = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value);

export const band = (mos: number): Band =>
  mos >= 4.3 ? 'excellent' : mos >= 4.0 ? 'good' : mos >= 3.6 ? 'fair' : 'poor';

export const bandLabel: Record<Band, string> = {
  excellent: 'Excellent',
  good: 'Good',
  fair: 'Fair',
  poor: 'Poor',
};

export function score(
  latencyMs: Num | undefined,
  jitterMs: Num | undefined,
  lossPercent: Num | undefined,
): Score | null {
  if (!finite(latencyMs) || !finite(jitterMs) || !finite(lossPercent)) return null;
  const el = latencyMs + 2 * jitterMs + 10;
  const base = el < 160 ? 93.2 - el / 40 : 93.2 - (el - 120) / 10;
  const r = Math.min(100, Math.max(0, base - 2.5 * lossPercent));
  const mos = 1 + 0.035 * r + 7e-6 * r * (r - 60) * (100 - r);
  return { el, r, mos, band: band(mos) };
}

export const formula = [
  'EL = latency_ms + 2 × jitter_ms + 10',
  'R  = 93.2 − EL / 40            if EL < 160',
  'R  = 93.2 − (EL − 120) / 10    otherwise',
  'R  = R − 2.5 × loss_percent, clamped to 0..100',
  'MOS = 1 + 0.035 R + 7e-6 × R × (R − 60) × (100 − R)',
].join('\n');
