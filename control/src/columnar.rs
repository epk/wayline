use crate::changes::{self, Event};
use crate::history::{self, LEVELS};
use crate::runtime::{self, RUNTIME};
use anyhow::{Context, Result, ensure};
use arrow_array::{Array, ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow_ipc::CompressionType;
use arrow_ipc::reader::FileReader;
use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use arrow_schema::{DataType, Field, Schema};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wayline_protocol::BUCKET_SECONDS;

const SPANS: [u64; 4] = [240, 360, 288, 168];
const MEMORY_CAP: u64 = 8 * 1024 * 1024;
const FLASH_CAP: u64 = 96 * 1024 * 1024;
const CHUNK_LIMIT: usize = 16 * 1024 * 1024;
const EXTENSION: &str = "arrow";
const DAMAGED: &str = "A history chunk was damaged and was reset.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fold {
    Sum,
    Min,
    Max,
    Median,
}

pub struct Kind {
    pub name: &'static str,
    pub fields: &'static [(&'static str, Fold)],
}

pub type Row = Vec<Option<i64>>;

pub struct Entry {
    pub link: String,
    pub at: u64,
    pub row: Row,
}

pub type Series = BTreeMap<String, BTreeMap<u64, Row>>;

type Chunk = BTreeMap<(String, u64), Row>;

fn tier_dir(root: &Path, kind: &Kind, table: &str) -> PathBuf {
    root.join(kind.name).join(table)
}

fn chunk_path(dir: &Path, start: u64) -> PathBuf {
    dir.join(format!("{start}.{EXTENSION}"))
}

fn schema(kind: &Kind) -> Arc<Schema> {
    let mut fields = vec![
        Field::new("series", DataType::Utf8, false),
        Field::new("period", DataType::Int64, false),
    ];
    fields.extend(
        kind.fields
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Int64, true)),
    );
    Arc::new(Schema::new(fields))
}

pub fn options() -> Result<IpcWriteOptions> {
    Ok(IpcWriteOptions::default().try_with_compression(Some(CompressionType::ZSTD))?)
}

fn encode(kind: &Kind, chunk: &Chunk) -> Result<Vec<u8>> {
    let schema = schema(kind);
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            chunk.keys().map(|(series, _)| series.as_str()),
        )),
        Arc::new(Int64Array::from_iter_values(
            chunk
                .keys()
                .map(|(_, period)| i64::try_from(*period).unwrap_or(i64::MAX)),
        )),
    ];
    for index in 0..kind.fields.len() {
        columns.push(Arc::new(Int64Array::from_iter(
            chunk.values().map(|row| row.get(index).copied().flatten()),
        )));
    }
    let batch = RecordBatch::try_new(Arc::clone(&schema), columns)?;
    let mut writer = FileWriter::try_new_with_options(Vec::new(), &schema, options()?)?;
    writer.write(&batch)?;
    writer.finish()?;
    Ok(writer.into_inner()?)
}

fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .with_context(|| format!("The history chunk has no valid {name} column."))
}

fn decode(kind: &Kind, bytes: Vec<u8>) -> Result<Chunk> {
    let mut chunk = Chunk::new();
    for batch in FileReader::try_new(Cursor::new(bytes), None)? {
        let batch = batch?;
        let series: &StringArray = column(&batch, "series")?;
        let periods: &Int64Array = column(&batch, "period")?;
        let values = kind
            .fields
            .iter()
            .map(|(name, _)| column::<Int64Array>(&batch, name))
            .collect::<Result<Vec<_>>>()?;
        for row in 0..batch.num_rows() {
            ensure!(
                series.is_valid(row) && periods.is_valid(row),
                "The history chunk has a row without a key."
            );
            let period = u64::try_from(periods.value(row))?;
            let cells = values
                .iter()
                .map(|array| array.is_valid(row).then(|| array.value(row)))
                .collect();
            chunk.insert((series.value(row).to_owned(), period), cells);
        }
    }
    Ok(chunk)
}

fn damaged(root: &Path, path: &Path) -> Result<()> {
    let mut broken = path.as_os_str().to_owned();
    broken.push(".broken");
    fs::rename(path, broken)?;
    changes::append(
        root,
        [Event {
            at: runtime::now() * 1000,
            subject: "history".into(),
            kind: "history".into(),
            text: DAMAGED.into(),
        }],
    )
}

fn load(root: &Path, kind: &Kind, path: &Path) -> Result<Chunk> {
    if !path.exists() {
        return Ok(Chunk::new());
    }
    if let Ok(chunk) =
        wayline_support::read(path, CHUNK_LIMIT).and_then(|bytes| decode(kind, bytes))
    {
        return Ok(chunk);
    }
    damaged(root, path)?;
    Ok(Chunk::new())
}

fn save(kind: &Kind, path: &Path, chunk: &Chunk) -> Result<()> {
    if chunk.is_empty() {
        return match fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    wayline_support::atomic(path, &encode(kind, chunk)?)
}

fn starts(dir: &Path) -> Vec<u64> {
    let mut found: Vec<u64> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_suffix(&format!(".{EXTENSION}"))?.parse().ok()
        })
        .collect();
    found.sort_unstable();
    found
}

pub fn fold(kind: &Kind, rows: &[&Row]) -> Row {
    kind.fields
        .iter()
        .enumerate()
        .map(|(index, (_, fold))| {
            let mut values: Vec<i64> = rows
                .iter()
                .filter_map(|row| row.get(index).copied().flatten())
                .collect();
            if values.is_empty() {
                return None;
            }
            match fold {
                Fold::Sum => Some(
                    values
                        .iter()
                        .fold(0_i64, |sum, value| sum.saturating_add(*value)),
                ),
                Fold::Min => values.iter().min().copied(),
                Fold::Max => values.iter().max().copied(),
                Fold::Median => {
                    values.sort_unstable();
                    values.get((values.len() - 1) / 2).copied()
                }
            }
        })
        .collect()
}

struct Tier {
    step: u64,
    table: &'static str,
    span: u64,
}

fn tiers() -> impl Iterator<Item = Tier> {
    LEVELS.iter().zip(SPANS).map(|((step, table), span)| Tier {
        step: *step,
        table,
        span,
    })
}

fn tier(step: u64) -> Tier {
    tiers().find(|tier| tier.step == step).unwrap_or(Tier {
        step: LEVELS[0].0,
        table: LEVELS[0].1,
        span: SPANS[0],
    })
}

fn rows_in(root: &Path, kind: &Kind, tier: &Tier, (first, last): (u64, u64)) -> Result<Chunk> {
    let dir = tier_dir(root, kind, tier.table);
    let mut found = Chunk::new();
    for start in starts(&dir) {
        if start > last || start + tier.span <= first {
            continue;
        }
        found.extend(
            load(root, kind, &chunk_path(&dir, start))?
                .into_iter()
                .filter(|((_, period), _)| (first..=last).contains(period)),
        );
    }
    Ok(found)
}

fn merge(root: &Path, kind: &Kind, tier: &Tier, rows: Chunk, replace: bool) -> Result<()> {
    let dir = tier_dir(root, kind, tier.table);
    let mut grouped: BTreeMap<u64, Chunk> = BTreeMap::new();
    for (key, row) in rows {
        grouped
            .entry(key.1 / tier.span * tier.span)
            .or_default()
            .insert(key, row);
    }
    for (start, rows) in grouped {
        let path = chunk_path(&dir, start);
        let mut chunk = load(root, kind, &path)?;
        let before = chunk.len();
        let mut changed = false;
        for (key, row) in rows {
            if replace {
                changed |= chunk.insert(key, row.clone()).is_none_or(|old| old != row);
            } else {
                chunk.entry(key).or_insert(row);
            }
        }
        if changed || chunk.len() != before {
            save(kind, &path, &chunk)?;
        }
    }
    Ok(())
}

fn rollup(
    root: &Path,
    kind: &Kind,
    child: &Tier,
    parent: &Tier,
    periods: &BTreeSet<u64>,
) -> Result<BTreeSet<u64>> {
    let ratio = parent.step / child.step;
    let parents: BTreeSet<u64> = periods.iter().map(|period| period / ratio).collect();
    let (Some(&low), Some(&high)) = (parents.first(), parents.last()) else {
        return Ok(parents);
    };
    let children = rows_in(root, kind, child, (low * ratio, (high + 1) * ratio - 1))?;
    let mut groups: BTreeMap<(String, u64), Vec<&Row>> = BTreeMap::new();
    for ((series, period), row) in &children {
        let parent_period = period / ratio;
        if parents.contains(&parent_period) {
            groups
                .entry((series.clone(), parent_period))
                .or_default()
                .push(row);
        }
    }
    let rolled: Chunk = groups
        .into_iter()
        .map(|(key, rows)| (key, fold(kind, &rows)))
        .collect();
    merge(root, kind, parent, rolled, true)?;
    Ok(parents)
}

fn expire(root: &Path, kind: &Kind, days: u32, now: u64) -> Result<()> {
    for tier in tiers() {
        let kept = history::retained(days, tier.step);
        let dir = tier_dir(root, kind, tier.table);
        for start in starts(&dir) {
            if kept == 0 || (start + tier.span) * tier.step <= now.saturating_sub(kept) {
                fs::remove_file(chunk_path(&dir, start))?;
            }
        }
    }
    Ok(())
}

fn files(root: &Path) -> Vec<(u64, u64, PathBuf)> {
    let mut found = Vec::new();
    for kind in fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        for tier in tiers() {
            let dir = kind.path().join(tier.table);
            for start in starts(&dir) {
                let path = chunk_path(&dir, start);
                let bytes = fs::metadata(&path).map_or(0, |meta| meta.len());
                found.push(((start + tier.span) * tier.step, bytes, path));
            }
        }
    }
    found.sort();
    found
}

pub fn cap(root: &Path) -> u64 {
    if root.starts_with(RUNTIME) {
        MEMORY_CAP
    } else {
        FLASH_CAP
    }
}

fn enforce(root: &Path) -> Result<()> {
    let found = files(root);
    let mut total: u64 = found.iter().map(|(_, bytes, _)| bytes).sum();
    for (_, bytes, path) in found {
        if total <= cap(root) {
            break;
        }
        fs::remove_file(path)?;
        total = total.saturating_sub(bytes);
    }
    Ok(())
}

pub fn store(root: &Path, days: u32, kind: &Kind, entries: &[Entry], now: u64) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    ensure!(
        entries
            .iter()
            .all(|entry| entry.row.len() == kind.fields.len()),
        "A history row has the wrong width."
    );
    let mut periods: BTreeSet<u64> = BTreeSet::new();
    let mut fresh = Chunk::new();
    for entry in entries {
        let period = entry.at / BUCKET_SECONDS;
        periods.insert(period);
        fresh.insert((entry.link.clone(), period), entry.row.clone());
    }
    let all: Vec<Tier> = tiers().collect();
    merge(root, kind, &all[0], fresh, false)?;
    for pair in all.windows(2) {
        let [child, parent] = pair else {
            continue;
        };
        if history::retained(days, parent.step) == 0 {
            break;
        }
        periods = rollup(root, kind, child, parent, &periods)?;
    }
    expire(root, kind, days, now)?;
    enforce(root)
}

fn coarsen(kind: &Kind, rows: &Chunk, ratio: u64) -> Chunk {
    let mut groups: BTreeMap<(String, u64), Vec<&Row>> = BTreeMap::new();
    for ((series, period), row) in rows {
        groups
            .entry((series.clone(), period / ratio))
            .or_default()
            .push(row);
    }
    groups
        .into_iter()
        .map(|(key, rows)| (key, fold(kind, &rows)))
        .collect()
}

pub fn series(
    root: &Path,
    kind: &Kind,
    step: u64,
    (first, last): (u64, u64),
    live: &[Entry],
) -> Result<Series> {
    let mut found = Series::new();
    for ((series, period), row) in rows_in(root, kind, &tier(step), (first, last))? {
        found.entry(series).or_default().insert(period, row);
    }
    let periods = live.iter().map(|entry| entry.at / BUCKET_SECONDS);
    if let (Some(low), Some(high)) = (periods.clone().min(), periods.max()) {
        let ratio = step / BUCKET_SECONDS;
        let bounds = history::aligned((low, high), ratio);
        let mut fine = rows_in(root, kind, &tier(LEVELS[0].0), bounds)?;
        for entry in live {
            fine.insert(
                (entry.link.clone(), entry.at / BUCKET_SECONDS),
                entry.row.clone(),
            );
        }
        for pair in LEVELS.windows(2) {
            let [(child, _), (parent, _)] = pair else {
                continue;
            };
            if *child >= step {
                break;
            }
            fine = coarsen(kind, &fine, parent / child);
        }
        for ((series, period), row) in fine {
            if (first..=last).contains(&period) {
                found.entry(series).or_default().insert(period, row);
            }
        }
    }
    Ok(found)
}

pub fn report(root: &Path) -> Vec<String> {
    let found = files(root);
    let total: u64 = found.iter().map(|(_, bytes, _)| bytes).sum();
    let mut lines = vec![format!(
        "history chunks: {} files, {total} bytes of {} bytes cap",
        found.len(),
        cap(root)
    )];
    let mut per: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for (_, bytes, path) in &found {
        let label = path
            .parent()
            .and_then(|tier| {
                Some(format!(
                    "{}/{}",
                    tier.parent()?.file_name()?.to_str()?,
                    tier.file_name()?.to_str()?
                ))
            })
            .unwrap_or_default();
        let entry = per.entry(label).or_default();
        entry.0 += 1;
        entry.1 += bytes;
    }
    lines.extend(
        per.into_iter().map(|(label, (count, bytes))| {
            format!("history {label}: {count} chunks, {bytes} bytes")
        }),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::{Entry, Fold, Kind, SPANS, fold, series, store};
    use crate::history::{HOUR, LEVELS};
    use crate::runtime;
    use anyhow::{Result, ensure};
    use std::fs;

    static TEST: Kind = Kind {
        name: "test",
        fields: &[
            ("total", Fold::Sum),
            ("low", Fold::Min),
            ("high", Fold::Max),
            ("middle", Fold::Median),
        ],
    };

    fn entry(series: &str, at: u64, value: i64) -> Entry {
        Entry {
            link: series.into(),
            at,
            row: vec![Some(1), Some(value), Some(value), Some(value)],
        }
    }

    #[test]
    fn folds_are_exact() {
        let rows = [
            vec![Some(1), Some(5), Some(5), Some(5)],
            vec![Some(2), Some(1), Some(9), Some(9)],
            vec![None, None, None, None],
            vec![Some(3), Some(4), Some(4), Some(1)],
        ];
        let refs: Vec<&Vec<Option<i64>>> = rows.iter().collect();
        assert_eq!(fold(&TEST, &refs), vec![Some(6), Some(1), Some(9), Some(5)]);
        assert_eq!(fold(&TEST, &[]), vec![None; 4]);
    }

    #[test]
    fn chunks_roll_up_expire_and_merge_live_rows() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let now = runtime::now();
        let start = now / HOUR * HOUR - 2 * HOUR;
        let entries: Vec<Entry> = (0..240)
            .flat_map(|index| {
                let at = start + index * 15;
                let value = i64::try_from(index % 17).unwrap_or(0);
                [entry("a", at, value), entry("b", at, value * 2)]
            })
            .collect();
        store(root, 30, &TEST, &entries, now)?;
        store(root, 30, &TEST, &entries, now)?;
        for (table, span) in LEVELS.iter().map(|level| level.1).zip(SPANS) {
            let dir = root.join("test").join(table);
            ensure!(dir.is_dir(), "{table} was not written.");
            ensure!(span > 0);
        }
        let hour = series(root, &TEST, 3_600, (start / HOUR, start / HOUR), &[])?;
        let row = hour
            .get("a")
            .and_then(|rows| rows.get(&(start / HOUR)))
            .cloned()
            .unwrap_or_default();
        ensure!(row[0] == Some(240), "Hour total {:?}", row[0]);
        ensure!(
            row[1] == Some(0) && row[2] == Some(16),
            "Hour bounds {row:?}"
        );
        let (old, live) = entries.split_at(400);
        let fresh = tempfile::tempdir()?;
        store(fresh.path(), 7, &TEST, old, now)?;
        let merged = series(fresh.path(), &TEST, 60, (start / 60, start / 60 + 59), live)?;
        let minutes = merged.get("a").map_or(0, std::collections::BTreeMap::len);
        ensure!(minutes == 60, "Missing minutes: {minutes}");
        let totals: i64 = merged
            .get("b")
            .into_iter()
            .flat_map(|rows| rows.values())
            .filter_map(|row| row[0])
            .sum();
        ensure!(totals == 240, "Live rows were double counted: {totals}");
        let later = now + 40 * 86_400;
        store(root, 30, &TEST, &[entry("a", later / 15 * 15, 1)], later)?;
        let remaining: usize = LEVELS
            .iter()
            .map(|(_, table)| {
                fs::read_dir(root.join("test").join(table)).map_or(0, |entries| {
                    entries
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with(&(start / 15 / 240 * 240).to_string())
                        })
                        .count()
                })
            })
            .sum();
        ensure!(remaining == 0, "Expired chunks remain: {remaining}");
        Ok(())
    }

    #[test]
    fn damaged_chunks_are_moved_aside() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let now = runtime::now();
        let at = now / 15 * 15 - 600;
        store(root, 7, &TEST, &[entry("a", at, 3)], now)?;
        let dir = root.join("test").join(LEVELS[0].1);
        let chunk = dir.join(format!("{}.arrow", at / 15 / 240 * 240));
        fs::write(&chunk, b"not arrow")?;
        let read = series(root, &TEST, 15, (at / 15, at / 15), &[])?;
        ensure!(read.is_empty(), "A damaged chunk was read.");
        ensure!(
            dir.join(format!("{}.arrow.broken", at / 15 / 240 * 240))
                .exists()
        );
        let events = fs::read_to_string(root.join("events.json"))?;
        ensure!(events.contains("damaged"));
        store(root, 7, &TEST, &[entry("a", at, 3)], now)?;
        ensure!(!series(root, &TEST, 15, (at / 15, at / 15), &[])?.is_empty());
        Ok(())
    }
}
