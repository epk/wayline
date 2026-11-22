import { Button } from '@ui-sdk/react/button';
import { Modal } from '../modal.tsx';
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from '@ui-sdk/react/segmented-control';
import { Switch, SwitchThumb } from '@ui-sdk/react/switch';
import { Toggle } from '@ui-sdk/react/toggle';
import { useMemo, useState } from 'react';
import { Chart, type Band, type Line, type Marker } from '../chart.tsx';
import { changeFor, IncidentList, rangeIncidents, subjectName, tunnelName } from '../common.tsx';
import {
  egressFor,
  endpointPrefix,
  latencyLines,
  latencyX,
  tunnelLoss,
  uplinkLoss,
  uplinkPrefix,
} from '../derived.ts';
import { dash, finite, fixed, lossShort, ms, percent, place } from '../format.ts';
import { score } from '../mos.ts';
import { some, summarize, type LatencyLine, type Stats } from '../series.ts';
import {
  askToggle,
  busy,
  busyAction,
  busyTunnel,
  can,
  chooseTarget,
  controlReason,
  cursor,
  destinations,
  exitTunnel,
  highlight,
  isolate,
  isolated,
  latencyData,
  pendingStop,
  primarySelected,
  readError,
  ready,
  selectable,
  selectedTarget,
  selectedTunnel,
  selectExit,
  selectReason,
  setTunnelEnabled,
  state,
  wanAvailable,
} from '../store.ts';
import type { Column, Tunnel, Uplink } from '../types.ts';
import {
  Badge,
  Mos,
  MosExplain,
  Panel,
  scaleNote,
  Strip,
  StripLegend,
  Tag,
  UptimeExplain,
  When,
} from '../ui.tsx';

const palette = Array.from({ length: 8 }, (_, i) => 'series-' + (i + 1));
const isServer = (id: string) => id.startsWith(endpointPrefix);
const isUplink = (id: string) => id.startsWith(uplinkPrefix);
const ownerOf = (id: string) => (isServer(id) ? id.slice(endpointPrefix.length) : id);

export const seriesColor = (id: string): string => {
  const owner = ownerOf(id);
  if (owner === 'wan' || isUplink(id)) return 'muted';
  const tunnels = state.value.tunnels;
  const index = tunnels.findIndex((t) => t.id === owner);
  if (index >= 0 && tunnels[index].preferred) return 'brand';
  return palette[(index < 0 ? 0 : index) % palette.length];
};

export const seriesName = (line: { id: string; name: string }): string => {
  if (line.id === 'wan') return 'Direct WAN (no VPN)';
  if (isUplink(line.id)) return line.id.slice(uplinkPrefix.length) + ' uplink';
  return line.name;
};

interface Row {
  key: string;
  owner: string;
  name: string;
  color: string;
  dashed: boolean;
  tunnel: boolean;
  line: LatencyLine;
  stats: Stats;
  index: number;
}

function StopDialog() {
  const t = state.value.tunnels.find((row) => row.id === pendingStop.value) || null;
  const name = t ? t.name : 'the tunnel';
  return (
    <Modal
      open={!!t}
      onClose={() => (pendingStop.value = '')}
      title={'Stop ' + name + '?'}
      className="wl-alert"
      alert
    >
      <h2>Stop {name}?</h2>
      <p>Traffic may use WAN. GL.iNet traffic policy applies while the tunnel is stopped.</p>
      <div className="wl-actions">
        <Button className="wl-button" onClick={() => (pendingStop.value = '')}>
          Keep running
        </Button>
        <Button
          className="wl-button wl-danger"
          data-agent-confirm={'Stops the VPN tunnel ' + name + '; traffic may leave over WAN'}
          onClick={() => {
            pendingStop.value = '';
            if (t) void setTunnelEnabled(t);
          }}
        >
          Stop {name}
        </Button>
      </div>
    </Modal>
  );
}

const tone = (t: Tunnel) => (!t.enabled ? 'off' : t.connected ? 'ok' : 'down');

function TunnelTable() {
  const s = state.value;
  const endpoints = s.config.endpointDiagnostics ? s.endpoints : [];
  const showServer = !!s.config.endpointDiagnostics;
  const diag = (id: string) => {
    const d = s.diagnostics[selectedTarget.value] || {};
    return finite(d[id]) ? d[id] : null;
  };
  const active = s.tunnels.find((t) => t.preferred);
  const activeMedian = active && finite(active.median) ? active.median : null;
  const comparable = s.tunnels.filter((t) => t.connected && finite(t.median)).length >= 2;
  const note = (t: Tunnel) => {
    if (t.samples && t.lost > 0)
      return {
        text: lossShort(t.lost, t.samples) + ' loss',
        cls: 'wl-note-loss',
        title: t.lost + ' of ' + t.samples + ' samples lost',
      };
    if (comparable && !t.preferred && activeMedian !== null && finite(t.median)) {
      const diff = activeMedian - t.median;
      if (Math.abs(diff) <= 3)
        return { text: 'About the same as the active exit', cls: '', title: '' };
      if (diff > 0)
        return {
          text: diff.toFixed(2) + ' ms faster than the active exit',
          cls: 'wl-note-faster',
          title: '',
        };
      return { text: (-diff).toFixed(2) + ' ms slower than the active exit', cls: '', title: '' };
    }
    return null;
  };
  const rowNote = (t: Tunnel) => {
    if (t.preferred || !t.enabled || !can.value.select) return '';
    if (!selectable(t)) return 'Cannot be the exit: ' + (t.selectReason || t.reason);
    if (s.auto.enabled && s.config.autoTunnels.includes(t.monitorId) && !t.eligible)
      return 'Automatic switching skips it: ' + t.reason;
    return '';
  };
  const uplinks = s.config.directWan
    ? s.uplinks.slice().sort((a, b) => Number(b.active) - Number(a.active))
    : [];
  const uplinkName = (u: Uplink) =>
    !u.active
      ? u.interface || u.id
      : u.interface
        ? 'Direct WAN (' + u.interface + ')'
        : 'Direct WAN';
  const unmeasurable = (u: Uplink) =>
    u.state === 'route_mismatch' || u.state === 'route_unavailable';
  const primary = primarySelected.value;
  const liveTitle = 'Live jitter, loss and MOS are measured to the primary destination.';
  return (
    <div
      className="wl-scroll wl-table-wrap"
      tabIndex={0}
      role="region"
      aria-label="Exit tunnels table"
      aria-busy={!ready.value && !readError.value ? 'true' : 'false'}
    >
      <table className="wl-table wl-exits">
        <thead>
          <tr>
            <th scope="col">Path</th>
            <th scope="col" className="wl-n">
              Latency
            </th>
            <th scope="col" className="wl-n">
              Jitter
            </th>
            <th scope="col" className="wl-n">
              Loss
            </th>
            <th scope="col">MOS</th>
            {showServer && <th scope="col">To server</th>}
            <th scope="col">
              <span className="wl-sr">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {s.tunnels.map((t) => {
            const n = primary ? note(t) : null;
            const id = egressFor(t.id);
            const exitIp = id && id.ip && !id.error ? id : null;
            const isExit = exitTunnel.value?.id === t.id;
            const change = changeFor(t.id);
            const loss = tunnelLoss(t);
            const jitter = finite(t.jitter) ? t.jitter : null;
            const server = endpoints.find((e) => e.tunnel === t.id) || null;
            const serverDiag =
              server && server.address ? (s.diagnostics[server.address] || {}).wan : null;
            const extra = rowNote(t);
            return (
              <tr
                key={t.id}
                className={
                  (selectedTunnel.value === t.id ? 'wl-selected ' : '') +
                  (t.preferred ? 'wl-active' : '')
                }
              >
                <th scope="row" className="wl-path">
                  <div className="wl-name-line">
                    <Badge tone={tone(t)}>
                      <span className="wl-sr">{t.stateLabel}</span>
                    </Badge>
                    <Toggle
                      className="wl-name"
                      title={'Show the min–max band of ' + t.name + ' in the latency chart'}
                      pressed={selectedTunnel.value === t.id}
                      onPressedChange={() => highlight(t.id)}
                    >
                      <span className="wl-sr">Band of </span>
                      {t.name}
                    </Toggle>
                    {t.preferred && <Tag>Active exit</Tag>}
                    {change && <Tag title={change.text}>Changed</Tag>}
                  </div>
                  <div className="wl-muted wl-status">
                    {exitIp && !isExit
                      ? 'Exit IP ' +
                        exitIp.ip +
                        (place(exitIp.location) ? ', ' + place(exitIp.location) : '')
                      : t.stateLabel}
                  </div>
                  {extra && <div className="wl-muted wl-status">{extra}</div>}
                </th>
                <td className="wl-n">
                  {primary ? (
                    <>
                      <span className="wl-num wl-big">{fixed(t.median)}</span>
                      {finite(t.median) && <small> ms</small>}
                      {n && (
                        <span className={'wl-latency-note ' + n.cls} title={n.title || undefined}>
                          {n.text}
                        </span>
                      )}
                    </>
                  ) : diag(t.id) !== null ? (
                    <>
                      <span className="wl-num wl-big">{fixed(diag(t.id))}</span>
                      <small> ms</small>
                    </>
                  ) : (
                    <span className="wl-muted">No data yet</span>
                  )}
                </td>
                <td className="wl-n wl-num" title={primary ? undefined : liveTitle}>
                  {primary ? ms(jitter) : dash}
                </td>
                <td
                  className="wl-n"
                  title={primary ? loss.lost + ' of ' + loss.sent + ' samples lost' : liveTitle}
                >
                  {primary && loss.sent ? (
                    <>
                      <span className="wl-num">{percent(loss.pct)}</span>
                      <small className="wl-block wl-num">
                        {loss.lost}/{loss.sent} probes
                      </small>
                    </>
                  ) : (
                    dash
                  )}
                </td>
                <td>
                  {primary ? (
                    <Mos score={score(t.median, jitter, loss.pct)} />
                  ) : (
                    <span title={liveTitle}>{dash}</span>
                  )}
                </td>
                {showServer && (
                  <td
                    title={server && !server.address && server.reason ? server.reason : undefined}
                  >
                    {server && server.address ? (
                      <>
                        <span className="wl-num">{finite(serverDiag) ? ms(serverDiag) : dash}</span>
                        <small className="wl-block">Server {server.address}</small>
                      </>
                    ) : (
                      <span className="wl-muted">No endpoint</span>
                    )}
                  </td>
                )}
                <td className="wl-action">
                  <div className="wl-actions">
                    {!t.preferred && t.enabled && (
                      <Button
                        className="wl-button"
                        focusableWhenDisabled
                        aria-label={'Use ' + t.name + ' as exit'}
                        title={
                          !can.value.select
                            ? selectReason.value
                            : !selectable(t)
                              ? t.selectReason || t.reason
                              : undefined
                        }
                        disabled={busy.value || !selectable(t) || !can.value.select}
                        data-agent-confirm={
                          'Makes ' +
                          t.name +
                          ' the VPN exit for new connections, replacing ' +
                          (exitTunnel.value ? exitTunnel.value.name : 'the direct WAN exit')
                        }
                        onClick={() => void selectExit(t)}
                      >
                        {busyTunnel.value === t.id && busyAction.value === 'select'
                          ? 'Applying…'
                          : 'Use as exit'}
                      </Button>
                    )}
                    {busyTunnel.value === t.id && busyAction.value === 'toggle' ? (
                      <span className="wl-muted" role="status">
                        {t.enabled ? 'Stopping…' : 'Starting…'}
                      </span>
                    ) : (
                      <label
                        className="wl-switch"
                        title={
                          !t.controllable
                            ? 'Native tunnel control is unavailable.'
                            : !can.value.control
                              ? controlReason.value
                              : undefined
                        }
                      >
                        <Switch
                          aria-label={'Run ' + t.name}
                          checked={t.enabled}
                          disabled={busy.value || !t.controllable || !can.value.control}
                          data-agent-confirm={
                            t.enabled
                              ? undefined
                              : 'Starts the VPN tunnel ' + t.name + ' on the router'
                          }
                          onCheckedChange={(on, details) => {
                            details.cancel();
                            if (on !== t.enabled) askToggle(t);
                          }}
                        >
                          <SwitchThumb />
                        </Switch>
                        <span aria-hidden="true">{t.enabled ? 'Running' : 'Stopped'}</span>
                      </label>
                    )}
                  </div>
                </td>
              </tr>
            );
          })}
          {uplinks.map((u) => {
            const change = changeFor(u.id);
            const value = diag(u.active ? 'wan' : u.id);
            const loss = uplinkLoss(u);
            const jitter = finite(u.jitter) ? u.jitter : null;
            return (
              <tr key={u.id} className="wl-uplink-row">
                <th scope="row" className="wl-path">
                  <div className="wl-name-line">
                    <span className="wl-name wl-wan-name">{uplinkName(u)}</span>
                    {u.active && <Tag>Active uplink</Tag>}
                    {change && <Tag title={change.text}>Changed</Tag>}
                  </div>
                  {!u.active && u.address && (
                    <div className="wl-muted wl-status">Address {u.address}</div>
                  )}
                </th>
                <td className="wl-n">
                  {!u.active && unmeasurable(u) ? (
                    <span title="The router sends this traffic through the active uplink.">
                      Not measurable
                    </span>
                  ) : value !== null ? (
                    <>
                      <span className="wl-sr">Without VPN: </span>
                      <span className="wl-num wl-big">{fixed(value)}</span>
                      <small> ms</small>
                    </>
                  ) : (
                    <span className="wl-muted">No data yet</span>
                  )}
                </td>
                <td className="wl-n wl-num">{primary ? ms(jitter) : dash}</td>
                <td
                  className="wl-n"
                  title={
                    loss.source === 'range'
                      ? 'Loss over the selected range from history'
                      : undefined
                  }
                >
                  {primary && loss.sent ? (
                    <>
                      <span className="wl-num">{percent(loss.pct)}</span>
                      <small className="wl-block wl-num">
                        {loss.lost}/{loss.sent} {loss.source === 'range' ? 'in range' : 'probes'}
                      </small>
                    </>
                  ) : (
                    dash
                  )}
                </td>
                <td>{primary ? <Mos score={score(value, jitter, loss.pct)} /> : dash}</td>
                {showServer && <td />}
                <td />
              </tr>
            );
          })}
        </tbody>
      </table>
      {!s.tunnels.length && (
        <div className="wl-empty" role="status">
          {!ready.value
            ? readError.value
              ? 'Tunnel state is unavailable.'
              : 'Loading tunnels…'
            : readError.value
              ? 'Tunnel state could not be read.'
              : 'No configured VPN tunnels available.'}
        </div>
      )}
    </div>
  );
}

const columns = [
  { key: 'last', label: 'Last' },
  { key: 'mean', label: 'Mean' },
  { key: 'min', label: 'Min' },
  { key: 'max', label: 'Max' },
  { key: 'jitter', label: 'Jitter' },
  { key: 'loss', label: 'Loss' },
  { key: 'mos', label: 'MOS' },
  { key: 'cursor', label: 'At cursor' },
] as const;
type SortKey = 'name' | (typeof columns)[number]['key'];

const read = (row: Row, key: SortKey, at: number): number | null => {
  if (key === 'cursor') return at >= 0 && finite(row.line.median[at]) ? row.line.median[at] : null;
  if (key === 'mos') return row.stats.score ? row.stats.score.mos : null;
  if (key === 'name') return null;
  return row.stats[key];
};

function History() {
  const [sort, setSort] = useState<{ key: SortKey; dir: number }>({ key: 'last', dir: -1 });
  const [focus, setFocus] = useState(-1);
  const lines = latencyLines.value;
  const x = latencyX.value;
  const doc = latencyData.value.doc;
  const tunnels = state.value.tunnels;
  const built = useMemo(() => {
    const order = (id: string) => (isServer(id) ? 0 : id === 'wan' || isUplink(id) ? 1 : 2);
    const kept = lines.filter(
      (l) =>
        !isServer(l.id) ||
        (state.value.config.endpointDiagnostics &&
          lines.some((o) => !isServer(o.id) && o.id === ownerOf(l.id)) &&
          some(l.median, finite)),
    );
    const sorted = kept.slice().sort((a, b) => order(a.id) - order(b.id));
    const latency: Line[] = [];
    const ys: Column[] = [];
    const bands: Band[] = [];
    const owners: string[] = [];
    const bandOwner: (string | null)[] = [];
    const rows: Row[] = [];
    const lineAt: LatencyLine[] = [];
    sorted.forEach((l) => {
      const server = isServer(l.id);
      const muted = l.id === 'wan' || isUplink(l.id);
      const owner = ownerOf(l.id);
      const preferred = !muted && !server && !!tunnels.find((t) => t.id === owner)?.preferred;
      const color = seriesColor(l.id);
      const name = seriesName(l);
      const index = latency.length;
      latency.push({
        label: name,
        color,
        width: server ? 1 : muted ? 1.2 : preferred ? 2 : 1.5,
        ...(server ? { dash: [6, 4] } : {}),
      });
      ys.push(l.median);
      owners.push(owner);
      bandOwner.push(null);
      if (!muted && !server) {
        latency.push({ label: name + ' min', color, helper: true, show: false });
        ys.push(l.min);
        owners.push(owner);
        bandOwner.push(owner);
        latency.push({ label: name + ' max', color, helper: true, show: false });
        ys.push(l.max);
        owners.push(owner);
        bandOwner.push(owner);
        bands.push({ upper: index + 2, lower: index + 1, color, alpha: 0.15 });
        latency.push({ label: name + ' p95', color, width: 1, dash: [5, 3], quiet: true });
        ys.push(l.p95);
        owners.push(owner);
        bandOwner.push(owner);
        latency.push({ label: name + ' p99', color, width: 1, dash: [1, 3], quiet: true });
        ys.push(l.p99);
        owners.push(owner);
        bandOwner.push(owner);
      }
      lineAt[index] = l;
      rows.push({
        key: l.id,
        owner,
        name,
        color,
        dashed: server,
        tunnel: !muted && !server,
        line: l,
        stats: summarize(l),
        index,
      });
    });
    const simple = (pick: (l: LatencyLine) => Column) => ({
      lines: rows.map((r) => ({
        label: r.name,
        color: r.color,
        width: r.dashed ? 1 : 1.5,
        ...(r.dashed ? { dash: [6, 4] } : {}),
      })),
      ys: rows.map((r) => pick(r.line)),
    });
    return {
      latency,
      ys,
      bands,
      owners,
      bandOwner,
      rows,
      lineAt,
      jitter: simple((l) => l.jitter),
      loss: simple((l) => l.loss),
    };
  }, [lines, tunnels]);

  const band = isolated.value.length === 1 ? isolated.value[0] : selectedTunnel.value;
  const shown = (owner: string) => !isolated.value.length || isolated.value.includes(owner);
  const visible = built.owners.map(
    (o, i) => shown(o) && (built.bandOwner[i] === null || o === band),
  );
  const rowVisible = built.rows.map((r) => shown(r.owner));
  const markers: Marker[] = useMemo(
    () => [
      ...(doc?.events || []).map((e) => ({ at: e.at, tone: 'event' as const, text: e.text })),
      ...(doc?.incidents || []).map((i) => ({
        at: i.start,
        tone: 'danger' as const,
        text: (i.name || subjectName(i.subject)) + ': ' + (i.detail || 'all probes lost'),
      })),
    ],
    [doc],
  );
  const at = cursor.value;
  const sorted = useMemo(() => {
    const { key, dir } = sort;
    const groups = new Map<string, Row[]>();
    built.rows.forEach((row) => {
      if (!groups.has(row.owner)) groups.set(row.owner, []);
      groups.get(row.owner)?.push(row);
    });
    const ordered = Array.from(groups.values()).map((rows) =>
      rows.sort((a, b) => Number(a.dashed) - Number(b.dashed)),
    );
    const lead = (rows: Row[]) => rows.find((r) => !r.dashed) || rows[0];
    ordered.sort((a, b) => {
      const p = lead(a);
      const q = lead(b);
      if (key === 'name') return dir * p.name.localeCompare(q.name);
      const u = read(p, key, at);
      const v = read(q, key, at);
      if (u === null || v === null) return Number(u === null) - Number(v === null);
      return dir * (u - v);
    });
    return ordered.flat();
  }, [built, sort, at]);
  const sortBy = (key: SortKey) =>
    setSort(sort.key === key ? { key, dir: -sort.dir } : { key, dir: key === 'name' ? 1 : -1 });
  const ariaSort = (key: SortKey) =>
    sort.key !== key ? 'none' : sort.dir > 0 ? 'ascending' : 'descending';
  const data = latencyData.value;
  let note = '';
  if (!ready.value)
    note = readError.value ? 'Tunnel state could not be read.' : 'Reading tunnel state…';
  else if (data.error) note = data.error;
  else if (!tunnels.length && !state.value.config.directWan) note = 'No tunnels to show.';
  else if (!lines.length || lines.every((l) => !some(l.median, finite)))
    note = data.loaded && !data.loading ? 'No measurements in this range.' : 'Loading history…';
  const target = selectedTarget.value;
  const fmt = (v: number) => v.toFixed(2) + ' ms';
  const active = (row: Row) => row.tunnel && !!tunnels.find((t) => t.id === row.owner)?.preferred;
  const pick = (row: Row, e: Event | undefined) => {
    const k = e as KeyboardEvent | MouseEvent | undefined;
    isolate(row.owner, row.tunnel, !!k && (k.ctrlKey || k.metaKey || k.shiftKey));
  };
  return (
    <Panel
      id="wl-latency-title"
      title={'Latency to ' + target}
      sub={
        scaleNote('medians') +
        '. Gaps mean no measurement. Drag across a chart to zoom, double-click to reset.'
      }
    >
      {built.rows.length > 0 && (
        <div className="wl-scroll">
          <table className="wl-table wl-series">
            <caption className="wl-sr">
              Per-series statistics for the selected range, in milliseconds
            </caption>
            <thead>
              <tr>
                <th scope="col" aria-sort={ariaSort('name')}>
                  <Button className="wl-sort" onClick={() => sortBy('name')}>
                    <span className="wl-sr">Sort by </span>
                    Series <span className="wl-muted">(ms)</span>
                  </Button>
                </th>
                {columns.map((c) => (
                  <th key={c.key} scope="col" className="wl-n" aria-sort={ariaSort(c.key)}>
                    <Button className="wl-sort" onClick={() => sortBy(c.key)}>
                      <span className="wl-sr">Sort by </span>
                      {c.label}
                    </Button>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {sorted.map((row) => (
                <tr
                  key={row.key}
                  className={
                    (isolated.value.includes(row.owner) ? 'wl-on ' : '') +
                    (isolated.value.length && !isolated.value.includes(row.owner) ? 'wl-dim' : '')
                  }
                  onMouseEnter={() => setFocus(row.index)}
                  onMouseLeave={() => setFocus(-1)}
                  onFocus={() => setFocus(row.index)}
                  onBlur={() => setFocus(-1)}
                >
                  <th scope="row">
                    <svg
                      className="wl-swatch"
                      width="20"
                      height="4"
                      viewBox="0 0 20 4"
                      aria-hidden="true"
                    >
                      <line
                        x1="0"
                        y1="2"
                        x2="20"
                        y2="2"
                        style={{ stroke: 'var(--wl-' + row.color + ')' }}
                        strokeDasharray={row.dashed ? '4 3' : undefined}
                      />
                    </svg>
                    <Toggle
                      className="wl-sort"
                      pressed={isolated.value.includes(row.owner)}
                      onPressedChange={(_on, details) => {
                        details.cancel();
                        pick(row, details.event);
                      }}
                    >
                      <span className="wl-sr">Show only </span>
                      {row.name}
                      {row.dashed ? '' : <span className="wl-sr"> path</span>}
                    </Toggle>
                    {active(row) && <Tag>Active exit</Tag>}
                  </th>
                  <td className="wl-n wl-num">{fixed(row.stats.last)}</td>
                  <td className="wl-n wl-num">{fixed(row.stats.mean)}</td>
                  <td className="wl-n wl-num">{fixed(row.stats.min)}</td>
                  <td className="wl-n wl-num">{fixed(row.stats.max)}</td>
                  <td className="wl-n wl-num">{fixed(row.stats.jitter)}</td>
                  <td
                    className="wl-n wl-num"
                    title={row.stats.lost + ' of ' + row.stats.sent + ' probes lost to ' + target}
                  >
                    {percent(row.stats.loss)}
                  </td>
                  <td className="wl-n wl-num">
                    {row.stats.score ? row.stats.score.mos.toFixed(2) : dash}
                  </td>
                  <td className="wl-n wl-num">
                    {at >= 0 && finite(row.line.median[at])
                      ? (row.line.median[at] as number).toFixed(2)
                      : dash}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="wl-muted wl-hint">
        Press a series name to show only that line. Ctrl- or Cmd-click adds more. MOS uses the range
        mean latency and jitter and the range loss.
      </p>
      <div className="wl-chart-area">
        {note && (
          <div className="wl-chart-note" role="status">
            {note}
          </div>
        )}
        <Chart
          title="Median latency (ms) with min–max band, p95 and p99"
          detail={scaleNote('medians') + ' · p95 dashed, p99 dotted for the highlighted path'}
          label={'Latency to ' + target + ' through each path'}
          extra={(i, at) => {
            const l = built.lineAt[i];
            if (!l) return '';
            const p = (name: string, col: Column) =>
              finite(col[at]) ? ' · ' + name + ' ' + (col[at] as number).toFixed(1) : '';
            return p('p50', l.p50) + p('p95', l.p95) + p('p99', l.p99);
          }}
          x={x}
          ys={built.ys}
          lines={built.latency}
          bands={built.bands}
          visible={visible}
          focus={focus}
          format={fmt}
          axis={(v) => String(Math.round(v))}
          markers={markers}
          legend={false}
          height={230}
          y={(_l, high) => [0, high > 0 ? high * 1.15 : 25]}
          onCursor={(i) => {
            if (cursor.value !== i) cursor.value = i;
          }}
        >
          <p
            className="wl-muted wl-small"
            title="Percentiles from a histogram; error within one bin"
          >
            Percentiles from a 16-bin RTT histogram; error within one bin (under 1 ms below 3 ms,
            under 5 ms below 20 ms, 50 ms between 100 and 150 ms).
          </p>
        </Chart>
        <Chart
          title="Jitter (ms)"
          detail={scaleNote('buckets') + ' · mean |RTTᵢ − RTTᵢ₋₁|'}
          label={'Jitter to ' + target + ' through each path'}
          x={x}
          ys={built.jitter.ys}
          lines={built.jitter.lines}
          visible={rowVisible}
          format={fmt}
          axis={(v) => (v < 10 ? v.toFixed(1) : String(Math.round(v)))}
          markers={markers}
          legend={false}
          height={130}
          y={(_l, high) => [0, high > 0 ? high * 1.2 : 2]}
        />
        <Chart
          title="Packet loss (%)"
          detail={scaleNote('buckets') + ' · lost / sent probes'}
          label={'Packet loss to ' + target + ' through each path'}
          x={x}
          ys={built.loss.ys}
          lines={built.loss.lines}
          visible={rowVisible}
          format={(v) => v.toFixed(1) + ' %'}
          axis={(v) => (Number.isInteger(v) ? String(v) : v.toFixed(1))}
          markers={markers}
          legend={false}
          height={110}
          y={(_l, high) => [0, Math.min(100, Math.max(2, high * 1.3))]}
        />
      </div>
      {doc && built.rows.length > 0 && (
        <section className="wl-sub" aria-labelledby="wl-strips-title">
          <h3 id="wl-strips-title">Uptime by bucket</h3>
          <p className="wl-muted">
            {scaleNote('buckets')}. Hover or focus a strip and use the arrow keys to inspect a
            bucket.
          </p>
          {built.rows
            .filter((r) => !r.dashed)
            .map((r) => (
              <Strip
                key={r.key}
                name={r.name}
                sent={r.line.sent}
                lost={r.line.lost}
                start={doc.start}
                step={doc.step}
              />
            ))}
          <StripLegend />
          <UptimeExplain />
        </section>
      )}
      <section className="wl-sub" aria-labelledby="wl-range-incidents">
        <h3 id="wl-range-incidents">Incidents in range</h3>
        <IncidentList items={rangeIncidents()} />
      </section>
      <MosExplain inputs="Per path and range it uses the mean of the bucket medians, the mean bucket jitter, and lost / sent probes." />
    </Panel>
  );
}

export function PathsView() {
  const s = state.value;
  const auto = s.auto;
  const probe = s.probe || {};
  return (
    <>
      {!can.value.select && selectReason.value && (
        <p className="wl-muted wl-hint">{selectReason.value}</p>
      )}
      {auto.enabled && (
        <div className="wl-panel wl-info">
          <strong>Automatic exit switching is on.</strong>
          {auto.needsAttention ? (
            <p role="alert">
              Automatic switching stopped after an uncertain request. Check the native VPN state.
              Disable automatic switching. Save Settings. Enable it again.
            </p>
          ) : auto.paused ? (
            <p>
              Automatic switching is paused because the current exit is outside the allowed pool.
            </p>
          ) : null}
          {auto.last ? (
            <span>
              {' '}
              Last switch <When at={auto.last.at} />: from {tunnelName(auto.last.from)} to{' '}
              {tunnelName(auto.last.to)}. {auto.last.reason}
            </span>
          ) : (
            <span className="wl-muted"> No switch yet.</span>
          )}
        </div>
      )}
      <div className="wl-table-head">
        {destinations.value.length > 1 && (
          <SegmentedControl
            className="wl-seg"
            aria-label="Destination"
            value={selectedTarget.value}
            onValueChange={(next) => {
              if (next) chooseTarget(next);
            }}
          >
            <SegmentedControlIndicator className="wl-seg-indicator" />
            {destinations.value.map((d) => (
              <SegmentedControlItem key={d} value={d}>
                {d}
              </SegmentedControlItem>
            ))}
          </SegmentedControl>
        )}
        <p className="wl-muted" title={s.scope || undefined}>
          ICMP echo to {selectedTarget.value} every {probe.interval || s.config.interval} s
          {finite(probe.timeout) ? ', timeout ' + probe.timeout + ' ms' : ''}. Loss and jitter use
          the recent window.
        </p>
      </div>
      <TunnelTable />
      <StopDialog />
      {s.config.directWan && s.uplinks.length > 0 && !wanAvailable.value && (
        <p className="wl-muted wl-note" role="note">
          Direct WAN is unavailable. {s.wan.reason || 'The router did not give a reason.'}
        </p>
      )}
      <p className="wl-muted wl-note">
        Changing the exit affects new connections. Stopping a tunnel may send traffic over WAN.
      </p>
      <History />
    </>
  );
}
