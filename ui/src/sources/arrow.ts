import { readIpc } from './ipc.ts';
import type {
  ChangeEvent,
  Column,
  Incident,
  LatencyDoc,
  TelemetryDoc,
  TelemetryInterface,
  WifiDoc,
  WifiKey,
} from '../types.ts';

type Decompress = (bytes: Uint8Array, out: Uint8Array) => Uint8Array;
let decompress: Decompress | null = null;

export const setZstd = (decoder: Decompress): void => {
  decompress = decoder;
};

const fromBase64 = (text: string): Uint8Array => {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
};

export interface Frame {
  start: number;
  step: number;
  count: number;
  meta: Record<string, unknown>;
  column: (name: string) => Column;
}

const empty = (count: number): Column => Array.from({ length: count }, () => null);

export async function decodeFrame(reply: unknown): Promise<Frame> {
  if (!reply || typeof reply !== 'object') throw Error('The router sent an empty document.');
  const r = reply as Record<string, unknown>;
  if (r.format !== 'arrow' || typeof r.data !== 'string')
    throw Error('The router sent history in an unknown format.');
  const { start, step, count } = r;
  if (typeof start !== 'number' || typeof step !== 'number' || typeof count !== 'number')
    throw Error('The router sent a history document without a time axis.');
  if (!decompress) throw Error('No zstd decoder is registered.');
  const table = readIpc(fromBase64(r.data), decompress);
  if (table.rows !== count)
    throw Error('The history table has ' + table.rows + ' rows, not ' + count + '.');
  return {
    start,
    step,
    count,
    meta: r,
    column: (name) => table.columns.get(name) || empty(count),
  };
}

const list = <T>(value: unknown): T[] => (Array.isArray(value) ? (value as T[]) : []);

export const latencyDoc = (f: Frame): LatencyDoc => ({
  start: f.start,
  step: f.step,
  count: f.count,
  series: list<{ id: string; name: string }>(f.meta.series).map((s, k) => ({
    id: s.id,
    name: s.name,
    median: f.column(k + '.median'),
    min: f.column(k + '.min'),
    max: f.column(k + '.max'),
    jitter: f.column(k + '.jitter'),
    p50: f.column(k + '.p50'),
    p95: f.column(k + '.p95'),
    p99: f.column(k + '.p99'),
    sent: f.column(k + '.sent'),
    lost: f.column(k + '.lost'),
  })),
  events: list<ChangeEvent>(f.meta.events),
  incidents: list<Incident>(f.meta.incidents),
});

export const wifiKeys: WifiKey[] = [
  'samples',
  'uptime',
  'signal',
  'signalMin',
  'signalMax',
  'noise',
  'snr',
  'txRate',
  'txRateMin',
  'rxRate',
  'expected',
  'retries',
  'failures',
  'busy',
  'rxAir',
  'txAir',
  'upload',
  'download',
  'txBytes',
  'rxBytes',
  'disconnects',
  'roams',
];

export const wifiDoc = (f: Frame): WifiDoc => ({
  start: f.start,
  step: f.step,
  count: f.count,
  summary: (f.meta.summary || {}) as WifiDoc['summary'],
  columns: Object.fromEntries(wifiKeys.map((k) => [k, f.column(k)])),
  events: list<ChangeEvent>(f.meta.events),
});

const ifaceKeys = [
  'rxBps',
  'txBps',
  'rxPeakBps',
  'txPeakBps',
  'rxBytes',
  'txBytes',
  'errors',
  'drops',
  'handshakeAgeMax',
] as const;

export const telemetryDoc = (f: Frame): TelemetryDoc => {
  const sys = (name: string) => f.column('system.' + name);
  const max = f.meta.conntrackMax;
  return {
    start: f.start,
    step: f.step,
    count: f.count,
    system: {
      cpu: sys('cpu'),
      cpuMax: sys('cpuMax'),
      load: sys('load'),
      memUsed: sys('memUsed'),
      memAvailableMin: sys('memAvailableMin'),
      temperature: sys('temperature'),
      temperatureMax: sys('temperatureMax'),
      conntrack: sys('conntrack'),
      conntrackMax: typeof max === 'number' ? max : null,
    },
    interfaces: list<{ id: string; name: string; device: string; kind: string }>(
      f.meta.interfaces,
    ).map((i, k) => {
      const out = { id: i.id, name: i.name, device: i.device, kind: i.kind } as TelemetryInterface;
      ifaceKeys.forEach((key) => {
        out[key] = f.column(k + '.' + key);
      });
      return out;
    }),
  };
};

export async function decodeHistory<T>(reply: unknown, shape: (frame: Frame) => T): Promise<T> {
  if (reply && typeof reply === 'object' && !('format' in reply)) return reply as T;
  return shape(await decodeFrame(reply));
}
