use crate::{
    router::{Inventory, PathInfo},
    settings::{self, RUNTIME, Settings},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fmt::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Default, Clone)]
struct Bin {
    values: Vec<u32>,
    min: Option<u32>,
    median: Option<u32>,
    max: Option<u32>,
    sent: u32,
    lost: u32,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn database(config: &Settings) -> PathBuf {
    if config.history_hours == 24 {
        PathBuf::from("/etc/wayline/history/latency.sqlite")
    } else {
        PathBuf::from(format!("{RUNTIME}/history/latency.sqlite"))
    }
}

fn sql(config: &Settings, statement: &str, readonly: bool) -> Result<String> {
    let path = database(config);
    if !readonly {
        settings::directory(path.parent().context("Invalid history directory.")?)?;
    }
    let mode = if readonly { "-readonly" } else { "-readwrite" };
    let input = format!(
        "PRAGMA busy_timeout=1000; PRAGMA cache_size=-128; PRAGMA mmap_size=0; {statement}"
    );
    Ok(String::from_utf8(wayline_support::input(
        "sqlite3",
        &[
            mode,
            "-batch",
            "-bail",
            "-separator",
            "|",
            path.to_str().context("Invalid history path.")?,
        ],
        input.as_bytes(),
        262_144,
        5,
    )?)?)
}

fn bins(samples: &Value) -> BTreeMap<u64, Bin> {
    let mut bins = BTreeMap::<u64, Bin>::new();
    let now = now() * 1000;
    for sample in samples.as_array().into_iter().flatten().take(64) {
        let Some(at) = sample["at_ms"]
            .as_u64()
            .filter(|at| *at >= now.saturating_sub(86_400_000) && *at <= now)
        else {
            continue;
        };
        let bin = bins.entry(at / 60_000).or_default();
        bin.sent += 1;
        if let Some(us) = sample["us"]
            .as_u64()
            .filter(|us| *us <= 5_000_000)
            .and_then(|us| u32::try_from(us).ok())
        {
            bin.values.push(us);
        } else {
            bin.lost += 1;
        }
    }
    for bin in bins.values_mut() {
        bin.values.sort_unstable();
        bin.min = bin.values.first().copied();
        bin.max = bin.values.last().copied();
        bin.median = bin
            .values
            .get(bin.values.len().saturating_sub(1) / 2)
            .copied();
    }
    bins
}

fn number(value: Option<u32>) -> String {
    value.map_or_else(|| "NULL".into(), |value| value.to_string())
}

pub fn collect(config: &Settings, snapshot: &Value) -> Result<()> {
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
    for series in snapshot["tunnels"]
        .as_array()
        .into_iter()
        .flatten()
        .take(45)
    {
        let id = series["id"]
            .as_str()
            .filter(|id| settings::id(id))
            .context("Invalid measurement identity.")?;
        for (minute, bin) in bins(&series["history"]) {
            if minute < current {
                write!(
                    statement,
                    "INSERT INTO bins VALUES({minute},'{id}',{},{},{},{},{}) ON CONFLICT(minute,series) DO UPDATE SET min_us=excluded.min_us,median_us=excluded.median_us,max_us=excluded.max_us,sent=excluded.sent,lost=excluded.lost WHERE excluded.sent>bins.sent;",
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
    sql(config, &statement, false)?;
    Ok(())
}

fn stored(config: &Settings, identity: &str, start: u64, end: u64) -> Result<BTreeMap<u64, Bin>> {
    let mut bins = BTreeMap::new();
    if !database(config).exists() {
        return Ok(bins);
    }
    let statement = format!(
        "SELECT minute,min_us,median_us,max_us,sent,lost FROM bins WHERE series='{identity}' AND minute>={start} AND minute<{end} ORDER BY minute LIMIT 1440;"
    );
    for line in sql(config, &statement, true)?.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() == 6 {
            bins.insert(
                fields[0].parse()?,
                Bin {
                    values: Vec::new(),
                    min: fields[1].parse().ok(),
                    median: fields[2].parse().ok(),
                    max: fields[3].parse().ok(),
                    sent: fields[4].parse()?,
                    lost: fields[5].parse()?,
                },
            );
        }
    }
    Ok(bins)
}

fn series(
    config: &Settings,
    snapshot: &Value,
    path: &PathInfo,
    target: &str,
    start: u64,
    end: u64,
    step: usize,
) -> Result<Value> {
    let identity = path.series(target);
    let mut history = stored(config, &identity, start, end)?;
    for measurement in snapshot["tunnels"].as_array().into_iter().flatten() {
        if measurement["id"] == identity {
            for (minute, bin) in bins(&measurement["history"]) {
                if history.get(&minute).is_none_or(|old| old.sent <= bin.sent) {
                    history.insert(minute, bin);
                }
            }
        }
    }
    let mut points = Vec::new();
    for minute in (start..end).step_by(step) {
        let mut values = Vec::new();
        let mut min = None;
        let mut max = None;
        let mut sent = 0;
        let mut lost = 0;
        for offset in 0..u64::try_from(step)? {
            if let Some(bin) = history.get(&(minute + offset)) {
                sent += bin.sent;
                lost += bin.lost;
                if let Some(value) = bin.median {
                    values.push(value);
                }
                if let Some(value) = bin.min {
                    min = Some(min.map_or(value, |old: u32| old.min(value)));
                }
                if let Some(value) = bin.max {
                    max = Some(max.map_or(value, |old: u32| old.max(value)));
                }
            }
        }
        values.sort_unstable();
        points.push(json!({"at":minute*60_000,"median":values.get(values.len().saturating_sub(1)/2).map(|value|f64::from(*value)/1000.0),"min":min.map(|value|f64::from(value)/1000.0),"max":max.map(|value|f64::from(value)/1000.0),"sent":sent,"lost":lost}));
    }
    Ok(json!({"id":path.id,"name":path.name,"points":points}))
}

pub fn query(
    config: &Settings,
    snapshot: &Value,
    inventory: &Inventory,
    args: &Value,
) -> Result<Value> {
    let start = args["start"].as_u64().context("Invalid history range.")?;
    let end = args["end"].as_u64().context("Invalid history range.")?;
    let step = args["step"].as_u64().context("Invalid history step.")?;
    let now = now() * 1000;
    ensure!(
        start < end
            && end - start <= 86_400_000
            && start >= now.saturating_sub(86_460_000)
            && end <= now + 60_000
            && [1, 5].contains(&step),
        "History range exceeds its limit."
    );
    let target = args["target"]
        .as_str()
        .context("A history target is required.")?;
    ensure!(settings::ip(target.parse()?), "Invalid history target.");
    let selected = inventory
        .paths
        .iter()
        .find(|path| args["tunnel"] == path.id)
        .context("The selected path is unavailable.")?;
    let step = usize::try_from(step)?;
    let mut result = vec![series(
        config,
        snapshot,
        selected,
        target,
        start / 60_000,
        end / 60_000,
        step,
    )?];
    if args["include_wan"] == true
        && config.diagnostics
        && config.direct_wan
        && let Some(wan) = &inventory.wan
    {
        result.push(series(
            config,
            snapshot,
            wan,
            target,
            start / 60_000,
            end / 60_000,
            step,
        )?);
    }
    Ok(json!({"series":result}))
}
