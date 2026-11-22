import type { Num } from './types.ts';

export const finite = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value);

export const dash = '—';

export const fixed = (value: Num | undefined, digits = 2): string =>
  finite(value) ? value.toFixed(digits) : dash;

export const ms = (value: Num | undefined): string =>
  finite(value) ? value.toFixed(2) + ' ms' : dash;

export const percent = (value: Num | undefined): string =>
  finite(value) ? (value < 10 ? value.toFixed(1) : String(Math.round(value))) + ' %' : dash;

export const ago = (at: number, now = Date.now()): string => {
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 60) return 'just now';
  if (s < 3600) return Math.round(s / 60) + ' min ago';
  if (s < 86400) return Math.round(s / 3600) + ' h ago';
  return Math.round(s / 86400) + ' d ago';
};

export const clock = (at: number): string =>
  new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });

export const stamp = (at: number, long = false): string =>
  long
    ? new Date(at).toLocaleDateString([], { month: 'short', day: 'numeric' }) + ' ' + clock(at)
    : clock(at);

export const dateTime = (at: number): string => new Date(at).toLocaleString();

export const span = (seconds: number): string => {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return s + ' s';
  if (s < 3600) return Math.floor(s / 60) + ' min' + (s % 60 ? ' ' + (s % 60) + ' s' : '');
  if (s < 86400) {
    const m = Math.floor((s % 3600) / 60);
    return Math.floor(s / 3600) + ' h' + (m ? ' ' + m + ' min' : '');
  }
  const h = Math.floor((s % 86400) / 3600);
  return Math.floor(s / 86400) + ' d' + (h ? ' ' + h + ' h' : '');
};

export const bytes = (value: Num | undefined): string => {
  if (!finite(value)) return dash;
  const units = ['B', 'kB', 'MB', 'GB', 'TB'];
  let i = 0;
  let v = value;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return (i && v < 10 ? v.toFixed(1) : String(Math.round(v))) + ' ' + units[i];
};

export const bits = (value: Num | undefined): string => {
  if (!finite(value)) return dash;
  const units = ['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s'];
  let i = 0;
  let v = value;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return (
    (v < 10 && i ? v.toFixed(2) : v < 100 && i ? v.toFixed(1) : String(Math.round(v))) +
    ' ' +
    units[i]
  );
};

export const mbit = (value: Num | undefined): string =>
  finite(value) ? (value >= 100 ? String(Math.round(value)) : value.toFixed(1)) + ' Mbit/s' : dash;

export const kbit = (kbps: Num | undefined): string => (finite(kbps) ? mbit(kbps / 1000) : dash);

export const dbm = (value: Num | undefined): string =>
  finite(value) ? Math.round(value) + ' dBm' : dash;

export const resolution = (step: number): string =>
  ({ 15: '15-second', 60: '1-minute', 300: '5-minute', 3600: '1-hour' })[step] || step + '-second';

export const kept = (seconds: number): string => {
  if (seconds >= 86400 && seconds % 86400 === 0) {
    const d = seconds / 86400;
    return d + (d === 1 ? ' day' : ' days');
  }
  if (seconds >= 3600) return Math.round(seconds / 3600) + ' h';
  return Math.round(seconds / 60) + ' min';
};

export const band = (frequency: number): string => {
  if (frequency < 3000) return '2.4 GHz';
  return frequency < 5925 ? '5 GHz' : '6 GHz';
};

export const validCountry = (code: unknown): code is string =>
  typeof code === 'string' && /^[A-Z]{2}$/.test(code) && code !== 'XX';

export const countryName = (code: string): string => {
  try {
    return new Intl.DisplayNames(['en'], { type: 'region' }).of(code) || code;
  } catch {
    return code;
  }
};

export const place = (code: unknown): string => (validCountry(code) ? countryName(code) : '');
