import type { ReactNode } from 'react';
import { incidents } from './derived.ts';
import { dateTime, span, stamp } from './format.ts';
import { clock, range, state, windowEnd, windowStart } from './store.ts';
import type { ChangeEvent, Incident } from './types.ts';
import { Badge, When } from './ui.tsx';

const changeWindow = 60 * 60000;

export const changeFor = (subject: string): ChangeEvent | null => {
  const s = state.value;
  const cutoff = Date.now() - changeWindow;
  const tunnel = s.tunnels.find((row) => row.id === subject);
  return (
    s.events.find(
      (e) =>
        (e.subject === subject || (tunnel && e.subject === tunnel.monitorId)) && e.at >= cutoff,
    ) || null
  );
};

// Change events name a tunnel by its monitor id (vpn:7013) while incidents
// use its config id (cfg0392bd). Both mean one tunnel, so fold them together.
export const subjectKey = (subject: string): string =>
  state.value.tunnels.find((x) => x.monitorId === subject)?.id || subject;

export const tunnelName = (id: string): string => {
  const t = state.value.tunnels.find((x) => x.id === id || x.monitorId === id || x.name === id);
  return t ? t.name : id;
};

export const subjectName = (subject: string): string => {
  if (subject === 'wan') return 'Direct internet';
  if (subject === 'wifi') return 'Upstream Wi-Fi';
  if (subject === 'router') return 'Router';
  if (subject === 'history') return 'History storage';
  if (subject.startsWith('uplink:')) return subject.slice(7) + ' uplink';
  if (subject.startsWith('endpoint:')) return tunnelName(subject.slice(9)) + ' server';
  return tunnelName(subject);
};

export const incidentKind = (kind: string): string =>
  kind === 'wifi'
    ? 'Wi-Fi disconnected'
    : kind === 'off_channel'
      ? 'Radio off channel'
      : 'Total loss';

export const inWindow = (i: Incident): boolean =>
  i.start <= windowEnd.value && (i.end === null || i.end >= windowStart.value);

export function IncidentList(props: { items: Incident[]; empty?: ReactNode; limit?: number }) {
  const items = props.limit ? props.items.slice(0, props.limit) : props.items;
  if (!items.length)
    return <p className="wl-muted">{props.empty || 'No incidents in this range.'}</p>;
  const now = clock.value;
  const long = range.value > 1440;
  return (
    <ol className="wl-incidents">
      {items.map((i) => (
        <li key={i.subject + i.kind + i.start} className={i.end === null ? 'wl-ongoing' : ''}>
          <Badge tone={i.end === null ? 'down' : 'poor'}>
            {i.end === null ? 'Ongoing' : 'Resolved'}
          </Badge>
          <span className="wl-incident-name">{i.name || subjectName(i.subject)}</span>
          <span className="wl-muted">{incidentKind(i.kind)}</span>
          <span className="wl-num" title={dateTime(i.start)}>
            {stamp(i.start, long)}
            {i.end !== null ? ' – ' + stamp(i.end, long) : ' – now'}
          </span>
          <span className="wl-num">{span(((i.end === null ? now : i.end) - i.start) / 1000)}</span>
          <When at={i.start} />
          {i.detail && <span className="wl-incident-detail">{i.detail}</span>}
        </li>
      ))}
    </ol>
  );
}

export const rangeIncidents = (): Incident[] => incidents.value.filter(inWindow);
