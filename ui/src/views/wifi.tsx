import { Button } from '@ui-sdk/react/button';
import { useMemo, type ReactNode } from 'react';
import { Chart, type Marker } from '../chart.tsx';
import { toneLabel, wifiLevel } from '../derived.ts';
import {
  band,
  bytes,
  dash,
  dbm,
  finite,
  kbit,
  mbit,
  offChannel,
  percent,
  span,
  stamp,
  dateTime,
} from '../format.ts';
import { column, mapColumn, some, times } from '../series.ts';
import { busy, can, enableSetting, range, state, wifiData } from '../store.ts';
import type { Column, Rate, WifiKey, WifiNetwork } from '../types.ts';
import { Badge, Explain, Panel, scaleNote, Tip, type Tone } from '../ui.tsx';

const generation: Record<string, string> = {
  EHT: 'Wi-Fi 7',
  HE: 'Wi-Fi 6',
  VHT: 'Wi-Fi 5',
  HT: 'Wi-Fi 4',
};

const rateDetail = (rate: Rate): string => {
  if (!rate || !rate.mode) return '';
  const parts = [generation[rate.mode] || rate.mode];
  if (finite(rate.mcs)) parts.push('rate index ' + rate.mcs);
  if (finite(rate.nss)) parts.push(rate.nss + (rate.nss === 1 ? ' stream' : ' streams'));
  if (finite(rate.mhz)) parts.push(rate.mhz + ' MHz');
  if (rate.short_gi) parts.push('short guard interval');
  return parts.join(' · ');
};

const level = (kind: keyof typeof wifiLevel, value: number | null | undefined) =>
  finite(value) ? (
    <Badge
      tone={wifiLevel[kind](value) as Tone}
      title="Heuristic band from common Wi-Fi survey guidance"
    >
      {toneLabel[wifiLevel[kind](value)]}
    </Badge>
  ) : (
    ''
  );

function Tile(props: {
  label: string;
  tip: string;
  value: string;
  rating?: ReactNode;
  sub: string;
}) {
  return (
    <div className="wl-tile" role="listitem">
      <span className="wl-tile-label">
        <Tip text={props.tip}>{props.label}</Tip>
      </span>
      <span className="wl-tile-value">
        <span className="wl-num">{props.value}</span>
        {props.rating}
      </span>
      <span className="wl-tile-sub">{props.sub}</span>
    </div>
  );
}

function Group(props: { title: string; rows: [string, string, ReactNode?][] }) {
  return (
    <section className="wl-group" aria-label={props.title}>
      <h4>{props.title}</h4>
      <dl>
        {props.rows.map(([label, value, rating]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd className="wl-num">{value}</dd>
            <dd className="wl-group-rating">{rating}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

const THROUGHPUT_NOTE =
  'Average over each interval from the interface byte counters. A short speed test fills only part of an interval, so it reads lower here than in the test itself.';

function NetworkCard(props: { network: WifiNetwork; current: boolean }) {
  const n = props.network;
  const sum = n.summary;
  const known = n.id !== 'upstream';
  const metric = (label: string, value: ReactNode, sub?: string, tip?: string) => (
    <div>
      <dt>{tip ? <Tip text={tip}>{label}</Tip> : label}</dt>
      <dd>
        <span className="wl-metric-value">{value}</span>
        {sub && <small>{sub}</small>}
      </dd>
    </div>
  );
  return (
    <li className={'wl-pathcard' + (props.current ? ' wl-active' : '')}>
      <div className="wl-pathcard-head">
        <div className="wl-pathcard-title">
          <div className="wl-name-line">
            <span className="wl-name">
              {known ? n.ssid || 'Hidden network' : 'Before access points were recorded'}
            </span>
            {props.current && <Badge tone="good">Connected now</Badge>}
          </div>
          <div className="wl-muted wl-status">
            {known
              ? [n.bssid ? 'Access point ' + n.bssid : '', n.channel ? 'channel ' + n.channel : '']
                  .filter(Boolean)
                  .join(' · ')
              : 'History saved before this version, for every network together'}
          </div>
        </div>
      </div>
      <dl className="wl-metrics">
        {metric(
          'Time on it',
          <span className="wl-num">{span(n.connectedSeconds)}</span>,
          n.first === n.last
            ? stamp(n.first, true)
            : stamp(n.first, true) + ' to ' + stamp(n.last, true),
        )}
        {metric(
          'Signal',
          <>
            <span className="wl-num">{dbm(sum.signal)}</span> {level('signal', sum.signal)}
          </>,
          finite(sum.signalMin) ? 'weakest ' + dbm(sum.signalMin) : undefined,
        )}
        {metric(
          'Throughput',
          <span className="wl-num">
            {finite(sum.download) ? sum.download.toFixed(1) : dash}
            {' / '}
            {finite(sum.upload) ? sum.upload.toFixed(1) : dash}
          </span>,
          'Mbit/s down / up, average',
          THROUGHPUT_NOTE,
        )}
        {metric(
          'Drops and roams',
          <span className="wl-num">{(sum.disconnects ?? 0) + ' / ' + (sum.roams ?? 0)}</span>,
          'retries ' + percent(sum.retries),
        )}
      </dl>
    </li>
  );
}

export function WifiView() {
  const s = state.value;
  const w = s.wifi;
  const data = wifiData.value;
  const doc = data.doc;
  const built = useMemo(() => {
    if (!doc) return null;
    const col = (key: WifiKey): Column => column(doc, doc.columns[key]);
    const rxAir = col('rxAir');
    const txAir = col('txAir');
    const busyCol = col('busy');
    const own = mapColumn(rxAir, (v, i) =>
      finite(v) || finite(txAir[i]) ? (v || 0) + (txAir[i] || 0) : null,
    );
    const others = mapColumn(busyCol, (b, i) =>
      finite(b) && finite(own[i]) ? Math.max(0, b - (own[i] as number)) : null,
    );
    const disconnects = col('disconnects');
    const roams = col('roams');
    const markers: Marker[] = [];
    Array.from(disconnects).forEach((v, i) => {
      if (v)
        markers.push({
          at: doc.start + i * doc.step * 1000,
          tone: 'danger',
          text: v + (v === 1 ? ' drop' : ' drops'),
        });
    });
    Array.from(roams).forEach((v, i) => {
      if (v)
        markers.push({
          at: doc.start + i * doc.step * 1000,
          tone: 'brand',
          text: v + (v === 1 ? ' roam' : ' roams') + ' to another access point',
        });
    });
    doc.events.forEach((e) => markers.push({ at: e.at, tone: 'event', text: e.text }));
    return {
      x: times(doc),
      samples: col('samples'),
      signal: [col('signal'), col('signalMin'), col('signalMax'), col('noise')],
      snr: [col('snr')],
      rate: [col('txRate'), col('txRateMin'), col('rxRate'), col('expected')],
      health: [col('retries'), col('failures'), busyCol],
      air: [busyCol, own, others],
      traffic: [col('download'), col('upload')],
      markers,
    };
  }, [doc]);
  if (!w.enabled)
    return (
      <Panel id="wl-wifi-title" title="Upstream Wi-Fi" sub="Monitoring is off.">
        <div className="wl-cta">
          <p>
            Record the signal, noise, link rate, retries, and channel load of the Wi-Fi network that
            this router joins as a repeater. History follows the History setting.
          </p>
          <Button
            className="wl-button wl-primary"
            disabled={!can.value.configure || busy.value}
            data-agent-confirm="Turns on upstream Wi-Fi sampling on the router"
            onClick={() =>
              void enableSetting(
                'upstreamWifi',
                'Upstream Wi-Fi monitoring is on. The first samples arrive within seconds.',
              )
            }
          >
            Monitor upstream Wi-Fi
          </Button>
        </div>
      </Panel>
    );
  const l = w.link;
  const snr = l && finite(l.signal) && finite(l.noise) ? l.signal - l.noise : null;
  const busyPct = l && finite(l.busy_permille) ? l.busy_permille / 10 : null;
  const uplink = s.uplinks.find((u) => u.id === w.uplink) || null;
  const age = w.sampleAgeMs;
  const sub =
    'Sampled every ' +
    (w.interval || 5) +
    ' s.' +
    (finite(age) ? ' Last sample ' + Math.round(age / 1000) + ' s ago.' : '');
  const sum = doc?.summary || {};
  const hasData = !!built && some(built.samples, (v) => finite(v) && v > 0);
  const own = finite(sum.rxAir) || finite(sum.txAir) ? (sum.rxAir || 0) + (sum.txAir || 0) : null;
  const pctFmt = (v: number) => v.toFixed(1) + ' %';
  return (
    <>
      <Panel id="wl-wifi-title" title="Upstream Wi-Fi" sub={sub}>
        {w.reason && (
          <p className="wl-muted" role="status">
            {w.reason}
          </p>
        )}
        {l && offChannel(l) && (
          <p className="wl-note" role="alert">
            {offChannel(l)}
          </p>
        )}
        {l && (
          <>
            <div className="wl-now-head">
              <h3 className="wl-now-name">{l.ssid || 'Hidden network'}</h3>
              <Badge tone="good">
                {finite(l.connected_seconds)
                  ? 'Connected · up ' + span(l.connected_seconds)
                  : 'Connected'}
              </Badge>
              <ul className="wl-chips-plain" aria-label="Channel">
                {[
                  finite(l.frequency) ? band(l.frequency) : '',
                  l.channel ? 'Channel ' + l.channel : '',
                  l.width ? l.width + ' MHz wide' : '',
                ]
                  .filter(Boolean)
                  .map((part) => (
                    <li key={part}>{part}</li>
                  ))}
              </ul>
            </div>
            <div className="wl-tiles" role="list" aria-label="Current link">
              <Tile
                label="Signal"
                tip="Received signal strength. Closer to 0 is stronger: −67 dBm or better is good, below −75 dBm is poor."
                value={dbm(l.signal)}
                rating={level('signal', l.signal)}
                sub={
                  finite(snr)
                    ? snr + ' dB above noise (' + dbm(l.noise) + ')'
                    : 'noise ' + dbm(l.noise)
                }
              />
              <Tile
                label="Link rate"
                tip="The radio rate negotiated with the access point, not the actual throughput."
                value={
                  finite(l.tx.kbps) && finite(l.rx.kbps)
                    ? Math.round(l.tx.kbps / 1000) +
                      ' / ' +
                      Math.round(l.rx.kbps / 1000) +
                      ' Mbit/s'
                    : kbit(l.tx.kbps)
                }
                sub="sending / receiving"
              />
              <Tile
                label="Channel busy"
                tip="Share of airtime the radio found the channel in use, by this router and others."
                value={percent(busyPct)}
                rating={level('busy', busyPct)}
                sub={l.channel ? 'on channel ' + l.channel : ''}
              />
              <Tile
                label="Latency"
                tip="Ping latency to the primary destination through this Wi-Fi link, without the VPN."
                value={uplink && finite(uplink.median) ? uplink.median.toFixed(2) + ' ms' : dash}
                sub={
                  !w.uplink
                    ? 'not an uplink right now'
                    : !uplink || !finite(uplink.median)
                      ? 'turn on Direct internet to measure'
                      : 'via ' + uplink.interface + (uplink.active ? ', the active uplink' : '')
                }
              />
            </div>
            <dl className="wl-spec" aria-label="Link details">
              {(
                [
                  ['Access point', l.bssid],
                  ['Interface', w.interface || ''],
                  [
                    'Frequency',
                    [
                      finite(l.frequency) ? l.frequency + ' MHz' : '',
                      l.width ? l.width + ' MHz wide' : '',
                    ]
                      .filter(Boolean)
                      .join(', '),
                  ],
                  [
                    'Standard',
                    [l.hwmode ? '802.11' + l.hwmode : '', l.htmode || '']
                      .filter(Boolean)
                      .join(', '),
                  ],
                  ['Security', l.encryption || ''],
                  ['Sending', rateDetail(l.tx)],
                  ['Receiving', rateDetail(l.rx)],
                  ['Expected throughput', finite(l.expected_kbps) ? kbit(l.expected_kbps) : ''],
                ] as [string, string][]
              )
                .filter(([, value]) => value)
                .map(([label, value]) => (
                  <div key={label}>
                    <dt>{label}</dt>
                    <dd>{value}</dd>
                  </div>
                ))}
            </dl>
          </>
        )}
        {doc && finite(sum.samples) && sum.samples > 0 && (
          <>
            <h3 className="wl-h3">Selected range</h3>
            <div className="wl-groups">
              <Group
                title="Connection"
                rows={[
                  ['Connected', percent(sum.uptime)],
                  ['Samples', String(sum.samples)],
                  ['Drops', String(sum.disconnects ?? 0)],
                  ['Roams', String(sum.roams ?? 0)],
                ]}
              />
              <Group
                title="Signal"
                rows={[
                  ['Average', dbm(sum.signal), level('signal', sum.signal)],
                  ['Weakest', dbm(sum.signalMin)],
                  ['Strongest', dbm(sum.signalMax)],
                  [
                    'Signal to noise',
                    finite(sum.snr) ? sum.snr.toFixed(0) + ' dB' : dash,
                    level('snr', sum.snr),
                  ],
                ]}
              />
              <Group
                title="Link rate when sending"
                rows={[
                  ['Average', mbit(sum.txRate)],
                  ['Lowest', mbit(sum.txRateMin)],
                  ['Retries', percent(sum.retries), level('retries', sum.retries)],
                  ['Failures', percent(sum.failures)],
                ]}
              />
              <Group
                title="Airtime and data"
                rows={[
                  ['Channel busy', percent(sum.busy), level('busy', sum.busy)],
                  ['This router', percent(own)],
                  [
                    'Other devices',
                    finite(sum.busy) && finite(own) ? percent(Math.max(0, sum.busy - own)) : dash,
                  ],
                  ['Downloaded', bytes(sum.rxBytes)],
                  ['Uploaded', bytes(sum.txBytes)],
                ]}
              />
            </div>
          </>
        )}
        {data.error && (
          <p className="wl-muted" role="status">
            Wi-Fi history is unavailable. {data.error}
          </p>
        )}
        {!data.error && data.loaded && !hasData && (
          <p className="wl-muted">
            No Wi-Fi history in this range yet. The monitor saves 15-second summaries.
          </p>
        )}
        {!data.loaded && <p className="wl-muted">Loading Wi-Fi history…</p>}
        <Explain>
          <p>
            Read straight from the kernel over nl80211 (interface, station, survey and associated
            BSS) and from <code>/sys/class/net/&lt;if&gt;/statistics</code>, and sampled again at
            every connect or disconnect. Signal and noise outside −110..−1 and −120..−50 dBm are
            driver placeholders and are dropped. SNR = signal − noise.
          </p>
          <p>
            Retries = retried / (sent + retried) frames. Failures = failed / sent frames. Counters
            reset on reassociation. Channel busy = Δbusy / Δactive time of the serving frequency.
            Own airtime = Δ(receive + send time) / Δactive. Others = busy − own.
          </p>
          <p>
            A drop is losing the connection, a silent reassociation with the same access point, or
            moving to another network without a gap. A roam is moving to another access point of the
            same network. The access point is the station the client interface is associated with.
          </p>
          <p>
            Each 15-second interval belongs to the access point with most of its samples. Network,
            access point and channel changes are logged at the sample they happen.
          </p>
          <p>
            Bands (heuristics): signal good ≥ −67 dBm, fair −68 to −75, poor &lt; −75. SNR good ≥ 25
            dB, fair 15–25, poor &lt; 15. Retries good ≤ 10 %, fair 10–25 %, poor &gt; 25 %. Channel
            busy good ≤ 50 %, fair 50–70 %, poor &gt; 70 %.
          </p>
        </Explain>
      </Panel>
      {built && hasData && (
        <div className="wl-panel wl-charts">
          <Chart
            title="Signal and noise (dBm)"
            detail={scaleNote('means') + ' · min/max dashed'}
            label="Signal and noise over the selected range"
            x={built.x}
            ys={built.signal}
            lines={[
              { label: 'Signal mean', color: 'series-2', width: 2 },
              { label: 'Signal min', color: 'series-2', width: 1, dash: [2, 3] },
              { label: 'Signal max', color: 'series-2', width: 1, dash: [2, 3] },
              { label: 'Noise', color: 'series-1', dash: [6, 4] },
            ]}
            thresholds={[
              { value: -67, label: 'good ≥ −67', tone: 'good' },
              { value: -75, label: 'poor < −75', tone: 'danger' },
            ]}
            format={(v) => Math.round(v) + ' dBm'}
            axis={(v) => String(Math.round(v))}
            markers={built.markers}
            y={(low, high) => [
              Math.floor((finite(low) ? low : -100) - 5),
              Math.min(0, Math.ceil((finite(high) ? high : -30) + 5)),
            ]}
          />
          <Chart
            title="Signal to noise (dB)"
            label="Signal-to-noise ratio over the selected range"
            x={built.x}
            ys={built.snr}
            lines={[{ label: 'Signal to noise', color: 'series-5', width: 2 }]}
            thresholds={[
              { value: 25, label: 'good ≥ 25', tone: 'good' },
              { value: 15, label: 'poor < 15', tone: 'danger' },
            ]}
            format={(v) => v.toFixed(1) + ' dB'}
            axis={(v) => String(Math.round(v))}
            markers={built.markers}
            height={140}
            y={(_l, high) => [0, Math.max(40, (finite(high) ? high : 30) + 5)]}
          />
          <Chart
            title="Link rate (Mbit/s)"
            label="Link rate over the selected range"
            x={built.x}
            ys={built.rate}
            lines={[
              { label: 'Transmit', color: 'series-2', width: 2 },
              { label: 'Transmit min', color: 'series-2', width: 1, dash: [2, 3] },
              { label: 'Receive', color: 'series-1' },
              { label: 'Expected', color: 'series-4', dash: [6, 4] },
            ]}
            format={(v) => v.toFixed(1) + ' Mbit/s'}
            axis={(v) => String(Math.round(v))}
            markers={built.markers}
            y={(_l, high) => [0, high > 0 ? high * 1.15 : 100]}
          />
          <Chart
            title="Retries, failures and channel busy (%)"
            detail={scaleNote('buckets') + ' · squares mark drops, dots roams'}
            label="Retries, failures and channel busy over the selected range"
            x={built.x}
            ys={built.health}
            lines={[
              { label: 'Retries', color: 'series-3', width: 2 },
              { label: 'Failures', color: 'danger', width: 1.2 },
              { label: 'Channel busy', color: 'series-2' },
            ]}
            thresholds={[{ value: 70, label: 'busy poor > 70', tone: 'danger' }]}
            format={pctFmt}
            axis={(v) => String(Math.round(v))}
            markers={built.markers}
            y={(_l, high) => [0, Math.min(100, Math.max(10, high * 1.2))]}
          />
          <Chart
            title="Airtime: this router vs others (%)"
            detail={scaleNote('buckets') + ' · this router = its own receive and send time'}
            label="Airtime used by this router versus other stations"
            x={built.x}
            ys={built.air}
            lines={[
              { label: 'Channel busy', color: 'series-6', width: 1 },
              { label: 'This router', color: 'series-2', width: 1.5, fill: 0.35 },
              { label: 'Others', color: 'series-1', width: 1.5 },
            ]}
            bands={[{ upper: 0, lower: 1, color: 'series-1', alpha: 0.18 }]}
            format={pctFmt}
            axis={(v) => String(Math.round(v))}
            markers={built.markers}
            y={(_l, high) => [0, Math.min(100, Math.max(10, high * 1.2))]}
          />
          <Chart
            title="Throughput over the link (Mbit/s)"
            detail={
              scaleNote('means') +
              ' of interface byte counters · bursts read lower than a speed test'
            }
            label="Download and upload throughput over the upstream Wi-Fi link"
            x={built.x}
            ys={built.traffic}
            lines={[
              { label: 'Download', color: 'series-2', width: 1.5 },
              { label: 'Upload', color: 'series-1', width: 1.5 },
            ]}
            format={(v) => v.toFixed(2) + ' Mbit/s'}
            axis={(v) => (v < 10 ? v.toFixed(1) : String(Math.round(v)))}
            markers={built.markers}
            height={150}
          />
        </div>
      )}
      {doc && doc.networks.length > 0 && (
        <Panel
          id="wl-wifi-networks"
          title="Networks in range"
          sub={
            doc.networks.length === 1
              ? 'One access point in this range.'
              : doc.networks.length +
                ' access points in this range, longest first. Each shows its metrics for its own time only.'
          }
        >
          <ul className="wl-pathlist">
            {doc.networks.map((n) => (
              <NetworkCard
                key={n.id}
                network={n}
                current={!!l && !!n.bssid && n.bssid === l.bssid}
              />
            ))}
          </ul>
        </Panel>
      )}
      {doc && doc.events.length > 0 && (
        <Panel id="wl-wifi-events" title="Wi-Fi events in range">
          <ul className="wl-changes">
            {doc.events
              .slice()
              .sort((a, b) => b.at - a.at)
              .map((e) => (
                <li key={e.at + e.text}>
                  <time className="wl-num" title={dateTime(e.at)}>
                    {stamp(e.at, range.value > 1440)}
                  </time>
                  <span>{e.text}</span>
                </li>
              ))}
          </ul>
        </Panel>
      )}
    </>
  );
}
