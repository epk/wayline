import { Button } from '@ui-sdk/react/button';
import type { ReactNode } from 'react';
import { IncidentList, tunnelName } from '../common.tsx';
import {
  activeUplink,
  egressFor,
  health,
  incidents,
  internet,
  lineFor,
  memUsed,
  routerLimits,
  toneLabel,
  vpn,
  wifiLevel,
  wifiRange,
  type Health,
  type PathHealth,
} from '../derived.ts';
import {
  band as bandName,
  bytes,
  dash,
  dbm,
  finite,
  kbit,
  ms,
  percent,
  place,
  span,
} from '../format.ts';
import {
  busy,
  can,
  enableSetting,
  exitTunnel,
  latencyData,
  primary,
  primarySelected,
  ready,
  selectedTarget,
  setView,
  state,
  wanAvailable,
} from '../store.ts';
import {
  Badge,
  CALL_QUALITY,
  Explain,
  Facts,
  Mos,
  MosExplain,
  scaleNote,
  Strip,
  Tag,
  When,
  type Tone,
} from '../ui.tsx';

function Hero() {
  const s = state.value;
  const exit = exitTunnel.value;
  const diag = (id: string) => {
    const d = s.diagnostics[selectedTarget.value] || {};
    return finite(d[id]) ? d[id] : null;
  };
  const direct = diag('wan');
  const exitLatency = !exit
    ? direct
    : primarySelected.value
      ? finite(exit.median)
        ? exit.median
        : null
      : diag(exit.id);
  const wanEgress = egressFor('wan');
  const heroEgress = exit ? egressFor(exit.id) : wanEgress;
  const uplink = activeUplink.value;
  const wanInterface = (uplink && uplink.interface) || (wanEgress && wanEgress.interface) || 'WAN';
  const route = (name: string, via: string) => (name ? name + ', through ' : 'Through ') + via;
  const columns = [
    {
      label: 'Current exit',
      egress: heroEgress,
      route: route(place(heroEgress?.location), exit ? exit.name : wanInterface + ' without VPN'),
      latency: exitLatency,
    },
  ];
  if (s.config.directWan && exit && wanEgress && (wanEgress.ip || wanEgress.error))
    columns.push({
      label: 'Without VPN',
      egress: wanEgress,
      route: route(place(wanEgress.location), wanInterface),
      latency: direct,
    });
  let comparison: {
    sentence: string;
    bars: { name: string; cls: string; width: string; text: string }[];
    label: string;
  } | null = null;
  if (exit && finite(exitLatency) && finite(direct)) {
    const diff = exitLatency - direct;
    const scale = Math.max(exitLatency, direct) || 1;
    let sentence = 'About the same as going direct.';
    if (diff >= 1) sentence = 'The VPN adds ' + diff.toFixed(2) + ' ms.';
    else if (diff <= -1) sentence = 'The VPN is ' + (-diff).toFixed(2) + ' ms faster.';
    comparison = {
      sentence,
      bars: [
        {
          name: 'VPN',
          cls: 'wl-bar-vpn',
          width: (exitLatency / scale) * 100 + '%',
          text: ms(exitLatency),
        },
        { name: 'Direct', cls: '', width: (direct / scale) * 100 + '%', text: ms(direct) },
      ],
      label: 'VPN ' + ms(exitLatency) + ', direct ' + ms(direct),
    };
  }
  const checked = s.egress?.checkedAt || 0;
  return (
    <section className="wl-panel wl-hero" aria-label="Exit addresses">
      <div className="wl-hero-grid">
        {columns.map((c, i) => {
          const ip = c.egress && !c.egress.error ? c.egress.ip : '';
          const unverified = ip && !c.egress?.verified;
          return (
            <div key={c.label} className={i ? 'wl-hero-secondary' : 'wl-hero-primary'}>
              <p className="wl-label">
                {c.label}
                {unverified && (
                  <span title="Read over plain HTTP, so the network path can change it.">
                    {' '}
                    (unverified)
                  </span>
                )}
              </p>
              {ip ? (
                <p className="wl-hero-ip wl-num">{ip}</p>
              ) : (
                <p
                  className="wl-hero-ip wl-hero-pending wl-muted"
                  title={c.egress?.error || undefined}
                >
                  {c.egress?.error ? 'Exit address unavailable' : 'Checking exit address…'}
                </p>
              )}
              <p>{c.route}</p>
              {c.latency !== null && (
                <p className="wl-num wl-muted">
                  {c.latency.toFixed(2)} ms to {selectedTarget.value}
                </p>
              )}
            </div>
          );
        })}
      </div>
      {comparison && (
        <div className="wl-hero-compare">
          <p>{comparison.sentence}</p>
          <div className="wl-bars" role="img" aria-label={comparison.label}>
            {comparison.bars.map((b) => (
              <div key={b.name} className="wl-bar-row">
                <span className="wl-muted">{b.name}</span>
                <span className="wl-bar-track">
                  <span className={'wl-bar-fill ' + b.cls} style={{ width: b.width }} />
                </span>
                <span className="wl-num">{b.text}</span>
              </div>
            ))}
          </div>
        </div>
      )}
      <p className="wl-muted wl-small">
        {checked ? (
          <>
            Exit addresses checked <When at={checked * 1000} />
          </>
        ) : (
          'Checking exit addresses…'
        )}
      </p>
    </section>
  );
}

function Card(props: { item: Health; title: string; children: ReactNode; foot?: ReactNode }) {
  return (
    <section
      className={'wl-card wl-card-' + props.item.tone}
      aria-labelledby={'wl-card-' + props.item.id}
    >
      <header className="wl-card-head">
        <h2 id={'wl-card-' + props.item.id}>{props.title}</h2>
        <Badge tone={props.item.tone as Tone} title={props.item.reason}>
          {toneLabel[props.item.tone]}
        </Badge>
      </header>
      {props.children}
      {props.foot && <div className="wl-card-foot">{props.foot}</div>}
    </section>
  );
}

const lossSub = (p: PathHealth, target: string) =>
  p.loss.sent
    ? p.loss.lost +
      ' of ' +
      p.loss.sent +
      ' probes to ' +
      target +
      (p.loss.source === 'range' ? ' in range' : '')
    : 'no probes yet';

function PathFacts(props: { p: PathHealth; target: string }) {
  const p = props.p;
  return (
    <Facts
      items={[
        { label: 'Latency', value: ms(p.median), sub: 'median to ' + props.target },
        {
          label: 'Jitter',
          value: ms(p.jitter),
          sub: 'variation between replies',
          title: 'Average change in round-trip time from one reply to the next.',
        },
        { label: 'Loss', value: percent(p.loss.pct), sub: lossSub(p, props.target) },
        {
          label: 'Call quality',
          title: CALL_QUALITY,
          value: <Mos score={p.score} />,
          sub: p.score ? 'score out of 5' : 'needs latency, jitter, loss',
        },
      ]}
    />
  );
}

function Enable(props: { text: string; label: string; onClick: () => void }) {
  return (
    <div className="wl-cta">
      <p className="wl-muted">{props.text}</p>
      <Button
        className="wl-button wl-primary"
        disabled={!can.value.configure || busy.value}
        data-agent-confirm={'Changes router settings: ' + props.label.toLowerCase()}
        onClick={props.onClick}
      >
        {props.label}
      </Button>
    </div>
  );
}

function StripFor(props: { id: string; name: string }) {
  const doc = latencyData.value.doc;
  const line = lineFor(props.id);
  if (!doc || !line) return null;
  return (
    <div className="wl-card-strip">
      <p className="wl-label">Uptime, {scaleNote()}</p>
      <Strip
        name={props.name}
        sent={line.sent}
        lost={line.lost}
        start={doc.start}
        step={doc.step}
      />
    </div>
  );
}

function InternetCard(props: { item: Health }) {
  const s = state.value;
  const p = internet.value;
  const u = activeUplink.value;
  return (
    <Card item={props.item} title="Direct internet">
      {!p ? (
        <Enable
          text="Measure the active uplink without the VPN: latency, jitter, loss and call quality to each destination."
          label="Measure direct internet"
          onClick={() =>
            void enableSetting(
              'directWan',
              'Direct internet measurement is on. The first samples arrive within seconds.',
            )
          }
        />
      ) : (
        <>
          <p className="wl-card-sub wl-muted">
            {u ? (u.interface || u.id) + (u.device ? ' · ' + u.device : '') : 'Internet uplink'}
            {p.down ? ' · down since ' : ''}
            {p.down && <When at={p.down.start} />}
          </p>
          {!wanAvailable.value && (
            <p className="wl-note">
              The direct internet connection is unavailable.{' '}
              {s.wan.reason || 'The router did not give a reason.'}
            </p>
          )}
          <PathFacts p={p} target={primary.value} />
          <StripFor id="wan" name={u?.interface || 'Direct internet'} />
          <MosExplain inputs="Inputs: live median latency and jitter of the active uplink, and its recent loss (or the selected range when the router does not report a recent probe count)." />
        </>
      )}
    </Card>
  );
}

function VpnCard(props: { item: Health }) {
  const s = state.value;
  const t = exitTunnel.value;
  const p = vpn.value;
  const egress = t ? egressFor(t.id) : null;
  const auto = s.auto;
  const autoText = !auto.enabled
    ? 'Off'
    : auto.needsAttention
      ? 'Stopped, needs attention'
      : auto.paused
        ? 'Paused, exit outside pool'
        : 'On';
  return (
    <Card
      item={props.item}
      title="VPN exit"
      foot={
        <Button className="wl-link" onClick={() => setView('paths')}>
          Compare paths
        </Button>
      }
    >
      {!t || !p ? (
        <p className="wl-muted">
          {s.tunnels.length
            ? 'No connected VPN tunnel is the exit. Traffic leaves without a VPN.'
            : 'No VPN tunnels are configured.'}
        </p>
      ) : (
        <>
          <p className="wl-card-sub">
            <strong>{t.name}</strong> <Tag>Active exit</Tag>
            <span className="wl-muted wl-block">
              {egress && egress.ip && !egress.error
                ? 'Egress ' +
                  egress.ip +
                  (place(egress.location) ? ' · ' + place(egress.location) : '')
                : egress?.error || 'Checking egress address…'}
            </span>
          </p>
          <PathFacts p={p} target={primary.value} />
          <StripFor id={t.id} name={t.name} />
        </>
      )}
      <p className="wl-small">
        <span className="wl-muted">Auto-switch:</span> {autoText}
        {auto.last && (
          <span className="wl-muted">
            {' '}
            · last <When at={auto.last.at} /> {tunnelName(auto.last.from)} →{' '}
            {tunnelName(auto.last.to)}
          </span>
        )}
      </p>
      {p && (
        <MosExplain inputs="Inputs: the exit tunnel's live median latency, jitter, and lost / sent probes in the recent window." />
      )}
    </Card>
  );
}

const levelBadge = (level: string) => <Badge tone={level as Tone}>{toneLabel[level]}</Badge>;

function WifiCard(props: { item: Health }) {
  const w = state.value.wifi;
  const l = w.link;
  const sum = wifiRange.value;
  const snr = l && finite(l.signal) && finite(l.noise) ? l.signal - l.noise : null;
  const busyPct = l && finite(l.busy_permille) ? l.busy_permille / 10 : null;
  return (
    <Card
      item={props.item}
      title="Upstream Wi-Fi"
      foot={
        w.enabled && (
          <Button className="wl-link" onClick={() => setView('wifi')}>
            Wi-Fi details
          </Button>
        )
      }
    >
      {!w.enabled ? (
        <Enable
          text="Record signal, noise, link rate, retries and channel load of the network this router joins as a repeater."
          label="Monitor upstream Wi-Fi"
          onClick={() =>
            void enableSetting(
              'upstreamWifi',
              'Upstream Wi-Fi monitoring is on. The first samples arrive within seconds.',
            )
          }
        />
      ) : !l ? (
        <p className="wl-muted">{w.reason || 'No Wi-Fi link data yet.'}</p>
      ) : (
        <>
          <p className="wl-card-sub">
            <strong>{l.ssid || 'Hidden network'}</strong>
            <span className="wl-muted wl-block">
              {[
                l.channel ? 'ch ' + l.channel : '',
                finite(l.frequency) ? bandName(l.frequency) : '',
                l.width ? l.width + ' MHz' : '',
                w.interface || '',
              ]
                .filter(Boolean)
                .join(' · ')}
            </span>
          </p>
          <Facts
            items={[
              {
                label: 'Signal',
                value: dbm(l.signal),
                sub: finite(l.signal) ? levelBadge(wifiLevel.signal(l.signal)) : '',
              },
              {
                label: 'Signal to noise',
                title: 'How far the signal stands above background noise, in dB. Higher is better.',
                value: finite(snr) ? snr + ' dB' : dash,
                sub: finite(snr) ? levelBadge(wifiLevel.snr(snr)) : '',
              },
              {
                label: 'Link rate',
                title: 'Negotiated radio rate to the access point, not actual throughput.',
                value: kbit(l.tx.kbps) + ' up',
                sub: kbit(l.rx.kbps) + ' down',
              },
              {
                label: 'Channel busy',
                value: percent(busyPct),
                sub: finite(busyPct) ? levelBadge(wifiLevel.busy(busyPct)) : '',
              },
              {
                label: 'Drops / roams',
                value: sum ? (sum.disconnects ?? 0) + ' / ' + (sum.roams ?? 0) : dash,
                sub: 'in range',
              },
              {
                label: 'Connected',
                value: finite(l.connected_seconds) ? span(l.connected_seconds) : dash,
                sub: l.bssid,
              },
            ]}
          />
        </>
      )}
    </Card>
  );
}

function RouterCard(props: { item: Health }) {
  const s = state.value;
  const sys = s.telemetry.system;
  const mem = memUsed(sys);
  const over = (value: number | null, limit: number) =>
    finite(value) && value >= limit ? <Badge tone="degraded">High</Badge> : '';
  const ct =
    sys && finite(sys.conntrack) && finite(sys.conntrackMax) && sys.conntrackMax > 0
      ? (sys.conntrack / sys.conntrackMax) * 100
      : null;
  return (
    <Card
      item={props.item}
      title="Router"
      foot={
        s.config.telemetry && (
          <Button className="wl-link" onClick={() => setView('router')}>
            Router history
          </Button>
        )
      }
    >
      {!s.config.telemetry ? (
        <Enable
          text="Router and interface telemetry: processor, memory, temperature, tracked connections, per-interface throughput, WireGuard handshakes."
          label="Record router telemetry"
          onClick={() =>
            void enableSetting(
              'telemetry',
              'Router telemetry is on. The first sample arrives within seconds.',
            )
          }
        />
      ) : !sys ? (
        <p className="wl-muted">Waiting for the first telemetry sample.</p>
      ) : (
        <Facts
          items={[
            { label: 'Processor', value: percent(sys.cpu), sub: over(sys.cpu, routerLimits.cpu) },
            {
              label: 'Load 1 / 5 / 15',
              value: sys.load ? sys.load.map((v) => v.toFixed(2)).join(' ') : dash,
            },
            {
              label: 'Memory used',
              value: percent(mem),
              sub:
                over(mem, routerLimits.mem) ||
                (finite(sys.memTotal) ? 'of ' + bytes(sys.memTotal) : ''),
            },
            {
              label: 'Temperature',
              value: finite(sys.temperature) ? sys.temperature.toFixed(1) + ' °C' : dash,
              sub: over(sys.temperature, routerLimits.temperature),
            },
            {
              label: 'Tracked connections',
              title:
                'Connections the router firewall is tracking (its connection table), against its limit.',
              value: finite(sys.conntrack) ? String(sys.conntrack) : dash,
              sub:
                over(ct, routerLimits.conntrack) ||
                (finite(sys.conntrackMax)
                  ? 'of ' + sys.conntrackMax + (finite(ct) ? ' (' + ct.toFixed(1) + ' %)' : '')
                  : ''),
            },
            { label: 'Uptime', value: finite(sys.uptime) ? span(sys.uptime) : dash },
          ]}
        />
      )}
    </Card>
  );
}

export function OverviewView() {
  if (!ready.value) return null;
  const items = health.value;
  const by = (id: Health['id']) => items.find((i) => i.id === id) as Health;
  const events = state.value.events.slice(0, 5);
  return (
    <>
      <Hero />
      <div className="wl-cards">
        <InternetCard item={by('internet')} />
        <VpnCard item={by('vpn')} />
        <WifiCard item={by('wifi')} />
        <RouterCard item={by('router')} />
      </div>
      <Explain summary="How health states are decided">
        <p>
          Health states are heuristics: Down means an ongoing incident (every probe lost for at
          least 15 s) or a disconnected link; Degraded means loss ≥ 1 %, call quality below 3.6, a
          Wi-Fi metric in its poor band, or router CPU, memory or tracked connections ≥{' '}
          {routerLimits.cpu} % or temperature ≥ {routerLimits.temperature} °C.
        </p>
      </Explain>
      <div className="wl-split">
        <section className="wl-panel" aria-labelledby="wl-ov-incidents">
          <header className="wl-panel-head">
            <h2 id="wl-ov-incidents">Incidents</h2>
            <Button className="wl-link" onClick={() => setView('events')}>
              All events
            </Button>
          </header>
          <IncidentList
            items={incidents.value}
            limit={5}
            empty="No incidents in the last 6 hours."
          />
        </section>
        <section className="wl-panel" aria-labelledby="wl-ov-changes">
          <header className="wl-panel-head">
            <h2 id="wl-ov-changes">Recent changes</h2>
          </header>
          {events.length ? (
            <ul className="wl-changes">
              {events.map((e) => (
                <li key={e.at + e.subject + e.text}>
                  <When at={e.at} />
                  <span>{e.text}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="wl-muted">No recent changes.</p>
          )}
        </section>
      </div>
    </>
  );
}
