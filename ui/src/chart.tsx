import { useEffect, useLayoutEffect, useRef, type KeyboardEvent, type ReactNode } from 'react';
import uPlot from 'uplot';
import './uplot.css';
import { finite, resolution, stamp } from './format.ts';
import {
  goNow,
  range,
  resetZoom,
  shift,
  step,
  themeKey,
  windowEnd,
  windowStart,
  zoom,
} from './store.ts';
import type { Column } from './types.ts';

export interface Line {
  label: string;
  color: string;
  width?: number;
  dash?: number[];
  fill?: number;
  helper?: boolean;
  quiet?: boolean;
  show?: boolean;
}

export interface Marker {
  at: number;
  tone: 'event' | 'danger' | 'brand';
  text: string;
}

export interface Threshold {
  value: number;
  label: string;
  tone: string;
}

export interface Band {
  upper: number;
  lower: number;
  color: string;
  alpha: number;
}

export interface ChartProps {
  title: string;
  label: string;
  x: ArrayLike<number>;
  ys: Column[];
  lines: Line[];
  detail?: string;
  bands?: Band[];
  y?: (min: number, max: number) => [number, number];
  format: (v: number) => string;
  axis?: (v: number) => string;
  markers?: Marker[];
  thresholds?: Threshold[];
  height?: number;
  visible?: boolean[];
  focus?: number;
  legend?: boolean;
  onCursor?: (index: number) => void;
  extra?: (line: number, index: number) => string;
  children?: ReactNode;
}

type Theme = Record<string, string>;

const tokens = [
  'text',
  'muted',
  'border',
  'card',
  'brand',
  'danger',
  'warning',
  'good',
  'event',
].concat(Array.from({ length: 8 }, (_, i) => 'series-' + (i + 1)));

let parser: CanvasRenderingContext2D | null = null;
export const withAlpha = (color: string, alpha: number): string => {
  parser ||= document.createElement('canvas').getContext('2d');
  if (!parser) return color;
  parser.fillStyle = '#000';
  parser.fillStyle = color;
  const value = String(parser.fillStyle);
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  const rgb = hex
    ? [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16))
    : (value.match(/[\d.]+/g) || ['0', '0', '0']).slice(0, 3).map(Number);
  return 'rgba(' + rgb.join(', ') + ', ' + alpha + ')';
};

let cached: { key: string; theme: Theme } | null = null;

const readTheme = (el: Element): Theme => {
  if (cached && cached.key === themeKey.value) return cached.theme;
  const root = el.closest('.wl') || el;
  const probe = document.createElement('span');
  probe.hidden = true;
  root.append(probe);
  const theme: Theme = {};
  tokens.forEach((name) => {
    probe.style.color = 'var(--wl-' + name + ')';
    theme[name] = getComputedStyle(probe).color || '#888';
  });
  probe.remove();
  cached = { key: themeKey.value, theme };
  return theme;
};

let drawable: boolean | null = null;
const canDraw = (): boolean => (drawable ??= !!document.createElement('canvas').getContext?.('2d'));

const color = (theme: Theme, token: string): string => theme[token] || token;

const solo = (values: Column): number[] => {
  const found: number[] = [];
  for (let i = 0; i < values.length; i++)
    if (finite(values[i]) && !finite(values[i - 1]) && !finite(values[i + 1])) found.push(i);
  return found;
};

let hovered: uPlot | null = null;

const applyShow = (u: uPlot | null, visible: boolean[] | undefined): void => {
  if (!u || !visible) return;
  visible.forEach((show, i) => {
    if (u.series[i + 1] && u.series[i + 1].show !== show) u.setSeries(i + 1, { show });
  });
};

export const scaleLabel = (): string => resolution(step.value) + ' buckets';

export function Chart(props: ChartProps) {
  const box = useRef<HTMLDivElement>(null);
  const host = useRef<HTMLDivElement>(null);
  const tip = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const built = useRef('');
  const fed = useRef<unknown[]>([]);
  const latest = useRef(props);
  useLayoutEffect(() => {
    latest.current = props;
  });
  const theme = themeKey.value;
  const height = props.height || 190;
  const key = JSON.stringify([
    props.lines.map((l) => [l.label, l.color, l.width, l.dash, l.fill, l.helper]),
    props.bands,
    height,
    theme,
  ]);
  const xRange = (): [number, number] => [windowStart.value / 1000, windowEnd.value / 1000];
  const data = [props.x, ...props.ys] as uPlot.AlignedData;
  // Views pass thresholds as inline literals, so compare them by value; the
  // rest keep their identity until the history document changes.
  const feed = [
    windowStart.value,
    windowEnd.value,
    props.markers,
    JSON.stringify(props.thresholds),
    ...data,
  ];

  const hideTip = () => {
    if (tip.current) tip.current.hidden = true;
  };

  const updateTip = (u: uPlot) => {
    const p = latest.current;
    const idx = u.cursor.idx == null ? -1 : u.cursor.idx;
    if (p.onCursor) p.onCursor(idx);
    const el = tip.current;
    if (!el || !box.current) return;
    const here = hovered === u || (!hovered && box.current.contains(document.activeElement));
    if (!here || idx < 0 || u.cursor.left == null || u.cursor.left < 0) {
      el.hidden = true;
      return;
    }
    const at = (u.data[0][idx] as number) * 1000;
    const pointer = u.posToVal(u.cursor.top || 0, 'y');
    const rows: { line: Line; value: number; i: number }[] = [];
    p.lines.forEach((line, i) => {
      const value = u.data[i + 1][idx];
      if (line.helper || line.quiet || !u.series[i + 1].show || !finite(value)) return;
      rows.push({ line, value, i });
    });
    let nearest = -1;
    rows.forEach((row, n) => {
      if (nearest < 0 || Math.abs(row.value - pointer) < Math.abs(rows[nearest].value - pointer))
        nearest = n;
    });
    const t = readTheme(box.current);
    const head = document.createElement('div');
    head.className = 'wl-tip-time';
    head.textContent = stamp(at, range.value > 1440) + ' · ' + resolution(step.value) + ' bucket';
    const children: HTMLElement[] = [head];
    if (!rows.length) {
      const none = document.createElement('div');
      none.textContent = 'No data';
      children.push(none);
    }
    rows.forEach((row, n) => {
      const line = document.createElement('div');
      line.className = 'wl-tip-row' + (n === nearest && rows.length > 1 ? ' wl-tip-near' : '');
      const swatch = document.createElement('span');
      swatch.className = 'wl-tip-swatch';
      swatch.style.background = color(t, row.line.color);
      const name = document.createElement('span');
      name.textContent = row.line.label;
      const value = document.createElement('span');
      value.className = 'wl-tip-value';
      value.textContent = p.format(row.value);
      line.append(swatch, name, value);
      children.push(line);
      const more = p.extra && n === nearest ? p.extra(row.i, idx) : '';
      if (more) {
        const detail = document.createElement('div');
        detail.className = 'wl-tip-extra';
        detail.textContent = more.replace(/^ · /, '');
        children.push(detail);
      }
    });
    const bucket = step.value * 1000;
    (p.markers || [])
      .filter((m) => m.at >= at - bucket / 2 && m.at < at + bucket * 1.5)
      .slice(0, 3)
      .forEach((m) => {
        const event = document.createElement('div');
        event.className = 'wl-tip-event';
        event.textContent = m.text;
        children.push(event);
      });
    el.replaceChildren(...children);
    el.hidden = false;
    const outer = box.current.getBoundingClientRect();
    const over = u.over.getBoundingClientRect();
    const left = over.left - outer.left;
    const top = over.top - outer.top;
    const x = left + u.cursor.left;
    const y = top + (u.cursor.top || 0);
    const gap = 12;
    const fitsRight = x + gap + el.offsetWidth <= outer.width;
    const fitsBelow = y + gap + el.offsetHeight <= outer.height;
    el.style.left = Math.max(0, fitsRight ? x + gap : x - gap - el.offsetWidth) + 'px';
    el.style.top = Math.max(0, fitsBelow ? y + gap : y - gap - el.offsetHeight) + 'px';
  };

  const draw = (u: uPlot) => {
    const p = latest.current;
    const t = readTheme(u.root);
    const { ctx, bbox } = u;
    const ratio = window.devicePixelRatio || 1;
    const right = bbox.left + bbox.width;
    ctx.save();
    ctx.lineWidth = ratio;
    (p.thresholds || []).forEach((h) => {
      const y = u.valToPos(h.value, 'y', true);
      if (y < bbox.top || y > bbox.top + bbox.height) return;
      ctx.strokeStyle = color(t, h.tone);
      ctx.fillStyle = color(t, h.tone);
      ctx.setLineDash([6 * ratio, 4 * ratio]);
      ctx.beginPath();
      ctx.moveTo(bbox.left, y);
      ctx.lineTo(right, y);
      ctx.stroke();
      ctx.font = 11 * ratio + 'px sans-serif';
      ctx.textAlign = 'right';
      // A halo in the card color keeps the label readable where a series crosses it.
      ctx.setLineDash([]);
      ctx.lineJoin = 'round';
      ctx.lineWidth = 3 * ratio;
      ctx.strokeStyle = t.card || '#fff';
      ctx.strokeText(h.label, right - 4 * ratio, y - 4 * ratio);
      ctx.lineWidth = ratio;
      ctx.fillText(h.label, right - 4 * ratio, y - 4 * ratio);
    });
    ctx.setLineDash([3 * ratio, 3 * ratio]);
    (p.markers || []).forEach((m) => {
      const x = u.valToPos(m.at / 1000, 'x', true);
      if (x < bbox.left || x > right) return;
      const tone = m.tone === 'event' ? t.event : color(t, m.tone);
      ctx.strokeStyle = withAlpha(tone, 0.65);
      ctx.fillStyle = tone;
      ctx.beginPath();
      ctx.moveTo(x, bbox.top);
      ctx.lineTo(x, bbox.top + bbox.height);
      ctx.stroke();
      ctx.beginPath();
      if (m.tone === 'danger') ctx.rect(x - 3 * ratio, bbox.top, 6 * ratio, 6 * ratio);
      else ctx.arc(x, bbox.top + 4 * ratio, 3 * ratio, 0, Math.PI * 2);
      ctx.fill();
    });
    ctx.restore();
  };

  useEffect(() => {
    if (!host.current || !box.current) return;
    if (built.current === key && plot.current) {
      const prev = fed.current;
      if (prev.length !== feed.length || feed.some((d, i) => d !== prev[i])) {
        fed.current = feed;
        plot.current.setData(data);
      }
      return;
    }
    if (!canDraw()) return;
    plot.current?.destroy();
    const t = readTheme(box.current);
    const p = latest.current;
    const font = '12px system-ui, -apple-system, "Segoe UI", sans-serif';
    const axis = (extra: Partial<uPlot.Axis>): uPlot.Axis => ({
      stroke: t.muted,
      font,
      labelFont: font,
      grid: { stroke: t.border, width: 1 },
      ticks: { stroke: t.border, width: 1 },
      ...extra,
    });
    const series: uPlot.Series[] = [
      {},
      ...p.lines.map((line) => {
        const stroke = color(t, line.color);
        return {
          label: line.label,
          stroke: line.helper ? 'transparent' : stroke,
          width: line.width || 1.5,
          spanGaps: false,
          show: line.show !== false,
          ...(line.dash ? { dash: line.dash } : {}),
          ...(line.fill ? { fill: withAlpha(stroke, line.fill) } : {}),
          points: line.helper
            ? { show: false }
            : {
                show: true,
                size: 4,
                space: 0,
                stroke,
                fill: stroke,
                filter: (u: uPlot, i: number) => solo(u.data[i] as Column),
              },
        } as uPlot.Series;
      }),
    ];
    const bands = (p.bands || []).map((b) => {
      const fill = withAlpha(color(t, b.color), b.alpha);
      return { series: [b.upper + 1, b.lower + 1] as [number, number], fill: () => fill };
    });
    const width = Math.max(240, box.current.clientWidth);
    plot.current = new uPlot(
      {
        width,
        height,
        legend: { show: false },
        focus: { alpha: 0.25 },
        cursor: {
          sync: { key: 'wayline' },
          drag: { x: true, y: false, setScale: false },
          focus: { prox: -1 },
        },
        scales: {
          x: { time: true, range: () => xRange() },
          y: {
            range: (_u: uPlot, low: number, high: number) =>
              latest.current.y ? latest.current.y(low, high) : [0, high > 0 ? high * 1.15 : 1],
          },
        },
        axes: [
          axis({
            grid: { show: false },
            values: (_u: uPlot, values: number[]) =>
              values.map((v) => stamp(v * 1000, range.value > 1440)),
          }),
          axis({
            size: 52,
            values: (_u: uPlot, values: number[]) =>
              values.map((v) => (latest.current.axis || latest.current.format)(v)),
          }),
        ],
        series,
        bands,
        hooks: {
          setCursor: [updateTip],
          setSelect: [
            (u: uPlot) => {
              if (!u.select.width) return;
              const from = u.posToVal(u.select.left, 'x') * 1000;
              const to = u.posToVal(u.select.left + u.select.width, 'x') * 1000;
              u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
              zoom(from, to);
            },
          ],
          draw: [draw],
        },
      },
      data,
      host.current,
    );
    plot.current.over.addEventListener('dblclick', resetZoom);
    const over = plot.current;
    over.over.addEventListener('mouseenter', () => {
      hovered = over;
    });
    over.over.addEventListener('mouseleave', () => {
      if (hovered === over) hovered = null;
      hideTip();
    });
    built.current = key;
    fed.current = feed;
    applyShow(plot.current, p.visible);
  });

  useEffect(() => applyShow(plot.current, props.visible), [props.visible]);

  useEffect(() => {
    const u = plot.current;
    if (!u) return;
    const f = props.focus === undefined ? -1 : props.focus;
    u.setSeries(f < 0 ? null : f + 1, { focus: true });
  }, [props.focus]);

  useEffect(() => {
    const el = box.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry && plot.current)
        plot.current.setSize({ width: Math.max(240, entry.contentRect.width), height });
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [height]);

  useEffect(
    () => () => {
      plot.current?.destroy();
      plot.current = null;
      built.current = '';
    },
    [],
  );

  const keys = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      goNow();
      return;
    }
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    shift(e.key === 'ArrowLeft' ? 1 : -1);
  };

  const shown = props.lines.filter(
    (l, i) => !l.helper && !l.quiet && (!props.visible || props.visible[i] !== false),
  );
  return (
    <figure className="wl-chart">
      <figcaption className="wl-chart-head">
        <h3>{props.title}</h3>
        <p className="wl-muted">{props.detail || scaleLabel()}</p>
      </figcaption>
      <div
        ref={box}
        className="wl-plot"
        tabIndex={0}
        role="img"
        aria-label={props.label + '. Arrow keys move through time, Escape returns to now.'}
        onKeyDown={keys}
        onBlur={hideTip}
      >
        <div ref={host} />
        <div ref={tip} className="wl-tooltip" hidden />
      </div>
      {props.legend !== false && shown.length > 0 && (
        <ul className="wl-legend" aria-label={props.title + ' series'}>
          {shown.map((l) => (
            <li key={l.label}>
              <svg width="18" height="6" viewBox="0 0 18 6" aria-hidden="true">
                <line
                  x1="0"
                  y1="3"
                  x2="18"
                  y2="3"
                  style={{ stroke: 'var(--wl-' + l.color + ', ' + l.color + ')' }}
                  strokeWidth={Math.max(2, l.width || 1.5)}
                  strokeDasharray={l.dash ? l.dash.join(' ') : undefined}
                />
              </svg>
              {l.label}
            </li>
          ))}
        </ul>
      )}
      {props.children}
    </figure>
  );
}
