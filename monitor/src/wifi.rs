use crate::nl80211::{self, Bss, Channel, Interface, Nl80211, Peer};
use crate::probe::{boot_start, bucket_start, millis, unix_millis};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use wayline_protocol::{
    BUCKET_SECONDS, WifiBucket, WifiChange, WifiConfig, WifiIdentity, WifiLink, WifiSample,
    WifiState, WifiStatus, access_point_id,
};

const RETAINED: usize = 80;
const SNAPSHOT: usize = 8;
const RECENT: usize = 64;
const CHANGES: usize = 64;
const REOPEN: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_millis(300);
const SERVING_REFRESH: Duration = Duration::from_secs(60);

pub trait Source {
    fn interfaces(&mut self) -> Option<Vec<Interface>>;
    fn stations(&mut self, index: u32) -> Option<Vec<Peer>>;
    fn survey(&mut self, index: u32) -> Option<Vec<Channel>>;
    fn associated(&mut self, index: u32) -> Option<Bss>;
    fn bytes(&mut self, device: &str) -> Option<(u64, u64)>;
    fn driver(&mut self, device: &str) -> Option<String>;
    fn mlme_group(&self) -> Option<u32> {
        None
    }
}

pub struct Kernel {
    link: Option<Nl80211>,
    failed: Option<Instant>,
}

impl Kernel {
    pub const fn new() -> Self {
        Self {
            link: None,
            failed: None,
        }
    }

    fn call<T>(&mut self, query: impl FnOnce(&mut Nl80211) -> io::Result<T>) -> Option<T> {
        if self.link.is_none() && self.failed.is_none_or(|at| at.elapsed() >= REOPEN) {
            match Nl80211::open() {
                Ok(link) => self.link = Some(link),
                Err(_) => self.failed = Some(Instant::now()),
            }
        }
        let link = self.link.as_mut()?;
        let answer = query(link).ok();
        if answer.is_none() {
            self.link = None;
            self.failed = Some(Instant::now());
        }
        answer
    }
}

impl Source for Kernel {
    fn interfaces(&mut self) -> Option<Vec<Interface>> {
        self.call(Nl80211::interfaces)
    }

    fn stations(&mut self, index: u32) -> Option<Vec<Peer>> {
        self.call(|link| link.stations(index))
    }

    fn survey(&mut self, index: u32) -> Option<Vec<Channel>> {
        self.call(|link| link.survey(index))
    }

    fn associated(&mut self, index: u32) -> Option<Bss> {
        self.call(|link| link.associated(index)).flatten()
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

    fn driver(&mut self, device: &str) -> Option<String> {
        valid_device(device).then_some(())?;
        let target = fs::read_link(format!("/sys/class/net/{device}/device/driver")).ok()?;
        Some(target.file_name()?.to_string_lossy().into_owned())
    }

    fn mlme_group(&self) -> Option<u32> {
        self.link.as_ref().and_then(Nl80211::mlme_group)
    }
}

pub fn valid_device(name: &str) -> bool {
    (1..=15).contains(&name.len())
        && name != "lo"
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Frames {
    pub sent: u64,
    pub retries: u64,
    pub failed: u64,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Survey {
    pub active: u64,
    pub busy: u64,
    pub rx: u64,
    pub tx: u64,
    pub noise: Option<i16>,
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

fn generation(mode: Option<&str>, frequency: Option<u32>) -> (Option<&'static str>, &'static str) {
    match mode {
        Some("EHT") => (Some("EHT"), "be"),
        Some("HE") => (Some("HE"), "ax"),
        Some("VHT") => (Some("VHT"), "ac"),
        Some("HT") => (Some("HT"), "n"),
        _ => (
            None,
            if frequency.is_some_and(|mhz| mhz < 3_000) {
                "g"
            } else {
                "a"
            },
        ),
    }
}

pub struct Sampler<S: Source> {
    source: S,
    candidates: Vec<String>,
    watched: Vec<u32>,
    serving: Option<Serving>,
}

struct Serving {
    bssid: String,
    security: Option<String>,
    ssid: Option<String>,
    frequency: Option<u32>,
    width: Option<u16>,
    read: Instant,
}

impl<S: Source> Sampler<S> {
    pub const fn new(source: S) -> Self {
        Self {
            source,
            candidates: Vec::new(),
            watched: Vec::new(),
            serving: None,
        }
    }

    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }

    pub fn watched(&self) -> &[u32] {
        &self.watched
    }

    pub fn mlme_group(&self) -> Option<u32> {
        self.source.mlme_group()
    }

    pub fn observe(&mut self, _now: Instant) -> Observation {
        let Some(all) = self.source.interfaces() else {
            self.candidates.clear();
            return Observation::QueryFailed;
        };
        let mut stations: Vec<Interface> = all
            .into_iter()
            .filter(|item| item.station && valid_device(&item.name))
            .collect();
        stations.sort_by(|left, right| {
            left.ssid
                .is_empty()
                .cmp(&right.ssid.is_empty())
                .then_with(|| left.name.cmp(&right.name))
        });
        self.watched = stations.iter().map(|item| item.index).collect();
        self.candidates = stations.iter().map(|item| item.name.clone()).collect();
        if stations.is_empty() {
            return Observation::NoClientInterface;
        }
        let mut answered = false;
        let mut first = None;
        for item in stations {
            let Some(peers) = self.source.stations(item.index) else {
                continue;
            };
            answered = true;
            if let Some(peer) = peers.into_iter().next() {
                let name = item.name.clone();
                self.candidates.retain(|other| *other != name);
                self.candidates.insert(0, name);
                return Observation::Connected(Box::new(self.read(item, peer)));
            }
            first.get_or_insert(item.name);
        }
        if !answered {
            return Observation::QueryFailed;
        }
        first.map_or(Observation::NoClientInterface, |interface| {
            Observation::Disconnected { interface }
        })
    }

    fn refresh(&mut self, index: u32, mac: &str) {
        let stale = self.serving.as_ref().is_none_or(|serving| {
            serving.bssid != mac || serving.read.elapsed() >= SERVING_REFRESH
        });
        if stale {
            let bss = self
                .source
                .associated(index)
                .filter(|bss| bss.bssid == *mac);
            let width = self
                .serving
                .as_ref()
                .filter(|serving| serving.bssid == *mac)
                .and_then(|serving| serving.width);
            self.serving = Some(Serving {
                bssid: mac.to_owned(),
                security: bss.as_ref().and_then(|bss| bss.security.clone()),
                ssid: bss.as_ref().and_then(|bss| bss.ssid.clone()),
                frequency: bss.and_then(|bss| bss.frequency),
                width,
                read: Instant::now(),
            });
        }
    }

    fn read(&mut self, item: Interface, peer: Peer) -> Reading {
        self.refresh(item.index, &peer.mac);
        let serving = self.serving.as_mut();
        let frequency = serving
            .as_ref()
            .and_then(|serving| serving.frequency)
            .or(item.frequency);
        let on_channel = item.frequency == frequency;
        let width = if on_channel {
            item.width
        } else {
            serving.as_ref().and_then(|serving| serving.width)
        }
        .or_else(|| peer.tx.mhz.filter(|mhz| *mhz >= 20));
        let (security, beacon_ssid) = serving.map_or((None, None), |serving| {
            if on_channel && item.width.is_some() {
                serving.width = item.width;
            }
            (serving.security.clone(), serving.ssid.clone())
        });
        let channel = self.source.survey(item.index).and_then(|list| {
            list.iter()
                .find(|channel| Some(channel.frequency) == frequency)
                .or_else(|| list.iter().find(|channel| channel.in_use))
                .copied()
        });
        let (family, hwmode) = generation(
            peer.tx.mode.as_deref().or(peer.rx.mode.as_deref()),
            frequency,
        );
        let survey = channel.and_then(|channel| {
            Some(Survey {
                active: channel.active_ms?,
                busy: channel.busy_ms?,
                rx: channel.rx_ms.unwrap_or(0),
                tx: channel.tx_ms.unwrap_or(0),
                noise: channel.noise,
            })
        });
        let frames = peer
            .tx_packets
            .zip(peer.tx_retries)
            .map(|(sent, retries)| Frames {
                sent,
                retries,
                failed: peer.tx_failed.unwrap_or(0),
            });
        let ssid = if item.ssid.is_empty() {
            beacon_ssid.unwrap_or_default()
        } else {
            item.ssid.clone()
        };
        let link = WifiLink {
            ssid,
            bssid: peer.mac.clone(),
            channel: frequency.and_then(nl80211::channel_of),
            frequency,
            width,
            htmode: family
                .zip(width)
                .map(|(family, mhz)| format!("{family}{mhz}")),
            hwmode: Some(hwmode.to_owned()),
            encryption: security,
            hardware: self.source.driver(&item.name),
            txpower: item.txpower,
            signal: peer.signal,
            signal_avg: peer.signal_avg,
            noise: channel.and_then(|channel| channel.noise),
            quality: None,
            quality_max: None,
            tx: peer.tx,
            rx: peer.rx,
            expected_kbps: peer.expected_kbps,
            connected_seconds: peer.connected_seconds,
            inactive_ms: peer.inactive_ms,
            busy_permille: None,
        };
        Reading {
            bytes: self.source.bytes(&item.name),
            interface: item.name,
            link,
            frames,
            survey,
        }
    }
}

#[derive(Clone)]
struct Baseline {
    interface: String,
    bssid: String,
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
    restarted: bool,
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
            restarted: !same_association,
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
    }
}

fn transition(
    previous: Option<&WifiIdentity>,
    now: &WifiIdentity,
    counted: bool,
    restarted: bool,
) -> (bool, bool) {
    let Some(previous) = previous else {
        return (false, false);
    };
    if previous.bssid == now.bssid {
        (false, restarted && !counted)
    } else if previous.ssid == now.ssid {
        (true, false)
    } else {
        (false, !counted)
    }
}

pub struct Record {
    state: WifiState,
    interface: Option<String>,
    candidates: Vec<String>,
    sampled: Option<Instant>,
    link: Option<WifiLink>,
    previous: Option<Baseline>,
    last: Option<WifiIdentity>,
    lost: bool,
    changes: VecDeque<WifiChange>,
    tally: Vec<(u64, u32)>,
    tally_at: u64,
    pending_roams: u32,
    pending_drops: u32,
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
            last: None,
            lost: false,
            changes: VecDeque::new(),
            tally: Vec::new(),
            tally_at: 0,
            pending_roams: 0,
            pending_drops: 0,
            recent: VecDeque::new(),
            open: None,
            closed: VecDeque::new(),
        }
    }

    fn credit(&mut self, wall_ms: u64, ap: u64) {
        let bucket = self.bucket(wall_ms);
        let (at, current) = (bucket.at, bucket.ap);
        if self.tally_at != at {
            self.tally.clear();
            self.tally_at = at;
        }
        match self.tally.iter_mut().find(|(id, _)| *id == ap) {
            Some(entry) => entry.1 += 1,
            None => self.tally.push((ap, 1)),
        }
        let count = |id: u64| {
            self.tally
                .iter()
                .find(|(other, _)| *other == id)
                .map_or(0, |(_, count)| *count)
        };
        let most = self
            .tally
            .iter()
            .map(|(_, count)| *count)
            .max()
            .unwrap_or(0);
        let best = if count(current) == most {
            current
        } else {
            self.tally
                .iter()
                .find(|(_, count)| *count == most)
                .map_or(0, |(id, _)| *id)
        };
        self.bucket(wall_ms).ap = best;
    }

    fn gap(&mut self, wall_ms: u64) {
        let ap = self
            .last
            .as_ref()
            .map_or(0, |last| access_point_id(&last.bssid));
        let count = self.last.is_some() && !self.lost;
        let bucket = self.bucket(wall_ms);
        bucket.samples = bucket.samples.saturating_add(1);
        if bucket.ap == 0 {
            bucket.ap = ap;
        }
        if count {
            bucket.disconnects = bucket.disconnects.saturating_add(1);
        }
        self.lost |= count;
        self.settle(wall_ms);
    }

    fn settle(&mut self, wall_ms: u64) {
        let (roams, drops) = (self.pending_roams, self.pending_drops);
        if roams == 0 && drops == 0 {
            return;
        }
        let bucket = self.bucket(wall_ms);
        bucket.roams = bucket.roams.saturating_add(roams);
        bucket.disconnects = bucket.disconnects.saturating_add(drops);
        self.pending_roams = 0;
        self.pending_drops = 0;
    }

    pub fn react(
        &mut self,
        now: Instant,
        wall_ms: u64,
        observation: Observation,
        candidates: &[String],
    ) {
        self.sampled = Some(now);
        self.candidates = candidates.to_vec();
        match observation {
            Observation::QueryFailed => {}
            Observation::NoClientInterface => {
                self.lose();
                self.state = WifiState::NoClientInterface;
                self.interface = None;
                self.link = None;
            }
            Observation::Disconnected { interface } => {
                self.lose();
                self.state = WifiState::Disconnected;
                self.interface = Some(interface);
                self.link = None;
            }
            Observation::Connected(reading) => {
                let Reading {
                    interface, link, ..
                } = *reading;
                let identity = WifiIdentity::of(&link);
                let restarted = self.previous.as_ref().is_some_and(|before| {
                    before.bssid == identity.bssid
                        && matches!(
                            (before.connected_seconds, link.connected_seconds),
                            (Some(before), Some(after)) if after < before
                        )
                });
                let (roamed, dropped) =
                    transition(self.last.as_ref(), &identity, self.lost, restarted);
                self.pending_roams = self.pending_roams.saturating_add(u32::from(roamed));
                self.pending_drops = self.pending_drops.saturating_add(u32::from(dropped));
                if restarted || self.last.as_ref() != Some(&identity) {
                    self.previous = None;
                }
                if self.last.as_ref() != Some(&identity) {
                    let from = self.last.take();
                    self.note(WifiChange {
                        at_ms: wall_ms,
                        from,
                        to: identity.clone(),
                    });
                }
                self.last = Some(identity);
                self.lost = false;
                self.state = WifiState::Connected;
                self.interface = Some(interface);
                self.link = Some(link);
            }
        }
    }

    fn lose(&mut self) {
        if self.last.is_some() && !self.lost {
            self.pending_drops = self.pending_drops.saturating_add(1);
            self.lost = true;
        }
        self.previous = None;
    }

    fn note(&mut self, change: WifiChange) {
        if self.changes.len() == CHANGES {
            self.changes.pop_front();
        }
        self.changes.push_back(change);
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
        self.open.get_or_insert_with(|| {
            let at = bucket_start(wall_ms);
            WifiBucket {
                uptime_ms: boot_start(wall_ms, at),
                ..WifiBucket::empty(at)
            }
        })
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
            Observation::QueryFailed => {
                self.state = WifiState::QueryFailed;
                self.interface = None;
                self.link = None;
                self.previous = previous;
                self.close(wall_ms);
                return;
            }
            Observation::NoClientInterface => {
                self.state = WifiState::NoClientInterface;
                self.interface = None;
                self.link = None;
                if self.last.is_some() {
                    self.gap(wall_ms);
                } else {
                    self.close(wall_ms);
                }
                return;
            }
            Observation::Disconnected { interface } => {
                self.state = WifiState::Disconnected;
                self.interface = Some(interface);
                self.link = None;
                self.gap(wall_ms);
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
        let identity = WifiIdentity::of(&link);
        let restarted = previous.is_some() && delta.restarted;
        let (roamed, dropped) = transition(self.last.as_ref(), &identity, self.lost, restarted);
        self.credit(wall_ms, access_point_id(&identity.bssid));
        let bucket = self.bucket(wall_ms);
        delta.add(bucket, &link);
        bucket.roams = bucket.roams.saturating_add(u32::from(roamed));
        bucket.disconnects = bucket.disconnects.saturating_add(u32::from(dropped));
        self.settle(wall_ms);
        if self.last.as_ref() != Some(&identity) {
            let from = self.last.take();
            self.note(WifiChange {
                at_ms: wall_ms,
                from,
                to: identity.clone(),
            });
        }
        self.last = Some(identity);
        self.lost = false;
        self.remember(WifiSample {
            at_ms: wall_ms,
            signal: link.signal,
            tx_kbps: link.tx.kbps,
            rx_kbps: link.rx.kbps,
        });
        self.previous = Some(Baseline {
            interface: interface.clone(),
            bssid: link.bssid.clone(),
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
            changes: self.changes.iter().cloned().collect(),
        }
    }
}

pub struct Wifi {
    record: Arc<Mutex<Record>>,
    stop: Arc<AtomicBool>,
    interval_ms: u64,
}

impl Wifi {
    pub fn start(config: &WifiConfig) -> io::Result<Self> {
        let record = Arc::new(Mutex::new(Record::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = (Arc::clone(&record), Arc::clone(&stop));
        let interval = Duration::from_millis(config.interval_ms);
        thread::Builder::new()
            .name("wifi".into())
            .spawn(move || run(&shared.0, &shared.1, Sampler::new(Kernel::new()), interval))?;
        Ok(Self {
            record,
            stop,
            interval_ms: config.interval_ms,
        })
    }

    pub const fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn status(&self, now: Instant, since: Option<u64>) -> WifiStatus {
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .status(now, since)
    }
}

fn run<S: Source>(
    record: &Mutex<Record>,
    stop: &AtomicBool,
    mut sampler: Sampler<S>,
    interval: Duration,
) {
    let mut events: Option<nl80211::Events> = None;
    let mut next = Instant::now();
    let mut timed = true;
    while !stop.load(Ordering::Relaxed) {
        let observation = sampler.observe(Instant::now());
        {
            let mut record = record.lock().unwrap_or_else(PoisonError::into_inner);
            if timed {
                record.apply(
                    Instant::now(),
                    unix_millis(),
                    observation,
                    sampler.candidates(),
                );
            } else {
                record.react(
                    Instant::now(),
                    unix_millis(),
                    observation,
                    sampler.candidates(),
                );
            }
        }
        let group = sampler.mlme_group();
        if events
            .as_ref()
            .is_none_or(|open| open.live() != (true, group.is_some_and(|id| id <= 32)))
            && group.is_some()
        {
            events = Some(nl80211::Events::open(group));
        }
        if timed {
            next += interval;
            let now = Instant::now();
            if next <= now {
                next = now + interval;
            }
        }
        timed = true;
        loop {
            let wait = next.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                break;
            }
            let woke = events.as_mut().map_or_else(
                || {
                    thread::sleep(wait);
                    false
                },
                |events| events.wait(wait, sampler.watched()),
            );
            if woke {
                thread::sleep(SETTLE.min(next.saturating_duration_since(Instant::now())));
                if let Some(events) = events.as_mut() {
                    events.wait(Duration::ZERO, sampler.watched());
                }
                timed = Instant::now() >= next;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Observation, Reading, Record, Sampler, Source};
    use crate::nl80211::{
        self, Bss, Channel, Interface, Peer, parse_interface, parse_station, parse_survey,
    };
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};
    use wayline_protocol::{WifiBucket, WifiLink, WifiState};

    #[derive(Clone)]
    struct Fake {
        interfaces: Option<Vec<Interface>>,
        stations: BTreeMap<u32, Option<Vec<Peer>>>,
        surveys: BTreeMap<u32, Vec<Channel>>,
        bss: Option<Bss>,
        bytes: (u64, u64),
    }

    impl Source for Fake {
        fn interfaces(&mut self) -> Option<Vec<Interface>> {
            self.interfaces.clone()
        }

        fn stations(&mut self, index: u32) -> Option<Vec<Peer>> {
            self.stations
                .get(&index)
                .cloned()
                .unwrap_or(Some(Vec::new()))
        }

        fn survey(&mut self, index: u32) -> Option<Vec<Channel>> {
            self.surveys.get(&index).cloned()
        }

        fn associated(&mut self, _: u32) -> Option<Bss> {
            self.bss.clone()
        }

        fn bytes(&mut self, _: &str) -> Option<(u64, u64)> {
            Some(self.bytes)
        }

        fn driver(&mut self, _: &str) -> Option<String> {
            Some("mt_wifi".into())
        }
    }

    fn iface(index: u32, name: &str, station: bool, ssid: &str, frequency: u32) -> Interface {
        parse_interface(&nl80211::tests::interface(
            index, name, station, ssid, frequency,
        ))
        .unwrap_or_default()
    }

    fn peer(mac: [u8; 6], packets: u32, retries: u32, connected: u32) -> Peer {
        parse_station(&nl80211::tests::station(mac, packets, retries, connected))
            .unwrap_or_default()
    }

    fn channels(active: u64, busy: u64) -> Vec<Channel> {
        [
            nl80211::tests::survey(5_180, false, 9, 1),
            nl80211::tests::survey(5_200, true, active, busy),
        ]
        .iter()
        .filter_map(|data| parse_survey(data))
        .collect()
    }

    const AP: [u8; 6] = [0x76, 0xfa, 0xce, 0x8a, 0xc4, 0xb3];

    fn fake() -> Fake {
        Fake {
            interfaces: Some(vec![
                iface(1, "ra0", false, "Home", 2_437),
                iface(6, "apcli0", true, "", 2_457),
                iface(19, "apclii0", true, "candelaria200", 5_200),
            ]),
            stations: BTreeMap::from([(19, Some(vec![peer(AP, 100, 10, 50)]))]),
            surveys: BTreeMap::from([(19, channels(1_000, 200))]),
            bss: Some(Bss {
                bssid: "76:fa:ce:8a:c4:b3".into(),
                frequency: Some(5_200),
                ssid: Some("candelaria200".into()),
                security: Some("WPA2 PSK".into()),
            }),
            bytes: (5_000, 9_000),
        }
    }

    #[test]
    fn the_sampler_reads_the_associated_station_interface() -> Result<(), &'static str> {
        let mut sampler = Sampler::new(fake());
        let Observation::Connected(reading) = sampler.observe(Instant::now()) else {
            return Err("expected a connection");
        };
        let link = &reading.link;
        assert_eq!(reading.interface, "apclii0");
        assert_eq!(sampler.candidates(), ["apclii0", "apcli0"]);
        assert_eq!(sampler.watched(), [19, 6]);
        assert_eq!(link.bssid, "76:fa:ce:8a:c4:b3");
        assert_eq!(link.ssid, "candelaria200");
        assert_eq!((link.channel, link.width), (Some(40), Some(80)));
        assert_eq!(link.htmode.as_deref(), Some("HE80"));
        assert_eq!(link.hwmode.as_deref(), Some("ax"));
        assert_eq!(link.encryption.as_deref(), Some("WPA2 PSK"));
        assert_eq!(link.hardware.as_deref(), Some("mt_wifi"));
        assert_eq!((link.signal, link.noise), (Some(-68), Some(-92)));
        assert_eq!(link.tx.kbps, Some(648_500));
        assert_eq!(reading.frames.map(|frames| frames.retries), Some(10));
        assert_eq!(reading.survey.map(|survey| survey.busy), Some(200));
        Ok(())
    }

    #[test]
    fn a_scan_on_another_channel_does_not_move_the_link() -> Result<(), &'static str> {
        let mut source = fake();
        let mut sampler = Sampler::new(source.clone());
        let Observation::Connected(first) = sampler.observe(Instant::now()) else {
            return Err("expected a connection");
        };
        assert_eq!((first.link.channel, first.link.width), (Some(40), Some(80)));
        let mut scanning = iface(19, "apclii0", true, "candelaria200", 5_660);
        scanning.width = Some(20);
        source.interfaces = Some(vec![scanning]);
        sampler.source = source;
        let Observation::Connected(during) = sampler.observe(Instant::now()) else {
            return Err("expected a connection");
        };
        assert_eq!(
            (during.link.channel, during.link.width),
            (Some(40), Some(80))
        );
        assert_eq!(during.survey.map(|survey| survey.busy), Some(200));
        Ok(())
    }

    #[test]
    fn interfaces_without_a_peer_are_disconnected_and_failures_are_failures() {
        let mut idle = fake();
        idle.stations.clear();
        assert!(matches!(
            Sampler::new(idle).observe(Instant::now()),
            Observation::Disconnected { .. }
        ));
        let mut none = fake();
        none.interfaces = Some(vec![iface(1, "ra0", false, "Home", 2_437)]);
        assert!(matches!(
            Sampler::new(none).observe(Instant::now()),
            Observation::NoClientInterface
        ));
        let mut broken = fake();
        broken.interfaces = None;
        assert!(matches!(
            Sampler::new(broken).observe(Instant::now()),
            Observation::QueryFailed
        ));
        let mut silent = fake();
        silent.stations = BTreeMap::from([(6, None), (19, None)]);
        assert!(matches!(
            Sampler::new(silent).observe(Instant::now()),
            Observation::QueryFailed
        ));
    }

    #[test]
    fn failed_queries_are_neither_samples_nor_disconnects() {
        let start = Instant::now();
        let base = 1_800_000_000_000;
        let mut record = Record::new();
        let mut sampler = Sampler::new(fake());
        let apply = |record: &mut Record, observation: Observation, offset: u64| {
            record.apply(
                start + Duration::from_millis(offset),
                base + offset,
                observation,
                &[],
            );
        };
        apply(&mut record, sampler.observe(start), 0);
        apply(&mut record, Observation::QueryFailed, 2_000);
        apply(&mut record, Observation::QueryFailed, 4_000);
        assert!(record.status(start, None).state == WifiState::QueryFailed);
        apply(&mut record, sampler.observe(start), 6_000);
        apply(&mut record, Observation::QueryFailed, 8_000);
        apply(
            &mut record,
            Observation::Disconnected {
                interface: "apclii0".into(),
            },
            10_000,
        );
        apply(&mut record, Observation::QueryFailed, 40_000);
        let status = record.status(start + Duration::from_secs(40), Some(0));
        assert_eq!(status.buckets.len(), 1);
        let bucket = status.buckets[0];
        assert_eq!(bucket.samples, 3);
        assert_eq!(bucket.connected, 2);
        assert_eq!(bucket.disconnects, 1);
    }

    fn total(buckets: &[WifiBucket]) -> WifiBucket {
        buckets
            .iter()
            .fold(WifiBucket::empty(0), |mut sum, bucket| {
                sum.merge(bucket);
                sum
            })
    }

    #[test]
    fn record_accumulates_deltas_roams_and_drops() {
        let start = Instant::now();
        let base = 1_800_000_000_000;
        let mut source = fake();
        let mut record = Record::new();
        let step = |record: &mut Record, source: &Fake, offset: u64| {
            let mut sampler = Sampler::new(source.clone());
            let observation = sampler.observe(start + Duration::from_millis(offset));
            record.apply(
                start + Duration::from_millis(offset),
                base + offset,
                observation,
                &[],
            );
        };
        step(&mut record, &source, 0);
        source
            .stations
            .insert(19, Some(vec![peer(AP, 150, 25, 55)]));
        source.surveys.insert(19, channels(6_000, 1_200));
        source.bytes = (8_000, 19_000);
        step(&mut record, &source, 5_000);
        let mut other = AP;
        other[5] = 0x83;
        source.stations.insert(19, Some(vec![peer(other, 5, 0, 1)]));
        step(&mut record, &source, 10_000);
        source.stations.insert(19, Some(Vec::new()));
        step(&mut record, &source, 20_000);
        step(&mut record, &source, 40_000);
        let status = record.status(start + Duration::from_secs(40), Some(0));
        assert!(status.state == WifiState::Disconnected);
        let sum = total(&status.buckets);
        assert_eq!(sum.connected, 3);
        assert_eq!(sum.frames, 50);
        assert_eq!(sum.retries, 15);
        assert_eq!(sum.tx_bytes, 3_000);
        assert_eq!(sum.rx_bytes, 10_000);
        assert_eq!(sum.active_ms, 5_000);
        assert_eq!(sum.busy_ms, 1_000);
        assert_eq!(sum.roams, 1);
        assert_eq!(sum.disconnects, 1);
        assert_eq!(sum.samples, 4);
    }

    #[test]
    fn events_count_at_once_and_land_in_the_next_timed_bucket() {
        let start = Instant::now();
        let base = 1_800_000_000_000;
        let mut record = Record::new();
        let at = |offset: u64| (start + Duration::from_millis(offset), base + offset);
        let timed = |record: &mut Record, offset: u64, observation: Observation| {
            let (now, wall) = at(offset);
            record.apply(now, wall, observation, &[]);
        };
        let event = |record: &mut Record, offset: u64, observation: Observation| {
            let (now, wall) = at(offset);
            record.react(now, wall, observation, &[]);
        };
        let gone = || Observation::Disconnected {
            interface: "apclii0".into(),
        };
        timed(
            &mut record,
            0,
            joined("apclii0", "A", "aa:00:00:00:00:01", 40),
        );
        event(
            &mut record,
            2_000,
            joined("apclii0", "A", "aa:00:00:00:00:02", 40),
        );
        timed(
            &mut record,
            5_000,
            joined("apclii0", "A", "aa:00:00:00:00:02", 40),
        );
        event(&mut record, 7_000, gone());
        event(
            &mut record,
            8_000,
            joined("apcli0", "B", "bb:00:00:00:00:01", 6),
        );
        timed(
            &mut record,
            10_000,
            joined("apcli0", "B", "bb:00:00:00:00:01", 6),
        );
        timed(
            &mut record,
            30_000,
            joined("apcli0", "B", "bb:00:00:00:00:01", 6),
        );
        let status = record.status(start + Duration::from_secs(30), Some(0));
        let sum = total(&status.buckets);
        assert_eq!((sum.roams, sum.disconnects), (1, 1));
        assert_eq!(sum.samples, 3);
        let stamps: Vec<u64> = status
            .changes
            .iter()
            .map(|change| change.at_ms - base)
            .collect();
        assert_eq!(stamps, [0, 2_000, 8_000]);
    }

    fn joined(interface: &str, ssid: &str, bssid: &str, channel: u16) -> Observation {
        Observation::Connected(Box::new(Reading {
            interface: interface.into(),
            link: WifiLink {
                ssid: ssid.into(),
                bssid: bssid.into(),
                channel: Some(channel),
                signal: Some(-60),
                ..WifiLink::default()
            },
            frames: None,
            survey: None,
            bytes: None,
        }))
    }

    #[test]
    fn switches_count_disconnects_roams_and_log_every_change() {
        let start = Instant::now();
        let base = 1_800_000_000_000;
        let mut record = Record::new();
        let gone = || Observation::Disconnected {
            interface: "apclii0".into(),
        };
        let steps = [
            (0, joined("apclii0", "A", "aa:00:00:00:00:01", 40)),
            (5_000, joined("apclii0", "A", "aa:00:00:00:00:01", 40)),
            (10_000, gone()),
            (15_000, gone()),
            (20_000, joined("apcli0", "B", "bb:00:00:00:00:01", 6)),
            (25_000, joined("apcli0", "B", "bb:00:00:00:00:02", 11)),
            (30_000, joined("apcli0", "B", "bb:00:00:00:00:02", 11)),
            (35_000, joined("apclii0", "A", "aa:00:00:00:00:01", 40)),
            (60_000, gone()),
        ];
        for (offset, observation) in steps {
            record.apply(
                start + Duration::from_millis(offset),
                base + offset,
                observation,
                &[],
            );
        }
        let status = record.status(start + Duration::from_secs(60), Some(0));
        let total = status.buckets.iter().fold(
            wayline_protocol::WifiBucket::empty(0),
            |mut sum, bucket| {
                sum.merge(bucket);
                sum
            },
        );
        assert_eq!(total.disconnects, 2);
        assert_eq!(total.roams, 1);
        let seen: Vec<&str> = status.changes.iter().map(|c| c.to.bssid.as_str()).collect();
        assert_eq!(
            seen,
            [
                "aa:00:00:00:00:01",
                "bb:00:00:00:00:01",
                "bb:00:00:00:00:02",
                "aa:00:00:00:00:01"
            ]
        );
        assert!(status.changes[0].from.is_none());
        assert_eq!(
            status.changes[1]
                .from
                .as_ref()
                .map(|from| from.ssid.as_str()),
            Some("A")
        );
        let tags: Vec<u64> = status.buckets.iter().map(|bucket| bucket.ap).collect();
        assert_eq!(
            tags,
            [
                wayline_protocol::access_point_id("aa:00:00:00:00:01"),
                wayline_protocol::access_point_id("bb:00:00:00:00:01"),
                wayline_protocol::access_point_id("bb:00:00:00:00:02"),
            ]
        );
    }
}
