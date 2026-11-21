import type { DataSource, WriteCommand } from './sources/source.ts';
import { computed, signal, type Signal } from './signal.ts';
import { finite } from './format.ts';
import { legacyStep, offered, stepFor } from './series.ts';
import type {
  LatencyDoc,
  Settings,
  State,
  TelemetryDoc,
  Tunnel,
  UpdateStatus,
  WifiDoc,
} from './types.ts';

export const defaults = (): Settings => ({
  enabled: false,
  interval: 5,
  tunnels: [],
  targets: ['1.1.1.1', '8.8.8.8', '9.9.9.9'],
  directWan: false,
  historyDays: 7,
  autoUpdates: false,
  autoExit: false,
  autoTunnels: [],
  endpointDiagnostics: false,
  upstreamWifi: false,
  telemetry: false,
});

export const settingsFrom = (source: Partial<Settings>): Settings => {
  const settings = defaults() as unknown as Record<string, unknown>;
  const from = source as Record<string, unknown>;
  Object.keys(settings).forEach((key) => {
    const value = from[key];
    if (value !== undefined) settings[key] = Array.isArray(value) ? value.slice() : value;
  });
  return settings as unknown as Settings;
};

export const validTarget = (value: unknown): boolean =>
  typeof value === 'string' &&
  value.length > 0 &&
  value.length <= 253 &&
  /^[A-Za-z0-9.-]+$/.test(value);

export const storage = {
  get(key: string): string {
    try {
      return window.localStorage.getItem(key) || '';
    } catch {
      return '';
    }
  },
  set(key: string, value: string): void {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      return;
    }
  },
};

export type ViewId = 'overview' | 'paths' | 'wifi' | 'traffic' | 'router' | 'events';
export const views: { id: ViewId; label: string }[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'paths', label: 'Paths' },
  { id: 'wifi', label: 'Wi-Fi' },
  { id: 'traffic', label: 'Traffic' },
  { id: 'router', label: 'Router' },
  { id: 'events', label: 'Events' },
];
const viewKey = 'wayline.view';
const targetKey = 'wayline.target';
const historyInterval = 30000;
export const updatePhases = ['checking', 'downloading', 'verifying', 'installing'];
export const presets = [
  { label: '15m', minutes: 15 },
  { label: '1h', minutes: 60 },
  { label: '6h', minutes: 360 },
  { label: '24h', minutes: 1440 },
  { label: '7d', minutes: 10080 },
  { label: '30d', minutes: 43200 },
];

const emptyState = (): State => ({
  version: '',
  config: defaults(),
  tunnels: [],
  diagnostics: {},
  endpoints: [],
  wan: { available: true, interface: null, reason: '' },
  uplinks: [],
  events: [],
  incidents: [],
  resolved: {},
  auto: { enabled: false, last: null },
  revision: '',
  scope: '',
  capabilities: { configure: false, select: false, control: false },
  compat: { level: 'full', firmware: '', message: '' },
  wifi: { enabled: false },
  telemetry: { enabled: false, system: null, interfaces: [] },
});

const list = <T>(value: unknown): T[] => (Array.isArray(value) ? (value as T[]) : []);

export const normalize = (raw: unknown): State => {
  if (!raw || typeof raw !== 'object') throw Error('The router sent an empty state.');
  const next = raw as State;
  next.tunnels = list(next.tunnels);
  const config = (next.config || {}) as Partial<Settings>;
  config.tunnels = list(config.tunnels);
  config.autoTunnels = list(config.autoTunnels);
  next.capabilities = Object.assign(
    { configure: false, select: false, control: false },
    next.capabilities,
  );
  next.config = Object.assign(defaults(), config);
  if (!Array.isArray(next.config.targets) || !next.config.targets.length)
    next.config.targets = defaults().targets;
  next.auto = next.auto || { enabled: false, last: null };
  next.endpoints = list(next.endpoints);
  next.uplinks = list(next.uplinks);
  next.events = list(next.events);
  next.incidents = list(next.incidents);
  next.resolved = next.resolved || {};
  next.diagnostics = next.diagnostics || {};
  next.compat = next.compat || {};
  next.wan = Object.assign({ available: true, interface: null, reason: '' }, next.wan);
  next.wifi = Object.assign({ enabled: false }, next.wifi);
  next.telemetry = Object.assign({ enabled: false, system: null }, next.telemetry);
  next.telemetry.interfaces = list(next.telemetry.interfaces);
  return next;
};

export const state = signal<State>(emptyState());
export const ready = signal(false);
export const readError = signal('');
export const loading = signal(false);
export const error = signal('');
export const notice = signal('');
export const busy = signal(false);
export const busyTunnel = signal('');
export const busyAction = signal('');
export const pendingStop = signal('');
export const purgeConfirm = signal(false);
export const updateError = signal('');
export const updateWaiting = signal(false);
export const settingsOpen = signal(false);
export const settingsUsed = signal(false);
export const draft = signal<Settings>(defaults());
export const isPreview = signal(false);
export const clock = signal(Date.now());
export const themeKey = signal<'light' | 'dark' | ''>('');

const storedView = storage.get(viewKey) as ViewId;
export const view = signal<ViewId>(
  views.some((v) => v.id === storedView) ? storedView : 'overview',
);
export const range = signal(60);
export const preset = signal(60);
export const offset = signal(0);
export const anchor = signal(Math.floor(Date.now() / 60000) * 60000);
export const selectedTarget = signal(storage.get(targetKey) || '1.1.1.1');
export const selectedTunnel = signal('');
export const isolated = signal<string[]>([]);
export const cursor = signal(-1);
export const eventSubject = signal('');

export interface Loaded<T> {
  doc: T | null;
  error: string;
  loading: boolean;
  loaded: boolean;
  at: number;
  key: string;
}
const blank = <T>(): Loaded<T> => ({
  doc: null,
  error: '',
  loading: false,
  loaded: false,
  at: 0,
  key: '',
});
export const latencyData = signal<Loaded<LatencyDoc>>(blank());
export const wifiData = signal<Loaded<WifiDoc>>(blank());
export const telemetryData = signal<Loaded<TelemetryDoc>>(blank());

export const compat = computed(() => {
  const c = state.value.compat || {};
  const level = c.level === 'monitoring' || c.level === 'unsupported' ? c.level : 'full';
  return {
    level,
    verified: c.verified !== false,
    firmware: c.firmware || '',
    message: c.message || '',
    features: Array.isArray(c.features) ? c.features.filter((f) => f && f.name) : [],
  };
});
export const level = computed(() => compat.value.level);
export const showCompat = computed(() => level.value !== 'full' || !compat.value.verified);
export const can = computed(() => {
  const c = state.value.capabilities || {};
  const full = level.value === 'full';
  return {
    configure: !!c.configure && level.value !== 'unsupported',
    select: !!c.select && full,
    control: !!c.control && full,
  };
});
export const configureReason = computed(() =>
  level.value === 'unsupported'
    ? 'This firmware is not supported. Settings are unavailable.'
    : 'Settings are unavailable. You do not have permission to change them.',
);
export const selectReason = computed(() => {
  if (level.value === 'monitoring')
    return "This firmware's VPN module is not verified. Monitoring still works.";
  if (level.value === 'unsupported')
    return 'This firmware is not supported. Use exit is unavailable.';
  return state.value.capabilities.reason || '';
});
export const controlReason = computed(() => {
  if (level.value === 'monitoring')
    return "This firmware's VPN module is not verified. Monitoring still works.";
  if (level.value === 'unsupported')
    return 'This firmware is not supported. Start and Stop are unavailable.';
  return 'Start and Stop are unavailable.';
});
export const bundleVersion = WAYLINE_VERSION;
export const staleBundle = computed(
  () => Boolean(state.value.version) && state.value.version !== WAYLINE_VERSION,
);
export const statusLine = computed(() => {
  if (!ready.value)
    return readError.value ? 'The tunnel state is not available.' : 'Reading tunnel state…';
  if (readError.value) return 'Showing the last known state. Trying again.';
  return 'Tunnel state is up to date.';
});

export const tiers = computed(() =>
  list<{ step: number; keptSeconds: number }>(state.value.history?.tiers),
);
export const maxMinutes = computed(() => {
  const seconds = Math.max(86400, ...tiers.value.map((t) => Number(t.keptSeconds) || 0));
  return Math.floor(seconds / 60);
});
export const ranges = computed(() =>
  presets.filter((r) =>
    tiers.value.length ? offered(tiers.value, r.minutes) : r.minutes <= maxMinutes.value,
  ),
);
export const step = computed(() =>
  tiers.value.length
    ? stepFor(tiers.value, range.value, offset.value) || legacyStep(range.value)
    : legacyStep(range.value),
);
export const tier = computed(() => tiers.value.find((t) => t.step === step.value) || null);
export const windowStart = computed(() => anchor.value - (offset.value + range.value) * 60000);
export const windowEnd = computed(() => windowStart.value + range.value * 60000);

export const destinations = computed(() => state.value.config.targets);
export const primary = computed(() => state.value.config.targets[0]);
export const primarySelected = computed(() => selectedTarget.value === primary.value);
export const exitTunnel = computed(() => {
  const t = state.value.tunnels.find((item) => item.preferred);
  return t && t.enabled && t.connected ? t : null;
});
export const wanAvailable = computed(() => !!state.value.wan.available);

export const updateState = computed(() => {
  const u: UpdateStatus = state.value.update || {};
  return {
    current: u.current || state.value.version || '',
    latest: u.latest || null,
    available: !!u.available,
    phase: u.phase || 'idle',
    message: updateError.value || u.message || '',
    failed: !!updateError.value || !!u.failed,
    checkedAt: u.checkedAt || 0,
  };
});
export const updateActive = computed(() => updatePhases.includes(state.value.update?.phase || ''));
export const updatesDisabled = computed(() => state.value.capabilities.update === false);

let source: DataSource | null = null;
let stopped = true;
let timer: ReturnType<typeof setTimeout> | undefined;
let updateTimer: ReturnType<typeof setInterval> | undefined;
let updatePolling = false;
let updateLostSince = 0;
let stateRead: Promise<boolean> | null = null;
const counters = { latency: 0, wifi: 0, telemetry: 0 };
const pending = { latency: '', wifi: '', telemetry: '' };

export const describe = (e: unknown, fallback: string): string => {
  if (typeof e === 'string' && e) return e;
  if (e && typeof e === 'object' && 'message' in e && e.message) return String(e.message);
  return fallback;
};

const write = <K extends WriteCommand>(command: K): NonNullable<DataSource[K]> => {
  if (!source || typeof source[command] !== 'function')
    throw Error('This firmware does not support that change.');
  const run = source[command] as unknown as (...args: unknown[]) => unknown;
  return run.bind(source) as unknown as NonNullable<DataSource[K]>;
};

const read = (): DataSource => {
  if (!source)
    throw Error('The router page API is missing. Reload the page or update the firmware.');
  return source;
};

export function refresh(): Promise<boolean> {
  if (stopped || document.hidden) return Promise.resolve(false);
  if (stateRead) return stateRead;
  stateRead = refreshState().finally(() => {
    stateRead = null;
  });
  return stateRead;
}

async function refreshState(): Promise<boolean> {
  clearTimeout(timer);
  loading.value = true;
  try {
    const next = normalize(await read().getState());
    if (stopped) return false;
    const previous = state.value.tunnels;
    const renamed = next.tunnels.some(
      (row) =>
        row.monitorId &&
        previous.some((old) => old.monitorId === row.monitorId && old.id !== row.id),
    );
    if (renamed) {
      counters.latency++;
      pending.latency = '';
      latencyData.value = { ...latencyData.value, loading: false, at: 0 };
    }
    {
      state.value = next;
      ready.value = true;
      readError.value = '';
      clock.value = Date.now();
      if (!next.config.targets.includes(selectedTarget.value))
        selectedTarget.value = next.config.targets[0];
      if (!next.tunnels.some((t) => t.id === selectedTunnel.value))
        selectedTunnel.value = (
          next.tunnels.find((t) => t.preferred) ||
          next.tunnels[0] || { id: '' }
        ).id;
    }
    if (updateActive.value && !updateTimer) startUpdatePoll();
    loadHistories(false);
    return true;
  } catch (e) {
    readError.value = 'Wayline is unavailable. ' + describe(e, 'Try again shortly.');
    return false;
  } finally {
    loading.value = false;
    if (!stopped && !document.hidden) timer = setTimeout(refresh, 5000);
  }
}

export function retry(): void {
  clearTimeout(timer);
  void refresh();
}

function visibility(): void {
  clearTimeout(timer);
  if (!document.hidden) void refresh();
}

type Kind = keyof typeof counters;

async function load<T>(
  kind: Kind,
  target: Signal<Loaded<T>>,
  fetch: () => Promise<T>,
  key: string,
  shape: (doc: T) => T,
): Promise<void> {
  const request = ++counters[kind];
  pending[kind] = key;
  target.value = { ...target.value, loading: true };
  try {
    const doc = shape(await fetch());
    if (request !== counters[kind] || stopped) return;
    target.value = { doc, error: '', loading: false, loaded: true, at: Date.now(), key };
  } catch (e) {
    if (request !== counters[kind] || stopped) return;
    target.value = {
      doc: null,
      error: describe(e, 'History unavailable.'),
      loading: false,
      loaded: true,
      at: Date.now(),
      key,
    };
  } finally {
    if (request === counters[kind]) pending[kind] = '';
  }
}

const docShape =
  <T extends { start: number; step: number; count: number }>(fields: (doc: T) => void) =>
  (doc: T): T => {
    if (!doc || typeof doc !== 'object') throw Error('The router sent an empty document.');
    if (!finite(doc.start) || !finite(doc.step) || !finite(doc.count))
      throw Error('The router sent a history document without a time axis.');
    fields(doc);
    return doc;
  };

const latencyShape = docShape<LatencyDoc>((doc) => {
  doc.series = list(doc.series);
  doc.events = list(doc.events);
  doc.incidents = list(doc.incidents);
});
const wifiShape = docShape<WifiDoc>((doc) => {
  doc.summary = doc.summary || {};
  doc.columns = doc.columns || {};
  doc.events = list(doc.events);
});
const telemetryShape = docShape<TelemetryDoc>((doc) => {
  doc.system = doc.system || ({} as TelemetryDoc['system']);
  doc.interfaces = list(doc.interfaces);
});

export const historyViews: Record<'wifi' | 'telemetry', ViewId[]> = {
  wifi: ['overview', 'wifi', 'events'],
  telemetry: ['traffic', 'router'],
};

export function loadHistories(force: boolean): void {
  if (stopped || !ready.value || level.value === 'unsupported') return;
  if (offset.value === 0) anchor.value = Math.floor(Date.now() / 60000) * 60000;
  const s = state.value;
  const span = { start: windowStart.value, end: windowEnd.value, step: step.value };
  const due = <T>(kind: Kind, data: Loaded<T>, key: string) =>
    force ||
    (pending[kind] !== key && (data.key !== key || Date.now() - data.at >= historyInterval));
  if (s.tunnels.length || s.config.directWan) {
    const endpoints = !!s.config.endpointDiagnostics;
    const params = {
      target: selectedTarget.value,
      all: true,
      uplinks: true,
      ...span,
      include_wan: !!(s.config.directWan && s.wan.available),
      ...(endpoints ? { endpoints: true } : {}),
    };
    const key = JSON.stringify(params);
    if (due('latency', latencyData.value, key))
      void load(
        'latency',
        latencyData,
        () => read().getHistory({ tunnel: selectedTunnel.value, ...params }),
        key,
        latencyShape,
      );
  }
  const key = JSON.stringify(span);
  if (
    s.config.upstreamWifi &&
    historyViews.wifi.includes(view.value) &&
    due('wifi', wifiData.value, key)
  )
    void load('wifi', wifiData, () => read().getWifiHistory(span), key, wifiShape);
  if (
    s.config.telemetry &&
    historyViews.telemetry.includes(view.value) &&
    due('telemetry', telemetryData.value, key)
  )
    void load(
      'telemetry',
      telemetryData,
      () => read().getTelemetryHistory(span),
      key,
      telemetryShape,
    );
}

export function setView(next: ViewId): void {
  view.value = next;
  storage.set(viewKey, next);
  cursor.value = -1;
  loadHistories(false);
}

export function chooseTarget(target: string): void {
  selectedTarget.value = target;
  storage.set(targetKey, target);
  loadHistories(false);
}

export function setOffset(minutes: number): void {
  offset.value = Math.round(Math.min(maxMinutes.value - range.value, Math.max(0, minutes)));
  loadHistories(false);
}

export function shift(direction: number): void {
  setOffset(offset.value + (direction * range.value) / 2);
}

export function goNow(): void {
  setOffset(0);
}

export function choosePreset(minutes: number): void {
  preset.value = minutes;
  range.value = minutes;
  setOffset(0);
}

export function resetZoom(): void {
  choosePreset(preset.value);
}

export function zoom(from: number, to: number): void {
  const minutes = Math.round((to - from) / 60000);
  if (minutes < 5) return;
  range.value = Math.min(maxMinutes.value, minutes);
  setOffset(Math.round((anchor.value - to) / 60000));
}

export function highlight(id: string): void {
  selectedTunnel.value = id;
}

export function isolate(owner: string, tunnel: boolean, multi: boolean): void {
  const current = isolated.value;
  if (multi)
    isolated.value = current.includes(owner)
      ? current.filter((other) => other !== owner)
      : current.concat(owner);
  else isolated.value = current.length === 1 && current[0] === owner ? [] : [owner];
  if (tunnel) highlight(owner);
}

export function openSettings(): void {
  if (!can.value.configure) return;
  purgeConfirm.value = false;
  draft.value = settingsFrom(state.value.config);
  settingsUsed.value = true;
  settingsOpen.value = !settingsOpen.value;
}

export function closeSettings(): void {
  if (busy.value) return;
  purgeConfirm.value = false;
  settingsOpen.value = false;
}

export const updateDraft = (patch: Partial<Settings>): void => {
  draft.value = { ...draft.value, ...patch };
};

export function pruneAutomaticTunnels(): void {
  const d = draft.value;
  updateDraft({ autoTunnels: d.autoTunnels.filter((id) => d.tunnels.includes(id)) });
}

export async function saveSettings(): Promise<void> {
  if (busy.value) return;
  error.value = '';
  const targets = draft.value.targets;
  if (
    !targets.length ||
    targets.some((t) => !validTarget(t)) ||
    new Set(targets.map((t) => t.toLowerCase())).size !== targets.length
  ) {
    error.value =
      'Use an IPv4 address or a host name for each destination. Each destination must be unique.';
    return;
  }
  if (draft.value.enabled && !draft.value.tunnels.length) {
    error.value = 'Choose at least one tunnel to monitor.';
    return;
  }
  pruneAutomaticTunnels();
  if (draft.value.autoExit && draft.value.autoTunnels.length < 2) {
    error.value = 'Choose at least two monitored tunnels for automatic switching.';
    return;
  }
  busy.value = true;
  busyAction.value = 'save';
  let saved = false;
  try {
    await write('setConfig')({
      config: settingsFrom(draft.value),
      expected_revision: state.value.revision,
    });
    saved = true;
  } catch (e) {
    error.value = describe(e, 'Settings could not be saved.');
  } finally {
    busy.value = false;
    busyAction.value = '';
  }
  if (saved) {
    closeSettings();
    say('Settings saved.');
  }
  await refresh();
  if (!saved && settingsOpen.value) draft.value = settingsFrom(state.value.config);
}

export async function enableSetting(
  key: 'upstreamWifi' | 'telemetry' | 'directWan',
  text: string,
): Promise<void> {
  if (busy.value || !can.value.configure) return;
  busy.value = true;
  busyAction.value = 'enable';
  error.value = '';
  try {
    await write('setConfig')({
      config: Object.assign(settingsFrom(state.value.config), { [key]: true }),
      expected_revision: state.value.revision,
    });
    say(text);
  } catch (e) {
    error.value = describe(e, 'Settings could not be saved.');
  } finally {
    busy.value = false;
    busyAction.value = '';
  }
  await refresh();
}

export async function purgeHistory(): Promise<void> {
  if (busy.value || !purgeConfirm.value) return;
  busy.value = true;
  busyAction.value = 'purge';
  error.value = '';
  counters.latency++;
  try {
    await write('purgeHistory')({ confirm: true });
    counters.latency++;
    counters.wifi++;
    counters.telemetry++;
    pending.latency = pending.wifi = pending.telemetry = '';
    {
      latencyData.value = blank();
      wifiData.value = blank();
      telemetryData.value = blank();
      state.value = { ...state.value, events: [], incidents: [] };
      purgeConfirm.value = false;
    }
    say('Stored history was purged. New measurements will appear shortly.');
    loadHistories(true);
  } catch (e) {
    error.value = describe(e, 'History could not be purged.');
  } finally {
    busy.value = false;
    busyAction.value = '';
  }
}

export const selectable = (t: Tunnel): boolean =>
  t.selectable === undefined ? t.eligible : t.selectable;

export async function selectExit(t: Tunnel): Promise<void> {
  if (busy.value || t.preferred || !selectable(t) || !can.value.select) return;
  busy.value = true;
  busyTunnel.value = t.id;
  busyAction.value = 'select';
  error.value = '';
  say('Changing the exit to ' + t.name + '…');
  try {
    await write('selectTunnel')({ section_id: t.id, expected_revision: state.value.revision });
    say('Now using ' + t.name + ' as the exit.');
    await refresh();
  } catch {
    say('');
    if (stateRead) await stateRead;
    const fresh = await refresh();
    if (
      fresh &&
      state.value.tunnels.some(
        (row) => t.monitorId && row.monitorId === t.monitorId && row.preferred,
      )
    )
      say('Now using ' + t.name + ' as the exit.');
    else
      error.value =
        'Could not confirm the exit change. Refresh, then check the active exit before trying again.';
  } finally {
    busy.value = false;
    busyTunnel.value = '';
    busyAction.value = '';
  }
}

export async function setTunnelEnabled(t: Tunnel): Promise<void> {
  if (busy.value || !t.controllable || !can.value.control) return;
  busy.value = true;
  busyTunnel.value = t.id;
  busyAction.value = 'toggle';
  pendingStop.value = '';
  error.value = '';
  say((t.enabled ? 'Stopping ' : 'Starting ') + t.name + '…');
  try {
    const result = await write('setTunnelEnabled')({
      section_id: t.id,
      enabled: !t.enabled,
      expected_revision: state.value.revision,
    });
    let text = result.enabled
      ? t.name + ' is enabled. Connection and measurements can take time.'
      : t.name + ' is stopped. GL.iNet traffic policy applies.';
    if (result.monitor_pending) text += ' Monitoring will reconcile within one minute.';
    say(text);
  } catch {
    say('');
    error.value =
      'Could not confirm the change to ' +
      t.name +
      '. Refresh, then check its state before trying again.';
  } finally {
    await refresh();
    busy.value = false;
    busyTunnel.value = '';
    busyAction.value = '';
  }
}

export function askToggle(t: Tunnel): void {
  if (!t.enabled) {
    void setTunnelEnabled(t);
    return;
  }
  pendingStop.value = t.id;
}

let noticeTimer: ReturnType<typeof setTimeout> | undefined;

export function say(text: string): void {
  notice.value = text;
  clearTimeout(noticeTimer);
  if (text && !text.endsWith('…')) noticeTimer = setTimeout(() => (notice.value = ''), 10000);
}

export async function installUpdate(): Promise<void> {
  updateError.value = '';
  try {
    const r = await write('installUpdate')();
    setUpdate(r.update);
  } catch (e) {
    updateError.value = describe(e, 'Update unavailable.');
  }
}

export async function checkUpdates(): Promise<void> {
  updateError.value = '';
  try {
    const r = await write('checkUpdate')();
    setUpdate(r.update);
  } catch (e) {
    updateError.value = describe(e, 'Update check unavailable.');
  }
}

function setUpdate(update: UpdateStatus | undefined): void {
  if (!update || typeof update !== 'object') return;
  state.value = { ...state.value, update };
  if (updateActive.value) startUpdatePoll();
  else stopUpdatePoll();
}

function startUpdatePoll(): void {
  clearInterval(updateTimer);
  updateTimer = setInterval(pollUpdate, 1000);
}

function stopUpdatePoll(): void {
  clearInterval(updateTimer);
  updateTimer = undefined;
  updateWaiting.value = false;
  updateLostSince = 0;
}

async function pollUpdate(): Promise<void> {
  if (updatePolling || stopped) return;
  updatePolling = true;
  try {
    const next = (await read().getState()) as State;
    if (stopped || !next || !next.update) return;
    state.value = { ...state.value, update: next.update };
    updateWaiting.value = false;
    updateLostSince = 0;
    if (!updateActive.value) stopUpdatePoll();
  } catch {
    if (!updateLostSince) updateLostSince = Date.now();
    updateWaiting.value = true;
    if (Date.now() - updateLostSince >= 60000) {
      stopUpdatePoll();
      updateError.value =
        'Lost contact with the router. Refresh the page to see the update status.';
    }
  } finally {
    updatePolling = false;
  }
}

export function reloadPage(): void {
  void fetch('/views/gl-sdk4-ui-wayline.common.js', { cache: 'reload' }).finally(() =>
    window.location.reload(),
  );
}

export function start(next: DataSource, preview: boolean): void {
  source = next;
  isPreview.value = preview;
  stopped = false;
  document.addEventListener('visibilitychange', visibility);
  void refresh();
  if (updateActive.value) startUpdatePoll();
}

export function stop(): void {
  stopped = true;
  counters.latency++;
  counters.wifi++;
  counters.telemetry++;
  pending.latency = pending.wifi = pending.telemetry = '';
  clearTimeout(timer);
  stopUpdatePoll();
  document.removeEventListener('visibilitychange', visibility);
  clearTimeout(noticeTimer);
  source = null;
}
