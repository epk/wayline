use crate::{
    changes::{self, Event},
    router::{Inventory, endpoint_series, endpoints_enabled, series_id, tunnel_prefix},
    runtime::{RUNTIME, directory, milliseconds, now, snapshot_since},
    settings::{self, Settings, Target},
};
use anyhow::{Context, Error, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    fs,
    path::{Path, PathBuf},
};
use wayline_protocol::{BUCKET_SECONDS, Bucket, MAX_SERIES, Series};
use wayline_support::{atomic, digest};

const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;
const MINUTE_MS: u64 = 60_000;
const MAX_POINTS: u64 = 2_500;
const MEMORY_PAGES: u64 = 1_024;
const PAGES: u64 = 8_192;
const PAGE_BYTES: u64 = 4_096;
const MARKER: &[u8] = br#"{"version":1}"#;
const OUTPUT_LIMIT: usize = 262_144;
const HISTORY_LIMIT: usize = 2 * 1024 * 1024;
const HEADER: &[u8] = b"SQLite format 3\0";
const LEVELS: [(u64, &str); 4] = [
    (BUCKET_SECONDS, "s15"),
    (60, "s1m"),
    (300, "s5m"),
    (3_600, "s1h"),
];

#[derive(Default, Clone, Copy)]
struct Bin {
    min: Option<u32>,
    median: Option<u32>,
    max: Option<u32>,
    sent: u32,
    lost: u32,
}

type Rows = BTreeMap<String, BTreeMap<u64, Bin>>;

struct Level {
    step: u64,
    table: &'static str,
    rows: Rows,
}

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

#[derive(Serialize)]
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
        PathBuf::from(format!("{RUNTIME}/history/latency.sqlite"))
    } else {
        PathBuf::from("/etc/wayline/history/latency.sqlite")
    }
}

fn known_cache() -> PathBuf {
    PathBuf::from(format!("{RUNTIME}/stored-series.json"))
}

pub fn known_prefixes() -> BTreeSet<String> {
    fs::read(known_cache())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn refresh_known(path: &Path) -> Result<()> {
    let prefixes: BTreeSet<String> = sql(path, "SELECT substr(name,1,48) FROM series;", true)?
        .lines()
        .map(str::to_owned)
        .collect();
    atomic(&known_cache(), &serde_json::to_vec(&prefixes)?)
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

pub fn migrate(config: &Settings, inventory: &Inventory) -> Result<()> {
    let path = database(config);
    let directory_path = path.parent().context("Invalid history directory.")?;
    directory(directory_path)?;
    let marker = path.with_file_name("migrated.json");
    if fs::read(&marker).is_ok_and(|data| data == MARKER) {
        return Ok(());
    }
    let legacy_file = path.with_file_name("paths.json");
    if path.exists() {
        let legacy: BTreeMap<String, String> = fs::read(&legacy_file)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        let (ids, prefixes) = renames(inventory, &legacy);
        migrate_at(&path, &ids, &prefixes)?;
    }
    atomic(&marker, MARKER)?;
    let _ = fs::remove_file(legacy_file);
    Ok(())
}

fn renames(
    inventory: &Inventory,
    legacy: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut prefixes: BTreeMap<String, String> = legacy
        .iter()
        .map(|(key, old)| (old.clone(), tunnel_prefix(key)))
        .collect();
    for path in &inventory.paths {
        if let Some(old) = &path.legacy {
            prefixes.insert(old.clone(), path.prefix().to_owned());
        }
    }
    let mut ids = BTreeMap::new();
    if let Some(wan) = &inventory.wan
        && let Some(old_wan) = &wan.legacy
    {
        prefixes.insert(old_wan.clone(), wan.prefix().to_owned());
        for tunnel in &inventory.tunnels {
            let old = format!(
                "{old_wan}_{}",
                &digest(format!("endpoint|{}", tunnel.key).as_bytes())[..14]
            );
            ids.insert(old, endpoint_series(&tunnel.key));
        }
    }
    (ids, prefixes)
}

fn merge(predicate: &str, target: &str) -> Result<String> {
    let mut statement = String::new();
    write!(
        statement,
        "INSERT OR IGNORE INTO series(name) SELECT {target} FROM series o WHERE {predicate}; "
    )?;
    for (_, table) in LEVELS {
        write!(
            statement,
            "INSERT OR IGNORE INTO {table} SELECT t.id,s.period,s.min_us,s.median_us,s.max_us,s.sent,s.lost FROM {table} s JOIN series o ON o.id=s.series JOIN series t ON t.name={target} WHERE {predicate}; \
             DELETE FROM {table} WHERE series IN (SELECT id FROM series o WHERE {predicate}); "
        )?;
    }
    write!(
        statement,
        "DELETE FROM series WHERE id IN (SELECT id FROM series o WHERE {predicate});"
    )?;
    Ok(statement)
}

fn migrate_at(
    path: &Path,
    ids: &BTreeMap<String, String>,
    prefixes: &BTreeMap<String, String>,
) -> Result<()> {
    convert(path)?;
    let mut statement = String::from("BEGIN IMMEDIATE;");
    for (old, new) in ids {
        ensure!(
            settings::id(old) && settings::id(new),
            "Invalid measurement identity."
        );
        statement.push_str(&merge(&format!("o.name='{old}'"), &format!("'{new}'"))?);
    }
    for (old, new) in prefixes {
        ensure!(
            old.len() == 48 && new.len() == 48 && settings::id(old) && settings::id(new),
            "Invalid measurement identity."
        );
        statement.push_str(&merge(
            &format!("substr(o.name,1,48)='{old}'"),
            &format!("'{new}'||substr(o.name,49)"),
        )?);
    }
    statement.push_str("COMMIT;");
    sql(path, &statement, false)?;
    Ok(())
}

fn healthy(path: &Path) -> bool {
    fs::read(path).is_ok_and(|data| data.starts_with(HEADER))
        && sql(path, "PRAGMA quick_check;", true).is_ok_and(|output| output.trim() == "ok")
}

fn pages(path: &Path) -> u64 {
    sql(path, "PRAGMA page_count;", true)
        .ok()
        .and_then(|output| output.trim().parse().ok())
        .unwrap_or_default()
}

fn cap(path: &Path) -> u64 {
    if path.starts_with(RUNTIME) {
        MEMORY_PAGES
    } else {
        PAGES
    }
}

fn pragmas(path: &Path) -> String {
    format!(
        "PRAGMA page_size=4096; PRAGMA max_page_count={}; PRAGMA journal_mode=TRUNCATE; PRAGMA synchronous=FULL;",
        cap(path)
    )
}

fn schema() -> String {
    let tables = LEVELS
        .iter()
        .map(|(_, table)| {
            format!(
                "CREATE TABLE IF NOT EXISTS {table}(series INTEGER NOT NULL, period INTEGER NOT NULL, min_us INTEGER, median_us INTEGER, max_us INTEGER, sent INTEGER NOT NULL, lost INTEGER NOT NULL, PRIMARY KEY(series, period)) WITHOUT ROWID;"
            )
        })
        .collect::<Vec<_>>()
        .concat();
    format!(
        "CREATE TABLE IF NOT EXISTS series(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE); CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID; {tables}"
    )
}

fn convert(path: &Path) -> Result<()> {
    let state = sql(
        path,
        "PRAGMA user_version; SELECT count(*) FROM sqlite_master WHERE name='bins';",
        false,
    )?;
    let mut lines = state.lines();
    let version: u32 = lines.next().context("Missing schema version.")?.parse()?;
    let legacy = lines.next() == Some("1");
    if version >= 3 {
        return Ok(());
    }
    let mut script = format!("{} BEGIN IMMEDIATE; {}", pragmas(path), schema());
    if legacy {
        ensure!(healthy(path), "History database is damaged.");
        script.push_str(
            "INSERT OR IGNORE INTO series(name) SELECT DISTINCT series FROM bins; \
             INSERT OR IGNORE INTO s1m SELECT s.id,b.minute,b.min_us,b.median_us,b.max_us,b.sent,b.lost FROM bins b JOIN series s ON s.name=b.series; \
             DROP TABLE bins;",
        );
    }
    if version == 2 {
        script.push_str(
            "INSERT OR IGNORE INTO s1m SELECT series,minute,min_us,median_us,max_us,sent,lost FROM samples; DROP TABLE samples;",
        );
    }
    script.push_str("PRAGMA user_version=3; COMMIT;");
    sql(path, &script, false)?;
    if legacy || version == 2 {
        sql(path, "VACUUM;", false)?;
    }
    Ok(())
}

fn sql(path: &Path, statement: &str, readonly: bool) -> Result<String> {
    sql_with(path, statement, readonly, OUTPUT_LIMIT)
}

fn sql_with(path: &Path, statement: &str, readonly: bool, limit: usize) -> Result<String> {
    let mut arguments = Vec::new();
    if readonly {
        arguments.push("-readonly");
    }
    arguments.extend([
        "-batch",
        "-bail",
        "-separator",
        "|",
        path.to_str().context("Invalid history path.")?,
    ]);
    let input = format!(
        ".output /dev/null\nPRAGMA busy_timeout=1000; PRAGMA cache_size=-128; PRAGMA mmap_size=0;\n.output stdout\n{statement}"
    );
    Ok(String::from_utf8(wayline_support::input(
        "sqlite3",
        &arguments,
        input.as_bytes(),
        limit,
        5,
    )?)?)
}

fn number(value: Option<u32>) -> String {
    value.map_or_else(|| "NULL".into(), |value| value.to_string())
}

fn bin(bucket: &Bucket) -> Bin {
    Bin {
        min: bucket.min_us,
        median: bucket.median_us,
        max: bucket.max_us,
        sent: u32::from(bucket.sent),
        lost: u32::from(bucket.lost),
    }
}

fn combine(bins: &[Bin]) -> Bin {
    let mut medians: Vec<u32> = bins.iter().filter_map(|bin| bin.median).collect();
    medians.sort_unstable();
    Bin {
        min: bins.iter().filter_map(|bin| bin.min).min(),
        median: medians.get(medians.len().saturating_sub(1) / 2).copied(),
        max: bins.iter().filter_map(|bin| bin.max).max(),
        sent: bins.iter().map(|bin| bin.sent).sum(),
        lost: bins.iter().map(|bin| bin.lost).sum(),
    }
}

fn parents(children: &BTreeMap<u64, Bin>, ratio: u64) -> BTreeMap<u64, Bin> {
    let mut groups: BTreeMap<u64, Vec<Bin>> = BTreeMap::new();
    for (period, bin) in children {
        groups.entry(period / ratio).or_default().push(*bin);
    }
    groups
        .into_iter()
        .map(|(period, bins)| (period, combine(&bins)))
        .collect()
}

fn cursor(path: &Path) -> u64 {
    sql(path, "SELECT value FROM meta WHERE key='cursor';", true)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_default()
}

fn stored_at(
    path: &Path,
    table: &str,
    identities: &[String],
    start: u64,
    last: u64,
) -> Result<Rows> {
    let mut result: Rows = BTreeMap::new();
    if !path.exists() || identities.is_empty() {
        return Ok(result);
    }
    ensure!(
        identities.iter().all(|identity| settings::id(identity)),
        "Invalid measurement identity."
    );
    let quoted = identities
        .iter()
        .map(|identity| format!("'{identity}'"))
        .collect::<Vec<_>>()
        .join(",");
    let statement = format!(
        "SELECT s.name,t.period,t.min_us,t.median_us,t.max_us,t.sent,t.lost FROM {table} t JOIN series s ON s.id=t.series WHERE s.name IN ({quoted}) AND t.period BETWEEN {start} AND {last};"
    );
    let output = match sql_with(path, &statement, true, HISTORY_LIMIT) {
        Ok(output) => output,
        Err(error) => {
            return if sql(path, "PRAGMA user_version;", true)?.trim() == "3" {
                Err(error)
            } else {
                Ok(result)
            };
        }
    };
    for line in output.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() == 7 {
            result.entry(fields[0].to_owned()).or_default().insert(
                fields[1].parse()?,
                Bin {
                    min: fields[2].parse().ok(),
                    median: fields[3].parse().ok(),
                    max: fields[4].parse().ok(),
                    sent: fields[5].parse()?,
                    lost: fields[6].parse()?,
                },
            );
        }
    }
    Ok(result)
}

fn fresh(snapshot: &[Series], since: u64) -> Rows {
    snapshot
        .iter()
        .take(MAX_SERIES)
        .map(|series| {
            (
                series.id.clone(),
                series
                    .buckets
                    .iter()
                    .filter(|bucket| bucket.at > since)
                    .map(|bucket| (bucket.at / BUCKET_SECONDS, bin(bucket)))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

fn levels(path: &Path, days: u32, fresh: Rows) -> Result<Vec<Level>> {
    let mut levels = vec![Level {
        step: LEVELS[0].0,
        table: LEVELS[0].1,
        rows: fresh,
    }];
    for (step, table) in LEVELS.into_iter().skip(1) {
        if retained(days, step) == 0 {
            continue;
        }
        let Some(child) = levels.last() else {
            break;
        };
        let periods: Vec<u64> = child
            .rows
            .values()
            .flat_map(|rows| rows.keys().copied())
            .collect();
        let (Some(&first), Some(&last)) = (periods.iter().min(), periods.iter().max()) else {
            break;
        };
        let ratio = step / child.step;
        let names: Vec<String> = child.rows.keys().cloned().collect();
        let mut children = stored_at(
            path,
            child.table,
            &names,
            first / ratio * ratio,
            (last / ratio + 1) * ratio - 1,
        )?;
        for (name, rows) in &child.rows {
            children
                .entry(name.clone())
                .or_default()
                .extend(rows.clone());
        }
        let rows = children
            .iter()
            .map(|(name, rows)| (name.clone(), parents(rows, ratio)))
            .collect();
        levels.push(Level { step, table, rows });
    }
    Ok(levels)
}

fn write(path: &Path, days: u32, levels: &[Level], newest: u64) -> Result<()> {
    let mut statement = format!("{} BEGIN IMMEDIATE;", pragmas(path));
    for (index, level) in levels.iter().enumerate() {
        let verb = if index == 0 {
            "INSERT OR IGNORE"
        } else {
            "INSERT OR REPLACE"
        };
        for (name, rows) in &level.rows {
            ensure!(settings::id(name), "Invalid measurement identity.");
            write!(
                statement,
                "INSERT OR IGNORE INTO series(name) VALUES('{name}');"
            )?;
            let values = rows
                .iter()
                .map(|(period, bin)| {
                    format!(
                        "({period},{},{},{},{},{})",
                        number(bin.min),
                        number(bin.median),
                        number(bin.max),
                        bin.sent,
                        bin.lost
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            write!(
                statement,
                "{verb} INTO {} SELECT s.id,v.column1,v.column2,v.column3,v.column4,v.column5,v.column6 FROM (SELECT id FROM series WHERE name='{name}') s, (VALUES {values}) v;",
                level.table
            )?;
        }
    }
    for (step, table) in LEVELS {
        let kept = retained(days, step);
        let cutoff = now().saturating_sub(kept) / step + u64::from(kept == 0);
        write!(
            statement,
            "DELETE FROM {table} WHERE period < {cutoff} OR series NOT IN (SELECT series FROM {table} GROUP BY series ORDER BY max(period) DESC LIMIT {MAX_SERIES});"
        )?;
    }
    let members = LEVELS
        .iter()
        .map(|(_, table)| format!("SELECT series FROM {table}"))
        .collect::<Vec<_>>()
        .join(" UNION ");
    write!(
        statement,
        "DELETE FROM series WHERE id NOT IN ({members}); INSERT OR REPLACE INTO meta VALUES('cursor',{newest}); COMMIT;"
    )?;
    sql(path, &statement, false)?;
    Ok(())
}

fn store(path: &Path, days: u32, snapshot: &[Series], since: u64) -> Result<bool> {
    if now() < 1_704_067_200 {
        return Ok(false);
    }
    convert(path)?;
    let newest = snapshot
        .iter()
        .flat_map(|series| &series.buckets)
        .map(|bucket| bucket.at)
        .filter(|at| *at > since)
        .max();
    let Some(newest) = newest else {
        return Ok(false);
    };
    let levels = levels(path, days, fresh(snapshot, since))?;
    write(path, days, &levels, newest)?;
    Ok(true)
}

fn repair(path: &Path, days: u32, snapshot: &[Series], since: u64, error: Error) -> Result<bool> {
    if path.exists() && !healthy(path) {
        fs::rename(path, path.with_file_name("latency.sqlite.broken"))?;
        let _ = fs::remove_file(path.with_file_name("latency.sqlite-journal"));
        changes::append(
            path,
            vec![Event {
                at: now() * 1000,
                subject: "history".into(),
                kind: "history".into(),
                text: "The history database was damaged and was reset.".into(),
            }],
        )?;
        return store(path, days, snapshot, 0);
    }
    if pages(path) >= cap(path) {
        let mut statement = format!("{} BEGIN IMMEDIATE;", pragmas(path));
        for (_, table) in LEVELS.iter().rev() {
            write!(
                statement,
                "DELETE FROM {table} WHERE period <= (SELECT period FROM {table} ORDER BY period LIMIT 1 OFFSET (SELECT count(*) / 10 FROM {table}));"
            )?;
        }
        statement.push_str("COMMIT;");
        sql(path, &statement, false)?;
        return store(path, days, snapshot, since);
    }
    Err(error)
}

fn collect_since(path: &Path, days: u32, snapshot: &[Series], since: u64) -> Result<bool> {
    store(path, days, snapshot, since).or_else(|error| repair(path, days, snapshot, since, error))
}

#[cfg(test)]
fn collect_at(path: &Path, days: u32, snapshot: &[Series]) -> Result<()> {
    collect_since(path, days, snapshot, cursor(path)).map(drop)
}

pub fn collect(config: &Settings) -> Result<()> {
    let path = database(config);
    directory(path.parent().context("Invalid history directory.")?)?;
    let since = cursor(&path);
    let written = collect_since(&path, config.history_days, &snapshot_since(since), since)?;
    if written || !known_cache().exists() {
        refresh_known(&path)?;
    }
    Ok(())
}

pub fn report(config: &Settings) -> Result<Report> {
    let path = database(config);
    let bytes = fs::metadata(&path).map_or(0, |meta| meta.len());
    let mut lines = Vec::new();
    if path.exists() {
        let query = LEVELS
            .iter()
            .map(|(_, table)| {
                format!("SELECT '{table}',count(*),min(period),max(period) FROM {table};")
            })
            .collect::<Vec<_>>()
            .concat();
        for line in sql(&path, &query, true)?.lines() {
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

fn point(period: u64, step: u64, bin: Option<&Bin>) -> Point {
    let bin = bin.copied().unwrap_or_default();
    Point {
        at: period * step * 1_000,
        median: bin.median.map(milliseconds),
        min: bin.min.map(milliseconds),
        max: bin.max.map(milliseconds),
        sent: bin.sent,
        lost: bin.lost,
    }
}

fn lines(
    path: &Path,
    step: u64,
    query: &Query,
    snapshot: &[Series],
    subjects: Vec<Subject>,
) -> Result<Vec<Line>> {
    let identities: Vec<String> = subjects
        .iter()
        .map(|subject| subject.identity.clone())
        .collect();
    let (first, last) = (query.start / 1_000 / step, query.end / 1_000 / step);
    let mut stored = stored_at(path, table(step), &identities, first, last)?;
    let live: BTreeMap<&str, BTreeMap<u64, Bin>> = snapshot
        .iter()
        .filter(|series| identities.contains(&series.id))
        .map(|series| {
            let bins = series
                .buckets
                .iter()
                .map(|bucket| (bucket.at / BUCKET_SECONDS, bin(bucket)))
                .collect();
            (series.id.as_str(), bins)
        })
        .filter(|(_, bins): &(_, BTreeMap<u64, Bin>)| !bins.is_empty())
        .collect();
    let ratio = step / BUCKET_SECONDS;
    let span = live
        .values()
        .flat_map(|bins| [bins.keys().next(), bins.keys().next_back()])
        .flatten()
        .copied()
        .fold(None, |span: Option<(u64, u64)>, at| {
            Some(span.map_or((at, at), |(low, high)| (low.min(at), high.max(at))))
        });
    let mut children = match span {
        Some((low, high)) => stored_at(
            path,
            LEVELS[0].1,
            &identities,
            low / ratio * ratio,
            (high / ratio + 1) * ratio - 1,
        )?,
        None => Rows::new(),
    };
    Ok(subjects
        .into_iter()
        .map(|subject| {
            let mut bins = stored.remove(&subject.identity).unwrap_or_default();
            if let Some(fresh) = live.get(subject.identity.as_str()) {
                let mut own = children.remove(&subject.identity).unwrap_or_default();
                own.extend(fresh.iter().map(|(period, bin)| (*period, *bin)));
                bins.extend(parents(&own, ratio));
            }
            Line {
                id: subject.id,
                name: subject.name,
                points: (first..=last)
                    .map(|period| point(period, step, bins.get(&period)))
                    .collect(),
            }
        })
        .collect())
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
    let mut subjects: Vec<Subject> = Vec::new();
    for tunnel in &inventory.tunnels {
        let prefix = tunnel_prefix(&tunnel.key);
        if (query.all || tunnel.id == query.tunnel)
            && (inventory.has_path(&tunnel.id) || known.contains(&prefix))
        {
            subjects.push(Subject {
                id: tunnel.id.clone(),
                name: tunnel.name.clone(),
                identity: series_id(&prefix, target),
            });
        }
    }
    ensure!(
        query.all || !subjects.is_empty(),
        "No history exists for this tunnel."
    );
    if query.endpoints && endpoints_enabled(config) {
        let servers: Vec<Subject> = inventory
            .tunnels
            .iter()
            .filter(|tunnel| query.all || tunnel.id == query.tunnel)
            .map(|tunnel| Subject {
                id: format!("endpoint:{}", tunnel.id),
                name: format!("{} server", tunnel.name),
                identity: endpoint_series(&tunnel.key),
            })
            .collect();
        subjects.extend(servers);
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
    let path = database(config);
    let snapshot = snapshot_since(cursor(&path));
    let series = lines(&path, step, query, &snapshot, subjects)?;
    let events = changes::read(config)
        .into_iter()
        .filter(|event| (query.start..=query.end).contains(&event.at))
        .collect();
    Ok(Response { series, events })
}

#[cfg(test)]
mod tests {
    use super::{
        DAY, HOUR, LEVELS, Query, Subject, collect_at, convert, lines, migrate_at, resolution, sql,
        stored_at,
    };
    use crate::runtime::now;
    use anyhow::{Result, ensure};
    use std::{collections::BTreeMap, fs, net::Ipv4Addr, process::Command};
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

    #[test]
    fn buckets_roll_up_exactly() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let start = now() / HOUR * HOUR - 2 * HOUR;
        collect_at(&database, 30, &[series(&identity(1), hour(start))])?;
        let row = sql(&database, &hour_query(start), true)?;
        ensure!(row.trim() == hour_row(), "Unexpected hour: {row}");
        let counts = sql(
            &database,
            "SELECT (SELECT count(*) FROM s15), (SELECT count(*) FROM s1m), (SELECT count(*) FROM s5m), (SELECT count(*) FROM s1h);",
            true,
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
        let row = sql(&database, &hour_query(start), true)?;
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
                &format!(
                    "INSERT INTO s15 SELECT id,{},1,1,1,1,0 FROM series; INSERT INTO s1m SELECT id,{},1,1,1,1,0 FROM series; INSERT INTO s5m SELECT id,{},1,1,1,1,0 FROM series;",
                    (now - 7 * HOUR) / 15,
                    (now - 2 * DAY) / 60,
                    (now - 8 * DAY) / 300
                ),
                false,
            )?;
            collect_at(
                &database,
                days,
                &[series(&identity(1), vec![bucket(minute + 15, 20_000, 0)])],
            )?;
            let counts = sql(
                &database,
                "SELECT (SELECT count(*) FROM s15), (SELECT count(*) FROM s1m), (SELECT count(*) FROM s5m), (SELECT count(*) FROM s1h);",
                true,
            )?;
            ensure!(counts.trim() == expected, "Days {days}: {counts}");
        }
        Ok(())
    }

    #[test]
    fn schema_v2_moves_samples_into_one_minute() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        sql(
            &database,
            "CREATE TABLE series(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE); CREATE TABLE samples(series INTEGER NOT NULL, minute INTEGER NOT NULL, min_us INTEGER, median_us INTEGER, max_us INTEGER, sent INTEGER NOT NULL, lost INTEGER NOT NULL, PRIMARY KEY(series, minute)) WITHOUT ROWID; INSERT INTO series(name) VALUES('x'),('y'); INSERT INTO samples VALUES(1,100,1,2,3,4,0),(1,101,1,2,3,4,0),(2,100,NULL,NULL,NULL,0,1); PRAGMA user_version=2;",
            false,
        )?;
        convert(&database)?;
        let state = sql(
            &database,
            "PRAGMA user_version; SELECT count(*), sum(sent), sum(lost) FROM s1m; SELECT count(*) FROM sqlite_master WHERE name='samples';",
            true,
        )?;
        let lines: Vec<&str> = state.lines().collect();
        ensure!(lines == ["3", "3|8|1", "0"], "Unexpected schema: {state}");
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
        let rows = sql(&database, "SELECT count(*) FROM s15;", true)?;
        ensure!(rows.trim() == "1", "Unexpected rows: {rows}");
        let events = fs::read_to_string(directory.path().join("events.json"))?;
        ensure!(
            events.contains("The history database was damaged and was reset."),
            "Missing recovery event."
        );
        Ok(())
    }

    #[test]
    fn migration_moves_rows_to_stable_identities() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = now() / 60 - 3;
        let old = "a".repeat(48);
        let new = "b".repeat(48);
        let old_endpoint = format!("{old}_{}", "c".repeat(14));
        let new_endpoint = format!("{new}_server");
        sql(
            &database,
            &format!(
                "CREATE TABLE bins(minute INTEGER,series TEXT,min_us INTEGER,median_us INTEGER,max_us INTEGER,sent INTEGER,lost INTEGER,PRIMARY KEY(minute,series)) WITHOUT ROWID; \
                 INSERT INTO bins VALUES({minute},'{old}_1_1_1_1',1,20000,3,6,0); \
                 INSERT INTO bins VALUES({minute},'{old}_9_9_9_9',1,20000,3,6,0); \
                 INSERT INTO bins VALUES({minute},'{new}_9_9_9_9',1,30000,3,9,0); \
                 INSERT INTO bins VALUES({minute},'{old_endpoint}',1,40000,3,6,0);"
            ),
            false,
        )?;
        let ids = BTreeMap::from([(old_endpoint, new_endpoint.clone())]);
        let prefixes = BTreeMap::from([(old.clone(), new.clone())]);
        migrate_at(&database, &ids, &prefixes)?;
        let identities = [
            format!("{new}_1_1_1_1"),
            format!("{new}_9_9_9_9"),
            new_endpoint,
        ];
        let stored = stored_at(&database, "s1m", &identities, minute, minute)?;
        ensure!(stored.len() == 3, "Expected three migrated series.");
        ensure!(
            stored[&identities[1]][&minute].sent == 9,
            "Existing stable rows must win over migrated rows."
        );
        let leftover = sql(
            &database,
            &format!(
                "SELECT count(*) FROM series WHERE substr(name,1,48)='{old}' OR name LIKE '{old}%';"
            ),
            true,
        )?;
        ensure!(
            leftover.lines().last() == Some("0"),
            "Old identities remain: {leftover}"
        );
        Ok(())
    }

    #[test]
    fn schema_v1_converts_to_v3() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let minute = now() / 60 - 3;
        sql(
            &database,
            &format!(
                "CREATE TABLE bins(minute INTEGER,series TEXT,min_us INTEGER,median_us INTEGER,max_us INTEGER,sent INTEGER,lost INTEGER,PRIMARY KEY(minute,series)) WITHOUT ROWID; \
                 INSERT INTO bins VALUES({minute},'{}',1,2,3,4,0); \
                 INSERT INTO bins VALUES({minute},'{}',1,2,3,4,0); \
                 INSERT INTO bins VALUES({},'{}',1,2,3,4,0);",
                identity(1),
                identity(2),
                minute + 1,
                identity(1)
            ),
            false,
        )?;
        convert(&database)?;
        let state = sql(
            &database,
            "PRAGMA user_version; SELECT count(*) FROM series; SELECT count(*) FROM s1m; SELECT count(*) FROM sqlite_master WHERE name='bins';",
            true,
        )?;
        let lines: Vec<&str> = state.lines().collect();
        ensure!(lines == ["3", "2", "3", "0"], "Unexpected schema: {state}");
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
        let query = Query {
            start: (now() - DAY) * 1_000,
            end: now() * 1_000,
            step: 60,
            target: None,
            tunnel: String::new(),
            include_wan: false,
            all: false,
            endpoints: false,
            uplinks: false,
        };
        let subject = Subject {
            id: "tunnel".into(),
            name: "Tunnel".into(),
            identity: identity(1),
        };
        let line = lines(&database, 60, &query, &[], vec![subject])?.remove(0);
        ensure!(line.points.len() <= 1_441, "Too many points.");
        let json = serde_json::to_vec(&line)?.len();
        ensure!(json < 262_144, "Response is {json} bytes.");
        ensure!(LEVELS.len() == 4, "Unexpected tiers.");
        Ok(())
    }
}
