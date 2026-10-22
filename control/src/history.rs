use crate::{
    changes::{self, Event},
    router::{Inventory, endpoint_series, endpoints_enabled, series_id, tunnel_prefix},
    runtime::{RUNTIME, directory, milliseconds, now, ring},
    settings::{self, Settings, Target},
};
use anyhow::{Context, Error, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write},
    fs,
    path::{Path, PathBuf},
};
use wayline_protocol::{BUCKET_SECONDS, MAX_SERIES, Series};
use wayline_support::atomic;

const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;
const MINUTE_MS: u64 = 60_000;
const MIN_CLOCK: u64 = 1_704_067_200;
const MAX_POINTS: u64 = 2_500;
const MEMORY_PAGES: u64 = 1_024;
const PAGES: u64 = 8_192;
const PAGE_BYTES: u64 = 4_096;
const VERSION: u32 = 4;
const LIMIT: usize = 2 * 1024 * 1024;
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

struct Null(Option<u32>);

impl fmt::Display for Null {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(formatter, "{value}"),
            None => formatter.write_str("NULL"),
        }
    }
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

pub fn known_prefixes() -> BTreeSet<String> {
    fs::read(known_cache())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn publish(cache: &Path, output: &str) -> Result<()> {
    let prefixes: BTreeSet<&str> = output.lines().collect();
    let encoded = serde_json::to_vec(&prefixes)?;
    if fs::read(cache).is_ok_and(|current| current == encoded) {
        return Ok(());
    }
    atomic(cache, &encoded)
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

fn retire(script: &mut String, predicate: &str) -> Result<()> {
    for (_, table) in LEVELS {
        write!(
            script,
            "DELETE FROM {table} WHERE series IN (SELECT o.id FROM series o WHERE {predicate});"
        )?;
    }
    write!(
        script,
        "DELETE FROM series WHERE id IN (SELECT o.id FROM series o WHERE {predicate});"
    )?;
    Ok(())
}

fn healthy(path: &Path) -> bool {
    sql(path, true, "PRAGMA quick_check;").is_ok_and(|output| output.trim() == "ok")
}

fn pages(path: &Path) -> u64 {
    sql(path, true, "PRAGMA page_count;")
        .ok()
        .and_then(|output| output.trim().parse().ok())
        .unwrap_or_default()
}

fn version(path: &Path) -> Result<u32> {
    Ok(sql(path, true, "PRAGMA user_version;")?.trim().parse()?)
}

fn cap(path: &Path) -> u64 {
    if path.starts_with(RUNTIME) {
        MEMORY_PAGES
    } else {
        PAGES
    }
}

fn schema() -> Result<String> {
    let mut text = String::from(
        "CREATE TABLE series(id INTEGER PRIMARY KEY,name TEXT NOT NULL UNIQUE,seen INTEGER NOT NULL);",
    );
    for (_, table) in LEVELS {
        write!(
            text,
            "CREATE TABLE {table}(period INTEGER NOT NULL,series INTEGER NOT NULL,min_us INTEGER,median_us INTEGER,max_us INTEGER,sent INTEGER NOT NULL,lost INTEGER NOT NULL,PRIMARY KEY(period,series)) WITHOUT ROWID;"
        )?;
    }
    Ok(text)
}

fn convert(path: &Path) -> Result<bool> {
    let state = sql(
        path,
        false,
        "PRAGMA user_version; SELECT count(*) FROM sqlite_master;",
    )?;
    let mut lines = state.lines();
    let version: u32 = lines.next().context("Missing schema version.")?.parse()?;
    let tables: u32 = lines.next().context("Missing schema size.")?.parse()?;
    if version >= VERSION {
        return Ok(false);
    }
    ensure!(now() >= MIN_CLOCK, "The clock is not synchronized.");
    let upgrade = version == 3;
    if upgrade {
        ensure!(healthy(path), "History database is damaged.");
    } else if tables > 0 {
        quarantine(path)?;
    }
    let mut script = format!("PRAGMA max_page_count={}; BEGIN IMMEDIATE;", 2 * cap(path));
    if upgrade {
        script.push_str("ALTER TABLE series RENAME TO old_series;");
        for (_, table) in LEVELS {
            write!(script, "ALTER TABLE {table} RENAME TO old_{table};")?;
        }
    }
    script.push_str(&schema()?);
    if upgrade {
        write!(
            script,
            "INSERT INTO series SELECT id,name,{} FROM old_series; DROP TABLE old_series;",
            now() / HOUR
        )?;
        for (_, table) in LEVELS {
            write!(
                script,
                "INSERT INTO {table}(series,period,{COLUMNS}) SELECT * FROM old_{table}; DROP TABLE old_{table};"
            )?;
        }
        script.push_str("DROP TABLE meta;");
    }
    write!(script, "PRAGMA user_version={VERSION}; COMMIT;")?;
    sql(path, false, &script)?;
    if upgrade {
        let _ = sql(path, false, "PRAGMA temp_store=DEFAULT; VACUUM;");
    }
    Ok(true)
}

fn sql(path: &Path, readonly: bool, statement: &str) -> Result<String> {
    let mut input = String::with_capacity(statement.len() + 320);
    input.push_str(".output /dev/null\nPRAGMA busy_timeout=1000; PRAGMA cache_size=-128; PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;");
    if !readonly {
        write!(
            input,
            " PRAGMA page_size=4096; PRAGMA max_page_count={}; PRAGMA journal_mode=TRUNCATE; PRAGMA synchronous=FULL; DROP TRIGGER IF EXISTS retire;",
            cap(path)
        )?;
    }
    input.push_str("\n.output stdout\n");
    input.push_str(statement);
    let path = path.to_str().context("Invalid history path.")?;
    let arguments: &[&str] = if readonly {
        &["-readonly", "-batch", "-bail", path]
    } else {
        &["-batch", "-bail", path]
    };
    Ok(String::from_utf8(wayline_support::input(
        "sqlite3",
        arguments,
        input.as_bytes(),
        LIMIT,
        5,
    )?)?)
}

fn span(live: &[&Series]) -> Option<(u64, u64)> {
    live.iter()
        .flat_map(|series| &series.buckets)
        .map(|bucket| bucket.at / BUCKET_SECONDS)
        .fold(None, |span, at| {
            Some(span.map_or((at, at), |(low, high): (u64, u64)| {
                (low.min(at), high.max(at))
            }))
        })
}

const fn aligned((low, high): (u64, u64), ratio: u64) -> (u64, u64) {
    (low / ratio * ratio, (high / ratio + 1) * ratio - 1)
}

fn live_table(script: &mut String, live: &[&Series]) -> Result<()> {
    script.push_str(
        "CREATE TEMP TABLE live(period INTEGER NOT NULL,series TEXT NOT NULL,min_us INTEGER,median_us INTEGER,max_us INTEGER,sent INTEGER NOT NULL,lost INTEGER NOT NULL,PRIMARY KEY(period,series)) WITHOUT ROWID;",
    );
    for series in live {
        ensure!(settings::id(&series.id), "Invalid measurement identity.");
        for (index, bucket) in series.buckets.iter().enumerate() {
            script.push_str(if index == 0 {
                "INSERT OR REPLACE INTO live VALUES"
            } else {
                ","
            });
            write!(
                script,
                "({},'{}',{},{},{},{},{})",
                bucket.at / BUCKET_SECONDS,
                series.id,
                Null(bucket.min_us),
                Null(bucket.median_us),
                Null(bucket.max_us),
                bucket.sent,
                bucket.lost
            )?;
        }
        script.push(';');
    }
    Ok(())
}

fn rollup(source: &str, ratio: u64, range: (u64, u64)) -> String {
    let (low, high) = aligned(range, ratio);
    format!(
        "SELECT p,series,min(min_us),max(CASE WHEN n=(c+1)/2 THEN median_us END),max(max_us),sum(sent),sum(lost) FROM (SELECT period/{ratio} AS p,series,min_us,median_us,max_us,sent,lost,row_number() OVER (w ORDER BY median_us IS NULL,median_us) AS n,count(median_us) OVER w AS c FROM {source} WHERE period BETWEEN {low} AND {high} WINDOW w AS (PARTITION BY series,period/{ratio})) GROUP BY p,series"
    )
}

fn collection(days: u32, now: u64, ring: &[Series]) -> Result<String> {
    let live: Vec<&Series> = ring
        .iter()
        .take(MAX_SERIES)
        .filter(|series| !series.buckets.is_empty())
        .collect();
    let mut script = String::from("BEGIN IMMEDIATE;");
    live_table(&mut script, &live)?;
    if let Some(mut range) = span(&live) {
        write!(
            script,
            "INSERT INTO series(name,seen) SELECT DISTINCT series,{} FROM live WHERE true ON CONFLICT(name) DO UPDATE SET seen=excluded.seen WHERE seen<excluded.seen; \
             INSERT OR IGNORE INTO s15 SELECT period,s.id,min_us,median_us,max_us,sent,lost FROM live l JOIN series s ON s.name=l.series;",
            now / HOUR
        )?;
        for pair in LEVELS.windows(2) {
            let [(child_step, child), (step, parent)] = pair else {
                continue;
            };
            if retained(days, *step) == 0 {
                break;
            }
            let ratio = step / child_step;
            write!(
                script,
                "INSERT OR REPLACE INTO {parent} {};",
                rollup(child, ratio, range)
            )?;
            range = (range.0 / ratio, range.1 / ratio);
        }
    }
    for (step, table) in LEVELS {
        let kept = retained(days, step);
        write!(
            script,
            "DELETE FROM {table} WHERE period<{};",
            now.saturating_sub(kept) / step + u64::from(kept == 0)
        )?;
    }
    let longest = LEVELS
        .iter()
        .map(|(step, _)| retained(days, *step))
        .max()
        .unwrap_or_default();
    retire(
        &mut script,
        &format!(
            "o.seen<{} OR o.id NOT IN (SELECT id FROM series ORDER BY (name IN (SELECT series FROM live)) DESC,seen DESC,id DESC LIMIT {MAX_SERIES})",
            now.saturating_sub(longest) / HOUR
        ),
    )?;
    script.push_str("COMMIT; SELECT DISTINCT substr(name,1,48) FROM series;");
    Ok(script)
}

fn quarantine(path: &Path) -> Result<()> {
    fs::rename(path, path.with_file_name("latency.sqlite.broken"))?;
    let _ = fs::remove_file(path.with_file_name("latency.sqlite-journal"));
    changes::append(
        path,
        [Event {
            at: now() * 1000,
            subject: "history".into(),
            kind: "history".into(),
            text: "The history database was damaged and was reset.".into(),
        }],
    )
}

fn shrink(path: &Path) -> Result<()> {
    let mut script = String::from("BEGIN IMMEDIATE;");
    for (_, table) in LEVELS.iter().rev() {
        write!(
            script,
            "DELETE FROM {table} WHERE period<=(SELECT period FROM {table} ORDER BY period LIMIT 1 OFFSET (SELECT count(*)/10 FROM {table}));"
        )?;
    }
    script.push_str("COMMIT;");
    sql(path, false, &script).map(drop)
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

fn store(path: &Path, days: u32, ring: &[Series]) -> Result<String> {
    let script = collection(days, now(), ring)?;
    sql(path, false, &script).or_else(|error| {
        recover(path, error)?;
        sql(path, false, &script)
    })
}

#[cfg(test)]
fn collect_at(path: &Path, days: u32, ring: &[Series]) -> Result<()> {
    store(path, days, ring).map(drop)
}

pub fn collect(config: &Settings) -> Result<()> {
    let path = database(config);
    directory(path.parent().context("Invalid history directory.")?)?;
    let cache = known_cache();
    let ring = ring();
    let idle = ring.iter().all(|series| series.buckets.is_empty());
    if now() < MIN_CLOCK || (idle && cache.exists()) {
        return Ok(());
    }
    publish(&cache, &store(&path, config.history_days, &ring)?)
}

pub fn report(config: &Settings) -> Result<Report> {
    let path = database(config);
    let bytes = fs::metadata(&path).map_or(0, |meta| meta.len());
    let mut lines = Vec::new();
    if path.exists() {
        let query = LEVELS
            .iter()
            .map(|(_, table)| {
                format!("SELECT '{table}',count(*),min(period),max(period) FROM {table}")
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        for line in sql(&path, true, &format!("{query};"))?.lines() {
            let fields: Vec<&str> = line.split('|').collect();
            if let &[table, rows, oldest, newest] = fields.as_slice() {
                lines.push(format!(
                    "{table}: {rows} rows, oldest period {oldest}, newest period {newest}"
                ));
            }
        }
    }
    Ok(Report {
        bytes,
        cap: cap(&path) * PAGE_BYTES,
        lines,
    })
}

fn stored(table: &str, low: u64, high: u64, names: &str) -> String {
    format!(
        "SELECT t.period,s.name,t.min_us,t.median_us,t.max_us,t.sent,t.lost FROM {table} t JOIN series s ON s.id=t.series WHERE t.period BETWEEN {low} AND {high} AND s.name IN ({names})"
    )
}

fn memory(script: &str) -> Result<String> {
    let mut full = schema()?;
    full.push_str(script);
    sql(Path::new(":memory:"), false, &full)
}

fn read(path: &Path, script: &str) -> Result<String> {
    match path.exists().then(|| sql(path, true, script)).transpose() {
        Ok(Some(output)) => Ok(output),
        Ok(None) => memory(script),
        Err(_) if version(path)? < 3 => memory(script),
        Err(error) => Err(error),
    }
}

fn parse(row: &str, step: u64) -> Option<(&str, u64, Point)> {
    let mut fields = row.split('|');
    let period: u64 = fields.next()?.parse().ok()?;
    let name = fields.next()?;
    let mut number = || fields.next().map(|value| value.parse::<u32>().ok());
    let (min, median, max) = (number()?, number()?, number()?);
    let (sent, lost) = (number()??, number()??);
    Some((
        name,
        period,
        Point {
            at: period * step * 1_000,
            median: median.map(milliseconds),
            min: min.map(milliseconds),
            max: max.map(milliseconds),
            sent,
            lost,
        },
    ))
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
    ensure!(
        subjects
            .iter()
            .all(|subject| settings::id(&subject.identity)),
        "Invalid measurement identity."
    );
    let (first, last) = (query.start / 1_000 / step, query.end / 1_000 / step);
    let names = subjects
        .iter()
        .map(|subject| format!("'{}'", subject.identity))
        .collect::<Vec<_>>()
        .join(",");
    let live: Vec<&Series> = ring
        .iter()
        .filter(|series| {
            !series.buckets.is_empty()
                && subjects.iter().any(|subject| subject.identity == series.id)
        })
        .collect();
    let range = span(&live);
    let ratio = step / BUCKET_SECONDS;
    let mut script = String::new();
    if let Some(range) = range {
        let (low, high) = aligned(range, ratio);
        live_table(&mut script, &live)?;
        write!(
            script,
            "INSERT OR IGNORE INTO live {};",
            stored(LEVELS[0].1, low, high, &names)
        )?;
    }
    write!(script, "{};", stored(table(step), first, last, &names))?;
    if let Some(range) = range {
        write!(script, "{};", rollup("live", ratio, range))?;
    }
    let output = read(path, &script)?;
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
    let index: BTreeMap<&str, usize> = identities.iter().map(String::as_str).zip(0..).collect();
    for row in output.lines() {
        if let Some((name, period, point)) = parse(row, step)
            && let Some(&line) = index.get(name)
            && let Some(slot) = period
                .checked_sub(first)
                .and_then(|offset| usize::try_from(offset).ok())
                .and_then(|offset| lines.get_mut(line)?.points.get_mut(offset))
        {
            *slot = point;
        }
    }
    Ok(lines)
}

pub fn query(config: &Settings, inventory: &Inventory, query: &Query) -> Result<Response> {
    let step = resolution(
        config.history_days,
        query.step,
        query.start,
        query.end,
        now() * 1_000,
    )?;
    let known = known_prefixes();
    let target = query.target.as_ref().unwrap_or_else(|| config.primary());
    let selected = |id: &str| query.all || id == query.tunnel;
    let mut subjects: Vec<Subject> = inventory
        .tunnels
        .iter()
        .filter_map(|tunnel| {
            let prefix = tunnel_prefix(&tunnel.key);
            (selected(&tunnel.id) && (inventory.has_path(&tunnel.id) || known.contains(&prefix)))
                .then(|| Subject {
                    id: tunnel.id.clone(),
                    name: tunnel.name.clone(),
                    identity: series_id(&prefix, target),
                })
        })
        .collect();
    ensure!(
        query.all || !subjects.is_empty(),
        "No history exists for this tunnel."
    );
    if query.endpoints && endpoints_enabled(config) {
        subjects.extend(
            inventory
                .tunnels
                .iter()
                .filter(|tunnel| selected(&tunnel.id))
                .map(|tunnel| Subject {
                    id: format!("endpoint:{}", tunnel.id),
                    name: format!("{} server", tunnel.name),
                    identity: endpoint_series(&tunnel.key),
                }),
        );
    }
    if query.uplinks && config.direct_wan {
        subjects.extend(inventory.standby_uplinks().map(|uplink| Subject {
            id: uplink.id.clone(),
            name: format!("{} uplink", uplink.name),
            identity: series_id(uplink.prefix(), target),
        }));
    }
    if query.include_wan
        && config.direct_wan
        && let Some(wan) = &inventory.wan
    {
        subjects.push(Subject {
            id: wan.id.clone(),
            name: wan.name.clone(),
            identity: series_id(wan.prefix(), target),
        });
    }
    let series = lines(&database(config), step, query, &ring(), subjects)?;
    let events = changes::read(config)
        .into_iter()
        .filter(|event| (query.start..=query.end).contains(&event.at))
        .collect();
    Ok(Response { series, events })
}

#[cfg(test)]
mod tests {
    use super::{
        DAY, HOUR, LEVELS, Query, Subject, collect_at, collection, convert, lines, resolution, sql,
    };
    use crate::runtime::now;
    use anyhow::{Result, ensure};
    use std::{fmt::Write, fs, net::Ipv4Addr, path::Path, process::Command};
    use wayline_protocol::{Bucket, Series, State};

    fn sqlite_available() -> bool {
        Command::new("sqlite3")
            .arg("-version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

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

    fn hour_row() -> String {
        let minutes: Vec<u32> = (0..60)
            .map(|minute| lower_median((4 * minute..4 * minute + 4).map(value).collect()))
            .collect();
        let fives: Vec<u32> = minutes
            .chunks(5)
            .map(|chunk| lower_median(chunk.to_vec()))
            .collect();
        let values: Vec<u32> = (0..240).map(value).collect();
        let lost = (0..240).filter(|index| index % 3 == 0).count();
        format!(
            "960|{lost}|{}|{}|{}",
            values.iter().min().copied().unwrap_or_default(),
            lower_median(fives),
            values.iter().max().copied().unwrap_or_default()
        )
    }

    fn hour_query(start: u64) -> String {
        format!(
            "SELECT sent,lost,min_us,median_us,max_us FROM s1h WHERE period={};",
            start / HOUR
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
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let start = now() / HOUR * HOUR - 2 * HOUR;
        collect_at(&database, 30, &[series(&identity(1), hour(start))])?;
        let row = sql(&database, true, &hour_query(start))?;
        ensure!(row.trim() == hour_row(), "Unexpected hour: {row}");
        let counts = sql(
            &database,
            true,
            "SELECT (SELECT count(*) FROM s15), (SELECT count(*) FROM s1m), (SELECT count(*) FROM s5m), (SELECT count(*) FROM s1h);",
        )?;
        ensure!(
            counts.trim() == "240|60|12|1",
            "Unexpected counts: {counts}"
        );
        Ok(())
    }

    #[test]
    fn repeated_collection_never_double_counts() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let start = now() / HOUR * HOUR - 2 * HOUR;
        let buckets = hour(start);
        collect_at(
            &database,
            30,
            &[series(&identity(1), buckets[..120].to_vec())],
        )?;
        collect_at(&database, 30, &[series(&identity(1), buckets.clone())])?;
        collect_at(&database, 30, &[series(&identity(1), buckets)])?;
        let row = sql(&database, true, &hour_query(start))?;
        ensure!(row.trim() == hour_row(), "Double count: {row}");
        Ok(())
    }

    #[test]
    fn tiers_prune_to_their_retention() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let now = now();
        let minute = now / 60 * 60 - 120;
        for (days, expected) in [(1_u32, "2|1|0|0"), (7, "2|2|1|0")] {
            let directory = tempfile::tempdir()?;
            let database = directory.path().join("latency.sqlite");
            collect_at(
                &database,
                days,
                &[series(&identity(1), vec![bucket(minute, 20_000, 0)])],
            )?;
            sql(
                &database,
                false,
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
            let counts = sql(
                &database,
                true,
                "SELECT (SELECT count(*) FROM s15), (SELECT count(*) FROM s1m), (SELECT count(*) FROM s5m), (SELECT count(*) FROM s1h);",
            )?;
            ensure!(counts.trim() == expected, "Days {days}: {counts}");
        }
        Ok(())
    }

    #[test]
    fn idle_series_expire_with_their_rows() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = now() / 60 * 60 - 120;
        collect_at(
            &database,
            1,
            &[
                series(&identity(1), vec![bucket(minute, 20_000, 0)]),
                series(&identity(2), vec![bucket(minute, 20_000, 0)]),
            ],
        )?;
        sql(
            &database,
            false,
            &format!(
                "UPDATE series SET seen={} WHERE name='{}';",
                now() / HOUR - 49,
                identity(2)
            ),
        )?;
        collect_at(
            &database,
            1,
            &[series(&identity(1), vec![bucket(minute + 15, 20_000, 0)])],
        )?;
        let state = sql(
            &database,
            true,
            "SELECT count(*) FROM series; SELECT count(DISTINCT series) FROM s15; SELECT count(DISTINCT series) FROM s1m;",
        )?;
        ensure!(
            state.lines().collect::<Vec<_>>() == ["1", "1", "1"],
            "Unexpected state: {state}"
        );
        Ok(())
    }

    #[test]
    fn retiring_many_series_leaves_no_orphans() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = now() / 60 * 60 - 120;
        let all: Vec<Series> = (1..=80)
            .map(|index| series(&identity(index), vec![bucket(minute, 20_000, 0)]))
            .collect();
        collect_at(&database, 30, &all)?;
        sql(
            &database,
            false,
            &format!("UPDATE series SET seen={} WHERE id>1;", now() / HOUR - 721),
        )?;
        collect_at(
            &database,
            30,
            &[series(&identity(1), vec![bucket(minute + 15, 20_000, 0)])],
        )?;
        let mut statement = String::from("SELECT count(*) FROM series;");
        for (_, table) in LEVELS {
            write!(
                statement,
                "SELECT count(*) FROM {table} WHERE series NOT IN (SELECT id FROM series);"
            )?;
        }
        let state = sql(&database, true, &statement)?;
        ensure!(
            state.lines().collect::<Vec<_>>() == ["1", "0", "0", "0", "0"],
            "Unexpected state: {state}"
        );
        Ok(())
    }

    #[test]
    fn series_cap_keeps_live_series_over_newer_ids() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = now() / 60 * 60 - 120;
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
        let state = sql(
            &database,
            true,
            &format!(
                "SELECT count(*) FROM series; SELECT count(*) FROM series WHERE name IN ('{}','{}');",
                identity(1),
                identity(91)
            ),
        )?;
        ensure!(
            state.lines().collect::<Vec<_>>() == ["90", "2"],
            "Unexpected state: {state}"
        );
        Ok(())
    }

    #[test]
    fn unknown_schema_is_reset() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        sql(
            &database,
            false,
            "CREATE TABLE samples(x INTEGER); PRAGMA user_version=2;",
        )?;
        ensure!(convert(&database)?, "Expected a conversion.");
        ensure!(
            directory.path().join("latency.sqlite.broken").exists(),
            "The old database was not moved aside."
        );
        let state = sql(
            &database,
            true,
            "PRAGMA user_version; SELECT count(*) FROM sqlite_master WHERE name='samples';",
        )?;
        ensure!(
            state.lines().collect::<Vec<_>>() == ["4", "0"],
            "Unexpected schema: {state}"
        );
        Ok(())
    }

    #[test]
    fn schema_v3_keeps_every_tier_when_converted() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let mut script = String::from(
            "CREATE TABLE series(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE); CREATE TABLE meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID; INSERT INTO series(name) VALUES('x'); INSERT INTO meta VALUES('cursor',5);",
        );
        for (index, (_, table)) in LEVELS.iter().enumerate() {
            write!(
                script,
                "CREATE TABLE {table}(series INTEGER NOT NULL, period INTEGER NOT NULL, min_us INTEGER, median_us INTEGER, max_us INTEGER, sent INTEGER NOT NULL, lost INTEGER NOT NULL, PRIMARY KEY(series, period)) WITHOUT ROWID; INSERT INTO {table} VALUES(1,{},1,2,3,4,{index});",
                100 + index
            )?;
        }
        script.push_str("PRAGMA user_version=3;");
        sql(&database, false, &script)?;
        ensure!(
            sql(&database, false, &collection(30, now(), &[])?).is_err(),
            "A stale schema must fail the write so that it is converted."
        );
        ensure!(convert(&database)?, "Expected a conversion.");
        let state = sql(
            &database,
            true,
            "PRAGMA user_version; SELECT period,series,lost FROM s15; SELECT period,series,lost FROM s1h; SELECT count(*) FROM sqlite_master WHERE name IN ('meta','old_series','old_s15'); SELECT count(*) FROM series WHERE seen>0;",
        )?;
        ensure!(
            state.lines().collect::<Vec<_>>() == ["4", "100|1|0", "103|1|3", "0", "1"],
            "Unexpected schema: {state}"
        );
        sql(&database, false, &collection(30, now() + 60 * DAY, &[])?)?;
        let rows = sql(&database, true, "SELECT count(*) FROM s1m;")?;
        ensure!(rows.trim() == "0", "Rows outlived their series: {rows}");
        Ok(())
    }

    #[test]
    fn live_buckets_merge_into_stored_periods() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let full = directory.path().join("full.sqlite");
        let half = directory.path().join("half.sqlite");
        let start = now() / HOUR * HOUR - 2 * HOUR;
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
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        fs::write(&database, [0x5a_u8; 8192])?;
        let fresh = now() / 15 * 15 - 60;
        collect_at(
            &database,
            7,
            &[series(&identity(1), vec![bucket(fresh, 20_000, 0)])],
        )?;
        ensure!(
            directory.path().join("latency.sqlite.broken").exists(),
            "Damaged database was not moved aside."
        );
        let rows = sql(&database, true, "SELECT count(*) FROM s15;")?;
        ensure!(rows.trim() == "1", "Unexpected rows: {rows}");
        let events = fs::read_to_string(directory.path().join("events.json"))?;
        ensure!(
            events.contains("The history database was damaged and was reset."),
            "Missing recovery event."
        );
        Ok(())
    }

    #[test]
    fn eleven_series_for_a_week_stay_small() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let start = now() / HOUR * HOUR - 168 * HOUR;
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
        let query = request(now() - DAY, now(), 60);
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
