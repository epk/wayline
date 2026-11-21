#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const HISTORY: usize = 64;
pub const MAX_SERIES: usize = 90;
pub const BUCKET_SECONDS: u64 = 15;
pub const MAX_STATUS: usize = 768 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Waiting,
    InterfaceUnavailable,
    InterfaceDown,
    InterfaceChanged,
    CheckingRoute,
    RouteQueryFailed,
    RouteQueryTimeout,
    RouteUnavailable,
    RouteMismatch,
    SocketSetupFailed,
    SendFailed,
    Probing,
    ReceiveFailed,
    ResponseLimitExceeded,
    Reachable,
    ProbeTimeout,
}

impl State {
    #[must_use]
    pub const fn live(self) -> bool {
        matches!(self, Self::Reachable | Self::Probing | Self::CheckingRoute)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub target: Ipv4Addr,
    #[serde(default = "default_interval")]
    pub interval_ms: u64,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    pub tunnels: Vec<Tunnel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi: Option<WifiConfig>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WifiConfig {
    #[serde(default = "default_wifi_interval")]
    pub interval_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tunnel {
    pub id: String,
    pub name: String,
    pub interface: String,
    pub source: Ipv4Addr,
    #[serde(default)]
    pub mark: u32,
    #[serde(default)]
    pub target: Option<Ipv4Addr>,
    #[serde(default)]
    pub path_id: String,
}

const fn default_interval() -> u64 {
    15_000
}

const fn default_timeout() -> u64 {
    2_000
}

const fn default_wifi_interval() -> u64 {
    5_000
}

#[derive(Serialize, Deserialize)]
pub struct Status {
    pub schema: u8,
    pub run_id: String,
    pub target: Ipv4Addr,
    pub uptime_ms: u64,
    pub interval_ms: u64,
    pub tunnels: Vec<Series>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi: Option<WifiStatus>,
}

#[derive(Serialize, Deserialize)]
pub struct Series {
    pub id: String,
    pub name: String,
    pub interface: String,
    pub path_id: String,
    pub target: Ipv4Addr,
    pub state: State,
    pub median_us: Option<u32>,
    pub sent: u64,
    pub received: u64,
    pub success_age_ms: Option<u64>,
    pub history: Vec<Sample>,
    #[serde(default)]
    pub buckets: Vec<Bucket>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Bucket {
    pub at: u64,
    pub sent: u16,
    pub lost: u16,
    pub min_us: Option<u32>,
    pub median_us: Option<u32>,
    pub max_us: Option<u32>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Sample {
    pub at_ms: u64,
    pub us: Option<u32>,
}

impl Series {
    #[must_use]
    pub fn lost(&self) -> usize {
        self.history
            .iter()
            .filter(|sample| sample.us.is_none())
            .count()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiState {
    Starting,
    QueryFailed,
    NoClientInterface,
    Disconnected,
    Connected,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rate {
    pub kbps: Option<u32>,
    pub mode: Option<String>,
    pub mcs: Option<u8>,
    pub nss: Option<u8>,
    pub mhz: Option<u16>,
    pub short_gi: Option<bool>,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiLink {
    pub ssid: String,
    pub bssid: String,
    pub channel: Option<u16>,
    pub frequency: Option<u32>,
    pub width: Option<u16>,
    pub htmode: Option<String>,
    pub hwmode: Option<String>,
    pub encryption: Option<String>,
    pub hardware: Option<String>,
    pub txpower: Option<i16>,
    pub signal: Option<i16>,
    pub signal_avg: Option<i16>,
    pub noise: Option<i16>,
    pub quality: Option<u16>,
    pub quality_max: Option<u16>,
    pub tx: Rate,
    pub rx: Rate,
    pub expected_kbps: Option<u32>,
    pub connected_seconds: Option<u64>,
    pub inactive_ms: Option<u64>,
    pub busy_permille: Option<u16>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WifiSample {
    pub at_ms: u64,
    pub signal: Option<i16>,
    pub tx_kbps: Option<u32>,
    pub rx_kbps: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WifiStatus {
    pub state: WifiState,
    pub interface: Option<String>,
    pub candidates: Vec<String>,
    pub sample_age_ms: Option<u64>,
    pub link: Option<WifiLink>,
    pub recent: Vec<WifiSample>,
    pub buckets: Vec<WifiBucket>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiBucket {
    pub at: u64,
    pub samples: u32,
    pub connected: u32,
    pub signal_n: u32,
    pub signal_sum: i64,
    pub signal_min: Option<i16>,
    pub signal_max: Option<i16>,
    pub noise_n: u32,
    pub noise_sum: i64,
    pub tx_rate_n: u32,
    pub tx_rate_sum: u64,
    pub tx_rate_min: Option<u32>,
    pub rx_rate_n: u32,
    pub rx_rate_sum: u64,
    pub expected_n: u32,
    pub expected_sum: u64,
    pub frames: u64,
    pub retries: u64,
    pub failed: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub active_ms: u64,
    pub busy_ms: u64,
    pub rx_ms: u64,
    pub tx_ms: u64,
    pub disconnects: u32,
    pub roams: u32,
}

fn lower<T: Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

fn higher<T: Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

impl WifiBucket {
    #[must_use]
    pub const fn empty(at: u64) -> Self {
        Self {
            at,
            samples: 0,
            connected: 0,
            signal_n: 0,
            signal_sum: 0,
            signal_min: None,
            signal_max: None,
            noise_n: 0,
            noise_sum: 0,
            tx_rate_n: 0,
            tx_rate_sum: 0,
            tx_rate_min: None,
            rx_rate_n: 0,
            rx_rate_sum: 0,
            expected_n: 0,
            expected_sum: 0,
            frames: 0,
            retries: 0,
            failed: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            active_ms: 0,
            busy_ms: 0,
            rx_ms: 0,
            tx_ms: 0,
            disconnects: 0,
            roams: 0,
        }
    }

    pub fn merge(&mut self, other: &Self) {
        self.samples = self.samples.saturating_add(other.samples);
        self.connected = self.connected.saturating_add(other.connected);
        self.signal_n = self.signal_n.saturating_add(other.signal_n);
        self.signal_sum = self.signal_sum.saturating_add(other.signal_sum);
        self.signal_min = lower(self.signal_min, other.signal_min);
        self.signal_max = higher(self.signal_max, other.signal_max);
        self.noise_n = self.noise_n.saturating_add(other.noise_n);
        self.noise_sum = self.noise_sum.saturating_add(other.noise_sum);
        self.tx_rate_n = self.tx_rate_n.saturating_add(other.tx_rate_n);
        self.tx_rate_sum = self.tx_rate_sum.saturating_add(other.tx_rate_sum);
        self.tx_rate_min = lower(self.tx_rate_min, other.tx_rate_min);
        self.rx_rate_n = self.rx_rate_n.saturating_add(other.rx_rate_n);
        self.rx_rate_sum = self.rx_rate_sum.saturating_add(other.rx_rate_sum);
        self.expected_n = self.expected_n.saturating_add(other.expected_n);
        self.expected_sum = self.expected_sum.saturating_add(other.expected_sum);
        self.frames = self.frames.saturating_add(other.frames);
        self.retries = self.retries.saturating_add(other.retries);
        self.failed = self.failed.saturating_add(other.failed);
        self.tx_bytes = self.tx_bytes.saturating_add(other.tx_bytes);
        self.rx_bytes = self.rx_bytes.saturating_add(other.rx_bytes);
        self.active_ms = self.active_ms.saturating_add(other.active_ms);
        self.busy_ms = self.busy_ms.saturating_add(other.busy_ms);
        self.rx_ms = self.rx_ms.saturating_add(other.rx_ms);
        self.tx_ms = self.tx_ms.saturating_add(other.tx_ms);
        self.disconnects = self.disconnects.saturating_add(other.disconnects);
        self.roams = self.roams.saturating_add(other.roams);
    }
}
