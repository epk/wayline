import { useMemo } from 'react';
import { Chart } from '../chart.tsx';
import { memUsed, routerLimits } from '../derived.ts';
import { bytes, dash, finite, percent, span } from '../format.ts';
import { column, times } from '../series.ts';
import type { Column } from '../types.ts';
import { state, telemetryData } from '../store.ts';
import { Explain, Facts, Panel } from '../ui.tsx';
import { HistoryStatus, TelemetryOff } from './traffic.tsx';

export function RouterView() {
  const s = state.value;
  const sys = s.telemetry.system;
  const doc = telemetryData.value.doc;
  const built = useMemo(() => {
    if (!doc) return null;
    const c = (v: Column | undefined) => column(doc, v);
    const sys = doc.system;
    const max = finite(sys.conntrackMax) ? sys.conntrackMax : null;
    return {
      x: times(doc),
      cpu: [c(sys.cpu), c(sys.cpuMax)],
      load: [c(sys.load)],
      mem: [c(sys.memUsed)],
      temp: [c(sys.temperature), c(sys.temperatureMax)],
      conntrack: [c(sys.conntrack)],
      max,
    };
  }, [doc]);
  if (!s.config.telemetry) return <TelemetryOff title="Router" />;
  const mem = memUsed(sys);
  const pct = (v: number) => v.toFixed(1) + ' %';
  return (
    <>
      <Panel
        id="wl-router-live"
        title="Router (live)"
        sub={
          'Sampled every ' +
          (s.telemetry.interval || 5) +
          ' s.' +
          (finite(s.telemetry.sampleAgeMs)
            ? ' Sample age ' + (s.telemetry.sampleAgeMs / 1000).toFixed(1) + ' s.'
            : '')
        }
      >
        {sys ? (
          <Facts
            items={[
              { label: 'CPU', value: percent(sys.cpu), sub: 'non-idle jiffies' },
              {
                label: 'Load 1 / 5 / 15 min',
                value: sys.load ? sys.load.map((v) => v.toFixed(2)).join(' ') : dash,
              },
              {
                label: 'Memory used',
                value: percent(mem),
                sub:
                  finite(sys.memTotal) && finite(sys.memAvailable)
                    ? bytes(sys.memTotal - sys.memAvailable) + ' of ' + bytes(sys.memTotal)
                    : '',
              },
              {
                label: 'Temperature',
                value: finite(sys.temperature) ? sys.temperature.toFixed(1) + ' °C' : dash,
                sub: 'hottest thermal zone',
              },
              {
                label: 'Conntrack',
                value: finite(sys.conntrack) ? String(sys.conntrack) : dash,
                sub: finite(sys.conntrackMax) ? 'max ' + sys.conntrackMax : '',
              },
              { label: 'Uptime', value: finite(sys.uptime) ? span(sys.uptime) : dash },
            ]}
          />
        ) : (
          <p className="wl-muted">Waiting for the first telemetry sample.</p>
        )}
        <Explain>
          <p>
            CPU = Δ(non-idle jiffies) / Δ(total jiffies) from <code>/proc/stat</code>. Load from{' '}
            <code>/proc/loadavg</code>. Memory used = 1 − MemAvailable / MemTotal. Temperature is
            the hottest <code>/sys/class/thermal/thermal_zone*/temp</code>. Conntrack is{' '}
            <code>nf_conntrack_count</code> and <code>nf_conntrack_max</code>.
          </p>
          <p>
            Warning limits used for the health state (Wayline heuristics): CPU or memory ≥{' '}
            {routerLimits.cpu} %, conntrack ≥ {routerLimits.conntrack} % of max, temperature ≥{' '}
            {routerLimits.temperature} °C.
          </p>
        </Explain>
      </Panel>
      <HistoryStatus />
      {built && (
        <div className="wl-panel wl-charts">
          <Chart
            title="CPU (%)"
            label="CPU utilisation, bucket mean and maximum"
            x={built.x}
            ys={built.cpu}
            lines={[
              { label: 'Mean', color: 'series-2', width: 1.8 },
              { label: 'Max', color: 'series-2', width: 1, dash: [2, 3] },
            ]}
            format={pct}
            axis={(v) => String(Math.round(v))}
            height={150}
            y={(_l, high) => [0, Math.min(100, Math.max(10, high * 1.2))]}
          />
          <Chart
            title="Load average (1 min)"
            label="One-minute load average"
            x={built.x}
            ys={built.load}
            lines={[{ label: 'Load', color: 'series-4', width: 1.8 }]}
            format={(v) => v.toFixed(2)}
            axis={(v) => v.toFixed(1)}
            height={130}
            y={(_l, high) => [0, Math.max(1, high * 1.2)]}
          />
          <Chart
            title="Memory used (%)"
            label="Memory used"
            x={built.x}
            ys={built.mem}
            lines={[{ label: 'Used', color: 'series-5', width: 1.8, fill: 0.15 }]}
            format={pct}
            axis={(v) => String(Math.round(v))}
            height={130}
            y={(_l, high) => [0, Math.min(100, Math.max(10, high * 1.2))]}
          />
          <Chart
            title="Temperature (°C)"
            label="Temperature, bucket mean and maximum"
            x={built.x}
            ys={built.temp}
            lines={[
              { label: 'Mean', color: 'series-3', width: 1.8 },
              { label: 'Max', color: 'series-3', width: 1, dash: [2, 3] },
            ]}
            thresholds={[
              {
                value: routerLimits.temperature,
                label: routerLimits.temperature + ' °C',
                tone: 'danger',
              },
            ]}
            format={(v) => v.toFixed(1) + ' °C'}
            axis={(v) => String(Math.round(v))}
            height={140}
            y={(low, high) => [
              Math.max(0, Math.floor((finite(low) ? low : 30) - 5)),
              Math.max(routerLimits.temperature + 5, Math.ceil(high + 5)),
            ]}
          />
          <Chart
            title="Conntrack entries"
            label="Connection tracking entries"
            x={built.x}
            ys={built.conntrack}
            lines={[{ label: 'Entries', color: 'series-7', width: 1.8 }]}
            thresholds={
              built.max ? [{ value: built.max, label: 'max ' + built.max, tone: 'danger' }] : []
            }
            format={(v) => String(Math.round(v))}
            axis={(v) =>
              v >= 1000 ? (v / 1000).toFixed(v >= 10000 ? 0 : 1) + 'k' : String(Math.round(v))
            }
            height={140}
            y={(_l, high) => [
              0,
              Math.max(high * 1.2, built.max && high > built.max * 0.5 ? built.max * 1.05 : 10),
            ]}
          />
        </div>
      )}
    </>
  );
}
