import { float64, int32, int64, tableFromArrays, tableToIPC, uint32 } from '@uwdata/flechette';
import { decompress } from 'fzstd';
import { decodeHistory, latencyDoc, setZstd, telemetryDoc, wifiDoc } from '../src/sources/arrow.ts';

setZstd(decompress);

const anchor = Math.floor(Date.now() / 60000) * 60000;
const names = {
  home: 'Tunnel A',
  backup: 'Tunnel B',
  tokyo: 'Tunnel C',
  zurich: 'Tunnel D',
  wan: 'Direct WAN',
};
const query = new URLSearchParams(window.location.search);
const level = ['monitoring', 'unsupported', 'newer'].includes(query.get('compat'))
  ? query.get('compat')
  : 'full';
const wanDown = query.get('wan') === 'unavailable';
const updateLatched = query.get('update') === 'latched';
const updateFailed = updateLatched || query.get('update') === 'failed';
const release = '0.1.0-alpha.27';
const installed = query.get('stale') === '1' ? '0.0.0' : release;
const directExit = query.get('exit') === 'direct';
const telemetryOn = query.get('telemetry') !== 'off';
const wifiMode = ['off', 'disconnected', 'none'].includes(query.get('wifi'))
  ? query.get('wifi')
  : 'connected';
const wanMs = wanDown ? null : 14.37;
const compatMessages = {
  full: '',
  monitoring: 'This firmware is newer than the tested range. Monitoring works.',
  unsupported: 'This firmware is older than 4.9.0. Wayline cannot control its VPN module.',
  newer: 'The VPN module is unverified. Monitoring and updates work. Tunnel controls are off.',
};
const featureList = (on) =>
  ['Monitoring', 'History', 'Exit selection', 'Start and Stop', 'Signed updates'].map(
    (name, i) => ({ name, available: i < on || i === 4 }),
  );
const features = { full: 4, newer: 2, monitoring: 2, unsupported: 0 };
const now = () => Date.now();
const nowSeconds = () => Math.floor(Date.now() / 1000);
let revision = 1;

const state = {
  version: installed,
  update: updateFailed
    ? {
        current: installed,
        latest: release,
        available: true,
        phase: 'failed',
        message: updateLatched
          ? 'Update stopped. opkg did not finish in time.'
          : 'The package signature did not match. Nothing was installed.',
        failed: true,
        needsAttention: updateLatched,
        checkedAt: nowSeconds() - 120,
        updatedAt: nowSeconds() - 120,
      }
    : {
        current: installed,
        latest: null,
        available: false,
        phase: 'idle',
        message: '',
        failed: false,
        needsAttention: false,
        checkedAt: 0,
        updatedAt: 0,
      },
  revision: '1',
  scope: 'All clients',
  compat: {
    level: level === 'newer' ? 'monitoring' : level,
    verified: level !== 'newer',
    firmware: level === 'unsupported' ? '4.8.1' : '4.9.2',
    message: compatMessages[level],
    features: level === 'full' ? [] : featureList(features[level]),
  },
  capabilities: {
    configure: level !== 'unsupported',
    select: level === 'full',
    control: level === 'full',
    update: true,
  },
  probe: { interval: 5, timeout: 2000 },
  device: '3f9c2a7e5b1d4c608a2e9f17c4b3d5e8',
  clock:
    query.get('clock') === 'unsynced'
      ? { synced: false, source: null, pending: true }
      : { synced: true, source: 'ntp', pending: false },
  history: {
    days: 7,
    tiers: [
      { step: 15, keptSeconds: 21600 },
      { step: 60, keptSeconds: 259200 },
      { step: 300, keptSeconds: 604800 },
    ],
  },
  config: {
    enabled: true,
    interval: 5,
    tunnels: ['home', 'backup', 'tokyo'],
    targets: ['1.1.1.1', '9.9.9.9', 'example.com', 'missing.invalid'],
    directWan: true,
    historyDays: 7,
    autoUpdates: false,
    autoExit: true,
    autoTunnels: ['home', 'backup'],
    endpointDiagnostics: true,
    upstreamWifi: wifiMode !== 'off',
    telemetry: telemetryOn,
  },
  egress: {
    checkedAt: nowSeconds() - 90,
    paths: [
      {
        id: 'home',
        name: names.home,
        interface: 'wg-home',
        ip: '203.0.113.5',
        location: 'NL',
        error: null,
      },
      {
        id: 'backup',
        name: names.backup,
        interface: 'wg-backup',
        ip: '198.51.100.7',
        location: 'DE',
        error: null,
      },
      {
        id: 'tokyo',
        name: names.tokyo,
        interface: 'wg-tokyo',
        ip: null,
        location: null,
        error: 'The IP check timed out after 6 seconds.',
      },
      {
        id: 'wan',
        name: names.wan,
        interface: wanDown ? null : 'eth1',
        ip: wanDown ? null : '203.0.113.80',
        location: wanDown ? null : 'GB',
        error: wanDown ? 'Direct WAN is unavailable.' : null,
      },
    ],
  },
  auto: {
    enabled: true,
    needsAttention: false,
    paused: false,
    last: { at: now() - 12 * 60000, from: 'backup', to: 'home', reason: 'Lower latency' },
  },
  wan: wanDown
    ? {
        available: false,
        interface: null,
        reason: 'The WAN probe cannot bypass the VPN on this firmware.',
      }
    : { available: true, interface: 'eth1', reason: '' },
  uplinks: [
    {
      id: 'wan',
      interface: wanDown ? null : 'eth1',
      device: wanDown ? null : 'eth1',
      address: null,
      active: true,
      state: wanDown ? 'route_unavailable' : 'reachable',
      median: wanMs,
      jitter: wanDown ? null : 0.84,
      samples: wanDown ? 0 : 64,
      lost: wanDown ? 0 : 1,
    },
    {
      id: 'uplink:wwan',
      interface: 'wwan',
      device: 'wwan0',
      address: '100.64.12.9',
      active: false,
      state: 'reachable',
      median: 21.84,
      jitter: 2.61,
      samples: 64,
      lost: 0,
    },
    {
      id: 'uplink:secondwan',
      interface: 'secondwan',
      device: 'eth2',
      address: '192.0.2.77',
      active: false,
      state: 'route_mismatch',
      median: null,
      jitter: null,
      samples: 0,
      lost: 0,
    },
  ],
  events: [
    {
      at: now() - 20 * 60000,
      subject: 'home',
      seq: 41,
      kind: 'egress',
      text: 'Tunnel A public exit IP changed from 203.0.113.4 to 203.0.113.5.',
    },
    {
      at: now() - 3 * 3600000,
      subject: 'wan',
      seq: 40,
      kind: 'uplink',
      text: 'Active uplink changed from wwan to eth1',
    },
  ],
  incidents: [
    {
      subject: 'tokyo',
      name: names.tokyo,
      kind: 'loss',
      start: now() - 95000,
      end: null,
      detail: 'All probes to 1.1.1.1 lost',
    },
    {
      subject: 'wifi',
      name: 'Upstream Wi-Fi',
      kind: 'wifi',
      start: anchor - 732 * 60000,
      end: anchor - 731 * 60000 + 30000,
      detail: 'apclix0 disconnected from Example Hotel',
    },
    {
      subject: 'home',
      name: names.home,
      kind: 'loss',
      start: anchor - 489 * 60000,
      end: anchor - 476 * 60000,
      detail: 'All probes to 1.1.1.1 lost',
    },
    {
      subject: 'wan',
      name: 'Direct WAN',
      kind: 'loss',
      start: anchor - 41 * 60000,
      end: anchor - 41 * 60000 + 45000,
      detail: 'All probes to 1.1.1.1 lost',
    },
  ],
  endpoints: [
    { tunnel: 'home', name: names.home, address: '203.0.113.10', reason: '' },
    { tunnel: 'backup', name: names.backup, address: '198.51.100.24', reason: '' },
    { tunnel: 'tokyo', name: names.tokyo, address: '192.0.2.44', reason: '' },
    {
      tunnel: 'zurich',
      name: names.zurich,
      address: null,
      reason: 'The server address is not known until the tunnel connects once.',
    },
  ],
  resolved: { 'example.com': '93.184.216.34', 'missing.invalid': null },
  tunnels: [
    {
      id: 'home',
      name: names.home,
      preferred: !directExit,
      enabled: true,
      connected: true,
      controllable: true,
      stateLabel: 'Connected',
      age: '4s',
      median: 32.41,
      jitter: 1.92,
      lost: 0,
      samples: 64,
      eligible: true,
      selectable: true,
    },
    {
      id: 'backup',
      name: names.backup,
      preferred: false,
      enabled: true,
      connected: true,
      controllable: true,
      stateLabel: 'Connected',
      age: '7s',
      median: 24.18,
      jitter: 1.27,
      lost: 0,
      samples: 64,
      eligible: true,
      selectable: true,
    },
    {
      id: 'tokyo',
      name: names.tokyo,
      preferred: false,
      enabled: true,
      connected: true,
      controllable: true,
      stateLabel: 'Connected',
      age: '10s',
      median: 131.07,
      jitter: 18.4,
      lost: 4,
      samples: 64,
      eligible: false,
      selectable: true,
      reason: 'Recent packet loss',
    },
    {
      id: 'zurich',
      name: names.zurich,
      preferred: false,
      enabled: false,
      connected: false,
      controllable: true,
      stateLabel: 'Stopped',
      age: 'unknown',
      median: null,
      jitter: null,
      lost: 0,
      samples: 0,
      eligible: false,
      selectable: false,
      selectReason: 'The tunnel is stopped.',
      historyAvailable: true,
    },
  ],
  diagnostics: {
    '1.1.1.1': {
      home: 32.41,
      backup: 24.18,
      tokyo: 131.07,
      zurich: null,
      wan: wanMs,
      'uplink:wwan': 21.84,
      'uplink:secondwan': null,
    },
    '9.9.9.9': {
      home: 36.02,
      backup: 28.55,
      tokyo: 128.3,
      zurich: null,
      wan: wanDown ? null : 15.1,
      'uplink:wwan': 23.9,
      'uplink:secondwan': null,
    },
    'example.com': {
      home: 39.76,
      backup: 31.2,
      tokyo: 124.48,
      zurich: null,
      wan: wanDown ? null : 18.05,
      'uplink:wwan': 26.4,
      'uplink:secondwan': null,
    },
    'missing.invalid': {
      home: null,
      backup: null,
      tokyo: null,
      zurich: null,
      wan: null,
      'uplink:wwan': null,
      'uplink:secondwan': null,
    },
    '203.0.113.10': { wan: 21 },
    '198.51.100.24': { wan: 29 },
    '192.0.2.44': { wan: 118 },
  },
};
state.tunnels.forEach((tunnel) => (tunnel.monitorId = tunnel.id));

const wifiReasons = {
  off: 'Upstream Wi-Fi monitoring is off.',
  disconnected: 'The repeater interface apclix0 is not connected to a Wi-Fi network.',
  none: "The router has no Wi-Fi client interface. Connect to a Wi-Fi network as a repeater in the router's Internet page.",
  connected: '',
};
state.wifi = {
  enabled: wifiMode !== 'off',
  state: { off: null, none: 'no_client_interface' }[wifiMode] || wifiMode,
  reason: wifiReasons[wifiMode],
  interface: ['connected', 'disconnected'].includes(wifiMode) ? 'apclix0' : null,
  candidates: ['connected', 'disconnected'].includes(wifiMode) ? ['apclix0', 'apcli0'] : [],
  sampleAgeMs: 1800,
  interval: 5,
  uplink: wifiMode === 'connected' ? 'uplink:wwan' : null,
  historyAvailable: wifiMode !== 'off',
  recent: [],
  link:
    wifiMode === 'connected'
      ? {
          ssid: 'Example Hotel',
          bssid: '02:00:5e:10:00:01',
          channel: 44,
          frequency: 5220,
          width: 80,
          htmode: 'HE80',
          hwmode: 'ax',
          encryption: 'WPA2 PSK',
          hardware: 'MediaTek',
          txpower: 20,
          signal: -63,
          signal_avg: -62,
          noise: -94,
          quality: 47,
          quality_max: 70,
          tx: { kbps: 576500, mode: 'HE', mcs: 9, nss: 2, mhz: 80, short_gi: null },
          rx: { kbps: 720600, mode: 'HE', mcs: 11, nss: 2, mhz: 80, short_gi: null },
          expected_kbps: 312000,
          connected_seconds: 15240,
          inactive_ms: 40,
          busy_permille: 284,
        }
      : null,
};

const interfaces = [
  { id: 'uplink:wan', name: 'wan', device: 'eth1', kind: 'uplink', base: 9e6, endpoint: null },
  { id: 'uplink:wwan', name: 'wwan', device: 'apclix0', kind: 'wifi', base: 2.4e6, endpoint: null },
  {
    id: 'tunnel:home',
    name: 'wg-home',
    device: 'wg0',
    kind: 'tunnel',
    base: 6e6,
    endpoint: '203.0.113.10:51820',
  },
  {
    id: 'tunnel:backup',
    name: 'wg-backup',
    device: 'wg1',
    kind: 'tunnel',
    base: 0.4e6,
    endpoint: '198.51.100.24:51820',
  },
];
const liveTelemetry = () => {
  const t = now() / 1000;
  return {
    enabled: telemetryOn,
    sampleAgeMs: 1200,
    interval: 5,
    system: telemetryOn
      ? {
          cpu: Math.round((14 + Math.sin(t / 40) * 6) * 10) / 10,
          load: [0.41, 0.33, 0.29],
          memTotal: 1032192000,
          memAvailable: 598000000,
          temperature: 58.2,
          conntrack: 1412,
          conntrackMax: 16384,
          uptime: 1234567,
        }
      : null,
    interfaces: telemetryOn
      ? interfaces.map((i, n) => ({
          id: i.id,
          name: i.name,
          device: i.device,
          kind: i.kind,
          present: true,
          rxBps: Math.round(i.base * (1 + 0.3 * Math.sin(t / 17 + n))),
          txBps: Math.round(i.base * 0.18 * (1 + 0.3 * Math.cos(t / 13 + n))),
          rxBytes: Math.round(i.base * 9000 * (n + 1)),
          txBytes: Math.round(i.base * 1300 * (n + 1)),
          errors: 0,
          drops: n === 1 ? 17 : 0,
          handshakeAge: i.kind === 'tunnel' ? (n === 3 ? 212 : 41) : null,
          endpoint: i.endpoint,
        }))
      : [],
  };
};

let historyCutoff = 0;
const later = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const copy = (o) => JSON.parse(JSON.stringify(o));
const delay = (o) => later(160).then(() => copy(o));

const slots = (start, end, step) => {
  const ms = step * 1000;
  const first = Math.floor(start / ms) * ms;
  const count = Math.max(1, Math.floor((end - first) / ms) + 1);
  return { start: first, step, count, at: (i) => first + i * ms };
};
const minuteOf = (at) => Math.round((at - anchor) / 60000) + 1440;

const base64 = (bytes) => {
  let text = '';
  for (let i = 0; i < bytes.length; i += 0x8000)
    text += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(text);
};
const arrow = (frame, columns, types, meta) => ({
  format: 'arrow',
  start: frame.start,
  step: frame.step,
  count: frame.count,
  ...meta,
  data: base64(tableToIPC(tableFromArrays(columns, { types }), { format: 'stream' })),
});

const measurableUplinks = state.uplinks
  .filter((u) => !u.active && u.state === 'reachable')
  .map((u) => u.id);

function latencySeries(id, target, frame) {
  const bias = target === 'example.com' ? 7 : target === '9.9.9.9' ? 4 : 0;
  const base =
    { home: 32, backup: 24, tokyo: 130, zurich: 62, wan: 14, 'uplink:wwan': 22 }[id] || 32;
  const out = {
    median: [],
    min: [],
    max: [],
    jitter: [],
    p50: [],
    p95: [],
    p99: [],
    sent: [],
    lost: [],
  };
  const per = Math.max(1, Math.round(frame.step / 5));
  for (let i = 0; i < frame.count; i++) {
    const at = frame.at(i);
    const n = minuteOf(at);
    const before = at <= historyCutoff || at > now();
    const outage = id === 'home' && n > 950 && n < 964;
    const ongoing = id === 'tokyo' && at > now() - 95000;
    const spike = id !== 'wan' && n > 640 && n < 670 ? 50 : 0;
    const v = Math.max(1, base + bias + Math.sin(n / 13) * 4 + Math.sin(n * 1.7) * 3 + spike);
    const lost =
      outage || ongoing
        ? per
        : id === 'wan' && n % 53 < 2
          ? 1
          : id === 'tokyo' && n % 7 === 0
            ? 1
            : 0;
    if (before) {
      ['median', 'min', 'max', 'jitter', 'p50', 'p95', 'p99'].forEach((k) => out[k].push(null));
      out.sent.push(0);
      out.lost.push(0);
      continue;
    }
    const dead = outage || ongoing;
    out.median.push(dead ? null : Math.round(v * 1000));
    out.min.push(dead ? null : Math.round((v - 3) * 1000));
    out.max.push(dead ? null : Math.round((v + 5 + (n % 91 === 0 ? 35 : 0)) * 1000));
    out.jitter.push(
      dead ? null : Math.round((id === 'tokyo' ? 14 : 1.2) * (1 + 0.4 * Math.sin(n / 9)) * 1000),
    );
    const spread = id === 'tokyo' ? 3.5 : 1;
    out.p50.push(dead ? null : Math.round((v + 0.4) * 1000));
    out.p95.push(dead ? null : Math.round((v + 3.5 * spread + (n % 91 === 0 ? 20 : 0)) * 1000));
    out.p99.push(dead ? null : Math.round((v + 4.8 * spread + (n % 91 === 0 ? 33 : 0)) * 1000));
    out.sent.push(per);
    out.lost.push(lost);
  }
  return out;
}

function serverSeries(id, target, frame) {
  const ratio = { home: 0.72, backup: 0.68, tokyo: 0.76 }[id] || 0.7;
  const through = latencySeries(id, target, frame);
  const scaleCol = (col) => col.map((v) => (v === null ? null : Math.round(v * ratio)));
  return {
    median: scaleCol(through.median),
    min: scaleCol(through.min),
    max: scaleCol(through.max),
    jitter: scaleCol(through.jitter),
    p50: scaleCol(through.p50),
    p95: scaleCol(through.p95),
    p99: scaleCol(through.p99),
    sent: through.sent,
    lost: through.sent.map(() => 0),
  };
}

function wifiHistory(start, end, step) {
  const frame = slots(start, end, step);
  const cols = {};
  const keys = [
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
  keys.forEach((k) => (cols[k] = []));
  const total = {
    samples: 0,
    signal: [],
    snr: [],
    txRate: [],
    retries: [],
    busy: [],
    rxAir: [],
    txAir: [],
    disconnects: 0,
    roams: 0,
    txBytes: 0,
    rxBytes: 0,
  };
  const per = Math.max(1, Math.round(step / 5));
  const mean = (list) => (list.length ? list.reduce((a, b) => a + b, 0) / list.length : null);
  for (let i = 0; i < frame.count; i++) {
    const at = frame.at(i);
    const n = minuteOf(at);
    const gap = n > 700 && n < 708;
    if (at <= historyCutoff || at > now() || wifiMode === 'none' || gap) {
      keys.forEach((k) =>
        cols[k].push(
          ['samples', 'disconnects', 'roams', 'txBytes', 'rxBytes'].includes(k) ? 0 : null,
        ),
      );
      if (gap) {
        cols.samples[i] = per;
        cols.uptime[i] = 0;
      }
      continue;
    }
    const evening = Math.sin(n / 180) * 6;
    const signal = Math.round((-62 + evening + Math.sin(n * 1.3) * 2) * 10) / 10;
    const busy = Math.max(4, Math.round((34 - evening * 2 + Math.sin(n / 7) * 8) * 10) / 10);
    const txRate = Math.round(560 + evening * 25 + Math.sin(n / 5) * 40);
    const retries = Math.max(
      0.5,
      Math.round((6 - evening * 0.6 + Math.sin(n * 0.7) * 2) * 10) / 10,
    );
    const disconnects = n === 708 ? 1 : 0;
    const roams = n % 397 === 0 ? 1 : 0;
    const txBytes = Math.round(step * 120000 * (1.2 + Math.sin(n / 30)));
    const rxBytes = txBytes * 6;
    const rxAir = Math.round(busy * 0.42 * 10) / 10;
    const txAir = Math.round(busy * 0.16 * 10) / 10;
    const row = {
      samples: per,
      uptime: 100,
      signal,
      signalMin: Math.round(signal - 4),
      signalMax: Math.round(signal + 3),
      noise: -94,
      snr: Math.round((signal + 94) * 10) / 10,
      txRate,
      txRateMin: txRate - 120,
      rxRate: txRate + 140,
      expected: Math.round(txRate * 0.55),
      retries,
      failures: 0.2,
      busy,
      rxAir,
      txAir,
      upload: (txBytes * 8) / step / 1e6,
      download: (rxBytes * 8) / step / 1e6,
      txBytes,
      rxBytes,
      disconnects,
      roams,
    };
    keys.forEach((k) => cols[k].push(row[k]));
    total.samples += per;
    ['signal', 'snr', 'txRate', 'retries', 'busy', 'rxAir', 'txAir'].forEach((k) =>
      total[k].push(row[k]),
    );
    total.disconnects += disconnects;
    total.roams += roams;
    total.txBytes += txBytes;
    total.rxBytes += rxBytes;
  }
  const signals = cols.signal.filter((v) => v !== null);
  const summary = {
    samples: total.samples,
    uptime: total.samples ? 99.4 : null,
    signal: mean(total.signal),
    signalMin: signals.length ? Math.min(...signals) - 4 : null,
    signalMax: signals.length ? Math.max(...signals) + 3 : null,
    noise: total.samples ? -94 : null,
    snr: mean(total.snr),
    txRate: mean(total.txRate),
    txRateMin: total.txRate.length ? Math.min(...total.txRate) - 120 : null,
    rxRate: null,
    expected: null,
    retries: mean(total.retries),
    failures: total.samples ? 0.2 : null,
    busy: mean(total.busy),
    rxAir: mean(total.rxAir),
    txAir: mean(total.txAir),
    upload: null,
    download: null,
    txBytes: total.txBytes,
    rxBytes: total.rxBytes,
    disconnects: total.disconnects,
    roams: total.roams,
  };
  const f64 = [
    'uptime',
    'signal',
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
  ];
  const types = {};
  f64.forEach((k) => (types[k] = float64()));
  types.signalMin = int32();
  types.signalMax = int32();
  ['samples', 'disconnects', 'roams'].forEach((k) => (types[k] = uint32()));
  types.txBytes = int64();
  types.rxBytes = int64();
  const events = [];
  for (let i = 0; i < frame.count; i++) {
    if (cols.disconnects[i])
      events.push({
        at: frame.at(i),
        subject: 'wifi',
        kind: 'drop',
        text: 'Disconnected from Example Hotel (02:00:5e:10:00:01)',
      });
    if (cols.roams[i])
      events.push({
        at: frame.at(i),
        subject: 'wifi',
        kind: 'roam',
        text: 'Roamed to 02:00:5e:10:00:02 on channel 36',
      });
  }
  return arrow(frame, cols, types, { summary, events });
}

function telemetryHistory(start, end, step) {
  const frame = slots(start, end, step);
  const sys = {
    cpu: [],
    cpuMax: [],
    load: [],
    memUsed: [],
    temperature: [],
    temperatureMax: [],
    memAvailableMin: [],
    conntrack: [],
  };
  const ifc = interfaces.map(() => ({
    rxBps: [],
    txBps: [],
    rxPeakBps: [],
    txPeakBps: [],
    rxBytes: [],
    txBytes: [],
    errors: [],
    drops: [],
    handshakeAgeMax: [],
  }));
  for (let i = 0; i < frame.count; i++) {
    const at = frame.at(i);
    const n = minuteOf(at);
    const none = at <= historyCutoff || at > now() || !telemetryOn;
    const cpu =
      12 + Math.sin(n / 40) * 6 + (n > 640 && n < 670 ? 30 : 0) + Math.abs(Math.sin(n * 2.3)) * 3;
    sys.cpu.push(none ? null : Math.round(cpu * 10) / 10);
    sys.cpuMax.push(
      none ? null : Math.min(100, Math.round((cpu + 12 + Math.abs(Math.sin(n)) * 20) * 10) / 10),
    );
    sys.load.push(none ? null : Math.round((0.3 + cpu / 60) * 100) / 100);
    sys.memUsed.push(none ? null : Math.round((41 + Math.sin(n / 300) * 3) * 10) / 10);
    sys.memAvailableMin.push(none ? null : 590000000 + Math.round(Math.sin(n / 50) * 1e7));
    sys.temperature.push(none ? null : Math.round((56 + cpu / 6) * 10) / 10);
    sys.temperatureMax.push(none ? null : Math.round((58 + cpu / 5) * 10) / 10);
    sys.conntrack.push(
      none ? null : Math.round(1200 + Math.sin(n / 60) * 400 + (n > 640 && n < 670 ? 3000 : 0)),
    );
    interfaces.forEach((iface, k) => {
      const c = ifc[k];
      const day = 1 + 0.6 * Math.sin(n / 230 + k);
      const rx = Math.round(iface.base * day * (1 + 0.25 * Math.sin(n * 1.1 + k)));
      const tx = Math.round(rx * 0.18);
      c.rxBps.push(none ? null : rx);
      c.txBps.push(none ? null : tx);
      c.rxPeakBps.push(none ? null : Math.round(rx * (1.8 + Math.abs(Math.sin(n * 0.7)))));
      c.txPeakBps.push(none ? null : Math.round(tx * 2.1));
      c.rxBytes.push(none ? null : Math.round((rx / 8) * step));
      c.txBytes.push(none ? null : Math.round((tx / 8) * step));
      c.errors.push(none ? null : 0);
      c.drops.push(none ? null : k === 1 && n % 37 === 0 ? 3 : 0);
      c.handshakeAgeMax.push(
        none || iface.kind !== 'tunnel'
          ? null
          : k === 3 && n > 1380
            ? 120 + (n - 1380) * 3
            : 60 + (n % 2) * 60 + 4,
      );
    });
  }
  const cols = {};
  const types = {};
  Object.entries(sys).forEach(([k, v]) => {
    cols['system.' + k] = v;
    types['system.' + k] = k === 'memAvailableMin' || k === 'conntrack' ? int64() : float64();
  });
  ifc.forEach((c, k) =>
    Object.entries(c).forEach(([key, v]) => {
      cols[k + '.' + key] = v;
      types[k + '.' + key] = int64();
    }),
  );
  return arrow(frame, cols, types, {
    interfaces: interfaces.map(({ id, name, device, kind }) => ({ id, name, device, kind })),
    conntrackMax: telemetryOn ? 16384 : null,
  });
}

const busyPhases = ['checking', 'downloading', 'verifying', 'installing'];
const updateFailure = {
  phase: 'failed',
  failed: true,
  message: 'The package signature did not match. Nothing was installed.',
};
async function runUpdate(steps) {
  for (const [ms, patch] of steps) {
    await later(ms);
    state.update = { ...state.update, ...patch(), updatedAt: nowSeconds() };
  }
}
function startUpdate(phase, steps) {
  if (busyPhases.includes(state.update.phase))
    return Promise.resolve({ started: false, update: copy(state.update) });
  state.update = { ...state.update, phase, message: '', failed: false };
  void runUpdate(steps);
  return Promise.resolve({ started: true, update: copy(state.update) });
}

let stateReads = 0;
window.previewApi = {
  get_state: async () => {
    if (query.get('switch') === 'lost-busy') await later(2500);
    if (query.get('slow') === '1' && stateReads++ === 0) await later(8000);
    if (query.get('fail') === '1') throw Error('RPC error: Object not found');
    const live = { ...copy(state), telemetry: liveTelemetry() };
    live.incidents = live.incidents.filter((i) => i.start > historyCutoff);
    return level === 'unsupported'
      ? {
          ...live,
          tunnels: [],
          endpoints: [],
          diagnostics: {},
          resolved: {},
          uplinks: [],
          events: [],
          incidents: [],
          auto: { enabled: false, last: null },
        }
      : live;
  },
  set_config: async ({ config, expected_revision }) => {
    if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
    const known = Object.keys(state.config);
    if (Object.keys(config).some((key) => !known.includes(key)))
      throw Error('The settings contain an unknown field.');
    state.config = copy(config);
    state.wifi.enabled = !!config.upstreamWifi;
    state.wifi.reason = config.upstreamWifi ? wifiReasons[wifiMode] : wifiReasons.off;
    if (wifiMode === 'off') state.wifi.reason = config.upstreamWifi ? '' : wifiReasons.off;
    state.revision = String(++revision);
    return delay({ applied: true });
  },
  select_tunnel: async ({ section_id, expected_revision }) => {
    if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
    if (query.get('switch') === 'rejected') throw Error('Selection was rejected.');
    const t = state.tunnels.find((row) => row.id === section_id);
    if (!t || !t.selectable) throw Error(t ? t.selectReason : 'Unknown tunnel.');
    if (!t.preferred) {
      state.tunnels.forEach((row) => (row.preferred = row.id === section_id));
      state.revision = String(++revision);
    }
    if (query.get('switch') === 'lost-renamed') {
      t.id += '-renamed';
      names[t.id] = t.name;
    }
    if (query.get('switch') === 'lost-busy') await later(6000);
    if (['lost', 'lost-busy', 'lost-renamed'].includes(query.get('switch')))
      throw Error('The response was lost.');
    return delay({ applied: true });
  },
  set_tunnel_enabled: async ({ section_id, enabled, expected_revision }) => {
    if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
    const tunnel = state.tunnels.find((item) => item.id === section_id);
    if (!tunnel || !tunnel.controllable || typeof enabled !== 'boolean')
      throw Error('The tunnel state is invalid.');
    if (tunnel.enabled !== enabled) {
      Object.assign(tunnel, {
        enabled,
        connected: false,
        eligible: false,
        selectable: true,
        historyAvailable: true,
        stateLabel: enabled ? 'Enabled, offline' : 'Stopped',
        age: 'unknown',
        median: null,
        jitter: null,
        samples: 0,
        lost: 0,
      });
      state.revision = String(++revision);
      Object.values(state.diagnostics).forEach((values) => (values[section_id] = null));
    }
    return delay({ applied: true, enabled });
  },
  purge_history: async ({ confirm }) => {
    if (confirm !== true) throw Error('Confirm history deletion first.');
    historyCutoff = now();
    state.events = [];
    return delay({ applied: true });
  },
  get_history: async ({
    target,
    start,
    end,
    step,
    include_wan,
    all,
    endpoints,
    uplinks,
    tunnel,
    format,
  }) => {
    if (format !== 'arrow') throw Error('The preview only serves Arrow history.');
    if (query.get('slow') === '1') await later(step > 15 ? 2000 : 700);
    const frame = slots(start, end, step);
    const ids = all
      ? state.tunnels.filter((t) => t.connected || t.historyAvailable).map((t) => t.id)
      : [tunnel];
    const withServers = all && endpoints && state.config.endpointDiagnostics;
    const series = [
      ...ids.map((id) => ({ id, name: names[id], data: latencySeries(id, target, frame) })),
      ...(include_wan && !wanDown
        ? [{ id: 'wan', name: names.wan, data: latencySeries('wan', target, frame) }]
        : []),
      ...(uplinks
        ? measurableUplinks.map((id) => ({ id, name: id, data: latencySeries(id, target, frame) }))
        : []),
      ...(withServers
        ? ids
            .filter((id) => id !== 'zurich')
            .map((id) => ({
              id: 'endpoint:' + id,
              name: names[id] + ' server',
              data: serverSeries(id, target, frame),
            }))
        : []),
    ];
    const cols = {};
    const types = {};
    series.forEach((s, k) => {
      ['median', 'min', 'max', 'jitter', 'p50', 'p95', 'p99'].forEach((key) => {
        cols[k + '.' + key] = s.data[key];
        types[k + '.' + key] = int32();
      });
      ['sent', 'lost'].forEach((key) => {
        cols[k + '.' + key] = s.data[key];
        types[k + '.' + key] = uint32();
      });
    });
    const events = state.events.filter((e) => e.at >= start && e.at <= end);
    const incidents = state.incidents.filter(
      (i) =>
        i.start <= end &&
        (i.end === null || i.end >= start) &&
        i.start > historyCutoff &&
        i.kind === 'loss',
    );
    await later(120);
    return arrow(frame, cols, types, {
      series: series.map(({ id, name }) => ({ id, name })),
      events,
      incidents,
    });
  },
  get_wifi_history: async ({ start, end, step, format }) => {
    if (format !== 'arrow') throw Error('The preview only serves Arrow history.');
    if (!state.config.upstreamWifi) throw Error('Upstream Wi-Fi monitoring is off.');
    await later(120);
    return wifiHistory(start, end, step);
  },
  get_diagnostics: async () => {
    await later(120);
    const now = Date.now();
    const hour = 3_600_000;
    const tier = (kind, step, files, size, span) => ({
      kind,
      step,
      files,
      bytes: size,
      first: now - span,
      last: now,
    });
    return {
      healthy: true,
      warnings: 0,
      generated: now,
      checks: [
        { status: 'info', label: 'package version', detail: release },
        { status: 'ok', label: 'firmware support', detail: 'full (tested module). Preview data.' },
        { status: 'ok', label: '/www/views/wayline', detail: '13 page chunks' },
        { status: 'ok', label: 'monitor service', detail: 'running' },
        { status: 'ok', label: 'WAN path', detail: 'wwan on apclii0' },
        { status: 'info', label: 'measurement series', detail: '7' },
        { status: 'info', label: 'probe interval', detail: '1 s' },
      ],
      storage: {
        root: '/etc/wayline/history',
        inMemory: false,
        days: 30,
        bytes: 398_862,
        cap: 100_663_296,
        tiers: [
          tier('latency', 15, 5, 95_138, 6 * hour),
          tier('latency', 60, 2, 32_334, 24 * hour),
          tier('latency', 300, 2, 21_807, 72 * hour),
          tier('latency', 3600, 1, 9_602, 168 * hour),
          tier('wifi', 15, 5, 51_036, 6 * hour),
          tier('system', 15, 5, 33_291, 6 * hour),
          tier('interface', 15, 5, 45_032, 6 * hour),
        ],
        kept: [
          { step: 15, keptSeconds: 21_600 },
          { step: 60, keptSeconds: 259_200 },
          { step: 300, keptSeconds: 2_592_000 },
          { step: 3600, keptSeconds: 2_592_000 },
        ],
        records: [
          { kind: 'event', files: 1, rows: 16, bytes: 2_558, first: now - 5 * hour, last: now },
          { kind: 'incident', files: 1, rows: 2, bytes: 1_802, first: now - 4 * hour, last: now },
        ],
        dimensions: 3_686,
        legacy: [],
        pending: 0,
        lastCollect: now - 120_000,
      },
    };
  },
  get_telemetry_history: async ({ start, end, step, format }) => {
    if (format !== 'arrow') throw Error('The preview only serves Arrow history.');
    if (!state.config.telemetry) throw Error('Router telemetry is off.');
    await later(120);
    return telemetryHistory(start, end, step);
  },
  check_update: () =>
    startUpdate('checking', [
      [1200, () => ({ phase: 'idle', available: true, latest: release, checkedAt: nowSeconds() })],
    ]),
  install_update: () =>
    startUpdate('downloading', [
      [1500, () => ({ phase: 'verifying' })],
      [1000, () => (updateFailed ? updateFailure : { phase: 'installing' })],
      ...(updateFailed
        ? []
        : [
            [
              1500,
              () => {
                state.version = release;
                return { phase: 'done', current: release, available: false, latest: release };
              },
            ],
          ]),
    ]),
  clear_update: () => {
    state.update = {
      ...state.update,
      phase: 'idle',
      failed: false,
      needsAttention: false,
      message: 'Wayline ' + installed + ' matches the package manager. Updates can run again.',
      updatedAt: nowSeconds(),
    };
    return Promise.resolve({ cleared: true, update: copy(state.update) });
  },
};

const reads = [
  'get_state',
  'get_history',
  'get_wifi_history',
  'get_telemetry_history',
  'get_diagnostics',
];
window.previewRpc = (method, params) => {
  const [sid, object, operation, request] = params;
  const expected = reads.slice(0, 4).includes(operation)
    ? 'wayline'
    : 'wayline_' + release.replace(/\./g, '_');
  if (
    method !== 'call' ||
    sid !== 'sid' ||
    object !== expected ||
    !Object.prototype.hasOwnProperty.call(window.previewApi, operation)
  )
    return Promise.reject(Error('RPC error: Method not found'));
  return window.previewApi[operation](request);
};

class PreviewSource {
  constructor(api) {
    this.api = api;
    this.setConfig = (request) => api.set_config(request);
    this.selectTunnel = (request) => api.select_tunnel(request);
    this.setTunnelEnabled = (request) => api.set_tunnel_enabled(request);
    this.purgeHistory = (request) => api.purge_history(request);
    this.checkUpdate = () => api.check_update();
    this.installUpdate = () => api.install_update();
    this.clearUpdate = () => api.clear_update();
  }
  getState() {
    return this.api.get_state();
  }
  getDiagnostics() {
    return this.api.get_diagnostics();
  }
  async getHistory(query) {
    return decodeHistory(await this.api.get_history({ ...query, format: 'arrow' }), latencyDoc);
  }
  async getWifiHistory(query) {
    return decodeHistory(await this.api.get_wifi_history({ ...query, format: 'arrow' }), wifiDoc);
  }
  async getTelemetryHistory(query) {
    return decodeHistory(
      await this.api.get_telemetry_history({ ...query, format: 'arrow' }),
      telemetryDoc,
    );
  }
}

window.previewSource =
  query.get('transport') === 'rpc' ? null : new PreviewSource(window.previewApi);
