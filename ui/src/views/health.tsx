import { Button } from '@ui-sdk/react/button';
import { useEffect, useState } from 'react';
import { bytes, dateTime, kept, resolution } from '../format.ts';
import { readDiagnostics } from '../store.ts';
import { Badge, Panel, When, type Tone } from '../ui.tsx';

interface Check {
  status: 'ok' | 'warn' | 'fail' | 'info';
  label: string;
  detail: string;
}

interface TierFiles {
  kind: string;
  step: number;
  files: number;
  bytes: number;
  first: number | null;
  last: number | null;
}

interface Records {
  kind: string;
  files: number;
  rows: number;
  bytes: number;
  first: number | null;
  last: number | null;
}

interface Storage {
  root: string;
  inMemory: boolean;
  days: number;
  bytes: number;
  cap: number;
  tiers: TierFiles[];
  kept: { step: number; keptSeconds: number }[];
  records: Records[];
  dimensions: number | null;
  legacy: string[];
  pending: number;
  lastCollect: number | null;
}

interface Diagnostics {
  healthy: boolean;
  warnings: number;
  checks: Check[];
  storage: Storage | null;
  generated: number;
}

const kindNames: Record<string, string> = {
  latency: 'Path latency',
  wifi: 'Upstream Wi-Fi',
  system: 'Router',
  interface: 'Interfaces',
  event: 'Change events',
  incident: 'Incidents',
};

const tones: Record<Check['status'], Tone> = {
  ok: 'good',
  warn: 'fair',
  fail: 'poor',
  info: 'off',
};

const labels: Record<Check['status'], string> = {
  ok: 'OK',
  warn: 'Warning',
  fail: 'Problem',
  info: 'Info',
};

const covers = (first: number | null, last: number | null): string => {
  if (first === null || last === null) return 'empty';
  const hours = (Math.min(last, Date.now()) - first) / 3_600_000;
  return hours >= 48 ? Math.round(hours / 24) + ' days' : Math.max(0, Math.round(hours)) + ' h';
};

function StoragePanel(props: { storage: Storage }) {
  const st = props.storage;
  const used = st.cap ? Math.min(100, (st.bytes / st.cap) * 100) : 0;
  const keptFor = (step: number) => st.kept.find((k) => k.step === step)?.keptSeconds;
  const kinds = Array.from(new Set(st.tiers.map((t) => t.kind)));
  const total = st.bytes + st.records.reduce((sum, r) => sum + r.bytes, 0) + (st.dimensions || 0);
  return (
    <Panel
      id="wl-health-storage"
      title="History storage"
      sub={
        'Parquet files in ' +
        st.root +
        (st.inMemory ? ' (memory only, lost on reboot)' : ' (flash)') +
        '. History is kept for ' +
        (st.days ? st.days + (st.days === 1 ? ' day.' : ' days.') : 'a few hours.')
      }
    >
      <div className="wl-gauge" role="img" aria-label={used.toFixed(0) + ' % of the storage cap'}>
        <div className="wl-gauge-bar">
          <span style={{ width: used + '%' }} />
        </div>
        <p>
          <strong className="wl-num">{bytes(total)}</strong>
          <span className="wl-muted">
            {' '}
            used of {bytes(st.cap)} cap · oldest metric chunks are removed when it fills
          </span>
        </p>
      </div>
      <dl className="wl-spec">
        <div>
          <dt>Last write</dt>
          <dd>{st.lastCollect ? <When at={st.lastCollect} /> : 'not yet'}</dd>
        </div>
        <div>
          <dt>Write cadence</dt>
          <dd>every 5 min, every minute during trouble</dd>
        </div>
        <div>
          <dt>Waiting for clock sync</dt>
          <dd>{st.pending ? st.pending + ' staged file(s)' : 'nothing'}</dd>
        </div>
        <div>
          <dt>Tunnel names over time</dt>
          <dd>{st.dimensions === null ? 'none' : bytes(st.dimensions)}</dd>
        </div>
      </dl>
      {st.legacy.length > 0 && (
        <p className="wl-notice" role="status">
          Old files still waiting to be removed: {st.legacy.join(', ')}. They go on the next write.
        </p>
      )}
      <div className="wl-store-grid">
        {kinds.map((kind) => (
          <section
            key={kind}
            className="wl-group wl-group-stack"
            aria-label={kindNames[kind] || kind}
          >
            <h4>
              {kindNames[kind] || kind} <span className="wl-muted wl-code">kind={kind}</span>
            </h4>
            <dl>
              {st.tiers
                .filter((t) => t.kind === kind)
                .map((t) => {
                  const keep = keptFor(t.step);
                  return (
                    <div key={t.step}>
                      <dt>{resolution(t.step)}</dt>
                      <dd>
                        <span className="wl-num">{bytes(t.bytes)}</span>
                        <small>
                          {t.files} {t.files === 1 ? 'file' : 'files'} · covers{' '}
                          {covers(t.first, t.last)}
                          {keep ? ' of ' + kept(keep) : ''}
                        </small>
                      </dd>
                    </div>
                  );
                })}
            </dl>
          </section>
        ))}
        {st.records.map((r) => (
          <section
            key={r.kind}
            className="wl-group wl-group-stack"
            aria-label={kindNames[r.kind] || r.kind}
          >
            <h4>
              {kindNames[r.kind] || r.kind} <span className="wl-muted wl-code">kind={r.kind}</span>
            </h4>
            <dl>
              <div>
                <dt>Records</dt>
                <dd>
                  <span className="wl-num">{r.rows}</span>
                  <small>
                    {r.files} daily {r.files === 1 ? 'file' : 'files'} · {bytes(r.bytes)}
                  </small>
                </dd>
              </div>
              <div>
                <dt>Oldest</dt>
                <dd>
                  <span className="wl-num">{r.first ? dateTime(r.first) : '—'}</span>
                </dd>
              </div>
            </dl>
          </section>
        ))}
      </div>
    </Panel>
  );
}

function ChecksPanel(props: { checks: Check[] }) {
  const flagged = props.checks.filter((c) => c.status !== 'info');
  const order = { fail: 0, warn: 1, ok: 2, info: 3 };
  const sorted = flagged.slice().sort((a, b) => order[a.status] - order[b.status]);
  const details = props.checks.filter((c) => c.status === 'info');
  return (
    <Panel
      id="wl-health-checks"
      title="Components"
      sub="The same checks as wayline-ctl doctor on the router."
    >
      <ul className="wl-checks-list">
        {sorted.map((c) => (
          <li key={c.status + c.label + c.detail}>
            <Badge tone={tones[c.status]}>{labels[c.status]}</Badge>
            <span className="wl-check-label">{c.label}</span>
            <span className="wl-muted wl-check-detail">{c.detail}</span>
          </li>
        ))}
      </ul>
      {details.length > 0 && (
        <details className="wl-explain">
          <summary>Monitor and telemetry details ({details.length})</summary>
          <dl className="wl-spec wl-spec-wide">
            {details.map((c) => (
              <div key={c.label + c.detail}>
                <dt>{c.label}</dt>
                <dd>{c.detail}</dd>
              </div>
            ))}
          </dl>
        </details>
      )}
    </Panel>
  );
}

export function HealthView() {
  const [data, setData] = useState<Diagnostics | null>(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const fetchNow = () =>
    readDiagnostics()
      .then((value) => {
        setData(value as Diagnostics);
        setError('');
      })
      .catch((e: unknown) =>
        setError(e && typeof e === 'object' && 'message' in e ? String(e.message) : String(e)),
      )
      .finally(() => setLoading(false));
  const load = () => {
    setLoading(true);
    void fetchNow();
  };
  useEffect(() => {
    void fetchNow();
    const timer = setInterval(() => {
      if (!document.hidden) void fetchNow();
    }, 60_000);
    return () => clearInterval(timer);
  }, []);
  const tone: Tone = !data ? 'off' : !data.healthy ? 'poor' : data.warnings ? 'fair' : 'good';
  return (
    <>
      <section className="wl-panel wl-health-head" aria-labelledby="wl-health-title">
        <div>
          <h2 id="wl-health-title">Wayline health</h2>
          <p className="wl-muted">
            {data ? (
              <>
                Checked <When at={data.generated} />.
              </>
            ) : (
              'Reading the router…'
            )}
          </p>
        </div>
        <div className="wl-actions">
          {data && (
            <Badge tone={tone}>
              {!data.healthy
                ? 'Problems found'
                : data.warnings
                  ? data.warnings + (data.warnings === 1 ? ' warning' : ' warnings')
                  : 'Healthy'}
            </Badge>
          )}
          <Button className="wl-button" disabled={loading} onClick={load}>
            {loading ? 'Checking…' : 'Check again'}
          </Button>
        </div>
      </section>
      {error && (
        <p className="wl-error" role="alert">
          {error}
        </p>
      )}
      {data?.storage && <StoragePanel storage={data.storage} />}
      {data && <ChecksPanel checks={data.checks} />}
    </>
  );
}
