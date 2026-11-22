import { Pick } from '../kit.tsx';
import { subjectKey, subjectName } from '../common.tsx';
import { incidents } from '../derived.ts';
import { dateTime, span } from '../format.ts';
import { clock, eventSubject, latencyData, state, wifiData } from '../store.ts';
import { Badge, Panel, Tag, When, windowText } from '../ui.tsx';

interface Item {
  at: number;
  subject: string;
  kind: string;
  text: string;
  end?: number | null;
  incident: boolean;
}

export function EventsView() {
  const seen = new Set<string>();
  const items: Item[] = [];
  const push = (raw: Item) => {
    const item = { ...raw, subject: subjectKey(raw.subject) };
    const key = item.at + '|' + item.subject + '|' + item.text;
    if (seen.has(key)) return;
    seen.add(key);
    items.push(item);
  };
  state.value.events.forEach((e) => push({ ...e, incident: false }));
  (latencyData.value.doc?.events || []).forEach((e) => push({ ...e, incident: false }));
  (wifiData.value.doc?.events || []).forEach((e) =>
    push({ ...e, subject: e.subject || 'wifi', incident: false }),
  );
  incidents.value.forEach((i) =>
    push({
      at: i.start,
      subject: i.subject,
      kind: i.kind === 'wifi' ? 'Wi-Fi disconnected' : 'Total loss',
      text: (i.name || subjectName(i.subject)) + ': ' + (i.detail || 'every probe lost'),
      end: i.end,
      incident: true,
    }),
  );
  items.sort((a, b) => b.at - a.at);
  const subjects = Array.from(new Set(items.map((i) => i.subject))).sort((a, b) =>
    subjectName(a).localeCompare(subjectName(b)),
  );
  const filter = eventSubject.value;
  const shown = filter ? items.filter((i) => i.subject === filter) : items;
  const now = clock.value;
  return (
    <Panel
      id="wl-events-title"
      title="Events"
      sub={
        'Change events, incidents and Wi-Fi events. Live state covers the last 6 hours; history covers ' +
        windowText() +
        '.'
      }
      actions={
        <Pick
          label="Subject"
          value={filter || 'all'}
          items={[
            { value: 'all', label: 'All (' + items.length + ')' },
            ...subjects.map((sub) => ({
              value: sub,
              label: subjectName(sub) + ' (' + items.filter((i) => i.subject === sub).length + ')',
            })),
          ]}
          onChange={(v) => (eventSubject.value = v === 'all' ? '' : v)}
        />
      }
    >
      {shown.length ? (
        <ol className="wl-timeline">
          {shown.map((i) => (
            <li key={i.at + i.subject + i.text} className={i.incident ? 'wl-tl-incident' : ''}>
              <div className="wl-tl-time">
                <time className="wl-num" dateTime={new Date(i.at).toISOString()}>
                  {dateTime(i.at)}
                </time>
                <When at={i.at} />
              </div>
              <div className="wl-tl-body">
                <div className="wl-name-line">
                  {i.incident ? (
                    <Badge tone={i.end === null ? 'down' : 'poor'}>
                      {i.end === null ? 'Ongoing incident' : 'Incident'}
                    </Badge>
                  ) : (
                    <Tag>{i.kind || 'event'}</Tag>
                  )}
                  <span className="wl-muted">{subjectName(i.subject)}</span>
                  {i.incident && (
                    <span className="wl-num wl-muted">
                      {span(((i.end === null || i.end === undefined ? now : i.end) - i.at) / 1000)}
                    </span>
                  )}
                </div>
                <p>{i.text}</p>
              </div>
            </li>
          ))}
        </ol>
      ) : (
        <p className="wl-muted">No events{filter ? ' for ' + subjectName(filter) : ''}.</p>
      )}
    </Panel>
  );
}
