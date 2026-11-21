import { Button } from '@ui-sdk/react/button';
import { useMemo } from 'react';
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
  percent,
  span,
  stamp,
  dateTime,
} from '../format.ts';
import { column, mapColumn, some, times } from '../series.ts';
import { busy, can, enableSetting, range, readOnly, state, wifiData } from '../store.ts';
import type { Column, Rate, WifiKey } from '../types.ts';
import { Badge, Explain, Facts, Panel, scaleNote, type Tone } from '../ui.tsx';

const rateDetail = (rate: Rate): string => {
  if (!rate || !rate.mode) return '';
  const parts = [rate.mode];
  if (finite(rate.mcs)) parts[0] += ' MCS ' + rate.mcs;
  if (finite(rate.nss)) parts.push(rate.nss + (rate.nss === 1 ? ' stream' : ' streams'));
  if (finite(rate.mhz)) parts.push(rate.mhz + ' MHz');
  if (rate.short_gi) parts.push('short GI');
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
            hidden={readOnly.value}
            disabled={!can.value.configure || busy.value}
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
    ' s from iwinfo.' +
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
        {l && (
          <Facts
            label="Current link"
            items={[
              {
                label: 'Network',
                value: l.ssid || 'Hidden network',
                sub: l.bssid + ' · ' + (w.interface || ''),
              },
              {
                label: 'Signal',
                value: dbm(l.signal),
                sub: (
                  <>
                    {level('signal', l.signal)} noise {dbm(l.noise)}
                  </>
                ),
              },
              { label: 'SNR', value: finite(snr) ? snr + ' dB' : dash, sub: level('snr', snr) },
              {
                label: 'Channel',
                value: l.channel ? String(l.channel) : 'Unknown',
                sub: [
                  finite(l.frequency) ? band(l.frequency) + ' (' + l.frequency + ' MHz)' : '',
                  l.width ? l.width + ' MHz wide' : '',
                  l.hwmode ? '802.11' + l.hwmode : '',
                  l.htmode || '',
                  l.encryption || '',
                ]
                  .filter(Boolean)
                  .join(' · '),
              },
              { label: 'Transmit PHY rate', value: kbit(l.tx.kbps), sub: rateDetail(l.tx) },
              { label: 'Receive PHY rate', value: kbit(l.rx.kbps), sub: rateDetail(l.rx) },
              {
                label: 'Expected throughput',
                value: kbit(l.expected_kbps),
                sub: 'driver estimate',
              },
              {
                label: 'Channel busy',
                value: percent(busyPct),
                sub: level('busy', busyPct),
              },
              {
                label: 'Link up',
                value: finite(l.connected_seconds) ? span(l.connected_seconds) : 'Connected',
                sub: finite(l.inactive_ms) ? 'inactive ' + l.inactive_ms + ' ms' : '',
              },
              {
                label: 'Latency over this link',
                value: uplink && finite(uplink.median) ? uplink.median.toFixed(2) + ' ms' : dash,
                sub: !w.uplink
                  ? 'The repeater is not an IPv4 uplink right now.'
                  : !uplink || !finite(uplink.median)
                    ? 'Turn on Direct internet in Settings to measure it.'
                    : (uplink.active ? 'Active uplink ' : 'Standby uplink ') + uplink.interface,
              },
            ]}
          />
        )}
        {doc && finite(sum.samples) && sum.samples > 0 && (
          <>
            <h3 className="wl-h3">Selected range</h3>
            <Facts
              label="Selected range"
              items={[
                { label: 'Connected', value: percent(sum.uptime), sub: sum.samples + ' samples' },
                {
                  label: 'Average signal',
                  value: dbm(sum.signal),
                  sub: level('signal', sum.signal),
                },
                {
                  label: 'Weakest / strongest',
                  value: dbm(sum.signalMin),
                  sub: dbm(sum.signalMax),
                },
                {
                  label: 'Average SNR',
                  value: finite(sum.snr) ? sum.snr.toFixed(0) + ' dB' : dash,
                  sub: level('snr', sum.snr),
                },
                {
                  label: 'Average tx rate',
                  value: mbit(sum.txRate),
                  sub: 'min ' + mbit(sum.txRateMin),
                },
                {
                  label: 'Retries',
                  value: percent(sum.retries),
                  sub: level('retries', sum.retries),
                },
                { label: 'Failures', value: percent(sum.failures) },
                { label: 'Channel busy', value: percent(sum.busy), sub: level('busy', sum.busy) },
                {
                  label: 'Own / others airtime',
                  value: percent(own),
                  sub:
                    finite(sum.busy) && finite(own)
                      ? percent(Math.max(0, sum.busy - own)) + ' others'
                      : '',
                },
                { label: 'Drops', value: String(sum.disconnects ?? 0) },
                { label: 'Roams', value: String(sum.roams ?? 0) },
                {
                  label: 'Data down / up',
                  value: bytes(sum.rxBytes),
                  sub: bytes(sum.txBytes) + ' up',
                },
              ]}
            />
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
            Read from <code>ubus call iwinfo info|assoclist|survey</code> for the client-mode
            interface and from <code>/sys/class/net/&lt;if&gt;/statistics</code>. Signal and noise
            outside −110..−1 and −120..−50 dBm are driver placeholders and are dropped. SNR = signal
            − noise.
          </p>
          <p>
            Retries = retried / (sent + retried) frames. Failures = failed / sent frames. Counters
            reset on reassociation. Channel busy = Δbusy / Δactive time of the serving frequency.
            Own airtime = Δ(rx + tx) / Δactive. Others = busy − own.
          </p>
          <p>
            A drop is connected → disconnected, a silent reassociation, or a change of network. A
            roam is a new access point on the same network.
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
            title="SNR (dB)"
            label="Signal-to-noise ratio over the selected range"
            x={built.x}
            ys={built.snr}
            lines={[{ label: 'SNR', color: 'series-5', width: 2 }]}
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
            title="PHY rate (Mbit/s)"
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
            title="Airtime: own vs others (%)"
            detail={scaleNote('buckets') + ' · own = rx + tx air, others = busy − own'}
            label="Airtime used by this router versus other stations"
            x={built.x}
            ys={built.air}
            lines={[
              { label: 'Channel busy', color: 'series-6', width: 1 },
              { label: 'Own (rx + tx)', color: 'series-2', width: 1.5, fill: 0.35 },
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
            detail={scaleNote('means') + ' · from interface byte counters'}
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
