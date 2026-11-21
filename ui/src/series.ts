import { finite } from './format.ts';
import { score, type Score } from './mos.ts';
import type { Column, Doc, LatencySeries, Num, Tier } from './types.ts';

export const pointLimit = 2400;

export const times = (doc: Doc): Float64Array => {
  const out = new Float64Array(doc.count);
  for (let i = 0; i < doc.count; i++) out[i] = doc.start / 1000 + i * doc.step;
  return out;
};

export const mapColumn = (values: Column, read: (value: Num, i: number) => Num): Num[] => {
  const out: Num[] = Array.from({ length: values.length });
  for (let i = 0; i < values.length; i++) out[i] = read(values[i], i);
  return out;
};

export const some = (values: Column, test: (value: Num) => boolean): boolean => {
  for (let i = 0; i < values.length; i++) if (test(values[i])) return true;
  return false;
};

const typed = (values: unknown): boolean => ArrayBuffer.isView(values);

export const column = (doc: Doc, values: Column | undefined): Column => {
  if (!values || values.length !== doc.count)
    return mapColumn({ length: doc.count } as Column, (_, i) =>
      values && finite(values[i]) ? values[i] : null,
    );
  if (typed(values)) return values;
  return mapColumn(values, (v) => (finite(v) ? v : null));
};

export const scale = (values: Column, factor: number): Column => {
  if (typed(values)) return Float64Array.from(values as ArrayLike<number>, (v) => v * factor);
  return mapColumn(values, (v) => (finite(v) ? v * factor : null));
};

export const micros = (values: Column): Column => scale(values, 0.001);

export const lossColumn = (sent: Column, lost: Column): Column =>
  mapColumn(sent, (s, i) => (finite(s) && s > 0 ? ((Number(lost[i]) || 0) / s) * 100 : null));

export interface LatencyLine {
  id: string;
  name: string;
  median: Column;
  min: Column;
  max: Column;
  jitter: Column;
  sent: Column;
  lost: Column;
  loss: Column;
}

export const latencyLine = (doc: Doc, s: LatencySeries): LatencyLine => {
  const sent = column(doc, s.sent);
  const lost = column(doc, s.lost);
  return {
    id: s.id,
    name: s.name,
    median: micros(column(doc, s.median)),
    min: micros(column(doc, s.min)),
    max: micros(column(doc, s.max)),
    jitter: micros(column(doc, s.jitter)),
    sent,
    lost,
    loss: lossColumn(sent, lost),
  };
};

export interface Stats {
  last: Num;
  mean: Num;
  min: Num;
  max: Num;
  jitter: Num;
  loss: Num;
  sent: number;
  lost: number;
  score: Score | null;
}

const mean = (values: Column): Num => {
  let total = 0;
  let n = 0;
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (finite(v)) {
      total += v;
      n++;
    }
  }
  return n ? total / n : null;
};

export const sum = (values: Column): number => {
  let total = 0;
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (finite(v)) total += v;
  }
  return total;
};

export const extreme = (values: Column, pick: (a: number, b: number) => number): Num => {
  let acc: Num = null;
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (finite(v)) acc = acc === null ? v : pick(acc, v);
  }
  return acc;
};

export const lastValue = (values: Column): Num => {
  for (let i = values.length - 1; i >= 0; i--) {
    const v = values[i];
    if (finite(v)) return v;
  }
  return null;
};

export const summarize = (line: LatencyLine): Stats => {
  const lows = mapColumn(line.min, (v, i) => (finite(v) ? v : line.median[i]));
  const highs = mapColumn(line.max, (v, i) => (finite(v) ? v : line.median[i]));
  const sent = sum(line.sent);
  const lost = sum(line.lost);
  const average = mean(line.median);
  const jitter = mean(line.jitter);
  const loss = sent ? (lost / sent) * 100 : null;
  return {
    last: lastValue(line.median),
    mean: average,
    min: extreme(lows, Math.min),
    max: extreme(highs, Math.max),
    jitter,
    loss,
    sent,
    lost,
    score: score(average, jitter, loss),
  };
};

export const lossClass = (sent: Num, lost: Num): number => {
  if (!finite(sent) || sent <= 0) return 0;
  const pct = ((Number(lost) || 0) / sent) * 100;
  if (pct <= 0) return 1;
  if (pct < 1) return 2;
  if (pct < 5) return 3;
  if (pct < 100) return 4;
  return 5;
};

export const lossLabels = [
  'No probes',
  '0 % loss',
  'Under 1 % loss',
  '1–5 % loss',
  '5 % or more loss',
  'All probes lost',
];

export interface Run {
  start: number;
  length: number;
  level: number;
}

export const runs = (sent: Column, lost: Column): Run[] => {
  const out: Run[] = [];
  for (let i = 0; i < sent.length; i++) {
    const level = lossClass(sent[i], lost[i]);
    const prev = out[out.length - 1];
    if (prev && prev.level === level) prev.length++;
    else out.push({ start: i, length: 1, level });
  }
  return out;
};

export interface Uptime {
  uptime: Num;
  full: number;
  probed: number;
}

export const uptime = (sent: Column, lost: Column): Uptime => {
  let full = 0;
  let probed = 0;
  for (let i = 0; i < sent.length; i++) {
    const s = sent[i];
    if (!finite(s) || s <= 0) continue;
    probed++;
    if ((Number(lost[i]) || 0) >= s) full++;
  }
  return { uptime: probed ? (1 - full / probed) * 100 : null, full, probed };
};

export const stepFor = (tiers: Tier[], minutes: number, offset = 0): number => {
  const need = (minutes + offset) * 60;
  const keeping = tiers.filter((t) => t.keptSeconds >= need).sort((a, b) => a.step - b.step);
  const fits = keeping.find((t) => (minutes * 60) / t.step <= pointLimit);
  if (fits) return fits.step;
  return keeping.length ? keeping[keeping.length - 1].step : 0;
};

export const offered = (tiers: Tier[], minutes: number): boolean =>
  tiers.some((t) => t.keptSeconds >= minutes * 60 && (minutes * 60) / t.step <= pointLimit);

export const legacyStep = (minutes: number): number => {
  if (minutes <= 60) return 15;
  if (minutes <= 360) return 60;
  return minutes <= 10080 ? 300 : 3600;
};
