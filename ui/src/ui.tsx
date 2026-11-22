import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from 'react';
import { ago, dateTime, resolution, stamp } from './format.ts';
import { bandLabel, formula, type Score } from './mos.ts';
import { lossLabels, runs, uptime as uptimeOf } from './series.ts';
import { range, step, windowStart } from './store.ts';
import type { Column } from './types.ts';

export type Tone =
  | 'good'
  | 'fair'
  | 'poor'
  | 'excellent'
  | 'unknown'
  | 'down'
  | 'off'
  | 'ok'
  | 'degraded';

const glyph: Record<string, string> = {
  good: '●',
  ok: '●',
  excellent: '●',
  fair: '▲',
  degraded: '▲',
  poor: '■',
  down: '■',
  off: '○',
  unknown: '◇',
};

export function Badge(props: { tone: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={'wl-badge wl-tone-' + props.tone} title={props.title}>
      <span aria-hidden="true" className="wl-glyph">
        {glyph[props.tone]}
      </span>
      {props.children}
    </span>
  );
}

export function Tag(props: { children: ReactNode; title?: string }) {
  return (
    <span className="wl-tag" title={props.title}>
      {props.children}
    </span>
  );
}

export function When(props: { at: number }) {
  return (
    <time dateTime={new Date(props.at).toISOString()} title={dateTime(props.at)}>
      {ago(props.at)}
    </time>
  );
}

export const CALL_QUALITY =
  'Estimated voice-call quality from latency, jitter and loss, on a 1 (bad) to 5 (excellent) scale. Known as MOS (mean opinion score).';

export function Mos(props: { score: Score | null; missing?: string; compact?: boolean }) {
  if (!props.score)
    return (
      <span className="wl-muted" title={props.missing || 'Needs latency, jitter and loss.'}>
        —
      </span>
    );
  const s = props.score;
  if (props.compact)
    return (
      <Badge
        tone={s.band}
        title={'Call quality ' + s.mos.toFixed(2) + ' out of 5. ' + CALL_QUALITY}
      >
        <span className="wl-num">{s.mos.toFixed(2)}</span> {bandLabel[s.band]}
      </Badge>
    );
  return (
    <span
      className="wl-mos"
      title={'Call quality ' + s.mos.toFixed(2) + ' out of 5. ' + CALL_QUALITY}
    >
      <span className="wl-num">{s.mos.toFixed(2)}</span>{' '}
      <Badge tone={s.band}>{bandLabel[s.band]}</Badge>
    </span>
  );
}

export function Explain(props: { summary?: string; children: ReactNode }) {
  return (
    <details className="wl-explain">
      <summary>{props.summary || 'How this is computed'}</summary>
      <div className="wl-explain-body">{props.children}</div>
    </details>
  );
}

export function MosExplain(props: { inputs?: string }) {
  return (
    <Explain>
      <p>
        Call quality is the mean opinion score (MOS): an estimate of how a voice call would sound,
        from 1 (bad) to 5 (excellent). It uses a simplified ITU-T G.107 E-model and models voice
        calls, not general browsing. {props.inputs}
      </p>
      <pre>{formula}</pre>
      <p>Bands: 4.3 and above excellent, 4.0–4.3 good, 3.6–4.0 fair, below 3.6 poor.</p>
      <p>
        Jitter is the mean absolute difference between consecutive successful RTTs on the same path
        (RFC 3550 interarrival form without its 1/16 smoothing). Loss is probes without a reply
        within the timeout divided by probes sent.
      </p>
    </Explain>
  );
}

// A tooltip that works on hover, keyboard focus and tap, unlike title, which
// touch screens never show and some browsers delay or hide. A click or tap
// toggles it; Escape, blur or a tap elsewhere closes it.
export function Tip(props: { text: string; children: ReactNode }) {
  const [open, setOpen] = useState<boolean | null>(null);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return undefined;
    const away = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(null);
    };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  }, [open]);
  return (
    <button
      ref={ref}
      type="button"
      className="wl-tip"
      data-tip={props.text}
      data-open={open === null ? undefined : String(open)}
      onClick={() => setOpen(!open)}
      onBlur={() => setOpen(null)}
      onMouseLeave={() => open === false && setOpen(null)}
      onKeyDown={(e) => {
        if (e.key !== 'Escape' || open === false) return;
        // Keep the Escape from also closing a surrounding dialog.
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
      }}
    >
      {props.children}
      <span className="wl-sr">. {props.text}</span>
    </button>
  );
}

export function Facts(props: {
  items: { label: string; value: ReactNode; sub?: ReactNode; title?: string }[];
  label?: string;
}) {
  return (
    <dl className="wl-facts" aria-label={props.label}>
      {props.items.map((item) => (
        <div key={item.label}>
          <dt>{item.title ? <Tip text={item.title}>{item.label}</Tip> : item.label}</dt>
          <dd>
            <span className="wl-num wl-fact">{item.value}</span>
            {item.sub !== undefined && item.sub !== '' && <small>{item.sub}</small>}
          </dd>
        </div>
      ))}
    </dl>
  );
}

export function Panel(props: {
  title: ReactNode;
  sub?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  foot?: ReactNode;
  id?: string;
  class?: string;
}) {
  return (
    <section className={'wl-panel ' + (props.class || '')} aria-labelledby={props.id}>
      <header className="wl-panel-head">
        <div>
          <h2 id={props.id}>{props.title}</h2>
          {props.sub && <p className="wl-muted">{props.sub}</p>}
        </div>
        {props.actions}
      </header>
      {props.children}
      {props.foot && <footer className="wl-panel-foot">{props.foot}</footer>}
    </section>
  );
}

export const scaleNote = (what = 'buckets'): string => resolution(step.value) + ' ' + what;

export function Strip(props: {
  name: string;
  sent: Column;
  lost: Column;
  start: number;
  step: number;
}) {
  const [hover, setHover] = useState(-1);
  const count = props.sent.length;
  const u = uptimeOf(props.sent, props.lost);
  const cells = runs(props.sent, props.lost);
  const pick = (e: MouseEvent<SVGSVGElement>) => {
    const el = e.currentTarget;
    const box = el.getBoundingClientRect();
    setHover(
      Math.min(count - 1, Math.max(0, Math.floor(((e.clientX - box.left) / box.width) * count))),
    );
  };
  const keys = (e: KeyboardEvent<SVGSVGElement>) => {
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      e.stopPropagation();
      const from = hover < 0 ? count : hover;
      setHover(Math.min(count - 1, Math.max(0, from + (e.key === 'ArrowLeft' ? -1 : 1))));
    } else if (e.key === 'Escape') setHover(-1);
  };
  const text = u.uptime === null ? 'no probes' : u.uptime.toFixed(2) + ' %';
  const label =
    props.name +
    ': uptime ' +
    text +
    ', ' +
    u.full +
    ' of ' +
    u.probed +
    ' ' +
    resolution(props.step) +
    ' buckets fully lost.';
  let detail = '';
  if (hover >= 0 && hover < count) {
    const at = props.start + hover * props.step * 1000;
    const sent = props.sent[hover] || 0;
    const lost = props.lost[hover] || 0;
    detail =
      stamp(at, range.value > 1440) +
      ' – ' +
      stamp(at + props.step * 1000, range.value > 1440) +
      ': ' +
      (sent
        ? ((lost / sent) * 100).toFixed(1) + ' % loss, ' + lost + ' of ' + sent + ' probes lost'
        : 'no probes');
  }
  return (
    <div className="wl-strip-row">
      <span className="wl-strip-name" title={props.name}>
        {props.name}
      </span>
      <svg
        className="wl-strip"
        viewBox={'0 0 ' + Math.max(1, count) + ' 1'}
        preserveAspectRatio="none"
        role="img"
        tabIndex={0}
        aria-label={label + ' Arrow keys inspect buckets.'}
        onMouseMove={pick}
        onMouseLeave={() => setHover(-1)}
        onKeyDown={keys}
        onBlur={() => setHover(-1)}
      >
        {cells.map((c) => (
          <rect
            key={c.start}
            x={c.start}
            width={c.length}
            y="0"
            height="1"
            className={'wl-u' + c.level}
          />
        ))}
        {hover >= 0 && <rect x={hover} width="1" y="0" height="1" className="wl-u-hover" />}
      </svg>
      <span className="wl-strip-value wl-num" title={label}>
        {text}
      </span>
      {detail && (
        <span className="wl-strip-tip" role="status">
          {detail}
        </span>
      )}
    </div>
  );
}

export function StripLegend() {
  return (
    <ul className="wl-strip-legend" aria-label="Uptime strip colours">
      {lossLabels.map((label, i) => (
        <li key={label}>
          <svg width="10" height="10" aria-hidden="true">
            <rect width="10" height="10" className={'wl-u' + i} />
          </svg>
          {label}
        </li>
      ))}
    </ul>
  );
}

export function UptimeExplain() {
  return (
    <Explain>
      <p>
        One cell per {resolution(step.value)} bucket in the selected range, coloured by the share of
        probes lost in that bucket. Uptime = 1 − fully lost buckets / buckets with probes. At
        15-second resolution a fully lost bucket is an incident bucket; coarser buckets only count
        as lost when every probe in them was lost.
      </p>
    </Explain>
  );
}

export const windowText = (): string => {
  const start = windowStart.value;
  const long = range.value > 1440;
  return stamp(start, long) + ' – ' + stamp(start + range.value * 60000, long);
};
