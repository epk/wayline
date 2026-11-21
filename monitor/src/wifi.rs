use crate::probe::{bucket_start, millis, unix_millis};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use wayline_protocol::{
    BUCKET_SECONDS, Rate, WifiBucket, WifiConfig, WifiLink, WifiSample, WifiState, WifiStatus,
};

const RETAINED: usize = 80;
const SNAPSHOT: usize = 8;
const RECENT: usize = 64;
const REPLY_LIMIT: usize = 256 * 1024;
const DISCONNECTED_DISCOVERY: Duration = Duration::from_secs(30);
const CONNECTED_DISCOVERY: Duration = Duration::from_secs(300);
const MAX_CANDIDATES: usize = 4;

pub trait Source {
    fn devices(&mut self) -> Option<Vec<String>>;
    fn call(&mut self, method: &str, device: &str) -> Option<Value>;
    fn bytes(&mut self, device: &str) -> Option<(u64, u64)>;
}

pub struct Ubus;

impl Source for Ubus {
    fn devices(&mut self) -> Option<Vec<String>> {
        let reply = ubus("devices", &json!({}))?;
        Some(
            reply
                .get("devices")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .filter(|name| valid_device(name))
                .map(str::to_owned)
                .collect(),
        )
    }

    fn call(&mut self, method: &str, device: &str) -> Option<Value> {
        valid_device(device).then_some(())?;
        ubus(method, &json!({ "device": device }))
    }

    fn bytes(&mut self, device: &str) -> Option<(u64, u64)> {
        valid_device(device).then_some(())?;
        let read = |name: &str| -> Option<u64> {
            fs::read_to_string(format!("/sys/class/net/{device}/statistics/{name}"))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        Some((read("tx_bytes")?, read("rx_bytes")?))
    }
}

fn ubus(method: &str, params: &Value) -> Option<Value> {
    let encoded = params.to_string();
    let reply = wayline_support::output(
        "ubus",
        &["-t", "2", "call", "iwinfo", method, &encoded],
        REPLY_LIMIT,
        3,
    )
    .ok()?;
    serde_json::from_slice(&reply).ok()
}

pub fn valid_device(name: &str) -> bool {
    (1..=15).contains(&name.len())
        && name != "lo"
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn integer(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64)
}

fn within<T: TryFrom<i64>>(value: Option<&Value>, low: i64, high: i64) -> Option<T> {
    integer(value)
        .filter(|number| (low..=high).contains(number))
        .and_then(|number| T::try_from(number).ok())
}

fn dbm(value: Option<&Value>) -> Option<i16> {
    within(value, -110, -1)
}

fn noise(value: Option<&Value>) -> Option<i16> {
    within(value, -120, -50)
}

fn kbps(value: Option<&Value>) -> Option<u32> {
    within(value, 1, 50_000_000)
}

fn counter(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64)
}

fn text(value: Option<&Value>, limit: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= limit && !text.contains(char::is_control))
        .map(str::to_owned)
}

fn bssid(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.to_ascii_lowercase();
    let octets: Vec<&str> = text.split(':').collect();
    (octets.len() == 6
        && octets
            .iter()
            .all(|octet| octet.len() == 2 && octet.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && octets.iter().any(|octet| *octet != "00")
        && octets.iter().any(|octet| *octet != "ff"))
    .then_some(text)
}

fn width(htmode: Option<&str>) -> Option<u16> {
    let digits: String = htmode?
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits
        .parse()
        .ok()
        .filter(|mhz| [20, 40, 80, 160, 320].contains(mhz))
}

fn encryption(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Some("Open".into());
    }
    if value.get("wep").is_some() {
        return Some("WEP".into());
    }
    let list = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.as_u64()
                    .map(|number| number.to_string())
                    .or_else(|| text(Some(item), 16))
            })
            .take(4)
            .collect()
    };
    let versions = list("wpa");
    let authentication = list("authentication");
    if versions.is_empty() {
        return None;
    }
    let mut description = format!("WPA{}", versions.join("/"));
    if !authentication.is_empty() {
        description.push(' ');
        description.push_str(&authentication.join("/").to_ascii_uppercase());
    }
    Some(description)
}

#[derive(Clone, Default)]
pub struct Info {
    pub client: bool,
    pub link: WifiLink,
}

pub fn parse_info(value: &Value) -> Info {
    let htmode = text(value.get("htmode"), 16);
    let link = WifiLink {
        ssid: text(value.get("ssid"), 64).unwrap_or_default(),
        bssid: bssid(value.get("bssid")).unwrap_or_default(),
        channel: within(value.get("channel"), 1, 255),
        frequency: within(value.get("frequency"), 2_000, 8_000),
        width: width(htmode.as_deref()),
        htmode,
        hwmode: text(value.get("hwmode"), 16),
        encryption: encryption(value.get("encryption")),
        hardware: value
            .get("hardware")
            .and_then(|hardware| text(hardware.get("name"), 64)),
        txpower: within(value.get("txpower"), -10, 40),
        signal: dbm(value.get("signal")),
        signal_avg: None,
        noise: noise(value.get("noise")),
        quality: within(value.get("quality"), 0, 255),
        quality_max: within(value.get("quality_max"), 1, 255),
        tx: Rate {
            kbps: kbps(value.get("bitrate")),
            ..Rate::default()
        },
        rx: Rate::default(),
        expected_kbps: None,
        connected_seconds: None,
        inactive_ms: None,
        busy_permille: None,
    };
    Info {
        client: value.get("mode").and_then(Value::as_str) == Some("Client"),
        link,
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Frames {
    pub sent: u64,
    pub retries: u64,
    pub failed: u64,
}

pub struct Station {
    pub signal: Option<i16>,
    pub signal_avg: Option<i16>,
    pub noise: Option<i16>,
    pub tx: Rate,
    pub rx: Rate,
    pub expected_kbps: Option<u32>,
    pub connected_seconds: Option<u64>,
    pub inactive_ms: Option<u64>,
    pub frames: Option<Frames>,
}

fn rate(value: Option<&Value>) -> Rate {
    let Some(value) = value else {
        return Rate::default();
    };
    let flag = |key: &str| value.get(key).and_then(Value::as_bool) == Some(true);
    let mode = if flag("eht") {
        "EHT"
    } else if flag("he") {
        "HE"
    } else if flag("vht") {
        "VHT"
    } else if flag("ht") {
        "HT"
    } else {
        "Legacy"
    };
    let mhz = within(value.get("mhz"), 5, 320).or_else(|| flag("40mhz").then_some(40));
    Rate {
        kbps: kbps(value.get("rate")),
        mode: Some(mode.into()),
        mcs: within(value.get("mcs"), 0, 31).filter(|_| mode != "Legacy"),
        nss: within(value.get("nss"), 1, 16),
        mhz,
        short_gi: value.get("short_gi").and_then(Value::as_bool),
    }
}

pub fn parse_station(value: &Value, bssid: &str) -> Option<Station> {
    let results = value.get("results")?.as_array()?;
    let entry = results
        .iter()
        .find(|entry| {
            entry
                .get("mac")
                .and_then(Value::as_str)
                .is_some_and(|mac| mac.eq_ignore_ascii_case(bssid))
        })
        .or(match results.as_slice() {
            [only] => Some(only),
            _ => None,
        })?;
    let tx = entry.get("tx");
    let frames = tx.and_then(|tx| {
        Some(Frames {
            sent: counter(tx.get("packets"))?,
            retries: counter(tx.get("retries"))?,
            failed: counter(tx.get("failed")).unwrap_or_default(),
        })
    });
    Some(Station {
        signal: dbm(entry.get("signal")),
        signal_avg: dbm(entry.get("signal_avg")),
        noise: noise(entry.get("noise")),
        tx: rate(tx),
        rx: rate(entry.get("rx")),
        expected_kbps: kbps(entry.get("thr")),
        connected_seconds: counter(entry.get("connected_time")),
        inactive_ms: counter(entry.get("inactive")),
        frames,
    })
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Survey {
    pub active: u64,
    pub busy: u64,
    pub rx: u64,
    pub tx: u64,
    pub noise: Option<i16>,
}

pub fn parse_survey(value: &Value, frequency: u32) -> Option<Survey> {
    let entry = value
        .get("results")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("mhz").and_then(Value::as_u64) == Some(u64::from(frequency)))?;
    Some(Survey {
        active: counter(entry.get("active_time"))?,
        busy: counter(entry.get("busy_time"))?,
        rx: counter(entry.get("rx_time")).unwrap_or_default(),
        tx: counter(entry.get("tx_time")).unwrap_or_default(),
        noise: noise(entry.get("noise")),
    })
}

pub struct Reading {
    pub interface: String,
    pub link: WifiLink,
    pub frames: Option<Frames>,
    pub survey: Option<Survey>,
    pub bytes: Option<(u64, u64)>,
}

pub enum Observation {
    QueryFailed,
    NoClientInterface,
    Disconnected { interface: String },
    Connected(Box<Reading>),
}

const fn connected(info: &Info) -> bool {
    !info.link.bssid.is_empty()
}

fn read<S: Source>(source: &mut S, interface: &str, info: Info) -> Reading {
    let mut link = info.link;
    let station = source
        .call("assoclist", interface)
        .and_then(|value| parse_station(&value, &link.bssid));
    let frames = station.and_then(|station| {
        link.signal = station.signal.or(link.signal);
        link.signal_avg = station.signal_avg;
        link.noise = link.noise.or(station.noise);
        if station.tx.kbps.is_some() {
            link.tx = station.tx;
        }
        link.rx = station.rx;
        link.expected_kbps = station.expected_kbps;
        link.connected_seconds = station.connected_seconds;
        link.inactive_ms = station.inactive_ms;
        station.frames
    });
    let survey = link.frequency.and_then(|frequency| {
        source
            .call("survey", interface)
            .and_then(|value| parse_survey(&value, frequency))
    });
    if let Some(survey) = survey {
        link.noise = survey.noise.or(link.noise);
    }
    if link.width.is_none() {
        link.width = link.tx.mhz.filter(|mhz| *mhz >= 20);
    }
    Reading {
        interface: interface.to_owned(),
        link,
        frames,
        survey,
        bytes: source.bytes(interface),
    }
}

pub struct Sampler<S: Source> {
    source: S,
    candidates: Vec<String>,
    discovered: Option<Instant>,
    failed: bool,
    connected: bool,
}

impl<S: Source> Sampler<S> {
    pub const fn new(source: S) -> Self {
        Self {
            source,
            candidates: Vec::new(),
            discovered: None,
            failed: false,
            connected: false,
        }
    }

    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }

    fn discover(&mut self, now: Instant) -> bool {
        self.discovered = Some(now);
        let Some(devices) = self.source.devices() else {
            return false;
        };
        let mut found: Vec<(bool, String)> = devices
            .into_iter()
            .filter_map(|device| {
                let info = parse_info(&self.source.call("info", &device)?);
                info.client.then(|| (connected(&info), device))
            })
            .collect();
        found.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        self.candidates = found
            .into_iter()
            .map(|(_, device)| device)
            .take(MAX_CANDIDATES)
            .collect();
        true
    }

    fn stale(&self, now: Instant) -> bool {
        let limit = if self.connected {
            CONNECTED_DISCOVERY
        } else {
            DISCONNECTED_DISCOVERY
        };
        self.discovered
            .is_none_or(|at| now.saturating_duration_since(at) >= limit)
    }

    pub fn observe(&mut self, now: Instant) -> Observation {
        if self.stale(now) {
            self.failed = !self.discover(now);
        }
        if self.failed || self.candidates.is_empty() {
            self.connected = false;
            return if self.failed {
                Observation::QueryFailed
            } else {
                Observation::NoClientInterface
            };
        }
        let mut first = None;
        let mut answered = false;
        for interface in self.candidates.clone() {
            let Some(value) = self.source.call("info", &interface) else {
                continue;
            };
            answered = true;
            let info = parse_info(&value);
            if !info.client {
                continue;
            }
            if connected(&info) {
                self.connected = true;
                return Observation::Connected(Box::new(read(&mut self.source, &interface, info)));
            }
            first.get_or_insert(interface);
        }
        self.connected = false;
        if !answered {
            self.discovered = None;
        }
        first.map_or(Observation::NoClientInterface, |interface| {
            Observation::Disconnected { interface }
        })
    }
}

#[derive(Clone)]
struct Baseline {
    interface: String,
    bssid: String,
    ssid: String,
    frequency: Option<u32>,
    connected_seconds: Option<u64>,
    frames: Option<Frames>,
    survey: Option<Survey>,
    bytes: Option<(u64, u64)>,
}

const fn grew(after: u64, before: u64) -> Option<u64> {
    after.checked_sub(before)
}

#[derive(Default)]
struct Delta {
    frames: Option<Frames>,
    survey: Option<Survey>,
    bytes: Option<(u64, u64)>,
    roamed: bool,
    dropped: bool,
}

impl Delta {
    fn between(
        previous: Option<&Baseline>,
        interface: &str,
        link: &WifiLink,
        frames: Option<Frames>,
        survey: Option<Survey>,
        bytes: Option<(u64, u64)>,
    ) -> Self {
        let Some(before) = previous else {
            return Self::default();
        };
        let same_interface = before.interface == interface;
        let same_association = same_interface
            && before.bssid == link.bssid
            && match (before.connected_seconds, link.connected_seconds) {
                (Some(before), Some(after)) => after >= before,
                _ => true,
            };
        let changed_access_point = before.bssid != link.bssid;
        let roamed = changed_access_point && before.ssid == link.ssid;
        let frames = before
            .frames
            .zip(frames)
            .filter(|_| same_association)
            .and_then(|(before, after)| {
                Some(Frames {
                    sent: grew(after.sent, before.sent)?,
                    retries: grew(after.retries, before.retries)?,
                    failed: grew(after.failed, before.failed)?,
                })
            });
        let survey = before
            .survey
            .zip(survey)
            .filter(|_| same_interface && before.frequency == link.frequency)
            .and_then(|(before, after)| {
                Some(Survey {
                    active: grew(after.active, before.active)?,
                    busy: grew(after.busy, before.busy)?,
                    rx: grew(after.rx, before.rx)?,
                    tx: grew(after.tx, before.tx)?,
                    noise: None,
                })
            })
            .filter(|delta| delta.active > 0 && delta.busy <= delta.active);
        let bytes = before.bytes.zip(bytes).filter(|_| same_interface).and_then(
            |((tx_before, rx_before), (tx_after, rx_after))| {
                Some((grew(tx_after, tx_before)?, grew(rx_after, rx_before)?))
            },
        );
        Self {
            frames,
            survey,
            bytes,
            roamed,
            dropped: !roamed && !same_association,
        }
    }

    fn add(&self, bucket: &mut WifiBucket, link: &WifiLink) {
        bucket.samples = bucket.samples.saturating_add(1);
        bucket.connected = bucket.connected.saturating_add(1);
        if let Some(signal) = link.signal {
            bucket.signal_n += 1;
            bucket.signal_sum += i64::from(signal);
            bucket.signal_min = Some(bucket.signal_min.map_or(signal, |low| low.min(signal)));
            bucket.signal_max = Some(bucket.signal_max.map_or(signal, |high| high.max(signal)));
        }
        if let Some(noise) = link.noise {
            bucket.noise_n += 1;
            bucket.noise_sum += i64::from(noise);
        }
        if let Some(rate) = link.tx.kbps {
            bucket.tx_rate_n += 1;
            bucket.tx_rate_sum += u64::from(rate);
            bucket.tx_rate_min = Some(bucket.tx_rate_min.map_or(rate, |low| low.min(rate)));
        }
        if let Some(rate) = link.rx.kbps {
            bucket.rx_rate_n += 1;
            bucket.rx_rate_sum += u64::from(rate);
        }
        if let Some(expected) = link.expected_kbps {
            bucket.expected_n += 1;
            bucket.expected_sum += u64::from(expected);
        }
        if let Some(delta) = self.frames {
            bucket.frames += delta.sent;
            bucket.retries += delta.retries;
            bucket.failed += delta.failed;
        }
        if let Some((tx, rx)) = self.bytes {
            bucket.tx_bytes += tx;
            bucket.rx_bytes += rx;
        }
        if let Some(delta) = self.survey {
            bucket.active_ms += delta.active;
            bucket.busy_ms += delta.busy;
            bucket.rx_ms += delta.rx;
            bucket.tx_ms += delta.tx;
        }
        bucket.roams += u32::from(self.roamed);
        bucket.disconnects += u32::from(self.dropped);
    }
}

pub struct Record {
    state: WifiState,
    interface: Option<String>,
    candidates: Vec<String>,
    sampled: Option<Instant>,
    link: Option<WifiLink>,
    previous: Option<Baseline>,
    recent: VecDeque<WifiSample>,
    open: Option<WifiBucket>,
    closed: VecDeque<WifiBucket>,
}

impl Default for Record {
    fn default() -> Self {
        Self::new()
    }
}

impl Record {
    pub const fn new() -> Self {
        Self {
            state: WifiState::Starting,
            interface: None,
            candidates: Vec::new(),
            sampled: None,
            link: None,
            previous: None,
            recent: VecDeque::new(),
            open: None,
            closed: VecDeque::new(),
        }
    }

    fn close(&mut self, wall_ms: u64) {
        let due = self
            .open
            .is_some_and(|bucket| wall_ms >= (bucket.at + BUCKET_SECONDS) * 1000);
        if !due {
            return;
        }
        if let Some(bucket) = self.open.take()
            && bucket.samples > 0
        {
            if self.closed.len() == RETAINED {
                self.closed.pop_front();
            }
            self.closed.push_back(bucket);
        }
    }

    fn bucket(&mut self, wall_ms: u64) -> &mut WifiBucket {
        self.close(wall_ms);
        self.open
            .get_or_insert_with(|| WifiBucket::empty(bucket_start(wall_ms)))
    }

    fn remember(&mut self, sample: WifiSample) {
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(sample);
    }

    pub fn apply(
        &mut self,
        now: Instant,
        wall_ms: u64,
        observation: Observation,
        candidates: &[String],
    ) {
        self.sampled = Some(now);
        self.candidates = candidates.to_vec();
        let previous = self.previous.take();
        let reading = match observation {
            Observation::QueryFailed | Observation::NoClientInterface => {
                self.state = if matches!(observation, Observation::QueryFailed) {
                    WifiState::QueryFailed
                } else {
                    WifiState::NoClientInterface
                };
                self.interface = None;
                self.link = None;
                if previous.is_some() {
                    let bucket = self.bucket(wall_ms);
                    bucket.samples = bucket.samples.saturating_add(1);
                    bucket.disconnects = bucket.disconnects.saturating_add(1);
                } else {
                    self.close(wall_ms);
                }
                return;
            }
            Observation::Disconnected { interface } => {
                self.state = WifiState::Disconnected;
                self.interface = Some(interface);
                self.link = None;
                let bucket = self.bucket(wall_ms);
                bucket.samples = bucket.samples.saturating_add(1);
                if previous.is_some() {
                    bucket.disconnects = bucket.disconnects.saturating_add(1);
                }
                self.remember(WifiSample {
                    at_ms: wall_ms,
                    signal: None,
                    tx_kbps: None,
                    rx_kbps: None,
                });
                return;
            }
            Observation::Connected(reading) => reading,
        };
        let Reading {
            interface,
            mut link,
            frames,
            survey,
            bytes,
        } = *reading;
        let delta = Delta::between(previous.as_ref(), &interface, &link, frames, survey, bytes);
        link.busy_permille = delta.survey.and_then(|survey| {
            u16::try_from(survey.busy.saturating_mul(1000) / survey.active).ok()
        });
        delta.add(self.bucket(wall_ms), &link);
        self.remember(WifiSample {
            at_ms: wall_ms,
            signal: link.signal,
            tx_kbps: link.tx.kbps,
            rx_kbps: link.rx.kbps,
        });
        self.previous = Some(Baseline {
            interface: interface.clone(),
            bssid: link.bssid.clone(),
            ssid: link.ssid.clone(),
            frequency: link.frequency,
            connected_seconds: link.connected_seconds,
            frames,
            survey,
            bytes,
        });
        self.state = WifiState::Connected;
        self.interface = Some(interface);
        self.link = Some(link);
    }

    pub fn status(&self, now: Instant, since: Option<u64>) -> WifiStatus {
        let skip = if since.is_some() {
            0
        } else {
            self.closed.len().saturating_sub(SNAPSHOT)
        };
        WifiStatus {
            state: self.state,
            interface: self.interface.clone(),
            candidates: self.candidates.clone(),
            sample_age_ms: self
                .sampled
                .map(|at| millis(now.saturating_duration_since(at))),
            link: self.link.clone(),
            recent: self.recent.iter().copied().collect(),
            buckets: self
                .closed
                .iter()
                .skip(skip)
                .filter(|bucket| since.is_none_or(|cursor| bucket.at > cursor))
                .copied()
                .collect(),
        }
    }
}

pub struct Wifi {
    record: Arc<Mutex<Record>>,
}

impl Wifi {
    pub fn start(config: &WifiConfig) -> io::Result<Self> {
        let record = Arc::new(Mutex::new(Record::new()));
        let shared = Arc::clone(&record);
        let interval = Duration::from_millis(config.interval_ms);
        thread::Builder::new()
            .name("wifi".into())
            .spawn(move || run(&shared, Sampler::new(Ubus), interval))?;
        Ok(Self { record })
    }

    pub fn status(&self, now: Instant, since: Option<u64>) -> WifiStatus {
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .status(now, since)
    }
}

fn run<S: Source>(record: &Mutex<Record>, mut sampler: Sampler<S>, interval: Duration) {
    let mut next = Instant::now();
    loop {
        let observation = sampler.observe(Instant::now());
        record.lock().unwrap_or_else(PoisonError::into_inner).apply(
            Instant::now(),
            unix_millis(),
            observation,
            sampler.candidates(),
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
    use super::{Observation, Record, Sampler, Source, parse_info, parse_station, parse_survey};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};
    use wayline_protocol::WifiState;

    struct Fake {
        devices: Option<Vec<String>>,
        replies: BTreeMap<(String, String), Value>,
        bytes: (u64, u64),
    }

    impl Source for Fake {
        fn devices(&mut self) -> Option<Vec<String>> {
            self.devices.clone()
        }

        fn call(&mut self, method: &str, device: &str) -> Option<Value> {
            self.replies
                .get(&(method.to_owned(), device.to_owned()))
                .cloned()
        }

        fn bytes(&mut self, _: &str) -> Option<(u64, u64)> {
            Some(self.bytes)
        }
    }

    fn info(mode: &str, bssid: &str, ssid: &str) -> Value {
        json!({
            "phy": "phy1", "ssid": ssid, "bssid": bssid, "mode": mode, "channel": 36,
            "frequency": 5180, "txpower": 20, "quality": 56, "quality_max": 70,
            "signal": -54, "noise": 0, "bitrate": 866_700, "htmode": "HE80",
            "hwmode": "ax", "hardware": {"name": "Generic MAC80211"},
            "encryption": {"enabled": true, "wpa": [2, 3], "authentication": ["psk", "sae"], "ciphers": ["ccmp"]}
        })
    }

    fn station(packets: u64, retries: u64, connected: u64) -> Value {
        json!({"results": [{
            "mac": "AA:BB:CC:DD:EE:01", "signal": -52, "signal_avg": -53, "noise": -95,
            "inactive": 120, "connected_time": connected, "thr": 420_500,
            "rx": {"ht": false, "vht": false, "he": true, "eht": false, "mhz": 80, "rate": 866_700,
                   "mcs": 9, "nss": 2, "he_gi": 0, "he_dcm": 0, "packets": 10, "bytes": 1000, "drop_misc": 0},
            "tx": {"ht": false, "vht": true, "he": false, "eht": false, "mhz": 80, "rate": 650_000,
                   "mcs": 7, "nss": 2, "short_gi": true, "packets": packets, "bytes": 2000,
                   "retries": retries, "failed": 1}
        }]})
    }

    fn survey(active: u64, busy: u64) -> Value {
        json!({"results": [
            {"mhz": 5180, "noise": -92, "active_time": active, "busy_time": busy, "busy_time_ext": 0, "rx_time": busy / 2, "tx_time": busy / 4},
            {"mhz": 5200, "noise": -91, "active_time": 9, "busy_time": 1}
        ]})
    }

    fn fake() -> Fake {
        let mut replies = BTreeMap::new();
        let key = |method: &str, device: &str| (method.to_owned(), device.to_owned());
        replies.insert(
            key("info", "ra0"),
            info("Master", "aa:bb:cc:dd:ee:99", "Home"),
        );
        replies.insert(
            key("info", "apclix0"),
            info("Client", "AA:BB:CC:DD:EE:01", "Hotel"),
        );
        replies.insert(
            key("info", "apcli0"),
            info("Client", "00:00:00:00:00:00", ""),
        );
        replies.insert(key("assoclist", "apclix0"), station(100, 10, 50));
        replies.insert(key("survey", "apclix0"), survey(1_000, 200));
        Fake {
            devices: Some(vec!["ra0".into(), "apcli0".into(), "apclix0".into()]),
            replies,
            bytes: (5_000, 9_000),
        }
    }

    #[test]
    fn info_parsing_rejects_placeholders() {
        let parsed = parse_info(
            &json!({"mode": "Client", "signal": -256, "noise": 0, "bssid": "00:00:00:00:00:00", "htmode": "VHT160"}),
        );
        assert!(parsed.client);
        assert_eq!(parsed.link.signal, None);
        assert_eq!(parsed.link.noise, None);
        assert_eq!(parsed.link.bssid, "");
        assert_eq!(parsed.link.width, Some(160));
        let parsed = parse_info(&info("Client", "AA:BB:CC:DD:EE:01", "Hotel"));
        assert_eq!(parsed.link.bssid, "aa:bb:cc:dd:ee:01");
        assert_eq!(parsed.link.encryption.as_deref(), Some("WPA2/3 PSK/SAE"));
        assert_eq!(parsed.link.width, Some(80));
        assert_eq!(parsed.link.tx.kbps, Some(866_700));
    }

    #[test]
    fn station_and_survey_parse() -> Result<(), &'static str> {
        let parsed =
            parse_station(&station(100, 10, 50), "aa:bb:cc:dd:ee:01").ok_or("missing station")?;
        assert_eq!(parsed.signal, Some(-52));
        assert_eq!(parsed.tx.mode.as_deref(), Some("VHT"));
        assert_eq!(parsed.tx.mcs, Some(7));
        assert_eq!(parsed.rx.mode.as_deref(), Some("HE"));
        assert_eq!(parsed.expected_kbps, Some(420_500));
        assert_eq!(parsed.frames.map(|frames| frames.retries), Some(10));
        assert!(parse_station(&json!({"results": []}), "aa").is_none());
        let found = parse_survey(&survey(1_000, 200), 5180);
        assert_eq!(found.map(|survey| survey.busy), Some(200));
        assert_eq!(found.and_then(|survey| survey.noise), Some(-92));
        assert!(parse_survey(&survey(1, 1), 2412).is_none());
        Ok(())
    }

    #[test]
    fn sampler_finds_the_connected_client_interface() -> Result<(), &'static str> {
        let mut sampler = Sampler::new(fake());
        let Observation::Connected(reading) = sampler.observe(Instant::now()) else {
            return Err("expected a connection");
        };
        assert_eq!(reading.interface, "apclix0");
        assert_eq!(sampler.candidates(), ["apclix0", "apcli0"]);
        assert_eq!(reading.link.noise, Some(-92));
        assert_eq!(reading.link.tx.kbps, Some(650_000));
        let mut none = fake();
        none.devices = Some(vec!["ra0".into()]);
        assert!(matches!(
            Sampler::new(none).observe(Instant::now()),
            Observation::NoClientInterface
        ));
        let mut failed = fake();
        failed.devices = None;
        assert!(matches!(
            Sampler::new(failed).observe(Instant::now()),
            Observation::QueryFailed
        ));
        Ok(())
    }

    #[test]
    fn failed_discovery_stays_failed_until_the_next_attempt() {
        let start = Instant::now();
        let mut failed = fake();
        failed.devices = None;
        let mut sampler = Sampler::new(failed);
        for offset in [0, 5, 10, 25] {
            assert!(matches!(
                sampler.observe(start + Duration::from_secs(offset)),
                Observation::QueryFailed
            ));
        }
        sampler.source.devices = Some(vec!["ra0".into()]);
        assert!(matches!(
            sampler.observe(start + Duration::from_secs(29)),
            Observation::QueryFailed
        ));
        assert!(matches!(
            sampler.observe(start + Duration::from_secs(31)),
            Observation::NoClientInterface
        ));
    }

    #[test]
    fn record_accumulates_deltas_roams_and_drops() {
        let start = Instant::now();
        let base = 1_800_000_000_000;
        let mut source = fake();
        let mut record = Record::new();
        let mut sampler = Sampler::new(fake());
        let mut step = |record: &mut Record, source: &Fake, offset: u64| {
            sampler.source = Fake {
                devices: source.devices.clone(),
                replies: source.replies.clone(),
                bytes: source.bytes,
            };
            let observation = sampler.observe(start + Duration::from_millis(offset));
            record.apply(
                start + Duration::from_millis(offset),
                base + offset,
                observation,
                &[],
            );
        };
        step(&mut record, &source, 0);
        let key = |method: &str| (method.to_owned(), "apclix0".to_owned());
        source
            .replies
            .insert(key("assoclist"), station(150, 25, 55));
        source.replies.insert(key("survey"), survey(6_000, 1_200));
        source.bytes = (8_000, 19_000);
        step(&mut record, &source, 5_000);
        source
            .replies
            .insert(key("info"), info("Client", "AA:BB:CC:DD:EE:02", "Hotel"));
        let mut roamed = station(5, 0, 1);
        roamed["results"][0]["mac"] = json!("AA:BB:CC:DD:EE:02");
        source.replies.insert(key("assoclist"), roamed);
        step(&mut record, &source, 10_000);
        source
            .replies
            .insert(key("info"), info("Client", "00:00:00:00:00:00", ""));
        step(&mut record, &source, 20_000);
        step(&mut record, &source, 40_000);
        let status = record.status(start + Duration::from_secs(40), Some(0));
        assert!(status.state == WifiState::Disconnected);
        let total = status.buckets.iter().fold(
            wayline_protocol::WifiBucket::empty(0),
            |mut sum, bucket| {
                sum.merge(bucket);
                sum
            },
        );
        assert_eq!(total.connected, 3);
        assert_eq!(total.frames, 50);
        assert_eq!(total.retries, 15);
        assert_eq!(total.tx_bytes, 3_000);
        assert_eq!(total.rx_bytes, 10_000);
        assert_eq!(total.active_ms, 5_000);
        assert_eq!(total.busy_ms, 1_000);
        assert_eq!(total.roams, 1);
        assert_eq!(total.disconnects, 1);
        assert_eq!(total.signal_min, Some(-52));
        assert_eq!(total.samples, 4);
    }
}
