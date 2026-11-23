import { computed } from './signal.ts';
import { finite, offChannel } from './format.ts';
import { score, type Score } from './mos.ts';
import { latencyLine, sum, times, type LatencyLine } from './series.ts';
import { exitTunnel, latencyData, primary, state, wifiData } from './store.ts';
import type { EgressPath, Incident, LatencyDoc, Num, Tunnel, Uplink } from './types.ts';

export const endpointPrefix = 'endpoint:';
export const uplinkPrefix = 'uplink:';

// Any store write invalidates every computed value. Rebuilding these only
// when the history document changes keeps their arrays stable, so charts
// skip redrawing on unrelated writes such as the 5-second state poll.
const perDoc = <T>(make: (doc: LatencyDoc) => T, empty: T) => {
  let seen: LatencyDoc | null = null;
  let out = empty;
  return computed(() => {
    const doc = latencyData.value.doc;
    if (doc !== seen) {
      seen = doc;
      out = doc ? make(doc) : empty;
    }
    return out;
  });
};

export const latencyX = perDoc<ArrayLike<number>>(times, []);

export const latencyLines = perDoc<LatencyLine[]>(
  (doc) => doc.series.map((s) => latencyLine(doc, s)),
  [],
);

export const lineFor = (id: string): LatencyLine | null =>
  latencyLines.value.find((l) => l.id === id) || null;

export const egressFor = (id: string): EgressPath | null =>
  (state.value.egress?.paths || []).find((p) => p.id === id) || null;

export const incidents = computed<Incident[]>(() => {
  const seen = new Set<string>();
  const out: Incident[] = [];
  const add = (list: Incident[]) =>
    list.forEach((i) => {
      const key = i.subject + '|' + i.start;
      if (seen.has(key)) return;
      seen.add(key);
      out.push(i);
    });
  add(state.value.incidents);
  add(latencyData.value.doc?.incidents || []);
  return out.sort((a, b) => b.start - a.start);
});

export const ongoing = computed(() => incidents.value.filter((i) => i.end === null));

export interface Loss {
  pct: Num;
  lost: number;
  sent: number;
  source: 'live' | 'range';
}

export interface PathHealth {
  median: Num;
  jitter: Num;
  loss: Loss;
  score: Score | null;
  down: Incident | null;
}

const rangeLoss = (id: string): Loss => {
  const line = lineFor(id);
  const sent = line ? sum(line.sent) : 0;
  const lost = line ? sum(line.lost) : 0;
  return { pct: sent ? (lost / sent) * 100 : null, lost, sent, source: 'range' };
};

export const liveLoss = (stat: { samples: number; lost: number } | null): Loss => ({
  pct: stat && stat.samples ? (stat.lost / stat.samples) * 100 : null,
  lost: stat ? stat.lost : 0,
  sent: stat ? stat.samples : 0,
  source: 'live',
});

export const tunnelLoss = (t: Tunnel): Loss => ({
  pct: t.samples ? (t.lost / t.samples) * 100 : null,
  lost: t.lost,
  sent: t.samples,
  source: 'live',
});

export const uplinkLoss = (u: Uplink): Loss =>
  finite(u.samples) && finite(u.lost)
    ? {
        pct: u.samples ? (u.lost / u.samples) * 100 : null,
        lost: u.lost,
        sent: u.samples,
        source: 'live',
      }
    : rangeLoss(u.active ? 'wan' : u.id);

const down = (ids: string[]): Incident | null =>
  ongoing.value.find((i) => ids.includes(i.subject)) || null;

export const activeUplink = computed(() => state.value.uplinks.find((u) => u.active) || null);

export const internet = computed<PathHealth | null>(() => {
  const s = state.value;
  if (!s.config.directWan) return null;
  const u = activeUplink.value;
  const fromDiag = (s.diagnostics[primary.value] || {}).wan;
  const median = finite(fromDiag) ? fromDiag : u && finite(u.median) ? u.median : null;
  const jitter = u && finite(u.jitter) ? u.jitter : null;
  const loss = u ? uplinkLoss(u) : rangeLoss('wan');
  return {
    median,
    jitter,
    loss,
    score: score(median, jitter, loss.pct),
    down: down(['wan', ...(u ? [u.id] : [])]),
  };
});

export const vpn = computed<PathHealth | null>(() => {
  const t = exitTunnel.value;
  if (!t) return null;
  const loss = tunnelLoss(t);
  const jitter = finite(t.jitter) ? t.jitter : null;
  return {
    median: t.median,
    jitter,
    loss,
    score: score(t.median, jitter, loss.pct),
    down: down([t.id, t.monitorId]),
  };
});

export type HealthTone = 'ok' | 'degraded' | 'down' | 'off' | 'unknown';
export interface Health {
  id: 'internet' | 'vpn' | 'wifi' | 'router';
  label: string;
  tone: HealthTone;
  text: string;
  reason: string;
}

const pathTone = (p: PathHealth): [HealthTone, string] => {
  if (p.down) return ['down', p.down.detail || 'Every probe lost'];
  if (!finite(p.median)) return ['unknown', 'No recent measurement'];
  if (finite(p.loss.pct) && p.loss.pct >= 1) return ['degraded', p.loss.pct.toFixed(1) + ' % loss'];
  if (p.score && p.score.band === 'poor')
    return ['degraded', 'Call quality ' + p.score.mos.toFixed(1) + ' of 5'];
  return ['ok', p.median.toFixed(2) + ' ms'];
};

export const wifiLevel = {
  signal: (v: number) => (v >= -67 ? 'good' : v >= -75 ? 'fair' : 'poor'),
  snr: (v: number) => (v >= 25 ? 'good' : v >= 15 ? 'fair' : 'poor'),
  retries: (v: number) => (v <= 10 ? 'good' : v <= 25 ? 'fair' : 'poor'),
  busy: (v: number) => (v <= 50 ? 'good' : v <= 70 ? 'fair' : 'poor'),
} as const;

export const toneLabel: Record<string, string> = {
  good: 'Good',
  fair: 'Fair',
  poor: 'Poor',
  ok: 'OK',
  degraded: 'Degraded',
  down: 'Down',
  off: 'Off',
  unknown: 'Unknown',
};

export const memUsed = (sys: { memTotal: Num; memAvailable: Num } | null): Num =>
  sys && finite(sys.memTotal) && finite(sys.memAvailable) && sys.memTotal > 0
    ? (1 - sys.memAvailable / sys.memTotal) * 100
    : null;

export const routerLimits = { cpu: 90, mem: 90, conntrack: 90, temperature: 85 };

export const health = computed<Health[]>(() => {
  const s = state.value;
  const items: Health[] = [];
  const net = internet.value;
  if (!net)
    items.push({
      id: 'internet',
      label: 'Internet',
      tone: 'off',
      text: 'Not measured',
      reason: 'Direct internet measurement is off.',
    });
  else if (!s.wan.available)
    items.push({
      id: 'internet',
      label: 'Internet',
      tone: 'unknown',
      text: 'Unavailable',
      reason: s.wan.reason || 'The direct internet connection is unavailable.',
    });
  else {
    const [tone, text] = pathTone(net);
    items.push({ id: 'internet', label: 'Internet', tone, text, reason: text });
  }
  const p = vpn.value;
  const preferred = s.tunnels.find((t) => t.preferred);
  if (!s.tunnels.length)
    items.push({
      id: 'vpn',
      label: 'VPN exit',
      tone: 'off',
      text: 'No tunnels',
      reason: 'No VPN tunnels are configured.',
    });
  else if (!p)
    items.push(
      preferred && preferred.enabled
        ? {
            id: 'vpn',
            label: 'VPN exit',
            tone: 'down',
            text: preferred.stateLabel,
            reason: preferred.name + ' is ' + preferred.stateLabel.toLowerCase() + '.',
          }
        : {
            id: 'vpn',
            label: 'VPN exit',
            tone: 'off',
            text: 'Direct',
            reason: 'Traffic leaves without a VPN tunnel.',
          },
    );
  else {
    let [tone, text] = pathTone(p);
    if (tone === 'ok' && s.auto.needsAttention) [tone, text] = ['degraded', 'Auto-switch stopped'];
    items.push({ id: 'vpn', label: 'VPN exit', tone, text, reason: text });
  }
  const w = s.wifi;
  if (w.radio !== false) {
    if (!w.enabled)
      items.push({
        id: 'wifi',
        label: 'Wi-Fi',
        tone: 'off',
        text: 'Not monitored',
        reason: 'Upstream Wi-Fi monitoring is off.',
      });
    else if (w.state === 'disconnected')
      items.push({
        id: 'wifi',
        label: 'Wi-Fi',
        tone: 'down',
        text: 'Disconnected',
        reason: w.reason || 'Disconnected.',
      });
    else if (!w.link)
      items.push({
        id: 'wifi',
        label: 'Wi-Fi',
        tone: 'unknown',
        text: 'No link',
        reason: w.reason || 'No link data.',
      });
    else if (offChannel(w.link))
      items.push({
        id: 'wifi',
        label: 'Wi-Fi',
        tone: 'down',
        text: 'Radio on the wrong channel',
        reason: offChannel(w.link) || '',
      });
    else {
      const l = w.link;
      const snr = finite(l.signal) && finite(l.noise) ? l.signal - l.noise : null;
      const busy = finite(l.busy_permille) ? l.busy_permille / 10 : null;
      const bad: string[] = [];
      if (finite(l.signal) && wifiLevel.signal(l.signal) === 'poor')
        bad.push('signal ' + l.signal + ' dBm');
      if (finite(snr) && wifiLevel.snr(snr) === 'poor') bad.push('weak signal over noise');
      if (finite(busy) && wifiLevel.busy(busy) === 'poor')
        bad.push('channel ' + busy.toFixed(0) + ' % busy');
      items.push({
        id: 'wifi',
        label: 'Wi-Fi',
        tone: bad.length ? 'degraded' : 'ok',
        text: bad.length ? bad[0] : finite(l.signal) ? l.signal + ' dBm' : 'Connected',
        reason: bad.join(', ') || 'Within the good bands.',
      });
    }
  }
  const sys = s.telemetry.system;
  if (!s.config.telemetry)
    items.push({
      id: 'router',
      label: 'Router',
      tone: 'off',
      text: 'Not monitored',
      reason: 'Router telemetry is off.',
    });
  else if (!sys)
    items.push({
      id: 'router',
      label: 'Router',
      tone: 'unknown',
      text: 'Waiting',
      reason: 'No telemetry sample yet.',
    });
  else {
    const bad: string[] = [];
    const mem = memUsed(sys);
    if (finite(sys.cpu) && sys.cpu >= routerLimits.cpu)
      bad.push('processor ' + sys.cpu.toFixed(0) + ' %');
    if (finite(mem) && mem >= routerLimits.mem) bad.push('memory ' + mem.toFixed(0) + ' %');
    if (
      finite(sys.conntrack) &&
      finite(sys.conntrackMax) &&
      sys.conntrackMax > 0 &&
      (sys.conntrack / sys.conntrackMax) * 100 >= routerLimits.conntrack
    )
      bad.push(
        'connection table ' + ((sys.conntrack / sys.conntrackMax) * 100).toFixed(0) + ' % full',
      );
    if (finite(sys.temperature) && sys.temperature >= routerLimits.temperature)
      bad.push(sys.temperature.toFixed(0) + ' °C');
    items.push({
      id: 'router',
      label: 'Router',
      tone: bad.length ? 'degraded' : 'ok',
      text: bad.length ? bad[0] : finite(sys.cpu) ? 'Processor ' + sys.cpu.toFixed(0) + ' %' : 'OK',
      reason: bad.join(', ') || 'Below the warning limits.',
    });
  }
  return items;
});

const severity: Record<HealthTone, number> = { down: 0, degraded: 1, unknown: 2, ok: 3, off: 4 };
export const chips = computed(() =>
  health.value.slice().sort((a, b) => severity[a.tone] - severity[b.tone]),
);

export const wifiRange = computed(() => wifiData.value.doc?.summary || null);
