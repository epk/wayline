use crate::{
    router::{Inventory, series_id},
    runtime::{RUNTIME, directory, milliseconds, now},
    settings::{self, Settings, Target},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
};
use wayline_protocol::{MAX_SERIES, Sample, Series};

const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;

#[derive(Default, Clone)]
struct Bin {
    min: Option<u32>,
    median: Option<u32>,
    max: Option<u32>,
    sent: u32,
    lost: u32,
}

#[derive(Deserialize)]
pub struct Query {
    start: u64,
    end: u64,
    step: usize,
    target: Target,
    tunnel: String,
    #[serde(default)]
    include_wan: bool,
    #[serde(default)]
    all: bool,
}

#[derive(Serialize)]
pub struct Response {
    series: Vec<Line>,
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

fn database(config: &Settings) -> PathBuf {
    if config.history_hours == 24 {
        PathBuf::from("/etc/wayline/history/latency.sqlite")
    } else {
        PathBuf::from(format!("{RUNTIME}/history/latency.sqlite"))
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Known(BTreeMap<String, String>);

impl Known {
    fn path(config: &Settings) -> PathBuf {
        database(config).with_file_name("paths.json")
    }

    pub fn read(config: &Settings) -> Self {
        fs::read(Self::path(config))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn remember(config: &Settings, inventory: &Inventory) -> Result<()> {
        let mut known = Self::read(config);
        let before = known.0.clone();
        for path in &inventory.paths {
            known.0.insert(path.key.clone(), path.prefix().to_owned());
        }
        if known.0 != before {
            let path = Self::path(config);
            directory(path.parent().context("Invalid history directory.")?)?;
            wayline_support::atomic(&path, &serde_json::to_vec(&known)?)?;
        }
        Ok(())
    }
}

fn sql(path: &Path, statement: &str, readonly: bool) -> Result<String> {
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
        "PRAGMA busy_timeout=1000; PRAGMA cache_size=-128; PRAGMA mmap_size=0; {statement}"
    );
    Ok(String::from_utf8(wayline_support::input(
        "sqlite3",
        &arguments,
        input.as_bytes(),
        262_144,
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
    collect_at(&path, snapshot)
}

fn collect_at(path: &Path, snapshot: &[Series]) -> Result<()> {
    if now() < 1_704_067_200 {
        return Ok(());
    }
    let current = now() / 60;
    let mut statement = String::from(
        "PRAGMA page_size=4096; PRAGMA max_page_count=2048; PRAGMA journal_mode=TRUNCATE; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS bins(minute INTEGER,series TEXT,min_us INTEGER,median_us INTEGER,max_us INTEGER,sent INTEGER,lost INTEGER,PRIMARY KEY(minute,series)) WITHOUT ROWID; BEGIN IMMEDIATE;",
    );
    write!(
        statement,
        "DELETE FROM bins WHERE minute < {} OR minute > {current};",
        current.saturating_sub(1439)
    )?;
    for series in snapshot.iter().take(MAX_SERIES) {
        ensure!(settings::id(&series.id), "Invalid measurement identity.");
        for (minute, bin) in bins(&series.history) {
            if minute < current {
                write!(
                    statement,
                    "INSERT INTO bins VALUES({minute},'{}',{},{},{},{},{}) ON CONFLICT(minute,series) DO UPDATE SET min_us=excluded.min_us,median_us=excluded.median_us,max_us=excluded.max_us,sent=excluded.sent,lost=excluded.lost WHERE excluded.sent>bins.sent;",
                    series.id,
                    number(bin.min),
                    number(bin.median),
                    number(bin.max),
                    bin.sent,
                    bin.lost
                )?;
            }
        }
    }
    statement.push_str("DELETE FROM bins WHERE series NOT IN (SELECT series FROM bins GROUP BY series ORDER BY max(minute) DESC LIMIT 45); COMMIT;");
    sql(path, &statement, false)?;
    Ok(())
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
    let quoted = identities
        .iter()
        .map(|identity| format!("'{identity}'"))
        .collect::<Vec<_>>()
        .join(",");
    let statement = format!(
        "SELECT series,minute,min_us,median_us,max_us,sent,lost FROM bins WHERE series IN ({quoted}) AND minute>={start} AND minute<={last} ORDER BY series,minute LIMIT {};",
        1441 * identities.len()
    );
    for line in sql(path, &statement, true)?.lines() {
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
    let known = Known::read(config);
    let prefix = |tunnel: &crate::router::TunnelInfo| {
        inventory
            .paths
            .iter()
            .find(|path| path.id == tunnel.id)
            .map(|path| path.prefix().to_owned())
            .or_else(|| known.0.get(&tunnel.key).cloned())
    };
    let mut subjects: Vec<Subject> = Vec::new();
    for tunnel in &inventory.tunnels {
        if !query.all && tunnel.id != query.tunnel {
            continue;
        }
        if let Some(prefix) = prefix(tunnel) {
            subjects.push(Subject {
                id: tunnel.id.clone(),
                name: tunnel.name.clone(),
                identity: series_id(&prefix, &query.target),
            });
        }
    }
    ensure!(
        query.all || !subjects.is_empty(),
        "No history exists for this tunnel."
    );
    if query.include_wan
        && config.diagnostics
        && config.direct_wan
        && let Some(wan) = &inventory.wan
    {
        subjects.push(Subject {
            id: wan.id.clone(),
            name: wan.name.clone(),
            identity: series_id(wan.prefix(), &query.target),
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
    Ok(Response { series })
}

#[cfg(test)]
mod tests {
    use super::{bins, collect_at, sql, stored_at};
    use crate::runtime::now;
    use anyhow::{Context, Result, ensure};
    use std::{net::Ipv4Addr, process::Command};
    use wayline_protocol::{Sample, Series, State};

    fn sqlite_available() -> bool {
        Command::new("sqlite3")
            .arg("-version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn series(history: Vec<Sample>) -> Series {
        Series {
            id: "abc_1_1_1_1".into(),
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
        let snapshot = [series(vec![
            sample(first, 0, Some(30_000)),
            sample(first, 1_000, Some(10_000)),
            sample(first, 2_000, Some(20_000)),
            sample(first, 3_000, None),
            sample(second, 0, Some(40_000)),
            sample(second, 1_000, None),
        ])];
        collect_at(&database, &snapshot)?;
        collect_at(&database, &snapshot)?;
        let rows = sql(&database, "SELECT count(*) FROM bins;", true)?;
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
