#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const HISTORY: usize = 64;
pub const MAX_SERIES: usize = 90;
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

#[derive(Serialize, Deserialize)]
pub struct Status {
    pub schema: u8,
    pub run_id: String,
    pub target: Ipv4Addr,
    pub uptime_ms: u64,
    pub interval_ms: u64,
    pub tunnels: Vec<Series>,
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
