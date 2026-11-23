use serde::Serialize;

pub const VERSION: &str = "1";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fold {
    Sum,
    Min,
    Max,
    MedianOfMedians,
}

impl Fold {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Min => "min",
            Self::Max => "max",
            Self::MedianOfMedians => "median_of_medians",
        }
    }
}

#[derive(Serialize)]
pub struct Measure {
    pub name: &'static str,
    #[serde(rename = "rollup")]
    pub fold: Fold,
    pub unit: &'static str,
    pub description: &'static str,
}

#[must_use]
pub const fn measure(
    name: &'static str,
    fold: Fold,
    unit: &'static str,
    description: &'static str,
) -> Measure {
    Measure {
        name,
        fold,
        unit,
        description,
    }
}

pub struct Kind {
    pub name: &'static str,
    pub description: &'static str,
    pub fields: &'static [Measure],
}

impl Kind {
    #[must_use]
    pub fn index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|field| field.name == name)
    }
}

pub const RTT_EDGES_US: [u32; 15] = [
    1_000, 2_000, 3_000, 5_000, 7_500, 10_000, 15_000, 20_000, 30_000, 50_000, 75_000, 100_000,
    150_000, 250_000, 500_000,
];
pub const RTT_BINS: usize = RTT_EDGES_US.len() + 1;
pub const RTT_FIRST: usize = 7;

#[must_use]
pub fn rtt_bin(micros: u32) -> usize {
    RTT_EDGES_US.partition_point(|edge| *edge <= micros)
}

use Fold::{Max, MedianOfMedians, Min, Sum};

pub static LATENCY: Kind = Kind {
    name: "latency",
    description: "ICMP echo through one path to one destination, per bucket",
    fields: &[
        measure("min_us", Min, "us", "Fastest successful round trip"),
        measure(
            "median_us",
            MedianOfMedians,
            "us",
            "Lower median round trip; coarser steps take the lower median of finer medians, so prefer the rtt_* histogram for percentiles across time or devices",
        ),
        measure("max_us", Max, "us", "Slowest successful round trip"),
        measure("sent", Sum, "probes", "Echo requests sent"),
        measure(
            "lost",
            Sum,
            "probes",
            "Echo requests without a matching reply within the timeout",
        ),
        measure(
            "jitter_sum",
            Sum,
            "us",
            "Sum of absolute differences between consecutive successful round trips",
        ),
        measure(
            "jitter_n",
            Sum,
            "pairs",
            "Round-trip pairs in jitter_sum; jitter = jitter_sum / jitter_n",
        ),
        measure(
            "rtt_lt_1ms",
            Sum,
            "probes",
            "Successful probes with round trip below 1 ms",
        ),
        measure(
            "rtt_1_2ms",
            Sum,
            "probes",
            "Successful probes with round trip in [1, 2) ms",
        ),
        measure(
            "rtt_2_3ms",
            Sum,
            "probes",
            "Successful probes with round trip in [2, 3) ms",
        ),
        measure(
            "rtt_3_5ms",
            Sum,
            "probes",
            "Successful probes with round trip in [3, 5) ms",
        ),
        measure(
            "rtt_5_7p5ms",
            Sum,
            "probes",
            "Successful probes with round trip in [5, 7.5) ms",
        ),
        measure(
            "rtt_7p5_10ms",
            Sum,
            "probes",
            "Successful probes with round trip in [7.5, 10) ms",
        ),
        measure(
            "rtt_10_15ms",
            Sum,
            "probes",
            "Successful probes with round trip in [10, 15) ms",
        ),
        measure(
            "rtt_15_20ms",
            Sum,
            "probes",
            "Successful probes with round trip in [15, 20) ms",
        ),
        measure(
            "rtt_20_30ms",
            Sum,
            "probes",
            "Successful probes with round trip in [20, 30) ms",
        ),
        measure(
            "rtt_30_50ms",
            Sum,
            "probes",
            "Successful probes with round trip in [30, 50) ms",
        ),
        measure(
            "rtt_50_75ms",
            Sum,
            "probes",
            "Successful probes with round trip in [50, 75) ms",
        ),
        measure(
            "rtt_75_100ms",
            Sum,
            "probes",
            "Successful probes with round trip in [75, 100) ms",
        ),
        measure(
            "rtt_100_150ms",
            Sum,
            "probes",
            "Successful probes with round trip in [100, 150) ms",
        ),
        measure(
            "rtt_150_250ms",
            Sum,
            "probes",
            "Successful probes with round trip in [150, 250) ms",
        ),
        measure(
            "rtt_250_500ms",
            Sum,
            "probes",
            "Successful probes with round trip in [250, 500) ms",
        ),
        measure(
            "rtt_ge_500ms",
            Sum,
            "probes",
            "Successful probes with round trip of 500 ms or more",
        ),
    ],
};

pub static WIFI: Kind = Kind {
    name: "wifi",
    description: "Upstream Wi-Fi link of the router's client (repeater) interface, per bucket",
    fields: &[
        measure(
            "samples",
            Sum,
            "samples",
            "Polls with a client interface present",
        ),
        measure(
            "connected",
            Sum,
            "samples",
            "Polls while associated to an access point",
        ),
        measure("signal_n", Sum, "samples", "Polls with a valid signal"),
        measure(
            "signal_sum",
            Sum,
            "dBm",
            "Sum of signal; average = signal_sum / signal_n",
        ),
        measure("signal_min", Min, "dBm", "Weakest signal"),
        measure("signal_max", Max, "dBm", "Strongest signal"),
        measure("noise_n", Sum, "samples", "Polls with a valid noise floor"),
        measure(
            "noise_sum",
            Sum,
            "dBm",
            "Sum of noise floor; average = noise_sum / noise_n",
        ),
        measure(
            "tx_rate_n",
            Sum,
            "samples",
            "Polls with a transmit PHY rate",
        ),
        measure(
            "tx_rate_sum",
            Sum,
            "kbit/s",
            "Sum of transmit PHY rate; average = tx_rate_sum / tx_rate_n",
        ),
        measure("tx_rate_min", Min, "kbit/s", "Slowest transmit PHY rate"),
        measure("rx_rate_n", Sum, "samples", "Polls with a receive PHY rate"),
        measure(
            "rx_rate_sum",
            Sum,
            "kbit/s",
            "Sum of receive PHY rate; average = rx_rate_sum / rx_rate_n",
        ),
        measure(
            "expected_n",
            Sum,
            "samples",
            "Polls with a driver throughput estimate",
        ),
        measure(
            "expected_sum",
            Sum,
            "kbit/s",
            "Sum of expected throughput; average = expected_sum / expected_n",
        ),
        measure("frames", Sum, "frames", "Transmitted frames"),
        measure(
            "retries",
            Sum,
            "frames",
            "Retried frames; retry share = retries / (frames + retries)",
        ),
        measure(
            "failed",
            Sum,
            "frames",
            "Frames that failed after all retries; failure share = failed / frames",
        ),
        measure(
            "tx_bytes",
            Sum,
            "bytes",
            "Bytes sent through the client interface",
        ),
        measure(
            "rx_bytes",
            Sum,
            "bytes",
            "Bytes received through the client interface",
        ),
        measure(
            "active_ms",
            Sum,
            "ms",
            "Time the radio spent on the serving channel",
        ),
        measure(
            "busy_ms",
            Sum,
            "ms",
            "Time the serving channel was busy; busy share = busy_ms / active_ms",
        ),
        measure(
            "rx_ms",
            Sum,
            "ms",
            "Time the radio spent receiving on the serving channel",
        ),
        measure(
            "tx_ms",
            Sum,
            "ms",
            "Time the radio spent transmitting on the serving channel",
        ),
        measure(
            "disconnects",
            Sum,
            "events",
            "Disconnections, silent reassociations, or changes of network",
        ),
        measure(
            "roams",
            Sum,
            "events",
            "Moves to another access point of the same network",
        ),
    ],
};

pub static SYSTEM: Kind = Kind {
    name: "system",
    description: "Router health, per bucket",
    fields: &[
        measure("samples", Sum, "samples", "Polls in the bucket"),
        measure("cpu_n", Sum, "samples", "Polls with a CPU reading"),
        measure(
            "cpu_sum",
            Sum,
            "permille",
            "Sum of busy CPU share; average = cpu_sum / cpu_n",
        ),
        measure("cpu_max", Max, "permille", "Highest busy CPU share"),
        measure(
            "load_sum",
            Sum,
            "load_x100",
            "Sum of the 1-minute load average times 100; average = load_sum / samples / 100",
        ),
        measure("mem_n", Sum, "samples", "Polls with a memory reading"),
        measure(
            "mem_used_sum",
            Sum,
            "permille",
            "Sum of used memory share; average = mem_used_sum / mem_n",
        ),
        measure("mem_available_min", Min, "bytes", "Lowest available memory"),
        measure("temp_n", Sum, "samples", "Polls with a temperature"),
        measure(
            "temp_sum",
            Sum,
            "millicelsius",
            "Sum of the hottest zone temperature; average = temp_sum / temp_n",
        ),
        measure("temp_max", Max, "millicelsius", "Hottest zone temperature"),
        measure("conntrack_max", Max, "entries", "Most tracked connections"),
        measure(
            "conntrack_limit",
            Max,
            "entries",
            "Connection tracking table size",
        ),
    ],
};

pub static INTERFACE: Kind = Kind {
    name: "interface",
    description: "Traffic counters of one network device (uplink or tunnel), per bucket",
    fields: &[
        measure("samples", Sum, "samples", "Polls in the bucket"),
        measure(
            "seconds_ms",
            Sum,
            "ms",
            "Time covered by counter differences; rate = bytes * 8000 / seconds_ms",
        ),
        measure("rx_bytes", Sum, "bytes", "Bytes received"),
        measure("tx_bytes", Sum, "bytes", "Bytes sent"),
        measure("rx_packets", Sum, "packets", "Packets received"),
        measure("tx_packets", Sum, "packets", "Packets sent"),
        measure("errors", Sum, "packets", "Receive and transmit errors"),
        measure("drops", Sum, "packets", "Receive and transmit drops"),
        measure(
            "rx_peak_bps",
            Max,
            "bit/s",
            "Highest receive rate between two polls",
        ),
        measure(
            "tx_peak_bps",
            Max,
            "bit/s",
            "Highest transmit rate between two polls",
        ),
        measure(
            "handshake_age_max",
            Max,
            "seconds",
            "Oldest WireGuard handshake seen; above 180 means the tunnel is not rekeying",
        ),
    ],
};

pub static SQM: Kind = Kind {
    name: "sqm",
    description: "CAKE autorate bandwidth shaping per interface, per bucket",
    fields: &[
        measure("samples", Sum, "samples", "Ticks in the bucket"),
        measure(
            "dl_kbps_sum",
            Sum,
            "kbit/s",
            "Sum of download shaper rates; average = dl_kbps_sum / samples",
        ),
        measure("dl_kbps_min", Min, "kbit/s", "Lowest download shaper rate"),
        measure("dl_kbps_max", Max, "kbit/s", "Highest download shaper rate"),
        measure(
            "ul_kbps_sum",
            Sum,
            "kbit/s",
            "Sum of upload shaper rates; average = ul_kbps_sum / samples",
        ),
        measure("ul_kbps_min", Min, "kbit/s", "Lowest upload shaper rate"),
        measure("ul_kbps_max", Max, "kbit/s", "Highest upload shaper rate"),
        measure(
            "baseline_us_n",
            Sum,
            "samples",
            "Ticks with a baseline RTT estimate",
        ),
        measure(
            "baseline_us_sum",
            Sum,
            "us",
            "Sum of baseline RTT estimates; average = baseline_us_sum / baseline_us_n",
        ),
        measure(
            "delta_us_max",
            Max,
            "us",
            "Highest RTT delta above baseline",
        ),
        measure("increases", Sum, "events", "Bandwidth increase decisions"),
        measure("decreases", Sum, "events", "Bandwidth decrease decisions"),
    ],
};

pub static KINDS: [&Kind; 5] = [&LATENCY, &WIFI, &SYSTEM, &INTERFACE, &SQM];

#[cfg(test)]
mod tests {
    use super::{KINDS, LATENCY, RTT_BINS, RTT_EDGES_US, RTT_FIRST, rtt_bin};

    #[test]
    fn rtt_bins_cover_every_round_trip() {
        assert_eq!(rtt_bin(0), 0);
        assert_eq!(rtt_bin(999), 0);
        assert_eq!(rtt_bin(1_000), 1);
        assert_eq!(rtt_bin(7_499), 4);
        assert_eq!(rtt_bin(7_500), 5);
        assert_eq!(rtt_bin(499_999), RTT_BINS - 2);
        assert_eq!(rtt_bin(u32::MAX), RTT_BINS - 1);
        assert!(RTT_EDGES_US.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(LATENCY.fields.len(), RTT_FIRST + RTT_BINS);
        assert!(LATENCY.fields[RTT_FIRST].name.starts_with("rtt_"));
    }

    #[test]
    fn names_are_unique_and_sql_safe() {
        for kind in KINDS {
            for (index, field) in kind.fields.iter().enumerate() {
                assert!(
                    field.name.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'),
                    "{}",
                    field.name
                );
                assert_eq!(kind.index(field.name), Some(index));
                assert!(!field.unit.is_empty() && !field.description.is_empty());
            }
        }
    }
}
