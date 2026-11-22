use crate::probe::{boot_start, bucket_start, millis, unix_millis};
use crate::wifi::valid_device;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use wayline_protocol::{
    BUCKET_SECONDS, Counter, Link, LinkBucket, LinkHistory, System, SystemBucket, TelemetryConfig,
    TelemetryStatus,
};

const RETAINED: usize = 80;
const SNAPSHOT: usize = 8;
const READ_LIMIT: u64 = 64 * 1024;
const WIREGUARD_EVERY: u32 = 3;
const STATISTICS: [&str; 8] = [
    "rx_bytes",
    "tx_bytes",
    "rx_packets",
    "tx_packets",
    "rx_errors",
    "tx_errors",
    "rx_dropped",
    "tx_dropped",
];

pub trait Host {
    fn read(&mut self, path: &str) -> Option<String>;
    fn zones(&mut self) -> Vec<String>;
    fn wireguard(&mut self) -> Option<String>;
}

pub struct Proc;

impl Host for Proc {
    fn read(&mut self, path: &str) -> Option<String> {
        let mut text = String::new();
        fs::File::open(path)
            .ok()?
            .take(READ_LIMIT)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }

    fn zones(&mut self) -> Vec<String> {
        fs::read_dir("/sys/class/thermal")
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| name.starts_with("thermal_zone") && valid_device(name))
                    .map(|name| format!("/sys/class/thermal/{name}/temp"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn wireguard(&mut self) -> Option<String> {
        String::from_utf8(
            wayline_support::output("wg", &["show", "all", "dump"], 256 * 1024, 2).ok()?,
        )
        .ok()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cpu {
    total: u64,
    idle: u64,
}

pub fn cpu(stat: &str) -> Option<Cpu> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [_, _, _, idle, iowait, ..] = values.as_slice() else {
        return None;
    };
    Some(Cpu {
        total: values.iter().sum(),
        idle: idle + iowait,
    })
}

fn centi(text: &str) -> Option<u32> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits: String = fraction.chars().chain("00".chars()).take(2).collect();
    whole
        .parse::<u32>()
        .ok()?
        .checked_mul(100)?
        .checked_add(digits.parse().ok()?)
}

pub fn load(text: &str) -> Option<[u32; 3]> {
    let mut fields = text.split_whitespace().map(centi);
    Some([fields.next()??, fields.next()??, fields.next()??])
}

pub fn memory(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        let line = text.lines().find(|line| line.starts_with(name))?;
        let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        kilobytes.checked_mul(1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

fn number(text: Option<String>) -> Option<u64> {
    text?.trim().parse().ok()
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Handshake {
    pub latest: Option<u64>,
    pub endpoint: Option<String>,
}

pub fn handshakes(dump: &str) -> BTreeMap<String, Handshake> {
    let mut found: BTreeMap<String, Handshake> = BTreeMap::new();
    for line in dump.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [device, _, _, endpoint, _, latest, _, _, _] = fields.as_slice() else {
            continue;
        };
        if !valid_device(device) {
            continue;
        }
        let latest = latest.parse::<u64>().ok().filter(|value| *value > 0);
        let endpoint =
            (*endpoint != "(none)" && endpoint.len() <= 64).then(|| (*endpoint).to_owned());
        let entry = found.entry((*device).to_owned()).or_default();
        if latest > entry.latest {
            entry.latest = latest;
            if endpoint.is_some() {
                entry.endpoint = endpoint;
            }
        } else if entry.endpoint.is_none() {
            entry.endpoint = endpoint;
        }
    }
    found
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    values: [u64; 8],
}

pub struct Observation {
    pub cpu: Option<Cpu>,
    pub load: Option<[u32; 3]>,
    pub memory: Option<(u64, u64)>,
    pub temperature: Option<i32>,
    pub conntrack: Option<u32>,
    pub conntrack_max: Option<u32>,
    pub uptime: Option<u64>,
    pub links: Vec<Option<Counters>>,
    pub handshakes: Option<BTreeMap<String, Handshake>>,
}

pub struct Sampler<H: Host> {
    host: H,
    counters: Vec<Counter>,
    ticks: u32,
}

impl<H: Host> Sampler<H> {
    pub fn new(host: H, counters: Vec<Counter>) -> Self {
        let counters = counters
            .into_iter()
            .filter(|counter| valid_device(&counter.device))
            .collect();
        Self {
            host,
            counters,
            ticks: 0,
        }
    }

    pub fn observe(&mut self) -> Observation {
        let host = &mut self.host;
        let temperature = host
            .zones()
            .iter()
            .filter_map(|path| host.read(path)?.trim().parse::<i32>().ok())
            .filter(|milli| (-40_000..=150_000).contains(milli))
            .max();
        let links = self
            .counters
            .iter()
            .map(|counter| {
                let mut values = [0; 8];
                for (slot, name) in values.iter_mut().zip(STATISTICS) {
                    *slot = number(host.read(&format!(
                        "/sys/class/net/{}/statistics/{name}",
                        counter.device
                    )))?;
                }
                Some(Counters { values })
            })
            .collect();
        let wireguard = self.counters.iter().any(|counter| counter.kind == "tunnel")
            && self.ticks.is_multiple_of(WIREGUARD_EVERY);
        self.ticks = self.ticks.wrapping_add(1);
        Observation {
            cpu: host.read("/proc/stat").as_deref().and_then(cpu),
            load: host.read("/proc/loadavg").as_deref().and_then(load),
            memory: host.read("/proc/meminfo").as_deref().and_then(memory),
            temperature,
            conntrack: number(host.read("/proc/sys/net/netfilter/nf_conntrack_count"))
                .and_then(|value| u32::try_from(value).ok()),
            conntrack_max: number(host.read("/proc/sys/net/netfilter/nf_conntrack_max"))
                .and_then(|value| u32::try_from(value).ok()),
            uptime: host
                .read("/proc/uptime")
                .and_then(|text| text.split(['.', ' ']).next()?.parse().ok()),
            links,
            handshakes: wireguard
                .then(|| host.wireguard().as_deref().map(handshakes))
                .flatten(),
        }
    }
}

struct Track {
    counter: Counter,
    previous: Option<(Instant, Counters)>,
    live: Link,
    handshake: Handshake,
    open: Option<LinkBucket>,
    closed: VecDeque<LinkBucket>,
}

pub struct Record {
    sampled: Option<Instant>,
    previous_cpu: Option<Cpu>,
    system: Option<System>,
    open: Option<SystemBucket>,
    load_sum: u64,
    load_n: u32,
    closed: VecDeque<SystemBucket>,
    tracks: Vec<Track>,
}

const fn due(at: u64, wall_ms: u64) -> bool {
    wall_ms >= (at + BUCKET_SECONDS) * 1000
}

fn push<T>(closed: &mut VecDeque<T>, bucket: T) {
    if closed.len() == RETAINED {
        closed.pop_front();
    }
    closed.push_back(bucket);
}

fn kept<T: Copy>(closed: &VecDeque<T>, since: Option<u64>, at: impl Fn(&T) -> u64) -> Vec<T> {
    let skip = if since.is_some() {
        0
    } else {
        closed.len().saturating_sub(SNAPSHOT)
    };
    closed
        .iter()
        .skip(skip)
        .filter(|bucket| since.is_none_or(|cursor| at(bucket) > cursor))
        .copied()
        .collect()
}

const fn delta(after: u64, before: u64) -> Option<u64> {
    after.checked_sub(before)
}

fn spread_load(sum: u64, readings: u32, samples: u32) -> u64 {
    let readings = u64::from(readings);
    sum.saturating_mul(u64::from(samples))
        .saturating_add(readings / 2)
        .checked_div(readings)
        .unwrap_or(0)
}

impl Record {
    pub fn new(counters: &[Counter]) -> Self {
        Self {
            sampled: None,
            previous_cpu: None,
            system: None,
            open: None,
            load_sum: 0,
            load_n: 0,
            closed: VecDeque::new(),
            tracks: counters
                .iter()
                .filter(|counter| valid_device(&counter.device))
                .map(|counter| Track {
                    counter: counter.clone(),
                    previous: None,
                    live: Link {
                        id: counter.id.clone(),
                        name: counter.name.clone(),
                        device: counter.device.clone(),
                        kind: counter.kind.clone(),
                        ..Link::default()
                    },
                    handshake: Handshake::default(),
                    open: None,
                    closed: VecDeque::new(),
                })
                .collect(),
        }
    }

    fn close(&mut self, wall_ms: u64) {
        if let Some(mut bucket) = self.open.filter(|bucket| due(bucket.at, wall_ms)) {
            self.open = None;
            bucket.load_sum = spread_load(self.load_sum, self.load_n, bucket.samples);
            self.load_sum = 0;
            self.load_n = 0;
            if bucket.samples > 0 {
                push(&mut self.closed, bucket);
            }
        }
        for track in &mut self.tracks {
            if let Some(bucket) = track.open.filter(|bucket| due(bucket.at, wall_ms)) {
                track.open = None;
                if bucket.samples > 0 {
                    push(&mut track.closed, bucket);
                }
            }
        }
    }

    pub fn apply(&mut self, now: Instant, wall_ms: u64, observation: Observation) {
        self.close(wall_ms);
        self.sampled = Some(now);
        let at = bucket_start(wall_ms);
        let cpu_permille = self
            .previous_cpu
            .zip(observation.cpu)
            .and_then(|(before, after)| {
                let total = delta(after.total, before.total)?;
                let idle = delta(after.idle, before.idle)?;
                (total > 0).then(|| total.saturating_sub(idle) * 1000 / total)
            })
            .and_then(|value| u32::try_from(value).ok());
        self.previous_cpu = observation.cpu.or(self.previous_cpu);
        let (mem_total, mem_available) = observation.memory.unwrap_or_default();
        let system = System {
            cpu_permille,
            load_centi: observation.load.unwrap_or_default(),
            mem_total,
            mem_available,
            temperature_milli: observation.temperature,
            conntrack: observation.conntrack,
            conntrack_max: observation.conntrack_max,
            uptime: observation.uptime.unwrap_or_default(),
        };
        self.system = Some(system);
        let uptime_ms = boot_start(wall_ms, at);
        let bucket = self.open.get_or_insert_with(|| SystemBucket {
            at,
            uptime_ms,
            ..SystemBucket::default()
        });
        bucket.samples += 1;
        if let Some(cpu) = cpu_permille {
            bucket.cpu_n += 1;
            bucket.cpu_sum += u64::from(cpu);
            bucket.cpu_max = Some(bucket.cpu_max.map_or(cpu, |high| high.max(cpu)));
        }
        if let Some([load, ..]) = observation.load {
            self.load_n += 1;
            self.load_sum += u64::from(load);
        }
        if let Some(used) = (mem_total.saturating_sub(mem_available) * 1000).checked_div(mem_total)
        {
            bucket.mem_n += 1;
            bucket.mem_used_sum += used;
            bucket.mem_available_min = Some(
                bucket
                    .mem_available_min
                    .map_or(mem_available, |low| low.min(mem_available)),
            );
        }
        if let Some(milli) = system.temperature_milli {
            bucket.temp_n += 1;
            bucket.temp_sum += i64::from(milli);
            bucket.temp_max = Some(bucket.temp_max.map_or(milli, |high| high.max(milli)));
        }
        bucket.conntrack_max = bucket.conntrack_max.max(system.conntrack);
        bucket.conntrack_limit = bucket.conntrack_limit.max(system.conntrack_max);
        let now_seconds = wall_ms / 1000;
        for (track, counters) in self.tracks.iter_mut().zip(observation.links) {
            if let Some(found) = &observation.handshakes {
                track.handshake = found
                    .get(&track.counter.device)
                    .cloned()
                    .unwrap_or_default();
            }
            Self::advance(track, now, (at, uptime_ms), now_seconds, counters);
        }
    }

    fn advance(
        track: &mut Track,
        now: Instant,
        (at, uptime_ms): (u64, u64),
        now_seconds: u64,
        counters: Option<Counters>,
    ) {
        let age = track
            .handshake
            .latest
            .map(|latest| now_seconds.saturating_sub(latest));
        track.live.present = counters.is_some();
        track.live.handshake_age = age;
        track.live.endpoint.clone_from(&track.handshake.endpoint);
        let Some(counters) = counters else {
            track.previous = None;
            track.live.rx_bps = None;
            track.live.tx_bps = None;
            return;
        };
        let [
            rx_bytes,
            tx_bytes,
            _,
            _,
            rx_errors,
            tx_errors,
            rx_drops,
            tx_drops,
        ] = counters.values;
        track.live.rx_bytes = rx_bytes;
        track.live.tx_bytes = tx_bytes;
        track.live.errors = rx_errors.saturating_add(tx_errors);
        track.live.drops = rx_drops.saturating_add(tx_drops);
        let bucket = track.open.get_or_insert_with(|| LinkBucket {
            at,
            uptime_ms,
            ..LinkBucket::default()
        });
        bucket.samples += 1;
        bucket.handshake_age_max = bucket.handshake_age_max.max(age);
        let previous = track.previous.replace((now, counters));
        let elapsed = previous.map(|(then, _)| millis(now.saturating_duration_since(then)));
        let changes = previous.and_then(|(_, before)| {
            let mut changes = [0; 8];
            for ((slot, after), before) in
                changes.iter_mut().zip(counters.values).zip(before.values)
            {
                *slot = delta(after, before)?;
            }
            Some(changes)
        });
        if let (Some(changes), Some(elapsed)) = (changes, elapsed.filter(|elapsed| *elapsed > 0)) {
            let rate = |bytes: u64| bytes.saturating_mul(8_000) / elapsed;
            let (rx_rate, tx_rate) = (rate(changes[0]), rate(changes[1]));
            track.live.rx_bps = Some(rx_rate);
            track.live.tx_bps = Some(tx_rate);
            bucket.seconds_ms += elapsed;
            bucket.rx_bytes += changes[0];
            bucket.tx_bytes += changes[1];
            bucket.rx_packets += changes[2];
            bucket.tx_packets += changes[3];
            bucket.errors += changes[4] + changes[5];
            bucket.drops += changes[6] + changes[7];
            bucket.rx_peak_bps = bucket.rx_peak_bps.max(Some(rx_rate));
            bucket.tx_peak_bps = bucket.tx_peak_bps.max(Some(tx_rate));
        } else {
            track.live.rx_bps = None;
            track.live.tx_bps = None;
        }
    }

    pub fn status(&self, now: Instant, since: Option<u64>) -> TelemetryStatus {
        TelemetryStatus {
            sample_age_ms: self
                .sampled
                .map(|at| millis(now.saturating_duration_since(at))),
            system: self.system,
            links: self.tracks.iter().map(|track| track.live.clone()).collect(),
            system_buckets: kept(&self.closed, since, |bucket| bucket.at),
            link_buckets: self
                .tracks
                .iter()
                .map(|track| LinkHistory {
                    id: track.counter.id.clone(),
                    buckets: kept(&track.closed, since, |bucket| bucket.at),
                })
                .collect(),
        }
    }
}

pub struct Telemetry {
    record: Arc<Mutex<Record>>,
}

impl Telemetry {
    pub fn start(config: &TelemetryConfig) -> io::Result<Self> {
        let record = Arc::new(Mutex::new(Record::new(&config.interfaces)));
        let shared = Arc::clone(&record);
        let interval = Duration::from_millis(config.interval_ms);
        let sampler = Sampler::new(Proc, config.interfaces.clone());
        thread::Builder::new()
            .name("telemetry".into())
            .spawn(move || run(&shared, sampler, interval))?;
        Ok(Self { record })
    }

    pub fn status(&self, now: Instant, since: Option<u64>) -> TelemetryStatus {
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .status(now, since)
    }
}

fn run<H: Host>(record: &Mutex<Record>, mut sampler: Sampler<H>, interval: Duration) {
    let mut next = Instant::now();
    loop {
        let observation = sampler.observe();
        record.lock().unwrap_or_else(PoisonError::into_inner).apply(
            Instant::now(),
            unix_millis(),
            observation,
        );
        next += interval;
        let now = Instant::now();
        if next <= now {
            next = now + interval;
        }
        thread::sleep(next.saturating_duration_since(now));
    }
}

#[cfg(test)]
mod tests {
    use super::{Host, Record, Sampler, cpu, handshakes, load, memory};
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};
    use wayline_protocol::Counter;

    struct Fake {
        files: BTreeMap<String, String>,
        dump: Option<String>,
    }

    impl Host for Fake {
        fn read(&mut self, path: &str) -> Option<String> {
            self.files.get(path).cloned()
        }

        fn zones(&mut self) -> Vec<String> {
            vec![
                "/sys/class/thermal/thermal_zone0/temp".into(),
                "/sys/class/thermal/thermal_zone1/temp".into(),
            ]
        }

        fn wireguard(&mut self) -> Option<String> {
            self.dump.clone()
        }
    }

    fn counters() -> Vec<Counter> {
        vec![
            Counter {
                id: "uplink:wan".into(),
                name: "wan".into(),
                device: "eth1".into(),
                kind: "uplink".into(),
            },
            Counter {
                id: "home".into(),
                name: "Home".into(),
                device: "wgclient1".into(),
                kind: "tunnel".into(),
            },
        ]
    }

    fn host(cpu_busy: u64, bytes: u64) -> Fake {
        let mut files = BTreeMap::new();
        files.insert(
            "/proc/stat".into(),
            format!("cpu  {cpu_busy} 0 0 10000 0 0 0 0 0 0\ncpu0 1 2 3 4\n"),
        );
        files.insert("/proc/loadavg".into(), "0.21 0.5 1.25 1/123 4567\n".into());
        files.insert(
            "/proc/meminfo".into(),
            "MemTotal:        1000000 kB\nMemFree: 1 kB\nMemAvailable:     250000 kB\n".into(),
        );
        files.insert(
            "/sys/class/thermal/thermal_zone0/temp".into(),
            "51200\n".into(),
        );
        files.insert(
            "/sys/class/thermal/thermal_zone1/temp".into(),
            "58700\n".into(),
        );
        files.insert(
            "/proc/sys/net/netfilter/nf_conntrack_count".into(),
            "412\n".into(),
        );
        files.insert(
            "/proc/sys/net/netfilter/nf_conntrack_max".into(),
            "16384\n".into(),
        );
        files.insert("/proc/uptime".into(), "123456.78 99.0\n".into());
        for device in ["eth1", "wgclient1"] {
            for (index, name) in super::STATISTICS.iter().enumerate() {
                files.insert(
                    format!("/sys/class/net/{device}/statistics/{name}"),
                    (bytes * (index as u64 + 1)).to_string(),
                );
            }
        }
        Fake {
            files,
            dump: Some(
                "wgclient1\tPRIVATE\tPUBLIC\t51820\toff\nwgclient1\tpeer\t(none)\t203.0.113.9:51820\t0.0.0.0/0\t1800000000\t10\t20\t25\n".into(),
            ),
        }
    }

    #[test]
    fn parsers_read_proc_formats() {
        let parsed = cpu("cpu  100 0 50 800 50 0 0 0 0 0\n");
        assert!(parsed.is_some_and(|value| value.total == 1000 && value.idle == 850));
        assert_eq!(load("0.21 0.5 12.345 1/2 3"), Some([21, 50, 1234]));
        assert_eq!(
            memory("MemTotal: 2 kB\nMemAvailable: 1 kB\n"),
            Some((2048, 1024))
        );
        let found = handshakes(
            "wg0\tPRIVATE\tPUBLIC\t51820\toff\nwg0\tpeer1\t(none)\t(none)\t10.0.0.0/8\t0\t0\t0\toff\nwg0\tpeer2\t(none)\t198.51.100.1:51820\t0.0.0.0/0\t1700000000\t5\t6\t25\n",
        );
        let wg = found.get("wg0").cloned().unwrap_or_default();
        assert_eq!(wg.latest, Some(1_700_000_000));
        assert_eq!(wg.endpoint.as_deref(), Some("198.51.100.1:51820"));
        assert!(!format!("{found:?}").contains("PRIVATE"));
    }

    #[test]
    fn missing_load_readings_do_not_pull_the_mean_down() {
        let start = Instant::now();
        let wall = 1_800_000_000_000;
        let mut record = Record::new(&counters());
        let mut sampler = Sampler::new(host(1_000, 1_000), counters());
        record.apply(start, wall, sampler.observe());
        sampler.host.files.remove("/proc/loadavg");
        record.apply(
            start + Duration::from_secs(5),
            wall + 5_000,
            sampler.observe(),
        );
        sampler
            .host
            .files
            .insert("/proc/loadavg".into(), "0.30 0.5 1.25 1/123 4567\n".into());
        record.apply(
            start + Duration::from_secs(10),
            wall + 10_000,
            sampler.observe(),
        );
        sampler.host.files.remove("/proc/loadavg");
        record.apply(
            start + Duration::from_secs(20),
            wall + 20_000,
            sampler.observe(),
        );
        record.apply(
            start + Duration::from_secs(35),
            wall + 35_000,
            sampler.observe(),
        );
        let history = record.status(start + Duration::from_secs(35), Some(0));
        assert_eq!(history.system_buckets.len(), 2);
        let first = history.system_buckets[0];
        assert_eq!(first.samples, 3);
        assert_eq!(first.load_sum, 77);
        let second = history.system_buckets[1];
        assert_eq!(second.samples, 1);
        assert_eq!(second.load_sum, 0);
    }

    #[test]
    fn record_turns_counters_into_rates_and_buckets() {
        let start = Instant::now();
        let wall = 1_800_000_000_000;
        let mut record = Record::new(&counters());
        let mut sampler = Sampler::new(host(1_000, 1_000), counters());
        record.apply(start, wall, sampler.observe());
        sampler.host = host(2_000, 1_000 + 125_000);
        record.apply(
            start + Duration::from_secs(5),
            wall + 5_000,
            sampler.observe(),
        );
        let live = record.status(start + Duration::from_secs(5), None);
        let system = live.system.unwrap_or_default();
        assert_eq!(system.cpu_permille, Some(1_000));
        assert_eq!(system.load_centi, [21, 50, 125]);
        assert_eq!(system.temperature_milli, Some(58_700));
        assert_eq!(system.uptime, 123_456);
        assert_eq!(live.links[0].rx_bps, Some(200_000));
        assert_eq!(live.links[1].endpoint.as_deref(), Some("203.0.113.9:51820"));
        assert_eq!(live.links[1].handshake_age, Some(5));
        record.apply(
            start + Duration::from_secs(20),
            wall + 20_000,
            sampler.observe(),
        );
        let history = record.status(start + Duration::from_secs(20), Some(0));
        assert_eq!(history.system_buckets.len(), 1);
        let bucket = history.system_buckets[0];
        assert_eq!(bucket.samples, 2);
        assert_eq!(bucket.mem_used_sum, 1_500);
        assert_eq!(bucket.temp_max, Some(58_700));
        assert_eq!(bucket.conntrack_limit, Some(16_384));
        let wan = history.link_buckets[0].buckets[0];
        assert_eq!(wan.rx_bytes, 125_000);
        assert_eq!(wan.seconds_ms, 5_000);
        assert_eq!(wan.rx_peak_bps, Some(200_000));
    }
}
