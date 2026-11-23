import './layer.css';
import '@ui-sdk/core/web.css';
import './styles.css';
import { Button } from '@ui-sdk/react/button';
import { Tabs, TabsIndicator, TabsList, TabsPanel, TabsTab } from '@ui-sdk/react/tabs';
import {
  Component,
  lazy,
  Suspense,
  useEffect,
  useRef,
  type ComponentType,
  type ReactNode,
} from 'react';
import { Header } from './header.tsx';
import { loadChunk, prefetch, startAgentBridge, type ChunkName } from './runtime.ts';
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
  settingsUsed,
  themeKey,
  shownViews,
  view,
  type ViewId,
} from './store.ts';
import type { DataSource } from './sources/source.ts';
import { UpdatePanel } from './update-panel.tsx';
import { OverviewView } from './views/overview.tsx';

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

// React keeps a failed lazy() rejected forever. After a failure, the next
// render following the view boundary's Try again builds a fresh one, which
// downloads the chunk again.
let attempt = 0;
const chunk = (name: ChunkName, exported: string): ComponentType => {
  let Current: ComponentType | null = null;
  let made = 0;
  let failed = false;
  return () => {
    if (!Current || (failed && made !== attempt)) {
      made = attempt;
      failed = false;
      Current = lazy(() =>
        loadChunk(name).then(
          (module) => ({ default: module[exported] as ComponentType }),
          (error: unknown) => {
            failed = true;
            throw error;
          },
        ),
      );
    }
    return <Current />;
  };
};

const screens: Record<ViewId, ComponentType> = {
  overview: OverviewView,
  paths: chunk('paths', 'PathsView'),
  wifi: chunk('wifi', 'WifiView'),
  traffic: chunk('traffic', 'TrafficView'),
  router: chunk('router', 'RouterView'),
  events: chunk('events', 'EventsView'),
  health: chunk('health', 'HealthView'),
};

const Settings = chunk('settings', 'SettingsDrawer');

class ViewBoundary extends Component<{ children: ReactNode }, { failure: string }> {
  state = { failure: '' };
  static getDerivedStateFromError(err: unknown) {
    return { failure: err instanceof Error ? err.message : String(err) };
  }
  render() {
    if (!this.state.failure) return this.props.children;
    return (
      <div className="wl-panel wl-fail" role="alert">
        <h2>This view could not be loaded.</h2>
        <p className="wl-fail-text">{this.state.failure}</p>
        <div className="wl-actions">
          <Button
            className="wl-button"
            onClick={() => {
              attempt++;
              this.setState({ failure: '' });
            }}
          >
            Try again
          </Button>
          <Button className="wl-button" onClick={reloadPage}>
            Reload page
          </Button>
        </div>
      </div>
    );
  }
}

const pending = <p className="wl-muted wl-loading">Loading view…</p>;

const luminance = (color: string): number => {
  const parts = (color.match(/[\d.]+/g) || []).slice(0, 4).map(Number);
  if (parts.length < 3 || parts[3] === 0) return 1;
  const [r, g, b] = parts.map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

// The page sits on the host's background, so its text must suit that
// background, not the OS preference. The first opaque ancestor decides.
const hostBackground = (from: Element): string => {
  for (let node = from.closest('.wl')?.parentElement; node; node = node.parentElement) {
    const color = getComputedStyle(node).backgroundColor;
    const alpha = (color.match(/[\d.]+/g) || []).map(Number)[3];
    if (color && color !== 'transparent' && alpha !== 0) return color;
  }
  return '';
};

// The new view fades in with CSS. A view transition here left iOS Safari
// sending taps to the wrong place once the admin page was scrolled.
function switchView(next: ViewId): void {
  if (next !== view.value) setView(next);
}

function Dashboard() {
  useStore();
  const probe = useRef<HTMLSpanElement>(null);
  useEffect(prefetch, []);
  useEffect(() => {
    const host = probe.current?.closest('.wl');
    return host ? startAgentBridge(host) : undefined;
  }, []);
  const current = view.value;
  const shown = ready.value;
  useEffect(() => {
    probe.current
      ?.closest('.wl')
      ?.querySelector('.wl-tabs [data-selected]')
      // container: 'nearest' scrolls only the tab row, not the admin page.
      ?.scrollIntoView({
        block: 'nearest',
        inline: 'nearest',
        container: 'nearest',
      } as ScrollIntoViewOptions);
  }, [current, shown]);
  useEffect(() => {
    const detect = () => {
      if (!probe.current) return;
      const color =
        hostBackground(probe.current) || getComputedStyle(probe.current).backgroundColor;
      themeKey.value = luminance(color) < 0.2 ? 'dark' : 'light';
    };
    detect();
    const media = matchMedia('(prefers-color-scheme: dark)');
    media.addEventListener('change', detect);
    // GL.iNet switches its theme by changing classes or styles on the root
    // elements, without touching the OS preference.
    const watcher = new MutationObserver(detect);
    for (const node of [document.documentElement, document.body])
      watcher.observe(node, {
        attributes: true,
        attributeFilter: ['class', 'style', 'data-theme'],
      });
    return () => {
      media.removeEventListener('change', detect);
      watcher.disconnect();
    };
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
      {s.clock && !s.clock.synced && (
        <p className="wl-notice wl-clock" role="status">
          Router clock not yet synced, times are provisional.
          {s.clock.pending && ' Data recorded before sync will be re-timed and stored.'}
        </p>
      )}
      {!can.value.configure && ready.value && (
        <p className="wl-muted wl-hint">{configureReason.value}</p>
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
          value={shownViews.value.some((v) => v.id === view.value) ? view.value : 'overview'}
          onValueChange={(next) => {
            if (next) switchView(next as ViewId);
          }}
        >
          <TabsList className="wl-tabs" aria-label="Views">
            {shownViews.value.map((v) => (
              <TabsTab key={v.id} value={v.id}>
                {v.label}
              </TabsTab>
            ))}
            <TabsIndicator className="wl-tabs-indicator" />
          </TabsList>
          {shownViews.value.map((v) => {
            const Screen = screens[v.id];
            return (
              <TabsPanel key={v.id} value={v.id} className="wl-view">
                <ViewBoundary>
                  <Suspense fallback={pending}>
                    <Screen />
                  </Suspense>
                </ViewBoundary>
              </TabsPanel>
            );
          })}
        </Tabs>
      )}
      {!ready.value && !readError.value && (
        <p className="wl-muted wl-loading">Reading tunnel state…</p>
      )}
      {settingsUsed.value && (
        <ViewBoundary>
          <Suspense fallback={null}>
            <Settings />
          </Suspense>
        </ViewBoundary>
      )}
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
