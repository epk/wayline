import { Button } from '@ui-sdk/react/button';
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from '@ui-sdk/react/segmented-control';
import { chips, toneLabel, type Health } from './derived.ts';
import {
  bundleVersion,
  can,
  choosePreset,
  goNow,
  maxMinutes,
  offset,
  openSettings,
  range,
  ranges,
  setView,
  shift,
  state,
  type ViewId,
} from './store.ts';
import { Badge, scaleNote, windowText, type Tone } from './ui.tsx';

const target: Record<Health['id'], ViewId> = {
  internet: 'paths',
  vpn: 'paths',
  wifi: 'wifi',
  router: 'router',
};

function Chevron(props: { left?: boolean }) {
  return (
    <svg className="wl-chevron" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <path d={props.left ? 'm10 3-5 5 5 5' : 'm6 3 5 5-5 5'} />
    </svg>
  );
}

export function Header() {
  const version = state.value.version || bundleVersion;
  const live = offset.value === 0;
  const preset = ranges.value.some((r) => r.minutes === range.value) ? String(range.value) : null;
  return (
    <header className="wl-header">
      <div className="wl-title">
        <h1>
          Wayline <span className="wl-version wl-num">{version}</span>
        </h1>
        <p className="wl-muted">
          Path latency, loss and exit selection, upstream Wi-Fi and router telemetry.
        </p>
      </div>
      <ul className="wl-chips" aria-label="Health summary, worst first">
        {chips.value.map((c) => (
          <li key={c.id}>
            <Button
              className={'wl-chip wl-chip-' + c.tone}
              title={c.reason}
              onClick={() => setView(target[c.id])}
            >
              <Badge tone={c.tone as Tone}>{c.label}</Badge>
              <span className="wl-chip-text">
                <span className="wl-sr">: {toneLabel[c.tone]}, </span>
                {c.text}
              </span>
            </Button>
          </li>
        ))}
      </ul>
      <div className="wl-controls">
        <div className="wl-seg wl-nav" role="group" aria-label="Move through history">
          <Button
            aria-label="Earlier"
            disabled={offset.value + range.value >= maxMinutes.value}
            onClick={() => shift(1)}
          >
            <Chevron left />
          </Button>
          <Button aria-label="Later" disabled={live} onClick={() => shift(-1)}>
            <Chevron />
          </Button>
          <Button disabled={live} onClick={goNow}>
            Now
          </Button>
        </div>
        <SegmentedControl
          className="wl-seg wl-range"
          aria-label="History range"
          value={preset}
          onValueChange={(next) => {
            if (next) choosePreset(Number(next));
          }}
        >
          <SegmentedControlIndicator className="wl-seg-indicator" />
          {ranges.value.map((r) => (
            <SegmentedControlItem key={r.minutes} value={String(r.minutes)}>
              {r.label}
            </SegmentedControlItem>
          ))}
        </SegmentedControl>
        <Button
          className="wl-button wl-settings"
          disabled={!can.value.configure}
          onClick={openSettings}
        >
          Settings
        </Button>
      </div>
      <p className="wl-window wl-muted">
        <span className={'wl-live' + (live ? ' wl-live-on' : '')}>
          {live ? 'Live' : 'Historical'}
        </span>{' '}
        <span className="wl-num">{windowText()}</span> · {scaleNote()}
      </p>
    </header>
  );
}
