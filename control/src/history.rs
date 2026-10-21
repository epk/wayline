use crate::{
    changes::{self, Event},
    router::{Inventory, endpoint_series, endpoints_enabled, series_id, tunnel_prefix},
    runtime::{RUNTIME, directory, milliseconds, now},
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
use wayline_protocol::{MAX_SERIES, Sample, Series};
use wayline_support::{atomic, digest};

const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;
const MARKER: &[u8] = br#"{"version":1}"#;
const OUTPUT_LIMIT: usize = 262_144;
const HISTORY_LIMIT: usize = 2 * 1024 * 1024;
const PAGE_LIMIT: u64 = 2048;
const HEADER: &[u8] = b"SQLite format 3\0";
const PRAGMAS: &str = "PRAGMA page_size=4096; PRAGMA max_page_count=2048; PRAGMA journal_mode=TRUNCATE; PRAGMA synchronous=FULL;";
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS series(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE); CREATE TABLE IF NOT EXISTS samples(series INTEGER NOT NULL, minute INTEGER NOT NULL, min_us INTEGER, median_us INTEGER, max_us INTEGER, sent INTEGER NOT NULL, lost INTEGER NOT NULL, PRIMARY KEY(series, minute)) WITHOUT ROWID;";

#[derive(Default, Clone)]
struct Bin {
    min: Option<u32>,
    median: Option<u32>,
    max: Option<u32>,
    sent: u32,
    lost: u32,
}

#[derive(Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field is an independent request option."
)]
pub struct Query {
    start: u64,
    end: u64,
    step: usize,
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

pub fn database(config: &Settings) -> PathBuf {
    if config.history_hours == 24 {
        PathBuf::from("/etc/wayline/history/latency.sqlite")
    } else {
        PathBuf::from(format!("{RUNTIME}/history/latency.sqlite"))
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

fn merge(predicate: &str, target: &str) -> String {
    format!(
        "INSERT OR IGNORE INTO series(name) SELECT {target} FROM series o WHERE {predicate}; \
         INSERT OR IGNORE INTO samples SELECT t.id,s.minute,s.min_us,s.median_us,s.max_us,s.sent,s.lost FROM samples s JOIN series o ON o.id=s.series JOIN series t ON t.name={target} WHERE {predicate}; \
         DELETE FROM samples WHERE series IN (SELECT id FROM series o WHERE {predicate}); \
         DELETE FROM series WHERE id IN (SELECT id FROM series o WHERE {predicate});"
    )
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
        statement.push_str(&merge(&format!("o.name='{old}'"), &format!("'{new}'")));
    }
    for (old, new) in prefixes {
        ensure!(
            old.len() == 48 && new.len() == 48 && settings::id(old) && settings::id(new),
            "Invalid measurement identity."
        );
        statement.push_str(&merge(
            &format!("substr(o.name,1,48)='{old}'"),
            &format!("'{new}'||substr(o.name,49)"),
        ));
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

fn convert(path: &Path) -> Result<()> {
    let state = sql(
        path,
        "PRAGMA user_version; SELECT count(*) FROM sqlite_master WHERE name='bins';",
        false,
    )?;
    let mut lines = state.lines();
    let version: u32 = lines.next().context("Missing schema version.")?.parse()?;
    let legacy = lines.next() == Some("1");
    if version >= 2 {
        return Ok(());
    }
    let mut script = format!("{PRAGMAS} BEGIN IMMEDIATE; {SCHEMA}");
    if legacy {
        ensure!(healthy(path), "History database is damaged.");
        script.push_str(
            "INSERT OR IGNORE INTO series(name) SELECT DISTINCT series FROM bins; \
             INSERT INTO samples SELECT s.id,b.minute,b.min_us,b.median_us,b.max_us,b.sent,b.lost FROM bins b JOIN series s ON s.name=b.series; \
             DROP TABLE bins;",
        );
    }
    script.push_str("PRAGMA user_version=2; COMMIT;");
    sql(path, &script, false)?;
    if legacy {
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

fn bins(samples: &[Sample]) -> BTreeMap<u64, Bin> {
    let now = now() * 1000;
    let mut grouped = BTreeMap::<u64, (Vec<u32>, u32)>::new();
    for sample in samples
        .iter()
        .filter(|sample| sample.at_ms >= now.saturating_sub(DAY_MS) && sample.at_ms <= now)
    {
        let (values, sent) = grouped.entry(sample.at_ms / MINUTE_MS).or_default();
        *sent += 1;
        if let Some(us) = sample.us.filter(|us| *us <= 5_000_000) {
            values.push(us);
        }
    }
    grouped
        .into_iter()
        .map(|(minute, (mut values, sent))| {
            values.sort_unstable();
            let bin = Bin {
                min: values.first().copied(),
                median: values.get(values.len().saturating_sub(1) / 2).copied(),
                max: values.last().copied(),
                sent,
                lost: sent - u32::try_from(values.len()).unwrap_or(sent),
            };
            (minute, bin)
        })
        .collect()
}

fn number(value: Option<u32>) -> String {
    value.map_or_else(|| "NULL".into(), |value| value.to_string())
}

pub fn collect(config: &Settings, snapshot: &[Series]) -> Result<()> {
    let path = database(config);
    directory(path.parent().context("Invalid history directory.")?)?;
    collect_at(&path, snapshot)?;
    refresh_known(&path)
}

fn collect_at(path: &Path, snapshot: &[Series]) -> Result<()> {
    store(path, snapshot).or_else(|error| repair(path, snapshot, error))
}

fn store(path: &Path, snapshot: &[Series]) -> Result<()> {
    if now() < 1_704_067_200 {
        return Ok(());
    }
    convert(path)?;
    let current = now() / 60;
    let mut statement = format!(
        "{PRAGMAS} BEGIN IMMEDIATE; DELETE FROM samples WHERE minute < {} OR minute > {current};",
        current.saturating_sub(1439)
    );
    for series in snapshot.iter().take(MAX_SERIES) {
        ensure!(settings::id(&series.id), "Invalid measurement identity.");
        let id = &series.id;
        write!(
            statement,
            "INSERT OR IGNORE INTO series(name) VALUES('{id}');"
        )?;
        for (minute, bin) in bins(&series.history)
            .into_iter()
            .filter(|(minute, _)| *minute < current)
        {
            write!(
                statement,
                "INSERT INTO samples SELECT id,{minute},{},{},{},{},{} FROM series WHERE name='{id}' ON CONFLICT(series,minute) DO UPDATE SET min_us=excluded.min_us,median_us=excluded.median_us,max_us=excluded.max_us,sent=excluded.sent,lost=excluded.lost WHERE excluded.sent>samples.sent;",
                number(bin.min),
                number(bin.median),
                number(bin.max),
                bin.sent,
                bin.lost
            )?;
        }
    }
    write!(
        statement,
        "DELETE FROM samples WHERE series NOT IN (SELECT series FROM samples GROUP BY series ORDER BY max(minute) DESC LIMIT {MAX_SERIES}); DELETE FROM series WHERE id NOT IN (SELECT series FROM samples); COMMIT;"
    )?;
    sql(path, &statement, false)?;
    Ok(())
}

fn repair(path: &Path, snapshot: &[Series], error: Error) -> Result<()> {
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
        return store(path, snapshot);
    }
    if pages(path) >= PAGE_LIMIT {
        sql(
            path,
            "BEGIN IMMEDIATE; DELETE FROM samples WHERE minute < (SELECT min(minute) FROM samples)+360; COMMIT;",
            false,
        )?;
        return store(path, snapshot);
    }
    Err(error)
}

fn stored(
    config: &Settings,
    identities: &[String],
    start: u64,
    last: u64,
) -> Result<BTreeMap<String, BTreeMap<u64, Bin>>> {
    stored_at(&database(config), identities, start, last)
}

fn stored_at(
    path: &Path,
    identities: &[String],
    start: u64,
    last: u64,
) -> Result<BTreeMap<String, BTreeMap<u64, Bin>>> {
    let mut result: BTreeMap<String, BTreeMap<u64, Bin>> = BTreeMap::new();
    if !path.exists() || identities.is_empty() {
        return Ok(result);
    }
    ensure!(
        identities.iter().all(|identity| settings::id(identity)),
        "Invalid measurement identity."
    );
    if sql(path, "PRAGMA user_version;", true)?.trim() != "2" {
        return Ok(result);
    }
    let quoted = identities
        .iter()
        .map(|identity| format!("'{identity}'"))
        .collect::<Vec<_>>()
        .join(",");
    let names: BTreeMap<String, String> = sql(
        path,
        &format!("SELECT id,name FROM series WHERE name IN ({quoted});"),
        true,
    )?
    .lines()
    .filter_map(|line| line.split_once('|'))
    .map(|(id, name)| (id.to_owned(), name.to_owned()))
    .collect();
    if names.is_empty() {
        return Ok(result);
    }
    let ids = names
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let statement = format!(
        "SELECT series,minute,min_us,median_us,max_us,sent,lost FROM samples WHERE series IN ({ids}) AND minute BETWEEN {start} AND {last} ORDER BY series,minute LIMIT {};",
        1441 * names.len()
    );
    for line in sql_with(path, &statement, true, HISTORY_LIMIT)?.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() == 7
            && let Some(name) = names.get(fields[0])
        {
            result.entry(name.clone()).or_default().insert(
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

struct Subject {
    id: String,
    name: String,
    identity: String,
}

fn line(
    mut history: BTreeMap<u64, Bin>,
    snapshot: &[Series],
    subject: Subject,
    query: &Query,
) -> Result<Line> {
    let (start, last) = (query.start / MINUTE_MS, query.end / MINUTE_MS);
    let identity = subject.identity;
    if let Some(live) = snapshot.iter().find(|series| series.id == identity) {
        for (minute, bin) in bins(&live.history) {
            if history.get(&minute).is_none_or(|old| old.sent <= bin.sent) {
                history.insert(minute, bin);
            }
        }
    }
    let width = u64::try_from(query.step)?;
    let points = (start..=last)
        .step_by(query.step)
        .map(|minute| {
            let group: Vec<_> = (minute..minute + width)
                .filter_map(|at| history.get(&at))
                .collect();
            let mut medians: Vec<u32> = group.iter().filter_map(|bin| bin.median).collect();
            medians.sort_unstable();
            Point {
                at: minute * MINUTE_MS,
                median: medians
                    .get(medians.len().saturating_sub(1) / 2)
                    .copied()
                    .map(milliseconds),
                min: group
                    .iter()
                    .filter_map(|bin| bin.min)
                    .min()
                    .map(milliseconds),
                max: group
                    .iter()
                    .filter_map(|bin| bin.max)
                    .max()
                    .map(milliseconds),
                sent: group.iter().map(|bin| bin.sent).sum(),
                lost: group.iter().map(|bin| bin.lost).sum(),
            }
        })
        .collect();
    Ok(Line {
        id: subject.id,
        name: subject.name,
        points,
    })
}

pub fn query(
    config: &Settings,
    snapshot: &[Series],
    inventory: &Inventory,
    query: &Query,
) -> Result<Response> {
    let now = now() * 1000;
    ensure!(
        query.start < query.end
            && query.end - query.start <= DAY_MS
            && query.start >= now.saturating_sub(DAY_MS + MINUTE_MS)
            && query.end <= now + MINUTE_MS
            && [1, 5].contains(&query.step),
        "History range exceeds its limit."
    );
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
    let identities: Vec<String> = subjects
        .iter()
        .map(|subject| subject.identity.clone())
        .collect();
    let mut stored = stored(
        config,
        &identities,
        query.start / MINUTE_MS,
        query.end / MINUTE_MS,
    )?;
    let series = subjects
        .into_iter()
        .map(|subject| {
            let history = stored.remove(&subject.identity).unwrap_or_default();
            line(history, snapshot, subject, query)
        })
        .collect::<Result<_>>()?;
    let events = changes::read(config)
        .into_iter()
        .filter(|event| (query.start..=query.end).contains(&event.at))
        .collect();
    Ok(Response { series, events })
}

#[cfg(test)]
mod tests {
    use super::{bins, collect_at, convert, migrate_at, sql, stored_at};
    use crate::runtime::now;
    use anyhow::{Context, Result, ensure};
    use std::{collections::BTreeMap, fs, net::Ipv4Addr, ops::Range, process::Command};
    use wayline_protocol::{Sample, Series, State};

    fn sqlite_available() -> bool {
        Command::new("sqlite3")
            .arg("-version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn identity(index: u8) -> String {
        format!("{}_1_1_1_{index}", "a".repeat(48))
    }

    fn series(id: &str, history: Vec<Sample>) -> Series {
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
            history,
        }
    }

    fn minutes(id: &str, range: Range<u64>) -> Series {
        let history = range
            .flat_map(|minute| {
                [
                    Sample {
                        at_ms: minute * 60_000,
                        us: Some(20_000),
                    },
                    Sample {
                        at_ms: minute * 60_000 + 1_000,
                        us: None,
                    },
                ]
            })
            .collect();
        series(id, history)
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
        let stored = stored_at(&database, &identities, minute, minute)?;
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
    fn schema_v1_converts_to_v2() -> Result<()> {
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
            "PRAGMA user_version; SELECT count(*) FROM series; SELECT count(*) FROM samples; SELECT count(*) FROM sqlite_master WHERE name='bins';",
            true,
        )?;
        let lines: Vec<&str> = state.lines().collect();
        ensure!(lines == ["2", "2", "3", "0"], "Unexpected schema: {state}");
        Ok(())
    }

    #[test]
    fn history_round_trips_through_sqlite() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let current = now() / 60;
        let (first, second) = (current - 3, current - 2);
        let sample = |minute: u64, offset: u64, us: Option<u32>| Sample {
            at_ms: minute * 60_000 + offset,
            us,
        };
        let snapshot = [series(
            "abc_1_1_1_1",
            vec![
                sample(first, 0, Some(30_000)),
                sample(first, 1_000, Some(10_000)),
                sample(first, 2_000, Some(20_000)),
                sample(first, 3_000, None),
                sample(second, 0, Some(40_000)),
                sample(second, 1_000, None),
            ],
        )];
        collect_at(&database, &snapshot)?;
        collect_at(&database, &snapshot)?;
        let rows = sql(&database, "SELECT count(*) FROM samples;", true)?;
        ensure!(
            rows.lines().last() == Some("2"),
            "Unexpected row count: {rows}"
        );
        let mut all = stored_at(&database, &["abc_1_1_1_1".to_owned()], first, second)?;
        let stored = all
            .remove("abc_1_1_1_1")
            .context("Missing stored series.")?;
        ensure!(stored.len() == 2, "Expected two stored minutes.");
        let check = |minute: u64, expected: (u32, u32, u32, u32, u32)| -> Result<()> {
            let bin = stored.get(&minute).context("Missing stored minute.")?;
            let actual = (
                bin.sent,
                bin.lost,
                bin.min.unwrap_or_default(),
                bin.median.unwrap_or_default(),
                bin.max.unwrap_or_default(),
            );
            ensure!(
                actual == expected,
                "Unexpected bin {actual:?} for minute {minute}."
            );
            Ok(())
        };
        check(first, (4, 1, 10_000, 20_000, 30_000))?;
        check(second, (2, 1, 40_000, 40_000, 40_000))
    }

    #[test]
    fn full_day_reads_without_hitting_limits() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let current = now() / 60;
        let first = current - 1439;
        let identities: Vec<String> = (1..=9).map(identity).collect();
        for start in (first..current).step_by(60) {
            let snapshot: Vec<Series> = identities
                .iter()
                .map(|id| minutes(id, start..start + 60))
                .collect();
            collect_at(&database, &snapshot)?;
        }
        let read = stored_at(&database, &identities, first, current - 1)?;
        let points: usize = read.values().map(BTreeMap::len).sum();
        ensure!(
            read.len() == 9 && points == 9 * 1439,
            "Unexpected points: {points}"
        );
        let size = fs::metadata(&database)?.len();
        ensure!(size < 4 * 1024 * 1024, "Database is {size} bytes.");
        Ok(())
    }

    #[test]
    fn retention_drops_rows_older_than_a_day() -> Result<()> {
        if !sqlite_available() {
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("latency.sqlite");
        let current = now() / 60;
        let snapshot = [minutes(&identity(1), current - 2..current - 1)];
        collect_at(&database, &snapshot)?;
        sql(
            &database,
            &format!(
                "INSERT INTO samples SELECT id,{},1,1,1,1,0 FROM series;",
                current - 1500
            ),
            false,
        )?;
        collect_at(&database, &snapshot)?;
        let old = sql(
            &database,
            &format!(
                "SELECT count(*) FROM samples WHERE minute<{};",
                current - 1439
            ),
            true,
        )?;
        ensure!(old.trim() == "0", "Old rows remain: {old}");
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
        let current = now() / 60;
        collect_at(
            &database,
            &[minutes(&identity(1), current - 2..current - 1)],
        )?;
        ensure!(
            directory.path().join("latency.sqlite.broken").exists(),
            "Damaged database was not moved aside."
        );
        let rows = sql(&database, "SELECT count(*) FROM samples;", true)?;
        ensure!(rows.trim() == "1", "Unexpected rows: {rows}");
        let events = fs::read_to_string(directory.path().join("events.json"))?;
        ensure!(
            events.contains("The history database was damaged and was reset."),
            "Missing recovery event."
        );
        Ok(())
    }

    #[test]
    fn bins_count_losses_and_take_the_median() {
        let base = (now() * 1000 / 60_000 - 2) * 60_000;
        let sample = |offset: u64, us: Option<u32>| Sample {
            at_ms: base + offset,
            us,
        };
        let samples = [
            sample(0, Some(30_000)),
            sample(1_000, Some(10_000)),
            sample(2_000, Some(20_000)),
            sample(3_000, None),
            sample(4_000, Some(9_000_000)),
            Sample {
                at_ms: 1,
                us: Some(1),
            },
        ];
        let result = bins(&samples);
        assert_eq!(result.len(), 1);
        let bin = &result[&(base / 60_000)];
        assert_eq!((bin.sent, bin.lost), (5, 2));
        assert_eq!(
            (bin.min, bin.median, bin.max),
            (Some(10_000), Some(20_000), Some(30_000))
        );
    }
}
