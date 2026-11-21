import './layer.css';
import '@ui-sdk/core/web.css';
import './uplot.css';
import './styles.css';
import { Button } from '@ui-sdk/react/button';
import { Tabs, TabsIndicator, TabsList, TabsPanel, TabsTab } from '@ui-sdk/react/tabs';
import { Component, useEffect, useRef, type ReactNode } from 'react';
import { themeKey } from './chart.tsx';
import { Header } from './header.tsx';
import { SettingsDrawer } from './settings.tsx';
import { useStore } from './signal.ts';
import {
  bundleVersion,
  can,
  compat,
  configureReason,
  error,
  notice,
  say,
  isPreview,
  level,
  loading,
  readError,
  readOnly,
  ready,
  reloadPage,
  retry,
  setView,
  showCompat,
  staleBundle,
  start,
  state,
  statusLine,
  stop,
  view,
  views,
  type ViewId,
} from './store.ts';
import type { DataSource } from './sources/source.ts';
import { UpdatePanel } from './update-panel.tsx';
import { EventsView } from './views/events.tsx';
import { OverviewView } from './views/overview.tsx';
import { PathsView } from './views/paths.tsx';
import { RouterView } from './views/router.tsx';
import { TrafficView } from './views/traffic.tsx';
import { WifiView } from './views/wifi.tsx';

function Features() {
  const list = compat.value.features;
  if (!list.length) return null;
  return (
    <ul className="wl-features" aria-label="Features">
      {list.map((f) => (
        <li key={f.name}>
          {f.name}: <strong>{f.available ? 'Available' : 'Off'}</strong>
        </li>
      ))}
    </ul>
  );
}

function Compat() {
  const c = compat.value;
  if (level.value === 'unsupported')
    return (
      <section className="wl-panel wl-compat wl-compat-unsupported" role="alert">
        <h2>This firmware is not supported.</h2>
        <p>
          <strong>Firmware {c.firmware || 'unknown'}</strong>
        </p>
        <p>{c.message}</p>
        <Features />
        <p>Update Wayline below. A newer release may support this firmware.</p>
        <p className="wl-muted">Run wayline-ctl doctor on the router and send the output.</p>
        <UpdatePanel compact />
      </section>
    );
  if (!showCompat.value) return null;
  return (
    <div
      className={
        'wl-panel wl-compat ' +
        (level.value === 'monitoring' ? 'wl-compat-monitoring' : 'wl-compat-note')
      }
      role="status"
    >
      <p>
        <strong>Firmware {c.firmware || 'unknown'}</strong> {c.message}
      </p>
      <Features />
      <UpdatePanel compact />
    </div>
  );
}

function Messages() {
  const alert = useRef<HTMLDivElement>(null);
  const message = error.value;
  useEffect(() => {
    if (message) alert.current?.scrollIntoView?.({ block: 'nearest' });
  }, [message]);
  return (
    <div role="alert">
      {!ready.value && readError.value && (
        <div className="wl-panel wl-fail">
          <h2>Wayline could not read the tunnel state.</h2>
          <p className="wl-fail-text">{readError.value}</p>
          <p className="wl-muted">Run wayline-ctl doctor on the router and send the output.</p>
          <Button className="wl-button" disabled={loading.value} onClick={retry}>
            Retry
          </Button>
          <UpdatePanel compact />
        </div>
      )}
      {(message || (ready.value && readError.value)) && (
        <div ref={alert} className="wl-error">
          <span>{message || readError.value}</span>
          {message && (
            <Button className="wl-link" onClick={() => (error.value = '')}>
              Dismiss
            </Button>
          )}
        </div>
      )}
    </div>
  );
}

function Notice() {
  return (
    <div role="status">
      {notice.value && (
        <div className="wl-notice">
          <span>{notice.value}</span>
          <Button className="wl-link" onClick={() => say('')}>
            Dismiss
          </Button>
        </div>
      )}
    </div>
  );
}

const screens: Record<ViewId, () => ReactNode> = {
  overview: OverviewView,
  paths: PathsView,
  wifi: WifiView,
  traffic: TrafficView,
  router: RouterView,
  events: EventsView,
};

const luminance = (color: string): number => {
  const parts = (color.match(/[\d.]+/g) || []).slice(0, 4).map(Number);
  if (parts.length < 3 || parts[3] === 0) return 1;
  const [r, g, b] = parts.map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

const motion = (): boolean =>
  typeof document.startViewTransition === 'function' &&
  !matchMedia('(prefers-reduced-motion: reduce)').matches;

function switchView(next: ViewId): void {
  if (next === view.value) return;
  if (!motion()) {
    setView(next);
    return;
  }
  document.startViewTransition(() => {
    setView(next);
    return new Promise<void>((resolve) => setTimeout(resolve, 0));
  });
}

function Dashboard() {
  useStore();
  const probe = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const detect = () => {
      if (!probe.current) return;
      const color = getComputedStyle(probe.current).backgroundColor;
      themeKey.value = luminance(color) < 0.2 ? 'dark' : 'light';
    };
    detect();
    const media = matchMedia('(prefers-color-scheme: dark)');
    media.addEventListener('change', detect);
    return () => media.removeEventListener('change', detect);
  }, []);
  const s = state.value;
  const active = ready.value && level.value !== 'unsupported';
  return (
    <main className="wl" data-scheme={themeKey.value || undefined}>
      <span ref={probe} className="wl-probe" aria-hidden="true" />
      {isPreview.value && (
        <div className="wl-preview">Interactive preview with sample data. No router changes.</div>
      )}
      <Compat />
      <Header />
      {staleBundle.value && (
        <div className="wl-notice" role="status">
          <span>
            This page is from Wayline {bundleVersion}, but the router runs {s.version}.
          </span>
          <Button className="wl-button" onClick={reloadPage}>
            Reload page
          </Button>
        </div>
      )}
      <p className="wl-statusline wl-muted" role="status">
        {statusLine.value}
      </p>
      {!can.value.configure && ready.value && (
        <p className="wl-muted wl-hint">
          {readOnly.value
            ? 'Read-only view. This data source accepts no changes.'
            : configureReason.value}
        </p>
      )}
      <Messages />
      <Notice />
      {s.monitoringIssue && (
        <p className="wl-notice" role="alert">
          {s.monitoringIssue}
        </p>
      )}
      {active && (
        <Tabs
          className="wl-tabs-root"
          value={view.value}
          onValueChange={(next) => {
            if (next) switchView(next as ViewId);
          }}
        >
          <TabsList className="wl-tabs" aria-label="Views">
            {views.map((v) => (
              <TabsTab key={v.id} value={v.id}>
                {v.label}
              </TabsTab>
            ))}
            <TabsIndicator className="wl-tabs-indicator" />
          </TabsList>
          {views.map((v) => {
            const Screen = screens[v.id];
            return (
              <TabsPanel key={v.id} value={v.id} className="wl-view">
                <Screen />
              </TabsPanel>
            );
          })}
        </Tabs>
      )}
      {!ready.value && !readError.value && (
        <p className="wl-muted wl-loading">Reading tunnel state…</p>
      )}
      <SettingsDrawer />
      <footer className="wl-footnote">
        <a href="https://github.com/epk/wayline" target="_blank" rel="noopener noreferrer">
          Wayline
        </a>
        <a
          href={'https://github.com/epk/wayline/releases/tag/v' + (s.version || bundleVersion)}
          target="_blank"
          rel="noopener noreferrer"
        >
          {s.version || bundleVersion}
        </a>
        <a
          href="https://github.com/epk/wayline/blob/main/docs/metrics.md"
          target="_blank"
          rel="noopener noreferrer"
        >
          How Wayline measures
        </a>
      </footer>
    </main>
  );
}

class Boundary extends Component<{ children: ReactNode }, { failure: string }> {
  state = { failure: '' };
  static getDerivedStateFromError(err: unknown) {
    const message =
      err && typeof err === 'object' && 'message' in err ? String(err.message) : String(err);
    return { failure: message || 'Unknown error.' };
  }
  render() {
    if (this.state.failure)
      return (
        <main className="wl">
          <h1>Wayline</h1>
          <p>The Wayline page failed to draw.</p>
          <p className="wl-error" role="alert">
            {this.state.failure}
          </p>
          <p>Run wayline-ctl doctor on the router and send the output.</p>
        </main>
      );
    return this.props.children;
  }
}

export function App(props: { source: DataSource; preview?: boolean }) {
  const { source, preview = false } = props;
  useEffect(() => {
    start(source, preview);
    return stop;
  }, [source, preview]);
  return (
    <Boundary>
      <Dashboard />
    </Boundary>
  );
}
