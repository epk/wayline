export type Num = number | null;
export type Column = ArrayLike<Num>;

export interface Settings {
  enabled: boolean;
  interval: number;
  tunnels: string[];
  targets: string[];
  directWan: boolean;
  historyDays: number;
  autoUpdates: boolean;
  autoExit: boolean;
  autoTunnels: string[];
  endpointDiagnostics: boolean;
  upstreamWifi: boolean;
  telemetry: boolean;
}

export interface Tunnel {
  id: string;
  monitorId: string;
  name: string;
  enabled: boolean;
  connected: boolean;
  historyAvailable?: boolean;
  controllable: boolean;
  preferred: boolean;
  selectable?: boolean;
  selectReason?: string;
  eligible: boolean;
  reason?: string;
  stateLabel: string;
  measurementState?: string | null;
  age?: string;
  median: Num;
  jitter?: Num;
  samples: number;
  lost: number;
}

export interface Uplink {
  id: string;
  interface: string | null;
  device: string | null;
  address: string | null;
  active: boolean;
  state: string | null;
  median: Num;
  jitter?: Num;
  samples?: number;
  lost?: number;
}

export interface ChangeEvent {
  at: number;
  subject: string;
  kind: string;
  text: string;
  seq?: number;
}

export interface Incident {
  subject: string;
  name: string;
  kind: string;
  start: number;
  end: Num;
  detail: string;
}

export interface Feature {
  name: string;
  available: boolean;
}

export interface Compat {
  level?: string;
  verified?: boolean;
  firmware?: string;
  message?: string;
  features?: Feature[];
}

export interface UpdateStatus {
  current?: string;
  latest?: string | null;
  available?: boolean;
  phase?: string;
  message?: string;
  failed?: boolean;
  needsAttention?: boolean;
  checkedAt?: number;
  updatedAt?: number;
}

export interface EgressPath {
  id: string;
  name?: string;
  interface?: string | null;
  ip?: string | null;
  location?: string | null;
  error?: string | null;
  verified?: boolean;
}

export interface Tier {
  step: number;
  keptSeconds: number;
}

export interface Rate {
  kbps?: Num;
  mode?: string | null;
  mcs?: Num;
  nss?: Num;
  mhz?: Num;
  short_gi?: boolean | null;
}

export interface WifiLink {
  ssid: string;
  bssid: string;
  channel?: Num;
  frequency?: Num;
  width?: Num;
  htmode?: string | null;
  hwmode?: string | null;
  encryption?: string | null;
  hardware?: string | null;
  txpower?: Num;
  signal?: Num;
  signal_avg?: Num;
  noise?: Num;
  quality?: Num;
  quality_max?: Num;
  tx: Rate;
  rx: Rate;
  expected_kbps?: Num;
  connected_seconds?: Num;
  inactive_ms?: Num;
  busy_permille?: Num;
  radio_frequency?: Num;
}

export interface LiveStat {
  median: Num;
  jitter: Num;
  samples: number;
  lost: number;
}

export interface WifiView {
  enabled: boolean;
  radio?: boolean;
  state?: string | null;
  reason?: string;
  interface?: string | null;
  candidates?: string[];
  sampleAgeMs?: Num;
  interval?: number;
  link?: WifiLink | null;
  uplink?: string | null;
  historyAvailable?: boolean;
}

export interface SystemSample {
  cpu: Num;
  load: number[] | null;
  memTotal: Num;
  memAvailable: Num;
  temperature: Num;
  conntrack: Num;
  conntrackMax: Num;
  uptime: Num;
}

export interface InterfaceSample {
  id: string;
  name: string;
  device: string;
  kind: string;
  rxBps: Num;
  txBps: Num;
  rxBytes: Num;
  txBytes: Num;
  errors: Num;
  drops: Num;
  handshakeAge: Num;
  endpoint: string | null;
  present?: boolean;
}

export interface Telemetry {
  enabled: boolean;
  sampleAgeMs?: Num;
  interval?: number;
  system: SystemSample | null;
  interfaces: InterfaceSample[];
}

export interface State {
  version: string;
  config: Settings;
  history?: { days?: number; tiers?: Tier[] };
  tunnels: Tunnel[];
  diagnostics: Record<string, Record<string, Num>>;
  live?: Record<string, Record<string, LiveStat>>;
  endpoints: { tunnel: string; name: string; address: string | null; reason: string }[];
  wan: { available: boolean; interface: string | null; reason: string };
  uplinks: Uplink[];
  events: ChangeEvent[];
  incidents: Incident[];
  resolved: Record<string, string | null>;
  auto: {
    enabled: boolean;
    last: { at: number; from: string; to: string; reason: string } | null;
    needsAttention?: boolean;
    paused?: boolean;
  };
  egress?: { checkedAt?: number; paths?: EgressPath[] };
  revision: string;
  scope: string;
  capabilities: {
    configure: boolean;
    select: boolean;
    control: boolean;
    update?: boolean;
    reason?: string;
  };
  compat: Compat;
  monitoringIssue?: string | null;
  update?: UpdateStatus;
  probe?: { interval?: number; timeout?: number };
  wifi: WifiView;
  telemetry: Telemetry;
  device?: string;
  clock?: { synced: boolean; source: string | null; pending: boolean };
}

export interface Doc {
  start: number;
  step: number;
  count: number;
}

export interface LatencySeries {
  id: string;
  name: string;
  median: Column;
  min: Column;
  max: Column;
  jitter: Column;
  p50?: Column;
  p95?: Column;
  p99?: Column;
  sent: Column;
  lost: Column;
}

export interface LatencyDoc extends Doc {
  series: LatencySeries[];
  events: ChangeEvent[];
  incidents: Incident[];
}

export type WifiKey =
  | 'samples'
  | 'uptime'
  | 'signal'
  | 'signalMin'
  | 'signalMax'
  | 'noise'
  | 'snr'
  | 'txRate'
  | 'txRateMin'
  | 'rxRate'
  | 'expected'
  | 'retries'
  | 'failures'
  | 'busy'
  | 'rxAir'
  | 'txAir'
  | 'upload'
  | 'download'
  | 'txBytes'
  | 'rxBytes'
  | 'disconnects'
  | 'roams';

/** One access point's share of a range, from get_wifi_history. */
export interface WifiNetwork {
  /** `ap:<12 hex digits>`, or `upstream` for history from before access points were told apart. */
  id: string;
  bssid: string | null;
  ssid: string | null;
  channel: string | null;
  connectedSeconds: number;
  first: number;
  last: number;
  summary: Partial<Record<WifiKey, Num>>;
}

export interface WifiDoc extends Doc {
  summary: Partial<Record<WifiKey, Num>>;
  columns: Partial<Record<WifiKey, Column>>;
  events: ChangeEvent[];
  networks: WifiNetwork[];
}

export interface TelemetryInterface {
  id: string;
  name: string;
  device: string;
  kind: string;
  rxBps: Column;
  txBps: Column;
  rxPeakBps: Column;
  txPeakBps: Column;
  rxBytes: Column;
  txBytes: Column;
  errors: Column;
  drops: Column;
  handshakeAgeMax: Column;
}

export interface TelemetryDoc extends Doc {
  system: {
    cpu: Column;
    cpuMax: Column;
    load: Column;
    memUsed: Column;
    memAvailableMin: Column;
    temperature: Column;
    temperatureMax: Column;
    conntrack: Column;
    conntrackMax: Num;
  };
  interfaces: TelemetryInterface[];
}
