use crate::history::{self, Access, HOUR, LEVELS, MIN_CLOCK};
use crate::runtime::{self, RUNTIME};
use crate::settings::Settings;
use crate::{telemetry, wifi};
use anyhow::{Error, Result, ensure};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, TransactionBehavior, params_from_iter};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use wayline_protocol::BUCKET_SECONDS;

const VERSION: u32 = 1;
const MEMORY_PAGES: u64 = 512;
const PAGES: u64 = 4_096;
const DAMAGED: &str = "The telemetry history database was damaged and was reset.";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fold {
    Sum,
    Min,
    Max,
}

pub struct Kind {
    pub table: &'static str,
    pub fields: &'static [(&'static str, Fold)],
}

pub type Row = Vec<Option<i64>>;

pub struct Entry {
    pub link: String,
    pub at: u64,
    pub row: Row,
}

pub type Series = BTreeMap<String, BTreeMap<u64, Row>>;

fn kinds() -> [&'static Kind; 3] {
    [&wifi::KIND, &telemetry::SYSTEM, &telemetry::INTERFACE]
}

pub fn database(config: &Settings) -> PathBuf {
    history::database(config).with_file_name("telemetry.sqlite")
}

pub fn cap(path: &Path) -> u64 {
    if path.starts_with(RUNTIME) {
        MEMORY_PAGES
    } else {
        PAGES
    }
}

fn open(path: &Path, access: Access) -> Result<Connection> {
    history::open_capped(path, access, cap(path))
}

fn tier(kind: &Kind, table: &str) -> String {
    format!("{}_{table}", kind.table)
}

fn columns(kind: &Kind) -> String {
    kind.fields
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",")
}

fn folds(kind: &Kind) -> String {
    kind.fields
        .iter()
        .map(|(name, fold)| match fold {
            Fold::Sum => format!("sum({name})"),
            Fold::Min => format!("min({name})"),
            Fold::Max => format!("max({name})"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn schema() -> String {
    let mut sql = String::from(
        "CREATE TABLE IF NOT EXISTS link(id INTEGER PRIMARY KEY,name TEXT NOT NULL UNIQUE,seen INTEGER NOT NULL);",
    );
    for kind in kinds() {
        let fields = kind
            .fields
            .iter()
            .fold(String::new(), |mut out, (name, _)| {
                let _ = write!(out, "{name} INTEGER,");
                out
            });
        for (_, table) in LEVELS {
            let _ = write!(
                sql,
                "CREATE TABLE IF NOT EXISTS {}(period INTEGER NOT NULL,link INTEGER NOT NULL,{fields}PRIMARY KEY(period,link)) WITHOUT ROWID;",
                tier(kind, table)
            );
        }
    }
    sql
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
    let tables: u32 =
        connection.query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))?;
    if tables > 0 {
        drop(connection);
        history::quarantine(path, DAMAGED)?;
        connection = open(path, Access::ReadWrite)?;
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(&schema())?;
    transaction.pragma_update(None, "user_version", VERSION)?;
    transaction.commit()?;
    Ok(true)
}

fn sql(value: Option<i64>) -> Sql {
    value.map_or(Sql::Null, Sql::Integer)
}

fn span(entries: &[Entry]) -> Option<(u64, u64)> {
    let periods = entries.iter().map(|entry| entry.at / BUCKET_SECONDS);
    Some((periods.clone().min()?, periods.max()?))
}

fn append(path: &Path, days: u32, now: u64, kind: &Kind, entries: &[Entry]) -> Result<()> {
    let mut connection = open(path, Access::ReadWrite)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut ids: BTreeMap<&str, i64> = BTreeMap::new();
    for entry in entries {
        let link = entry.link.as_str();
        if ids.contains_key(link) {
            continue;
        }
        transaction.execute(
            "INSERT INTO link(name,seen) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET seen=excluded.seen WHERE seen<excluded.seen",
            (link, now / HOUR),
        )?;
        let id: i64 =
            transaction.query_row("SELECT id FROM link WHERE name=?1", [link], |row| {
                row.get(0)
            })?;
        ids.insert(link, id);
    }
    let marks = vec!["?"; kind.fields.len()].join(",");
    {
        let mut insert = transaction.prepare(&format!(
            "INSERT OR IGNORE INTO {}(period,link,{}) VALUES(?,?,{marks})",
            tier(kind, LEVELS[0].1),
            columns(kind)
        ))?;
        for entry in entries {
            ensure!(
                entry.row.len() == kind.fields.len(),
                "A telemetry row has the wrong width."
            );
            let mut arguments = vec![
                Sql::Integer(i64::try_from(entry.at / BUCKET_SECONDS)?),
                Sql::Integer(ids.get(entry.link.as_str()).copied().unwrap_or_default()),
            ];
            arguments.extend(entry.row.iter().copied().map(sql));
            insert.execute(params_from_iter(arguments))?;
        }
    }
    if let Some(mut range) = span(entries) {
        for pair in LEVELS.windows(2) {
            let [(child_step, child), (step, parent)] = pair else {
                continue;
            };
            if history::retained(days, *step) == 0 {
                break;
            }
            let ratio = step / child_step;
            let (low, high) = history::aligned(range, ratio);
            transaction.execute(
                &format!(
                    "INSERT OR REPLACE INTO {}(period,link,{}) SELECT period/?1,link,{} FROM {} WHERE period BETWEEN ?2 AND ?3 GROUP BY period/?1,link",
                    tier(kind, parent),
                    columns(kind),
                    folds(kind),
                    tier(kind, child)
                ),
                [ratio, low, high],
            )?;
            range = (range.0 / ratio, range.1 / ratio);
        }
    }
    for (step, table) in LEVELS {
        let kept = history::retained(days, step);
        transaction.execute(
            &format!("DELETE FROM {} WHERE period<?1", tier(kind, table)),
            [now.saturating_sub(kept) / step + u64::from(kept == 0)],
        )?;
    }
    let longest = LEVELS
        .iter()
        .map(|(step, _)| history::retained(days, *step))
        .max()
        .unwrap_or_default();
    let retired = now.saturating_sub(longest) / HOUR;
    for kind in kinds() {
        for (_, table) in LEVELS {
            transaction.execute(
                &format!(
                    "DELETE FROM {} WHERE link IN (SELECT id FROM link WHERE seen<?1)",
                    tier(kind, table)
                ),
                [retired],
            )?;
        }
    }
    transaction.execute("DELETE FROM link WHERE seen<?1", [retired])?;
    transaction.commit()?;
    Ok(())
}

fn shrink(path: &Path) -> Result<()> {
    let mut connection = open(path, Access::ReadWrite)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for kind in kinds() {
        for (_, table) in LEVELS.iter().rev() {
            let table = tier(kind, table);
            transaction.execute(
                &format!(
                    "DELETE FROM {table} WHERE period<=(SELECT period FROM {table} ORDER BY period LIMIT 1 OFFSET (SELECT count(*)/10 FROM {table}))"
                ),
                [],
            )?;
        }
    }
    Ok(transaction.commit()?)
}

fn recover(path: &Path, error: Error) -> Result<()> {
    if path.exists() && !history::healthy(path) {
        history::quarantine(path, DAMAGED)?;
    }
    if convert(path)? {
        return Ok(());
    }
    if history::pragma::<u64>(path, "page_count").unwrap_or_default() < cap(path) {
        return Err(error);
    }
    shrink(path)
}

pub fn store(path: &Path, days: u32, kind: &Kind, entries: &[Entry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let now = runtime::now();
    append(path, days, now, kind, entries).or_else(|error| {
        recover(path, error)?;
        append(path, days, now, kind, entries)
    })
}

fn stored(
    path: &Path,
    kind: &Kind,
    table: &str,
    (first, last): (u64, u64),
    into: &mut Series,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let connection = open(path, Access::ReadOnly)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < VERSION {
        return Ok(());
    }
    let selected = kind
        .fields
        .iter()
        .map(|(name, _)| format!("t.{name}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = connection.prepare(&format!(
        "SELECT l.name,t.period,{selected} FROM {} t JOIN link l ON l.id=t.link WHERE t.period BETWEEN ?1 AND ?2",
        tier(kind, table)
    ))?;
    let mut rows = statement.query((first, last))?;
    while let Some(row) = rows.next()? {
        let link: String = row.get(0)?;
        let period: u64 = row.get(1)?;
        let values = (0..kind.fields.len())
            .map(|index| row.get::<_, Option<i64>>(index + 2))
            .collect::<rusqlite::Result<Row>>()?;
        into.entry(link).or_default().insert(period, values);
    }
    Ok(())
}

pub fn merge(kind: &Kind, into: &mut Row, other: &[Option<i64>]) {
    for ((slot, value), (_, fold)) in into.iter_mut().zip(other).zip(kind.fields) {
        *slot = match (*slot, *value) {
            (Some(left), Some(right)) => Some(match fold {
                Fold::Sum => left.saturating_add(right),
                Fold::Min => left.min(right),
                Fold::Max => left.max(right),
            }),
            (left, right) => left.or(right),
        };
    }
}

pub fn series(
    path: &Path,
    kind: &Kind,
    step: u64,
    (first, last): (u64, u64),
    live: &[Entry],
) -> Result<Series> {
    let mut periods = Series::new();
    stored(
        path,
        kind,
        history::table(step),
        (first, last),
        &mut periods,
    )?;
    if let Some(range) = span(live) {
        let ratio = step / BUCKET_SECONDS;
        let bounds = history::aligned(range, ratio);
        let mut fine = Series::new();
        stored(path, kind, LEVELS[0].1, bounds, &mut fine)?;
        for entry in live {
            fine.entry(entry.link.clone())
                .or_default()
                .insert(entry.at / BUCKET_SECONDS, entry.row.clone());
        }
        for (link, rows) in fine {
            let target = periods.entry(link).or_default();
            let mut coarse: BTreeMap<u64, Row> = BTreeMap::new();
            for (period, row) in rows {
                let slot = coarse
                    .entry(period / ratio)
                    .or_insert_with(|| vec![None; kind.fields.len()]);
                merge(kind, slot, &row);
            }
            target.extend(
                coarse
                    .into_iter()
                    .filter(|(period, _)| (first..=last).contains(period)),
            );
        }
    }
    Ok(periods)
}

pub fn report(config: &Settings) -> Vec<String> {
    let path = database(config);
    let mut lines = vec![format!(
        "telemetry history database: {} bytes of {} bytes cap",
        fs::metadata(&path).map_or(0, |meta| meta.len()),
        cap(&path) * history::PAGE_BYTES
    )];
    let Ok(connection) = open(&path, Access::ReadOnly) else {
        return lines;
    };
    for kind in kinds() {
        for (_, table) in LEVELS {
            let table = tier(kind, table);
            if let Ok((rows, oldest, newest)) = connection.query_row(
                &format!("SELECT count(*),min(period),max(period) FROM {table}"),
                [],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<u64>>(1)?,
                        row.get::<_, Option<u64>>(2)?,
                    ))
                },
            ) {
                lines.push(format!(
                    "telemetry {table}: {rows} rows, oldest period {}, newest period {}",
                    oldest.map_or_else(String::new, |value| value.to_string()),
                    newest.map_or_else(String::new, |value| value.to_string())
                ));
            }
        }
    }
    lines
}

#[cfg(test)]
pub fn open_for_test(path: &Path) -> Result<Connection> {
    open(path, Access::ReadOnly)
}
