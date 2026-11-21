import { Button } from '@ui-sdk/react/button';
import { useMemo } from 'react';
import { Chart, type Line } from '../chart.tsx';
import { bits, bytes, dash, finite, span } from '../format.ts';
import { column, some, times } from '../series.ts';
import { busy, can, enableSetting, readOnly, state, telemetryData } from '../store.ts';
import type { Column } from '../types.ts';
import { Badge, Explain, Panel } from '../ui.tsx';

const palette = Array.from({ length: 8 }, (_, i) => 'series-' + (i + 1));
export const handshakeLimit = 180;

export function TelemetryOff(props: { title: string }) {
  return (
    <Panel id="wl-telemetry-off" title={props.title} sub="Router telemetry is off.">
      <div className="wl-cta">
        <p>
          Router and interface telemetry — CPU, memory, temperature, conntrack, per-interface
          throughput, WireGuard handshakes. Sampled every 5 seconds and kept with the same tiers as
          latency.
        </p>
        <Button
          className="wl-button wl-primary"
          hidden={readOnly.value}
          disabled={!can.value.configure || busy.value}
          onClick={() =>
            void enableSetting(
              'telemetry',
              'Router telemetry is on. The first sample arrives within seconds.',
            )
          }
        >
          Record router telemetry
        </Button>
      </div>
    </Panel>
  );
}

export function HistoryStatus() {
  const data = telemetryData.value;
  if (data.error)
    return (
      <p className="wl-muted" role="status">
        Telemetry history is unavailable. {data.error}
      </p>
    );
  if (!data.loaded) return <p className="wl-muted">Loading telemetry history…</p>;
  if (!data.doc || !data.doc.count)
    return <p className="wl-muted">No telemetry history in this range yet.</p>;
  return null;
}

const rateAxis = (v: number) => {
  if (v >= 1e9) return (v / 1e9).toFixed(1) + 'G';
  if (v >= 1e6) return (v / 1e6).toFixed(v >= 1e7 ? 0 : 1) + 'M';
  if (v >= 1e3) return Math.round(v / 1e3) + 'k';
  return String(Math.round(v));
};

export function TrafficView() {
  const s = state.value;
  const t = s.telemetry;
  const doc = telemetryData.value.doc;
  const built = useMemo(() => {
    if (!doc) return null;
    const x = times(doc);
    const col = (c: Column | undefined) => column(doc, c);
    const ifaces = doc.interfaces.map((i, n) => ({
      id: i.id,
      name: i.name + (i.device && i.device !== i.name ? ' (' + i.device + ')' : ''),
      kind: i.kind,
      color: palette[n % palette.length],
      rx: col(i.rxBps),
      tx: col(i.txBps),
      rxPeak: col(i.rxPeakBps),
      txPeak: col(i.txPeakBps),
      errors: col(i.errors),
      drops: col(i.drops),
      handshake: col(i.handshakeAgeMax),
    }));
    const counters: Line[] = [];
    const counterYs: Column[] = [];
    ifaces.forEach((i) => {
      counters.push({ label: i.name + ' errors', color: i.color, width: 1.5 });
      counterYs.push(i.errors);
      counters.push({ label: i.name + ' drops', color: i.color, width: 1.2, dash: [4, 3] });
      counterYs.push(i.drops);
    });
    const wg = ifaces.filter((i) => i.kind === 'tunnel' && some(i.handshake, finite));
    return { x, ifaces, counters, counterYs, wg };
  }, [doc]);
  if (!s.config.telemetry) return <TelemetryOff title="Traffic" />;
  const age = t.sampleAgeMs;
  return (
    <>
      <Panel
        id="wl-traffic-live"
        title="Interfaces (live)"
        sub={
          'Rates over the last ' +
          (t.interval || 5) +
          '-second sample from /sys/class/net counters.' +
          (finite(age) ? ' Sample age ' + (age / 1000).toFixed(1) + ' s.' : '')
        }
      >
        {t.interfaces.length ? (
          <div className="wl-scroll" tabIndex={0} role="region" aria-label="Live interface table">
            <table className="wl-table">
              <thead>
                <tr>
                  <th scope="col">Interface</th>
                  <th scope="col">Kind</th>
                  <th scope="col" className="wl-n">
                    Rx rate
                  </th>
                  <th scope="col" className="wl-n">
                    Tx rate
                  </th>
                  <th scope="col" className="wl-n">
                    Rx total
                  </th>
                  <th scope="col" className="wl-n">
                    Tx total
                  </th>
                  <th scope="col" className="wl-n">
                    Errors
                  </th>
                  <th scope="col" className="wl-n">
                    Drops
                  </th>
                  <th scope="col" className="wl-n">
                    Handshake age
                  </th>
                  <th scope="col">Endpoint</th>
                </tr>
              </thead>
              <tbody>
                {t.interfaces.map((i) => {
                  const stale = finite(i.handshakeAge) && i.handshakeAge > handshakeLimit;
                  return (
                    <tr key={i.id}>
                      <th scope="row">
                        {i.name}
                        {i.device && i.device !== i.name && (
                          <small className="wl-block wl-muted">{i.device}</small>
                        )}
                      </th>
                      <td>{i.kind}</td>
                      <td className="wl-n wl-num">{bits(i.rxBps)}</td>
                      <td className="wl-n wl-num">{bits(i.txBps)}</td>
                      <td className="wl-n wl-num">{bytes(i.rxBytes)}</td>
                      <td className="wl-n wl-num">{bytes(i.txBytes)}</td>
                      <td className="wl-n wl-num">{finite(i.errors) ? i.errors : dash}</td>
                      <td className="wl-n wl-num">{finite(i.drops) ? i.drops : dash}</td>
                      <td className="wl-n">
                        {finite(i.handshakeAge) ? (
                          <>
                            <span className="wl-num">{span(i.handshakeAge)}</span>
                            {stale && (
                              <Badge
                                tone="degraded"
                                title={
                                  'Older than ' +
                                  handshakeLimit +
                                  ' s: the tunnel is not rekeying if it carries traffic'
                                }
                              >
                                Stale
                              </Badge>
                            )}
                          </>
                        ) : (
                          dash
                        )}
                      </td>
                      <td className="wl-num">{i.endpoint || dash}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="wl-muted">No interface sample yet.</p>
        )}
        <Explain>
          <p>
            Rates = Δbytes × 8 / Δt from <code>/sys/class/net/&lt;if&gt;/statistics</code>. Errors
            and drops add the receive and transmit counters. WireGuard handshake age = now − newest{' '}
            <code>latest-handshake</code> across the interface's peers (
            <code>wg show all dump</code>). Above {handshakeLimit} s with traffic means the tunnel
            is not rekeying.
          </p>
        </Explain>
      </Panel>
      <HistoryStatus />
      {built && built.ifaces.length > 0 && (
        <div className="wl-panel wl-charts">
          {built.ifaces.map((i) => (
            <Chart
              key={i.id}
              title={i.name + ' throughput (bit/s)'}
              label={'Receive and transmit throughput of ' + i.name}
              x={built.x}
              ys={[i.rx, i.rxPeak, i.tx, i.txPeak]}
              lines={[
                { label: 'Rx average', color: 'series-2', width: 1.8 },
                { label: 'Rx peak', color: 'series-2', width: 1, dash: [2, 3] },
                { label: 'Tx average', color: 'series-1', width: 1.8 },
                { label: 'Tx peak', color: 'series-1', width: 1, dash: [2, 3] },
              ]}
              format={bits}
              axis={rateAxis}
              height={150}
            />
          ))}
          <Chart
            title="Errors and drops per bucket"
            label="Interface errors and drops"
            x={built.x}
            ys={built.counterYs}
            lines={built.counters}
            format={(v) => String(Math.round(v))}
            axis={(v) => String(Math.round(v))}
            height={150}
            y={(_l, high) => [0, high > 0 ? high * 1.2 : 5]}
          />
          {built.wg.length > 0 && (
            <Chart
              title="WireGuard handshake age (s, bucket maximum)"
              label="Maximum WireGuard handshake age per tunnel"
              x={built.x}
              ys={built.wg.map((w) => w.handshake)}
              lines={built.wg.map((w) => ({ label: w.name, color: w.color, width: 1.5 }))}
              thresholds={[
                {
                  value: handshakeLimit,
                  label: handshakeLimit + ' s: not rekeying',
                  tone: 'danger',
                },
              ]}
              format={(v) => span(v)}
              axis={(v) => String(Math.round(v))}
              height={150}
              y={(_l, high) => [0, Math.max(handshakeLimit * 1.25, high * 1.15)]}
            />
          )}
        </div>
      )}
    </>
  );
}
