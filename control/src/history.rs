use crate::changes::{self, Event};
use crate::router;
use crate::router::Inventory;
use crate::runtime;
use crate::runtime::RUNTIME;
use crate::settings::{Settings, Target};
use anyhow::{Context, Error, Result, ensure};
use rusqlite::types::{FromSql, ToSql};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use wayline_protocol::{BUCKET_SECONDS, MAX_SERIES, Series};

const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;
const MINUTE_MS: u64 = 60_000;
const MIN_CLOCK: u64 = 1_704_067_200;
const MAX_POINTS: u64 = 2_500;
const MEMORY_PAGES: u64 = 1_024;
const PAGES: u64 = 8_192;
const PAGE_BYTES: u64 = 4_096;
const VERSION: u32 = 4;
const COLUMNS: &str = "min_us,median_us,max_us,sent,lost";
const LEVELS: [(u64, &str); 4] = [
    (BUCKET_SECONDS, "s15"),
    (60, "s1m"),
    (300, "s5m"),
    (3_600, "s1h"),
];

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
    series: Vec<Line>,
    events: Vec<Event>,
}

#[derive(Serialize)]
struct Line {
    id: String,
    name: String,
    points: Vec<Point>,
}

#[derive(Clone, Default, Serialize)]
struct Point {
    at: u64,
    median: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    sent: u32,
    lost: u32,
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

pub fn database(config: &Settings) -> PathBuf {
    if config.history_days == 0 {
        Path::new(RUNTIME).join("history/latency.sqlite")
    } else {
        PathBuf::from("/etc/wayline/history/latency.sqlite")
    }
}

fn known_cache() -> PathBuf {
    Path::new(RUNTIME).join("stored-series.json")
}

fn fresh_ring() -> Result<Vec<Series>> {
    let cutoff = match fs::read_to_string(Path::new(RUNTIME).join("history-cutoff")) {
        Ok(value) => value
            .parse::<u64>()
            .context("Invalid history reset marker.")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    let mut samples = runtime::ring();
    discard_before(&mut samples, cutoff);
    Ok(samples)
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
            for name in [
                "latency.sqlite",
                "latency.sqlite-journal",
                "latency.sqlite-wal",
                "latency.sqlite-shm",
                "latency.sqlite.broken",
                "events.json",
                "facts.json",
            ] {
                remove(&root.join(name))?;
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

const fn retained(days: u32, step: u64) -> u64 {
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

fn table(step: u64) -> &'static str {
    LEVELS
        .iter()
        .find(|(width, _)| *width == step)
        .map_or(LEVELS[0].1, |level| level.1)
}

fn resolution(days: u32, step: u64, start: u64, end: u64, now: u64) -> Result<u64> {
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

fn cap(path: &Path) -> u64 {
    if path.starts_with(RUNTIME) {
        MEMORY_PAGES
    } else {
        PAGES
    }
}

fn levels(statement: impl Fn(&str) -> String) -> String {
    LEVELS.iter().map(|(_, table)| statement(table)).collect()
}

fn schema() -> String {
    let tiers = levels(|table| {
        format!(
            "CREATE TABLE {table}(period INTEGER NOT NULL,series INTEGER NOT NULL,min_us INTEGER,median_us INTEGER,max_us INTEGER,sent INTEGER NOT NULL,lost INTEGER NOT NULL,PRIMARY KEY(period,series)) WITHOUT ROWID;"
        )
    });
    format!(
        "CREATE TABLE series(id INTEGER PRIMARY KEY,name TEXT NOT NULL UNIQUE,seen INTEGER NOT NULL);{tiers}"
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    ReadOnly,
    ReadWrite,
}

fn open(path: &Path, access: Access) -> Result<Connection> {
    let connection = if access == Access::ReadOnly {
        Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?
    } else {
        Connection::open(path)?
    };
    connection.busy_timeout(Duration::from_secs(1))?;
    connection.pragma_update(None, "cache_size", -128)?;
    connection.pragma_update(None, "mmap_size", 0)?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    if access == Access::ReadWrite {
        connection.pragma_update(None, "page_size", PAGE_BYTES)?;
        connection.pragma_update(None, "max_page_count", cap(path))?;
        connection.pragma_update(None, "journal_mode", "TRUNCATE")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
    }
    Ok(connection)
}

fn pragma<T: FromSql>(path: &Path, name: &str) -> Option<T> {
    open(path, Access::ReadOnly)
        .ok()?
        .pragma_query_value(None, name, |row| row.get(0))
        .ok()
}

fn healthy(path: &Path) -> bool {
    pragma::<String>(path, "quick_check").is_some_and(|status| status == "ok")
}

fn pages(path: &Path) -> u64 {
    pragma(path, "page_count").unwrap_or_default()
}

fn convert(path: &Path) -> Result<bool> {
    let mut connection = open(path, Access::ReadWrite)?;
    let current: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current >= VERSION {
        return Ok(false);
    }
    ensure!(
        runtime::now() >= MIN_CLOCK,
        "The clock is not synchronized."
    );
    let upgrade = current == 3;
    let tables: u32 =
        connection.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))?;
    if !upgrade && tables > 0 {
        drop(connection);
        quarantine(path)?;
        connection = open(path, Access::ReadWrite)?;
    }
    connection.pragma_update(None, "max_page_count", 2 * cap(path))?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if upgrade {
        transaction.execute_batch(&format!(
            "ALTER TABLE series RENAME TO old_series;{}",
            levels(|table| format!("ALTER TABLE {table} RENAME TO old_{table};"))
        ))?;
    }
    transaction.execute_batch(&schema())?;
    if upgrade {
        transaction.execute(
            "INSERT INTO series SELECT id,name,?1 FROM old_series",
            [runtime::now() / HOUR],
        )?;
        transaction.execute_batch(&format!(
            "DROP TABLE old_series;{}DROP TABLE meta;",
            levels(|table| format!(
                "INSERT INTO {table}(series,period,{COLUMNS}) SELECT * FROM old_{table}; DROP TABLE old_{table};"
            ))
        ))?;
    }
    transaction.pragma_update(None, "user_version", VERSION)?;
    transaction.commit()?;
    if upgrade {
        let _ = connection.execute_batch("PRAGMA temp_store=DEFAULT; VACUUM;");
    }
    Ok(true)
}

fn span(live: &[&Series]) -> Option<(u64, u64)> {
    let at = live
        .iter()
        .flat_map(|series| &series.buckets)
        .map(|bucket| bucket.at / BUCKET_SECONDS);
    Some((at.clone().min()?, at.max()?))
}

const fn aligned((low, high): (u64, u64), ratio: u64) -> (u64, u64) {
    (low / ratio * ratio, (high / ratio + 1) * ratio - 1)
}

fn fill(connection: &Connection, live: &[&Series]) -> Result<()> {
    connection.execute_batch(
        "CREATE TEMP TABLE live(period INTEGER NOT NULL,series TEXT NOT NULL,min_us INTEGER,median_us INTEGER,max_us INTEGER,sent INTEGER NOT NULL,lost INTEGER NOT NULL,PRIMARY KEY(period,series)) WITHOUT ROWID;",
    )?;
    let mut insert =
        connection.prepare("INSERT OR REPLACE INTO live VALUES(?1,?2,?3,?4,?5,?6,?7)")?;
    for series in live {
        for bucket in &series.buckets {
            insert.execute(params![
                bucket.at / BUCKET_SECONDS,
                series.id,
                bucket.min_us,
                bucket.median_us,
                bucket.max_us,
                bucket.sent,
                bucket.lost
            ])?;
        }
    }
    Ok(())
}

fn rollup(source: &str) -> String {
    format!(
        "SELECT p,series,min(min_us),max(CASE WHEN n=(c+1)/2 THEN median_us END),max(max_us),sum(sent),sum(lost) FROM (SELECT period/?1 AS p,series,min_us,median_us,max_us,sent,lost,row_number() OVER (w ORDER BY median_us IS NULL,median_us) AS n,count(median_us) OVER w AS c FROM {source} WHERE period BETWEEN ?2 AND ?3 WINDOW w AS (PARTITION BY series,period/?1)) GROUP BY p,series"
    )
}

const RETIRED: &str = "SELECT o.id FROM series o WHERE o.seen<?1 OR o.id NOT IN (SELECT id FROM series ORDER BY (name IN (SELECT series FROM live)) DESC,seen DESC,id DESC LIMIT ?2)";

fn append(path: &Path, days: u32, now: u64, ring: &[Series]) -> Result<BTreeSet<String>> {
    let live: Vec<&Series> = ring
        .iter()
        .take(MAX_SERIES)
        .filter(|series| !series.buckets.is_empty())
        .collect();
    let mut connection = open(path, Access::ReadWrite)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    fill(&transaction, &live)?;
    if let Some(mut range) = span(&live) {
        transaction.execute(
            "INSERT INTO series(name,seen) SELECT DISTINCT series,?1 FROM live WHERE true ON CONFLICT(name) DO UPDATE SET seen=excluded.seen WHERE seen<excluded.seen",
            [now / HOUR],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO s15 SELECT period,s.id,min_us,median_us,max_us,sent,lost FROM live l JOIN series s ON s.name=l.series",
            [],
        )?;
        for pair in LEVELS.windows(2) {
            let [(child_step, child), (step, parent)] = pair else {
                continue;
            };
            if retained(days, *step) == 0 {
                break;
            }
            let ratio = step / child_step;
            let (low, high) = aligned(range, ratio);
            transaction.execute(
                &format!("INSERT OR REPLACE INTO {parent} {}", rollup(child)),
                [ratio, low, high],
            )?;
            range = (range.0 / ratio, range.1 / ratio);
        }
    }
    for (step, table) in LEVELS {
        let kept = retained(days, step);
        transaction.execute(
            &format!("DELETE FROM {table} WHERE period<?1"),
            [now.saturating_sub(kept) / step + u64::from(kept == 0)],
        )?;
    }
    let longest = LEVELS
        .iter()
        .map(|(step, _)| retained(days, *step))
        .max()
        .unwrap_or_default();
    let retired = (now.saturating_sub(longest) / HOUR, MAX_SERIES);
    for (_, table) in LEVELS {
        transaction.execute(
            &format!("DELETE FROM {table} WHERE series IN ({RETIRED})"),
            retired,
        )?;
    }
    transaction.execute(
        &format!("DELETE FROM series WHERE id IN ({RETIRED})"),
        retired,
    )?;
    transaction.commit()?;
    Ok(connection
        .prepare("SELECT DISTINCT substr(name,1,48) FROM series")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn quarantine(path: &Path) -> Result<()> {
    fs::rename(path, path.with_file_name("latency.sqlite.broken"))?;
    let _ = fs::remove_file(path.with_file_name("latency.sqlite-journal"));
    changes::append(
        path,
        [Event {
            at: runtime::now() * 1000,
            subject: "history".into(),
            kind: "history".into(),
            text: "The history database was damaged and was reset.".into(),
        }],
    )
}

fn shrink(path: &Path) -> Result<()> {
    let mut connection = open(path, Access::ReadWrite)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (_, table) in LEVELS.iter().rev() {
        transaction.execute(
            &format!(
                "DELETE FROM {table} WHERE period<=(SELECT period FROM {table} ORDER BY period LIMIT 1 OFFSET (SELECT count(*)/10 FROM {table}))"
            ),
            [],
        )?;
    }
    Ok(transaction.commit()?)
}

fn recover(path: &Path, error: Error) -> Result<()> {
    if path.exists() && !healthy(path) {
        quarantine(path)?;
    }
    if convert(path)? {
        return Ok(());
    }
    if pages(path) < cap(path) {
        return Err(error);
    }
    shrink(path)
}

fn store(path: &Path, days: u32, ring: &[Series]) -> Result<BTreeSet<String>> {
    let now = runtime::now();
    append(path, days, now, ring).or_else(|error| {
        recover(path, error)?;
        append(path, days, now, ring)
    })
}

#[cfg(test)]
fn collect_at(path: &Path, days: u32, ring: &[Series]) -> Result<()> {
    store(path, days, ring).map(drop)
}

pub fn collect(config: &Settings) -> Result<()> {
    runtime::locked("history", || collect_inner(config))
}

fn collect_inner(config: &Settings) -> Result<()> {
    let path = database(config);
    runtime::directory(path.parent().context("Invalid history directory.")?)?;
    let cache = known_cache();
    let ring = fresh_ring()?;
    let idle = ring.iter().all(|series| series.buckets.is_empty());
    if runtime::now() < MIN_CLOCK || (idle && cache.exists()) {
        return Ok(());
    }
    publish(&cache, &store(&path, config.history_days, &ring)?)
}

pub fn report(config: &Settings) -> Result<Report> {
    let path = database(config);
    let bytes = fs::metadata(&path).map_or(0, |meta| meta.len());
    let mut lines = Vec::new();
    if path.exists() {
        let connection = open(&path, Access::ReadOnly)?;
        let show = |value: Option<u64>| value.map_or_else(String::new, |value| value.to_string());
        for (_, table) in LEVELS {
            let (rows, oldest, newest): (u64, Option<u64>, Option<u64>) = connection.query_row(
                &format!("SELECT count(*),min(period),max(period) FROM {table}"),
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            lines.push(format!(
                "{table}: {rows} rows, oldest period {}, newest period {}",
                show(oldest),
                show(newest)
            ));
        }
    }
    Ok(Report {
        bytes,
        cap: cap(&path) * PAGE_BYTES,
        lines,
    })
}

fn reader(path: &Path) -> Result<Connection> {
    if path.exists() {
        let connection = open(path, Access::ReadOnly)?;
        if connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))? >= 3 {
            return Ok(connection);
        }
    }
    let connection = Connection::open_in_memory()?;
    connection.execute_batch(&schema())?;
    Ok(connection)
}

fn lines(
    path: &Path,
    step: u64,
    query: &Query,
    ring: &[Series],
    subjects: Vec<Subject>,
) -> Result<Vec<Line>> {
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    let (first, last) = (query.start / 1_000 / step, query.end / 1_000 / step);
    let live: Vec<&Series> = ring
        .iter()
        .filter(|series| {
            !series.buckets.is_empty()
                && subjects.iter().any(|subject| subject.identity == series.id)
        })
        .collect();
    let ratio = step / BUCKET_SECONDS;
    let blank: Vec<Point> = (first..=last)
        .map(|period| Point {
            at: period * step * 1_000,
            ..Point::default()
        })
        .collect();
    let (identities, mut lines): (Vec<String>, Vec<Line>) = subjects
        .into_iter()
        .map(|subject| {
            (
                subject.identity,
                Line {
                    id: subject.id,
                    name: subject.name,
                    points: blank.clone(),
                },
            )
        })
        .unzip();
    let names: Vec<&dyn ToSql> = identities.iter().map(|id| id as &dyn ToSql).collect();
    let marks = vec!["?"; names.len()].join(",");
    let stored = |table: &str| {
        format!(
            "SELECT t.period,s.name,t.min_us,t.median_us,t.max_us,t.sent,t.lost FROM {table} t JOIN series s ON s.id=t.series WHERE t.period BETWEEN ? AND ? AND s.name IN ({marks})"
        )
    };
    let index: BTreeMap<&str, usize> = identities.iter().map(String::as_str).zip(0..).collect();
    let mut connection = reader(path)?;
    let transaction = connection.transaction()?;
    let bounds = span(&live).map(|range| aligned(range, ratio));
    if let Some((low, high)) = bounds {
        fill(&transaction, &live)?;
        let mut arguments: Vec<&dyn ToSql> = vec![&low, &high];
        arguments.extend(names.iter().copied());
        transaction.execute(
            &format!("INSERT OR IGNORE INTO live {}", stored(LEVELS[0].1)),
            arguments.as_slice(),
        )?;
    }
    let mut load = |statement: &str, arguments: &[&dyn ToSql]| -> Result<()> {
        let mut prepared = transaction.prepare(statement)?;
        let mut rows = prepared.query(arguments)?;
        while let Some(row) = rows.next()? {
            let period: u64 = row.get(0)?;
            let name: String = row.get(1)?;
            if let Some(&line) = index.get(name.as_str())
                && let Some(slot) = period
                    .checked_sub(first)
                    .and_then(|offset| usize::try_from(offset).ok())
                    .and_then(|offset| lines.get_mut(line)?.points.get_mut(offset))
            {
                *slot = Point {
                    at: period * step * 1_000,
                    min: row.get::<_, Option<u32>>(2)?.map(runtime::milliseconds),
                    median: row.get::<_, Option<u32>>(3)?.map(runtime::milliseconds),
                    max: row.get::<_, Option<u32>>(4)?.map(runtime::milliseconds),
                    sent: row.get(5)?,
                    lost: row.get(6)?,
                };
            }
        }
        Ok(())
    };
    let mut arguments: Vec<&dyn ToSql> = vec![&first, &last];
    arguments.extend(names.iter().copied());
    load(&stored(table(step)), &arguments)?;
    if let Some((low, high)) = bounds {
        load(&rollup("live"), &[&ratio, &low, &high])?;
    }
    Ok(lines)
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
    let series = lines(&database(config), step, query, &fresh_ring()?, subjects)?;
    let events = changes::read(config)
        .into_iter()
        .filter(|event| (query.start..=query.end).contains(&event.at))
        .collect();
    Ok(Response { series, events })
}

#[cfg(test)]
mod tests {
    use super::{
        Access, DAY, HOUR, LEVELS, Query, Subject, append, collect_at, convert, lines, open,
        resolution,
    };
    use crate::runtime;
    use anyhow::{Result, ensure};
    use std::fmt::Write;
    use std::fs;
    use std::net::Ipv4Addr;
    use std::path::Path;
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
        super::discard_before(&mut live, 105);
        assert_eq!(live[0].buckets.len(), 1);
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

    fn hour_row() -> (u32, usize, u32, u32, u32) {
        let minutes: Vec<u32> = (0..60)
            .map(|minute| lower_median((4 * minute..4 * minute + 4).map(value).collect()))
            .collect();
        let fives: Vec<u32> = minutes
            .chunks(5)
            .map(|chunk| lower_median(chunk.to_vec()))
            .collect();
        let values: Vec<u32> = (0..240).map(value).collect();
        (
            960,
            (0..240).filter(|index| index % 3 == 0).count(),
            values.iter().min().copied().unwrap_or_default(),
            lower_median(fives),
            values.iter().max().copied().unwrap_or_default(),
        )
    }

    fn scalar(path: &Path, statement: &str) -> Result<i64> {
        Ok(open(path, Access::ReadOnly)?.query_row(statement, [], |row| row.get(0))?)
    }

    fn edit(path: &Path, statement: &str) -> Result<()> {
        Ok(open(path, Access::ReadWrite)?.execute_batch(statement)?)
    }

    fn counts(path: &Path) -> Result<Vec<i64>> {
        LEVELS
            .iter()
            .map(|(_, table)| scalar(path, &format!("SELECT count(*) FROM {table}")))
            .collect()
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

    fn rendered(path: &Path, step: u64, start: u64, ring: &[Series]) -> Result<serde_json::Value> {
        let query = request(start, start + HOUR, step);
        let mut value = serde_json::to_value(lines(path, step, &query, ring, vec![subject(1)])?)?;
        if step > 60 {
            for point in value[0]["points"].as_array_mut().into_iter().flatten() {
                point["median"] = serde_json::Value::Null;
            }
        }
        Ok(value)
    }

    #[test]
    fn buckets_roll_up_exactly() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        collect_at(&database, 30, &[series(&identity(1), hour(start))])?;
        let row = open(&database, Access::ReadOnly)?.query_row(
            "SELECT sent,lost,min_us,median_us,max_us FROM s1h WHERE period=?1",
            [start / HOUR],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        ensure!(row == hour_row(), "Unexpected hour: {row:?}");
        let counts = counts(&database)?;
        ensure!(counts == [240, 60, 12, 1], "Unexpected counts: {counts:?}");
        Ok(())
    }

    #[test]
    fn repeated_collection_never_double_counts() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        let buckets = hour(start);
        collect_at(
            &database,
            30,
            &[series(&identity(1), buckets[..120].to_vec())],
        )?;
        collect_at(&database, 30, &[series(&identity(1), buckets.clone())])?;
        collect_at(&database, 30, &[series(&identity(1), buckets)])?;
        ensure!(
            scalar(&database, "SELECT sum(sent) FROM s1h")? == 960,
            "Double count."
        );
        Ok(())
    }

    #[test]
    fn tiers_prune_to_their_retention() -> Result<()> {
        let now = runtime::now();
        let minute = now / 60 * 60 - 120;
        for (days, expected) in [(1_u32, [2, 1, 0, 0]), (7, [2, 2, 1, 0])] {
            let directory = tempfile::tempdir()?;
            let database = directory.path().join("latency.sqlite");
            collect_at(
                &database,
                days,
                &[series(&identity(1), vec![bucket(minute, 20_000, 0)])],
            )?;
            edit(
                &database,
                &format!(
                    "INSERT INTO s15 SELECT {},id,1,1,1,1,0 FROM series; INSERT INTO s1m SELECT {},id,1,1,1,1,0 FROM series; INSERT INTO s5m SELECT {},id,1,1,1,1,0 FROM series;",
                    (now - 7 * HOUR) / 15,
                    (now - 2 * DAY) / 60,
                    (now - 8 * DAY) / 300
                ),
            )?;
            collect_at(
                &database,
                days,
                &[series(&identity(1), vec![bucket(minute + 15, 20_000, 0)])],
            )?;
            let counts = counts(&database)?;
            ensure!(counts == expected, "Days {days}: {counts:?}");
        }
        Ok(())
    }

    #[test]
    fn idle_series_expire_with_their_rows() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = runtime::now() / 60 * 60 - 120;
        collect_at(
            &database,
            1,
            &[
                series(&identity(1), vec![bucket(minute, 20_000, 0)]),
                series(&identity(2), vec![bucket(minute, 20_000, 0)]),
            ],
        )?;
        edit(
            &database,
            &format!(
                "UPDATE series SET seen={} WHERE name='{}';",
                runtime::now() / HOUR - 49,
                identity(2)
            ),
        )?;
        collect_at(
            &database,
            1,
            &[series(&identity(1), vec![bucket(minute + 15, 20_000, 0)])],
        )?;
        for statement in [
            "SELECT count(*) FROM series",
            "SELECT count(DISTINCT series) FROM s15",
            "SELECT count(DISTINCT series) FROM s1m",
        ] {
            ensure!(scalar(&database, statement)? == 1, "{statement}");
        }
        Ok(())
    }

    #[test]
    fn retiring_many_series_leaves_no_orphans() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = runtime::now() / 60 * 60 - 120;
        let all: Vec<Series> = (1..=80)
            .map(|index| series(&identity(index), vec![bucket(minute, 20_000, 0)]))
            .collect();
        collect_at(&database, 30, &all)?;
        edit(
            &database,
            &format!(
                "UPDATE series SET seen={} WHERE id>1;",
                runtime::now() / HOUR - 721
            ),
        )?;
        collect_at(
            &database,
            30,
            &[series(&identity(1), vec![bucket(minute + 15, 20_000, 0)])],
        )?;
        ensure!(scalar(&database, "SELECT count(*) FROM series")? == 1);
        for (_, table) in LEVELS {
            ensure!(
                scalar(
                    &database,
                    &format!(
                        "SELECT count(*) FROM {table} WHERE series NOT IN (SELECT id FROM series)"
                    )
                )? == 0,
                "Orphans in {table}."
            );
        }
        Ok(())
    }

    #[test]
    fn series_cap_keeps_live_series_over_newer_ids() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = runtime::now() / 60 * 60 - 120;
        let all: Vec<Series> = (1..=90)
            .map(|index| series(&identity(index), vec![bucket(minute, 20_000, 0)]))
            .collect();
        collect_at(&database, 1, &all)?;
        collect_at(
            &database,
            1,
            &[
                series(&identity(91), vec![bucket(minute + 15, 20_000, 0)]),
                series(&identity(1), vec![bucket(minute + 15, 20_000, 0)]),
            ],
        )?;
        ensure!(scalar(&database, "SELECT count(*) FROM series")? == 90);
        ensure!(
            scalar(
                &database,
                &format!(
                    "SELECT count(*) FROM series WHERE name IN ('{}','{}')",
                    identity(1),
                    identity(91)
                )
            )? == 2
        );
        Ok(())
    }

    #[test]
    fn unknown_schema_is_reset() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        edit(
            &database,
            "CREATE TABLE samples(x INTEGER); PRAGMA user_version=2;",
        )?;
        ensure!(convert(&database)?, "Expected a conversion.");
        ensure!(
            directory.path().join("latency.sqlite.broken").exists(),
            "The old database was not moved aside."
        );
        ensure!(scalar(&database, "PRAGMA user_version")? == 4);
        ensure!(
            scalar(
                &database,
                "SELECT count(*) FROM sqlite_master WHERE name='samples'"
            )? == 0
        );
        Ok(())
    }

    #[test]
    fn schema_v3_keeps_every_tier_when_converted() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let mut tiers = String::new();
        for (index, (_, table)) in LEVELS.iter().enumerate() {
            write!(
                tiers,
                "CREATE TABLE {table}(series INTEGER NOT NULL, period INTEGER NOT NULL, min_us INTEGER, median_us INTEGER, max_us INTEGER, sent INTEGER NOT NULL, lost INTEGER NOT NULL, PRIMARY KEY(series, period)) WITHOUT ROWID; INSERT INTO {table} VALUES(1,{},1,2,3,4,{index});",
                100 + index
            )?;
        }
        let script = format!(
            "CREATE TABLE series(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE); CREATE TABLE meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID; INSERT INTO series(name) VALUES('x'); INSERT INTO meta VALUES('cursor',5);{tiers}PRAGMA user_version=3;"
        );
        edit(&database, &script)?;
        ensure!(
            append(&database, 30, runtime::now(), &[]).is_err(),
            "A stale schema must fail the write so that it is converted."
        );
        ensure!(convert(&database)?, "Expected a conversion.");
        ensure!(scalar(&database, "PRAGMA user_version")? == 4);
        ensure!(
            scalar(
                &database,
                "SELECT lost FROM s15 WHERE period=100 AND series=1"
            )? == 0
        );
        ensure!(
            scalar(
                &database,
                "SELECT lost FROM s1h WHERE period=103 AND series=1"
            )? == 3
        );
        ensure!(
            scalar(
                &database,
                "SELECT count(*) FROM sqlite_master WHERE name IN ('meta','old_series','old_s15')"
            )? == 0
        );
        ensure!(scalar(&database, "SELECT count(*) FROM series WHERE seen>0")? == 1);
        append(&database, 30, runtime::now() + 60 * DAY, &[])?;
        ensure!(
            scalar(&database, "SELECT count(*) FROM s1m")? == 0,
            "Rows outlived their series."
        );
        ensure!(!convert(&database)?, "A current schema must be left alone.");
        Ok(())
    }

    #[test]
    fn live_buckets_merge_into_stored_periods() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let full = directory.path().join("full.sqlite");
        let half = directory.path().join("half.sqlite");
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
            let missing = directory.path().join("missing.sqlite");
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
    fn damaged_database_is_moved_aside_and_recorded() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        fs::write(&database, [0x5a_u8; 8192])?;
        let fresh = runtime::now() / 15 * 15 - 60;
        collect_at(
            &database,
            7,
            &[series(&identity(1), vec![bucket(fresh, 20_000, 0)])],
        )?;
        ensure!(
            directory.path().join("latency.sqlite.broken").exists(),
            "Damaged database was not moved aside."
        );
        ensure!(scalar(&database, "SELECT count(*) FROM s15")? == 1);
        let events = fs::read_to_string(directory.path().join("events.json"))?;
        ensure!(
            events.contains("The history database was damaged and was reset."),
            "Missing recovery event."
        );
        Ok(())
    }

    #[test]
    fn eleven_series_for_a_week_stay_small() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let start = runtime::now() / HOUR * HOUR - 168 * HOUR;
        let identities: Vec<String> = (1..=11).map(identity).collect();
        for hours in 0..168 {
            let snapshot: Vec<Series> = identities
                .iter()
                .map(|id| series(id, hour(start + hours * HOUR)))
                .collect();
            collect_at(&database, 7, &snapshot)?;
        }
        let size = fs::metadata(&database)?.len();
        ensure!(size < 4 * 1024 * 1024, "Database is {size} bytes.");
        let query = request(runtime::now() - DAY, runtime::now(), 60);
        let line = lines(&database, 60, &query, &[], vec![subject(1)])?.remove(0);
        ensure!(line.points.len() <= 1_441, "Too many points.");
        ensure!(
            line.points.iter().filter(|point| point.sent > 0).count() > 1_000,
            "Stored minutes are missing."
        );
        let json = serde_json::to_vec(&line)?.len();
        ensure!(json < 262_144, "Response is {json} bytes.");
        ensure!(LEVELS.len() == 4, "Unexpected tiers.");
        Ok(())
    }
}
