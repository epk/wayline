use crate::changes::{self, Event};
use crate::columnar::{self, Entry};
use crate::router;
use crate::router::Inventory;
use crate::runtime;
use crate::runtime::RUNTIME;
use crate::settings::{Settings, Target};
use crate::{clock, encoding, telemetry, wifi};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use wayline_protocol::schema::{RTT_BINS, RTT_EDGES_US, RTT_FIRST};
use wayline_protocol::{BUCKET_SECONDS, MAX_SERIES, Series, TelemetryStatus, WifiBucket};

pub const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;
const MINUTE_MS: u64 = 60_000;
pub const MIN_CLOCK: u64 = 1_704_067_200;
const MAX_POINTS: u64 = 2_500;
pub const LEVELS: [(u64, &str); 4] = [
    (BUCKET_SECONDS, "s15"),
    (60, "s1m"),
    (300, "s5m"),
    (3_600, "s1h"),
];
const LEGACY: [&str; 9] = [
    "latency.sqlite",
    "latency.sqlite-journal",
    "latency.sqlite-wal",
    "latency.sqlite-shm",
    "latency.sqlite.broken",
    "telemetry.sqlite",
    "telemetry.sqlite-journal",
    "telemetry.sqlite.broken",
    "series.json",
];

pub use wayline_protocol::schema::LATENCY;

#[derive(Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field is an independent request option."
)]
pub struct Query {
    start: u64,
    end: u64,
    step: u64,
    #[serde(default)]
    target: Option<Target>,
    tunnel: String,
    #[serde(default)]
    include_wan: bool,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    endpoints: bool,
    #[serde(default)]
    uplinks: bool,
}

#[derive(Serialize)]
pub struct Response {
    start: u64,
    step: u64,
    count: usize,
    series: Vec<Line>,
    events: Vec<Event>,
    incidents: Vec<Incident>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Incident {
    pub subject: String,
    pub name: String,
    pub kind: String,
    pub start: u64,
    pub end: Option<u64>,
    pub detail: String,
}

#[derive(Serialize)]
struct Line {
    id: String,
    name: String,
    median: Vec<Option<u32>>,
    min: Vec<Option<u32>>,
    max: Vec<Option<u32>>,
    jitter: Vec<Option<u32>>,
    p50: Vec<Option<u32>>,
    p95: Vec<Option<u32>>,
    p99: Vec<Option<u32>>,
    sent: Vec<u32>,
    lost: Vec<u32>,
}

struct Track {
    id: String,
    name: String,
    points: Vec<Point>,
}

#[derive(Clone, Copy, Default)]
struct Point {
    median: Option<u32>,
    min: Option<u32>,
    max: Option<u32>,
    sent: u32,
    lost: u32,
    jitter_sum: u64,
    jitter_n: u32,
    rtt: [u64; RTT_BINS],
}

pub fn percentile(
    rtt: &[u64; RTT_BINS],
    permille: u64,
    min: Option<u32>,
    max: Option<u32>,
) -> Option<u32> {
    let total: u64 = rtt.iter().sum();
    if total == 0 {
        return None;
    }
    let rank = (total * permille).div_ceil(1_000).max(1);
    let mut before = 0;
    for (bin, count) in rtt.iter().enumerate() {
        if *count == 0 || before + count < rank {
            before += count;
            continue;
        }
        let low = bin
            .checked_sub(1)
            .map_or(0, |edge| u64::from(RTT_EDGES_US[edge]));
        let high = RTT_EDGES_US
            .get(bin)
            .map(|edge| u64::from(*edge))
            .or_else(|| max.map(u64::from))
            .unwrap_or(low);
        let low = min.map_or(low, |min| low.max(u64::from(min)).min(high));
        let high = max.map_or(high, |max| high.min(u64::from(max)).max(low));
        let value = low + (high - low) * (2 * (rank - before) - 1) / (2 * count);
        return u32::try_from(value).ok();
    }
    None
}

impl Point {
    fn percentile(&self, permille: u64) -> Option<u32> {
        percentile(&self.rtt, permille, self.min, self.max)
    }

    fn jitter(&self) -> Option<u32> {
        (self.jitter_n > 0)
            .then(|| u32::try_from(self.jitter_sum / u64::from(self.jitter_n)).unwrap_or(u32::MAX))
    }

    const fn down(&self) -> bool {
        self.sent > 0 && self.lost == self.sent
    }
}

impl From<Track> for Line {
    fn from(track: Track) -> Self {
        let column = |read: fn(&Point) -> Option<u32>| track.points.iter().map(read).collect();
        Self {
            median: column(|point| point.median),
            min: column(|point| point.min),
            max: column(|point| point.max),
            jitter: column(Point::jitter),
            p50: track
                .points
                .iter()
                .map(|point| point.percentile(500))
                .collect(),
            p95: track
                .points
                .iter()
                .map(|point| point.percentile(950))
                .collect(),
            p99: track
                .points
                .iter()
                .map(|point| point.percentile(990))
                .collect(),
            sent: track.points.iter().map(|point| point.sent).collect(),
            lost: track.points.iter().map(|point| point.lost).collect(),
            id: track.id,
            name: track.name,
        }
    }
}

impl encoding::Document for Response {
    fn frame(&self) -> Result<encoding::Frame> {
        let mut meta = serde_json::Map::new();
        meta.insert("start".into(), self.start.into());
        meta.insert("step".into(), self.step.into());
        meta.insert("count".into(), self.count.into());
        meta.insert(
            "series".into(),
            self.series
                .iter()
                .map(|line| serde_json::json!({"id": line.id, "name": line.name}))
                .collect(),
        );
        meta.insert("events".into(), serde_json::to_value(&self.events)?);
        meta.insert("incidents".into(), serde_json::to_value(&self.incidents)?);
        let micros = |values: &[Option<u32>]| {
            encoding::Column::I32(
                values
                    .iter()
                    .map(|value| value.and_then(|value| i32::try_from(value).ok()))
                    .collect(),
            )
        };
        let columns = self
            .series
            .iter()
            .enumerate()
            .flat_map(|(index, line)| {
                [
                    (format!("{index}.median"), micros(&line.median)),
                    (format!("{index}.min"), micros(&line.min)),
                    (format!("{index}.max"), micros(&line.max)),
                    (format!("{index}.jitter"), micros(&line.jitter)),
                    (format!("{index}.p50"), micros(&line.p50)),
                    (format!("{index}.p95"), micros(&line.p95)),
                    (format!("{index}.p99"), micros(&line.p99)),
                    (
                        format!("{index}.sent"),
                        encoding::Column::U32(line.sent.clone()),
                    ),
                    (
                        format!("{index}.lost"),
                        encoding::Column::U32(line.lost.clone()),
                    ),
                ]
            })
            .collect();
        Ok(encoding::Frame { meta, columns })
    }
}

pub fn outages(
    subject: &str,
    name: &str,
    detail: &str,
    start: u64,
    step: u64,
    down: impl IntoIterator<Item = Option<bool>>,
    now: u64,
) -> Vec<Incident> {
    let mut found: Vec<Incident> = Vec::new();
    let mut open: Option<usize> = None;
    let mut last_end = start;
    for (index, state) in down.into_iter().enumerate() {
        let at = start + u64::try_from(index).unwrap_or(u64::MAX) * step * 1_000;
        let end = at + step * 1_000;
        match state {
            Some(true) => {
                if open.is_none() {
                    found.push(Incident {
                        subject: subject.into(),
                        name: name.into(),
                        kind: "loss".into(),
                        start: at,
                        end: None,
                        detail: detail.into(),
                    });
                    open = Some(found.len() - 1);
                }
                last_end = end;
            }
            Some(false) | None => {
                if let Some(index) = open.take() {
                    found[index].end = Some(last_end);
                }
            }
        }
    }
    if let Some(index) = open
        && last_end + 2 * step * 1_000 < now
    {
        found[index].end = Some(last_end);
    }
    found
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    days: u32,
    tiers: Vec<Kept>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    step: u64,
    kept_seconds: u64,
}

pub struct Report {
    pub bytes: u64,
    pub cap: u64,
    pub lines: Vec<String>,
}

struct Subject {
    id: String,
    name: String,
    identity: String,
}

pub fn root(config: &Settings) -> PathBuf {
    if config.history_days == 0 {
        Path::new(RUNTIME).join("history")
    } else {
        PathBuf::from("/etc/wayline/history")
    }
}

fn known_cache() -> PathBuf {
    Path::new(RUNTIME).join("stored-series.json")
}

fn cutoff() -> Result<u64> {
    match fs::read_to_string(Path::new(RUNTIME).join("history-cutoff")) {
        Ok(value) => value
            .parse::<u64>()
            .context("Invalid history reset marker."),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn fresh_ring() -> Result<Vec<Series>> {
    Ok(fresh()?.0)
}

pub fn fresh() -> Result<(Vec<Series>, Vec<WifiBucket>)> {
    let (series, wifi, _) = fresh_all()?;
    Ok((series, wifi))
}

pub fn fresh_all() -> Result<(Vec<Series>, Vec<WifiBucket>, Option<TelemetryStatus>)> {
    let snapshot = snapshot()?;
    Ok((snapshot.series, snapshot.wifi, snapshot.telemetry))
}

#[derive(Default, Serialize, Deserialize)]
struct Staged {
    series: Vec<Series>,
    wifi: Vec<WifiBucket>,
    telemetry: Option<TelemetryStatus>,
}

struct Snapshot {
    series: Vec<Series>,
    wifi: Vec<WifiBucket>,
    telemetry: Option<TelemetryStatus>,
    clock: Option<clock::Clock>,
}

fn restamp_all(staged: &mut Staged, offset_ms: i64) {
    for series in &mut staged.series {
        clock::restamp(&mut series.buckets, offset_ms);
    }
    clock::restamp(&mut staged.wifi, offset_ms);
    if let Some(telemetry) = &mut staged.telemetry {
        clock::restamp(&mut telemetry.system_buckets, offset_ms);
        for link in &mut telemetry.link_buckets {
            clock::restamp(&mut link.buckets, offset_ms);
        }
    }
}

fn snapshot() -> Result<Snapshot> {
    let cutoff = cutoff()?;
    let parts = runtime::ring();
    let clock = if parts.boot_id.is_empty() {
        None
    } else {
        Some(clock::observe(&clock::Observation {
            boot: &parts.boot_id,
            boot_ms: parts.boot_ms,
            wall_ms: parts.wall_ms,
            reachable: parts
                .series
                .iter()
                .any(|series| series.state.live() && series.received > 0),
        })?)
    };
    let mut staged = Staged {
        series: parts.series,
        wifi: parts.wifi.map(|wifi| wifi.buckets).unwrap_or_default(),
        telemetry: parts.telemetry,
    };
    if let Some(clock) = clock.as_ref().filter(|clock| clock.synced) {
        restamp_all(&mut staged, clock.offset_ms);
    }
    discard_before(&mut staged.series, cutoff);
    staged.wifi.retain(|bucket| bucket.at > cutoff);
    if let Some(telemetry) = &mut staged.telemetry {
        telemetry.system_buckets.retain(|bucket| bucket.at > cutoff);
        for link in &mut telemetry.link_buckets {
            link.buckets.retain(|bucket| bucket.at > cutoff);
        }
    }
    Ok(Snapshot {
        series: staged.series,
        wifi: staged.wifi,
        telemetry: staged.telemetry,
        clock,
    })
}

const PENDING_LIMIT: u64 = 16 * 1024 * 1024;
const PENDING_READ: usize = 32 * 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    boot: String,
    uptime_ms: u64,
}

fn pending_dir(root: &Path) -> PathBuf {
    root.join("pending")
}

pub fn pending(config: &Settings) -> bool {
    fs::read_dir(pending_dir(&root(config)))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "ndjson")
        })
}

fn stage(root: &Path, boot: &str, snapshot: &Snapshot) -> Result<()> {
    use std::io::Write as _;
    let cursor_path = pending_dir(root).join("cursor.json");
    let cursor: Cursor = runtime::load(&cursor_path);
    let after = if cursor.boot == boot {
        cursor.uptime_ms
    } else {
        0
    };
    let newer = |uptime: u64| uptime > after;
    fs::create_dir_all(pending_dir(root))?;
    let staged = Staged {
        series: snapshot
            .series
            .iter()
            .map(|series| Series {
                buckets: series
                    .buckets
                    .iter()
                    .filter(|bucket| newer(bucket.uptime_ms))
                    .copied()
                    .collect(),
                history: Vec::new(),
                ..series.clone()
            })
            .filter(|series| !series.buckets.is_empty())
            .collect(),
        wifi: snapshot
            .wifi
            .iter()
            .filter(|bucket| newer(bucket.uptime_ms))
            .copied()
            .collect(),
        telemetry: snapshot
            .telemetry
            .as_ref()
            .map(|telemetry| TelemetryStatus {
                system_buckets: telemetry
                    .system_buckets
                    .iter()
                    .filter(|bucket| newer(bucket.uptime_ms))
                    .copied()
                    .collect(),
                link_buckets: telemetry
                    .link_buckets
                    .iter()
                    .map(|link| wayline_protocol::LinkHistory {
                        id: link.id.clone(),
                        buckets: link
                            .buckets
                            .iter()
                            .filter(|bucket| newer(bucket.uptime_ms))
                            .copied()
                            .collect(),
                    })
                    .collect(),
                ..TelemetryStatus::default()
            }),
    };
    let latest = staged
        .series
        .iter()
        .flat_map(|series| series.buckets.iter().map(|bucket| bucket.uptime_ms))
        .chain(staged.wifi.iter().map(|bucket| bucket.uptime_ms))
        .chain(staged.telemetry.iter().flat_map(|telemetry| {
            telemetry
                .system_buckets
                .iter()
                .map(|bucket| bucket.uptime_ms)
                .chain(
                    telemetry
                        .link_buckets
                        .iter()
                        .flat_map(|link| link.buckets.iter().map(|bucket| bucket.uptime_ms)),
                )
        }))
        .max();
    let Some(latest) = latest else {
        return Ok(());
    };
    let directory = pending_dir(root);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{boot}.ndjson"));
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > PENDING_LIMIT) {
        return Ok(());
    }
    let mut line = serde_json::to_vec(&staged)?;
    line.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(&line)?;
    file.sync_all()?;
    runtime::store(
        &cursor_path,
        &Cursor {
            boot: boot.to_owned(),
            uptime_ms: latest,
        },
    )
}

fn flush_pending(root: &Path, days: u32, now: u64, clock: Option<&clock::Clock>) -> Result<()> {
    let directory = pending_dir(root);
    let files: Vec<PathBuf> = fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "ndjson")
        })
        .collect();
    for path in files {
        let boot = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        let text =
            String::from_utf8_lossy(&wayline_support::read(&path, PENDING_READ)?).into_owned();
        let mut staged = Staged::default();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(part) = serde_json::from_str::<Staged>(line) else {
                continue;
            };
            staged.series.extend(part.series);
            staged.wifi.extend(part.wifi);
            if let Some(telemetry) = part.telemetry {
                let target = staged
                    .telemetry
                    .get_or_insert_with(TelemetryStatus::default);
                target.system_buckets.extend(telemetry.system_buckets);
                for link in telemetry.link_buckets {
                    match target
                        .link_buckets
                        .iter_mut()
                        .find(|known| known.id == link.id)
                    {
                        Some(known) => known.buckets.extend(link.buckets),
                        None => target.link_buckets.push(link),
                    }
                }
            }
        }
        match clock.filter(|clock| clock.boot == boot) {
            Some(clock) => restamp_all(&mut staged, clock.offset_ms),
            None => changes::append(
                root,
                [Event {
                    at: now * 1_000,
                    seq: 0,
                    subject: "history".into(),
                    kind: "clock".into(),
                    text: "History from a boot that never set its clock was saved with the router's unsynchronized time.".into(),
                }],
            )?,
        }
        let series = merge_series(staged.series);
        persist(
            root,
            days,
            now,
            &series,
            &staged.wifi,
            staged.telemetry.as_ref(),
        )?;
        fs::remove_file(&path)?;
    }
    Ok(())
}

fn merge_series(items: Vec<Series>) -> Vec<Series> {
    let mut merged: Vec<Series> = Vec::new();
    for item in items {
        match merged.iter_mut().find(|known| known.id == item.id) {
            Some(known) => known.buckets.extend(item.buckets),
            None => merged.push(item),
        }
    }
    merged
}

fn discard_before(samples: &mut [Series], cutoff: u64) {
    for series in samples {
        series.buckets.retain(|bucket| bucket.at > cutoff);
    }
}

pub fn purge() -> Result<()> {
    runtime::locked("history", || {
        let roots = [
            PathBuf::from("/etc/wayline/history"),
            Path::new(RUNTIME).join("history"),
        ];
        for root in &roots {
            if root.try_exists()? {
                runtime::directory(root)?;
            }
        }
        wayline_support::atomic(
            &Path::new(RUNTIME).join("history-cutoff"),
            runtime::now().to_string().as_bytes(),
        )?;
        for root in roots {
            for directory in columnar::kinds(&root) {
                match fs::remove_dir_all(&directory) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error).with_context(|| {
                            format!("Cannot remove history {}.", directory.display())
                        });
                    }
                    _ => {}
                }
            }
            for name in LEGACY.iter().chain(&[
                "events.json",
                "facts.json",
                "incidents.json",
                "index.json",
                "dimensions.parquet",
            ]) {
                remove(&root.join(name))?;
            }
        }
        for root in [
            PathBuf::from("/etc/wayline/history"),
            Path::new(RUNTIME).join("history"),
        ] {
            match fs::remove_dir_all(pending_dir(&root)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error).context("Cannot remove staged history.");
                }
                _ => {}
            }
        }
        remove(&known_cache())?;
        remove(&Path::new(RUNTIME).join("last-collect"))
    })
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("Cannot remove history file {}.", path.display()))
        }
    }
}

pub fn known_prefixes() -> BTreeSet<String> {
    fs::read(known_cache())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn publish(cache: &Path, prefixes: &BTreeSet<String>) -> Result<()> {
    let encoded = serde_json::to_vec(prefixes)?;
    if fs::read(cache).is_ok_and(|current| current == encoded) {
        return Ok(());
    }
    wayline_support::atomic(cache, &encoded)
}

pub const fn retained(days: u32, step: u64) -> u64 {
    match (days, step) {
        (0, 15) => 3 * HOUR,
        (1 | 7 | 30, 15) => 6 * HOUR,
        (0 | 1, 60) => DAY,
        (7 | 30, 60) => 3 * DAY,
        (7, 300) => 7 * DAY,
        (30, 300 | 3_600) => 30 * DAY,
        _ => 0,
    }
}

pub fn resolution(days: u32, step: u64, start: u64, end: u64, now: u64) -> Result<u64> {
    let step = if matches!(step, 1 | 5) {
        step * 60
    } else {
        step
    };
    let kept = retained(days, step);
    ensure!(start < end, "Invalid history range.");
    ensure!(
        kept > 0 && start.saturating_add(kept * 1_000 + MINUTE_MS) >= now && end <= now + MINUTE_MS,
        "That range is not kept at this resolution."
    );
    ensure!(
        end / 1_000 / step - start / 1_000 / step < MAX_POINTS,
        "That range has too many points."
    );
    Ok(step)
}

pub fn availability(config: &Settings) -> Availability {
    Availability {
        days: config.history_days,
        tiers: LEVELS
            .iter()
            .map(|(step, _)| Kept {
                step: *step,
                kept_seconds: retained(config.history_days, *step),
            })
            .filter(|kept| kept.kept_seconds > 0)
            .collect(),
    }
}

pub const fn aligned((low, high): (u64, u64), ratio: u64) -> (u64, u64) {
    (low / ratio * ratio, (high / ratio + 1) * ratio - 1)
}

fn latency_entries(ring: &[Series]) -> Vec<Entry> {
    ring.iter()
        .take(MAX_SERIES)
        .flat_map(|series| {
            series.buckets.iter().map(|bucket| Entry {
                link: series.id.clone(),
                at: bucket.at,
                row: vec![
                    bucket.min_us.map(i64::from),
                    bucket.median_us.map(i64::from),
                    bucket.max_us.map(i64::from),
                    Some(i64::from(bucket.sent)),
                    Some(i64::from(bucket.lost)),
                    Some(i64::try_from(bucket.jitter_sum_us).unwrap_or(i64::MAX)),
                    Some(i64::from(bucket.jitter_n)),
                ]
                .into_iter()
                .chain(bucket.rtt.iter().map(|count| Some(i64::from(*count))))
                .collect(),
            })
        })
        .collect()
}

fn longest(days: u32) -> u64 {
    LEVELS
        .iter()
        .map(|(step, _)| retained(days, *step))
        .max()
        .unwrap_or_default()
}

fn index(root: &Path, days: u32, now: u64, ring: &[Series]) -> Result<BTreeSet<String>> {
    let path = root.join("index.json");
    let mut seen: BTreeMap<String, u64> = runtime::load(&path);
    let before = seen.clone();
    for series in ring.iter().filter(|series| !series.buckets.is_empty()) {
        seen.insert(series.id.clone(), now / HOUR);
    }
    let cutoff = now.saturating_sub(longest(days)) / HOUR;
    seen.retain(|_, hour| *hour >= cutoff);
    if seen.len() > MAX_SERIES {
        let mut order: Vec<(u64, String)> = seen
            .iter()
            .map(|(name, hour)| (*hour, name.clone()))
            .collect();
        order.sort();
        for (_, name) in order.iter().take(seen.len() - MAX_SERIES) {
            seen.remove(name);
        }
    }
    if seen != before {
        runtime::store(&path, &seen)?;
    }
    Ok(seen
        .keys()
        .map(|name| name.chars().take(48).collect())
        .collect())
}

fn store(root: &Path, days: u32, now: u64, ring: &[Series]) -> Result<BTreeSet<String>> {
    columnar::store(root, days, &LATENCY, &latency_entries(ring), now)?;
    index(root, days, now, ring)
}

#[cfg(test)]
fn collect_at(root: &Path, days: u32, ring: &[Series]) -> Result<()> {
    store(root, days, runtime::now(), ring).map(drop)
}

pub fn collect(config: &Settings) -> Result<()> {
    runtime::locked("history", || collect_inner(config))
}

fn collect_inner(config: &Settings) -> Result<()> {
    let root = root(config);
    runtime::directory(&root)?;
    let snapshot = snapshot()?;
    let now = runtime::now();
    if let Some(clock) = snapshot.clock.as_ref().filter(|clock| !clock.synced) {
        return stage(&root, &clock.boot, &snapshot);
    }
    if now < MIN_CLOCK {
        return Ok(());
    }
    let days = config.history_days;
    let flushed = flush_pending(&root, days, now, snapshot.clock.as_ref());
    let prefixes = persist(
        &root,
        days,
        now,
        &snapshot.series,
        &snapshot.wifi,
        snapshot.telemetry.as_ref(),
    )?;
    publish(&known_cache(), &prefixes)?;
    flushed
}

fn persist(
    root: &Path,
    days: u32,
    now: u64,
    ring: &[Series],
    buckets: &[WifiBucket],
    status: Option<&TelemetryStatus>,
) -> Result<BTreeSet<String>> {
    let wifi = columnar::store(root, days, &wifi::KIND, &wifi::entries(buckets), now);
    let system = status.map_or(Ok(()), |status| {
        columnar::store(
            root,
            days,
            &telemetry::SYSTEM,
            &telemetry::system_entries(status),
            now,
        )
    });
    let links = status.map_or(Ok(()), |status| {
        columnar::store(
            root,
            days,
            &telemetry::INTERFACE,
            &telemetry::link_entries(status),
            now,
        )
    });
    let logged = record_incidents(root, ring, buckets);
    let prefixes = store(root, days, now, ring)?;
    wifi.and(system).and(links).and(logged)?;
    Ok(prefixes)
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Logged {
    pub series: String,
    pub kind: String,
    pub start: u64,
    pub end: Option<u64>,
}

const MAX_INCIDENTS: usize = 200;

pub fn ring_incidents(ring: &[Series], wifi: &[WifiBucket], now: u64) -> Vec<Logged> {
    let mut found = Vec::new();
    let mut runs = |series: &str, kind: &str, states: Vec<(u64, Option<bool>)>| {
        let Some(&(first, _)) = states.first() else {
            return;
        };
        let mut timeline: Vec<Option<bool>> = Vec::new();
        let mut expected = first;
        for (at, state) in states {
            while expected < at {
                timeline.push(None);
                expected += BUCKET_SECONDS;
            }
            timeline.push(state);
            expected = at + BUCKET_SECONDS;
        }
        found.extend(
            outages(series, "", "", first * 1_000, BUCKET_SECONDS, timeline, now)
                .into_iter()
                .map(|incident| Logged {
                    series: series.into(),
                    kind: kind.into(),
                    start: incident.start,
                    end: incident.end,
                }),
        );
    };
    for series in ring {
        runs(
            &series.id,
            "loss",
            series
                .buckets
                .iter()
                .map(|bucket| {
                    (
                        bucket.at,
                        (bucket.sent > 0).then_some(bucket.lost == bucket.sent),
                    )
                })
                .collect(),
        );
    }
    runs("wifi", "wifi", wifi::down(wifi).collect());
    found
}

pub fn merge_incidents(stored: &mut Vec<Logged>, fresh: Vec<Logged>) {
    for incident in fresh {
        let overlapping = stored.iter_mut().find(|known| {
            known.series == incident.series
                && known.kind == incident.kind
                && known.start <= incident.start
                && known.end.is_none_or(|end| end >= incident.start)
        });
        match overlapping {
            Some(known) => {
                known.end = match (known.end, incident.end) {
                    (_, None) => None,
                    (Some(left), Some(right)) => Some(left.max(right)),
                    (None, Some(right)) => Some(right),
                };
            }
            None => stored.push(incident),
        }
    }
    stored.sort_by_key(|incident| incident.start);
    let excess = stored.len().saturating_sub(MAX_INCIDENTS);
    stored.drain(..excess);
}

fn incidents_path(root: &Path) -> PathBuf {
    root.join("incidents.json")
}

fn record_incidents(path: &Path, ring: &[Series], wifi: &[WifiBucket]) -> Result<()> {
    let file = incidents_path(path);
    let mut stored: Vec<Logged> = runtime::load(&file);
    let before = stored.clone();
    merge_incidents(
        &mut stored,
        ring_incidents(ring, wifi, runtime::now() * 1_000),
    );
    if stored != before {
        runtime::store(&file, &stored)?;
    }
    Ok(())
}

pub fn logged_incidents(config: &Settings, ring: &[Series], wifi: &[WifiBucket]) -> Vec<Logged> {
    let mut stored: Vec<Logged> = runtime::load(incidents_path(&root(config)));
    merge_incidents(
        &mut stored,
        ring_incidents(ring, wifi, runtime::now() * 1_000),
    );
    stored
}

pub fn report(config: &Settings) -> Report {
    let root = root(config);
    Report {
        bytes: columnar::bytes(&root),
        cap: columnar::cap(&root),
        lines: columnar::report(&root),
    }
}

fn lines(
    root: &Path,
    step: u64,
    query: &Query,
    ring: &[Series],
    subjects: Vec<Subject>,
) -> Result<Vec<Track>> {
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    let (first, last) = (query.start / 1_000 / step, query.end / 1_000 / step);
    let wanted: Vec<Series> = ring
        .iter()
        .filter(|series| subjects.iter().any(|subject| subject.identity == series.id))
        .map(|series| Series {
            buckets: series.buckets.clone(),
            history: Vec::new(),
            id: series.id.clone(),
            name: String::new(),
            interface: String::new(),
            path_id: String::new(),
            target: series.target,
            state: series.state,
            median_us: None,
            sent: 0,
            received: 0,
            success_age_ms: None,
            jitter_us: None,
        })
        .collect();
    let mut found = columnar::series(
        root,
        &LATENCY,
        step,
        (first, last),
        &latency_entries(&wanted),
    )?;
    let blank = vec![Point::default(); usize::try_from(last - first + 1)?];
    Ok(subjects
        .into_iter()
        .map(|subject| {
            let mut points = blank.clone();
            for (period, row) in found.remove(&subject.identity).unwrap_or_default() {
                let cell = |index: usize| row.get(index).copied().flatten();
                let small = |index: usize| cell(index).and_then(|value| u32::try_from(value).ok());
                if let Some(slot) = period
                    .checked_sub(first)
                    .and_then(|offset| usize::try_from(offset).ok())
                    .and_then(|offset| points.get_mut(offset))
                {
                    *slot = Point {
                        min: small(0),
                        median: small(1),
                        max: small(2),
                        sent: small(3).unwrap_or(0),
                        lost: small(4).unwrap_or(0),
                        jitter_sum: cell(5)
                            .and_then(|value| u64::try_from(value).ok())
                            .unwrap_or(0),
                        jitter_n: small(6).unwrap_or(0),
                        rtt: std::array::from_fn(|bin| {
                            cell(RTT_FIRST + bin)
                                .and_then(|value| u64::try_from(value).ok())
                                .unwrap_or(0)
                        }),
                    };
                }
            }
            Track {
                id: subject.id,
                name: subject.name,
                points,
            }
        })
        .collect())
}

pub fn query(config: &Settings, inventory: &Inventory, query: &Query) -> Result<Response> {
    runtime::locked("history", || query_inner(config, inventory, query))
}

fn query_inner(config: &Settings, inventory: &Inventory, query: &Query) -> Result<Response> {
    let step = resolution(
        config.history_days,
        query.step,
        query.start,
        query.end,
        runtime::now() * 1_000,
    )?;
    let known = known_prefixes();
    let target = query.target.as_ref().unwrap_or_else(|| config.primary());
    let selected = |id: &str| query.all || id == query.tunnel;
    let mut subjects: Vec<Subject> = inventory
        .tunnels
        .iter()
        .filter_map(|tunnel| {
            let prefix = router::tunnel_prefix(&tunnel.key);
            (selected(&tunnel.id) && (inventory.has_path(&tunnel.id) || known.contains(&prefix)))
                .then(|| Subject {
                    id: tunnel.id.clone(),
                    name: tunnel.name.clone(),
                    identity: router::series_id(&prefix, target),
                })
        })
        .collect();
    ensure!(
        query.all || !subjects.is_empty(),
        "No history exists for this tunnel."
    );
    if query.endpoints && router::endpoints_enabled(config) {
        subjects.extend(
            inventory
                .tunnels
                .iter()
                .filter(|tunnel| selected(&tunnel.id))
                .map(|tunnel| Subject {
                    id: format!("endpoint:{}", tunnel.id),
                    name: format!("{} server", tunnel.name),
                    identity: router::endpoint_series(&tunnel.key),
                }),
        );
    }
    if query.uplinks && config.direct_wan {
        subjects.extend(inventory.standby_uplinks().map(|uplink| Subject {
            id: uplink.id.clone(),
            name: format!("{} uplink", uplink.name),
            identity: router::series_id(uplink.prefix(), target),
        }));
    }
    if query.include_wan
        && config.direct_wan
        && let Some(wan) = &inventory.wan
    {
        subjects.push(Subject {
            id: wan.id.clone(),
            name: wan.name.clone(),
            identity: router::series_id(wan.prefix(), target),
        });
    }
    let tracks = lines(&root(config), step, query, &fresh_ring()?, subjects)?;
    let start = query.start / 1_000 / step * step * 1_000;
    let detail = format!("Every probe to {target} was lost");
    let now = runtime::now() * 1_000;
    let mut incidents: Vec<Incident> = tracks
        .iter()
        .filter(|track| !track.id.starts_with("endpoint:"))
        .flat_map(|track| {
            outages(
                &track.id,
                &track.name,
                &detail,
                start,
                step,
                track
                    .points
                    .iter()
                    .map(|point| (point.sent > 0).then(|| point.down())),
                now,
            )
        })
        .collect();
    incidents.sort_by_key(|incident| std::cmp::Reverse(incident.start));
    let count = usize::try_from(query.end / 1_000 / step - query.start / 1_000 / step + 1)?;
    let events = changes::read(config)
        .into_iter()
        .filter(|event| (query.start..=query.end).contains(&event.at))
        .collect();
    Ok(Response {
        start,
        step,
        count,
        series: tracks.into_iter().map(Line::from).collect(),
        events,
        incidents,
    })
}

#[cfg(test)]
mod tests {
    use super::{DAY, HOUR, LATENCY, Line, Query, Subject, collect_at, lines, outages, resolution};
    use crate::columnar;
    use crate::runtime;
    use anyhow::{Result, ensure};
    use std::net::Ipv4Addr;
    use std::path::Path;
    use wayline_protocol::schema::{RTT_BINS, rtt_bin};
    use wayline_protocol::{Bucket, Series, State};

    fn identity(index: u8) -> String {
        format!("{}_1_1_1_{index}", "a".repeat(48))
    }

    fn series(id: &str, buckets: Vec<Bucket>) -> Series {
        Series {
            id: id.into(),
            name: "Test".into(),
            interface: "eth0".into(),
            path_id: "abc".into(),
            target: Ipv4Addr::new(1, 1, 1, 1),
            state: State::Probing,
            median_us: Some(20_000),
            sent: 6,
            received: 4,
            success_age_ms: Some(0),
            jitter_us: None,
            history: Vec::new(),
            buckets,
        }
    }

    fn bucket(at: u64, value: u32, lost: u16) -> Bucket {
        Bucket {
            at,
            sent: 4,
            lost,
            min_us: Some(value),
            median_us: Some(value),
            max_us: Some(value),
            jitter_sum_us: u64::from(value / 10),
            jitter_n: 2,
            rtt: {
                let mut rtt = [0; RTT_BINS];
                rtt[rtt_bin(value)] = 4 - lost;
                rtt
            },
            uptime_ms: 0,
        }
    }

    #[test]
    fn purged_buckets_cannot_return_from_the_live_ring() {
        let mut live = vec![series(
            "test",
            vec![bucket(90, 1, 0), bucket(105, 1, 0), bucket(120, 1, 0)],
        )];
        super::discard_before(&mut live, 105);
        assert_eq!(live[0].buckets.len(), 1);
        assert_eq!(live[0].buckets[0].at, 120);
    }

    fn value(index: u64) -> u32 {
        u32::try_from(1_000 * ((index * 37) % 97 + 1)).unwrap_or_default()
    }

    fn hour(start: u64) -> Vec<Bucket> {
        (0..240)
            .map(|index| bucket(start + 15 * index, value(index), u16::from(index % 3 == 0)))
            .collect()
    }

    fn lower_median(mut values: Vec<u32>) -> u32 {
        values.sort_unstable();
        values
            .get(values.len().saturating_sub(1) / 2)
            .copied()
            .unwrap_or_default()
    }

    fn expected_hour() -> Vec<Option<i64>> {
        let minutes: Vec<u32> = (0..60)
            .map(|minute| lower_median((4 * minute..4 * minute + 4).map(value).collect()))
            .collect();
        let fives: Vec<u32> = minutes
            .chunks(5)
            .map(|chunk| lower_median(chunk.to_vec()))
            .collect();
        let values: Vec<u32> = (0..240).map(value).collect();
        let lost = (0..240).filter(|index| index % 3 == 0).count();
        vec![
            values.iter().min().copied().map(i64::from),
            Some(i64::from(lower_median(fives))),
            values.iter().max().copied().map(i64::from),
            Some(960),
            i64::try_from(lost).ok(),
            Some(values.iter().map(|value| i64::from(value / 10)).sum()),
            Some(480),
        ]
        .into_iter()
        .chain((0..RTT_BINS).map(|bin| {
            Some(
                (0..240_u64)
                    .filter(|index| rtt_bin(value(*index)) == bin)
                    .map(|index| 4 - i64::from(index % 3 == 0))
                    .sum(),
            )
        }))
        .collect()
    }

    fn stored(root: &Path, step: u64, period: u64) -> Result<Vec<Option<i64>>> {
        Ok(
            columnar::series(root, &LATENCY, step, (period, period), &[])?
                .remove(&identity(1))
                .and_then(|mut rows| rows.remove(&period))
                .unwrap_or_default(),
        )
    }

    fn request(start: u64, end: u64, step: u64) -> Query {
        Query {
            start: start * 1_000,
            end: end * 1_000,
            step,
            target: None,
            tunnel: String::new(),
            include_wan: false,
            all: false,
            endpoints: false,
            uplinks: false,
        }
    }

    fn subject(index: u8) -> Subject {
        Subject {
            id: "tunnel".into(),
            name: "Tunnel".into(),
            identity: identity(index),
        }
    }

    fn rendered(root: &Path, step: u64, start: u64, ring: &[Series]) -> Result<serde_json::Value> {
        let query = request(start, start + HOUR, step);
        let tracks: Vec<Line> = lines(root, step, &query, ring, vec![subject(1)])?
            .into_iter()
            .map(Line::from)
            .collect();
        Ok(serde_json::to_value(tracks)?)
    }

    #[test]
    fn buckets_roll_up_exactly() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        collect_at(root, 30, &[series(&identity(1), hour(start))])?;
        let row = stored(root, 3_600, start / HOUR)?;
        ensure!(row == expected_hour(), "Unexpected hour: {row:?}");
        Ok(())
    }

    #[test]
    fn repeated_collection_never_double_counts() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        let buckets = hour(start);
        collect_at(root, 30, &[series(&identity(1), buckets[..120].to_vec())])?;
        collect_at(root, 30, &[series(&identity(1), buckets.clone())])?;
        collect_at(root, 30, &[series(&identity(1), buckets)])?;
        ensure!(
            stored(root, 3_600, start / HOUR)?[3] == Some(960),
            "Double count."
        );
        Ok(())
    }

    #[test]
    fn live_buckets_merge_into_stored_periods() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let full = directory.path().join("full");
        let half = directory.path().join("half");
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        let buckets = hour(start);
        let id = identity(1);
        collect_at(&full, 30, &[series(&id, buckets.clone())])?;
        collect_at(&half, 30, &[series(&id, buckets[..130].to_vec())])?;
        let live = [series(&id, buckets[100..].to_vec())];
        for step in [15, 60, 300, 3_600] {
            let expected = rendered(&full, step, start, &[])?;
            ensure!(
                rendered(&half, step, start, &live)? == expected,
                "Live merge differs at step {step}."
            );
            let missing = directory.path().join("missing");
            ensure!(
                rendered(&missing, step, start, &[series(&id, buckets.clone())])? == expected,
                "Ring alone differs at step {step}."
            );
        }
        Ok(())
    }

    #[test]
    fn query_tiers_and_limits_are_enforced() -> Result<()> {
        let now = 1_800_000_000_000;
        ensure!(resolution(7, 60, now - DAY * 1_000, now, now)? == 60);
        ensure!(resolution(7, 1, now - DAY * 1_000, now, now)? == 60);
        ensure!(resolution(30, 3_600, now - 20 * DAY * 1_000, now, now)? == 3_600);
        for (days, step, back) in [
            (7, 15, DAY),
            (7, 3_600, DAY),
            (1, 300, DAY),
            (7, 60, 4 * DAY),
            (7, 60, 3 * DAY),
        ] {
            ensure!(
                resolution(days, step, now - back * 1_000, now, now).is_err(),
                "Accepted {days} {step} {back}"
            );
        }
        Ok(())
    }

    #[test]
    fn unsynced_buckets_are_staged_then_restamped_on_sync() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let actual = runtime::now() / HOUR * HOUR - HOUR;
        let skewed = actual - 3 * 86_400;
        let offset_ms = i64::try_from((actual - 600) * 1_000)?;
        let staged_buckets = |from: u64| -> Vec<Bucket> {
            (from..from + 8)
                .map(|index| Bucket {
                    uptime_ms: 600_000 + index * 15_000,
                    ..bucket(skewed + index * 15, 20_000, 0)
                })
                .collect()
        };
        let snapshot = |buckets: Vec<Bucket>| super::Snapshot {
            series: vec![series(&identity(1), buckets)],
            wifi: Vec::new(),
            telemetry: None,
            clock: None,
        };
        super::stage(root, "boot1", &snapshot(staged_buckets(0)))?;
        super::stage(root, "boot1", &snapshot(staged_buckets(4)))?;
        let staged = std::fs::read_to_string(root.join("pending/boot1.ndjson"))?;
        ensure!(staged.lines().count() == 2, "Expected two staged lines.");
        let synced = crate::clock::Clock {
            boot: "boot1".into(),
            synced: true,
            source: Some(crate::clock::Source::Ntp),
            offset_ms,
        };
        super::flush_pending(root, 7, runtime::now(), Some(&synced))?;
        ensure!(
            !root.join("pending/boot1.ndjson").exists(),
            "Staged file remains."
        );
        let first = actual / 15;
        let read = columnar::series(root, &LATENCY, 15, (first, first + 20), &[])?;
        let periods: Vec<u64> = read
            .get(&identity(1))
            .map(|rows| rows.keys().copied().collect())
            .unwrap_or_default();
        ensure!(
            periods.len() == 12,
            "Stored {} restamped buckets.",
            periods.len()
        );
        ensure!(
            periods[0] == first,
            "First bucket at {} not {first}.",
            periods[0]
        );
        let old = columnar::series(root, &LATENCY, 15, (skewed / 15, skewed / 15 + 20), &[])?;
        ensure!(old.is_empty(), "Buckets were stored at the skewed time.");
        Ok(())
    }

    #[test]
    fn percentiles_land_inside_their_bin() {
        let mut rtt = [0; RTT_BINS];
        rtt[rtt_bin(12_000)] = 90;
        rtt[rtt_bin(120_000)] = 10;
        let p50 = super::percentile(&rtt, 500, None, None).unwrap_or(0);
        let p95 = super::percentile(&rtt, 950, None, None).unwrap_or(0);
        let p99 = super::percentile(&rtt, 990, None, None).unwrap_or(0);
        assert!((10_000..15_000).contains(&p50), "{p50}");
        assert!((100_000..150_000).contains(&p95), "{p95}");
        assert!(p99 >= p95, "{p99} < {p95}");
        let tight = super::percentile(&rtt, 990, Some(11_000), Some(121_000)).unwrap_or(0);
        assert!((100_000..=121_000).contains(&tight), "{tight}");
        let mut slow = [0; RTT_BINS];
        slow[RTT_BINS - 1] = 3;
        assert_eq!(
            super::percentile(&slow, 500, Some(600_000), Some(900_000)),
            Some(750_000)
        );
        assert_eq!(super::percentile(&[0; RTT_BINS], 500, None, None), None);
    }

    #[test]
    fn outages_cover_fully_lost_runs() {
        let states = [
            Some(false),
            Some(true),
            Some(true),
            None,
            Some(true),
            Some(false),
            Some(true),
        ];
        let found = outages("wan", "WAN", "lost", 0, 15, states, 1_000_000);
        assert_eq!(found.len(), 3);
        assert_eq!((found[0].start, found[0].end), (15_000, Some(45_000)));
        assert_eq!((found[1].start, found[1].end), (60_000, Some(75_000)));
        assert_eq!((found[2].start, found[2].end), (90_000, Some(105_000)));
        let ongoing = outages("wan", "WAN", "lost", 0, 15, states, 110_000);
        assert_eq!(ongoing[2].end, None);
    }

    #[test]
    fn eleven_series_for_a_week_stay_small() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let start = runtime::now() / HOUR * HOUR - 168 * HOUR;
        let identities: Vec<String> = (1..=11).map(identity).collect();
        for hours in 0..168 {
            let snapshot: Vec<Series> = identities
                .iter()
                .map(|id| series(id, hour(start + hours * HOUR)))
                .collect();
            collect_at(root, 7, &snapshot)?;
        }
        let size = columnar::bytes(root);
        ensure!(size < 2 * 1024 * 1024, "History is {size} bytes.");
        let query = request(runtime::now() - DAY, runtime::now(), 60);
        let line = lines(root, 60, &query, &[], vec![subject(1)])?.remove(0);
        ensure!(line.points.len() <= 1_441, "Too many points.");
        ensure!(
            line.points.iter().filter(|point| point.sent > 0).count() > 1_000,
            "Stored minutes are missing."
        );
        Ok(())
    }
}
