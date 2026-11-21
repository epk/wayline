use crate::changes::{self, Event};
use crate::history::{self, LEVELS};
use crate::runtime::{self, RUNTIME};
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::column::reader::ColumnReader;
use parquet::data_type::{ByteArray, ByteArrayType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use parquet::schema::types::{ColumnPath, Type};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wayline_protocol::BUCKET_SECONDS;

const SPANS: [u64; 4] = [240, 360, 288, 168];
const MEMORY_CAP: u64 = 8 * 1024 * 1024;
const FLASH_CAP: u64 = 96 * 1024 * 1024;
const CHUNK_LIMIT: usize = 16 * 1024 * 1024;
const EXTENSION: &str = "parquet";
const ZSTD_LEVEL: i32 = 3;
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

fn schema(kind: &Kind) -> Result<Arc<Type>> {
    let fields = kind
        .fields
        .iter()
        .map(|(name, _)| format!("optional int64 {name};"))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Arc::new(parse_message_type(&format!(
        "message {} {{ required binary series (UTF8); required int64 period; {fields} }}",
        kind.name
    ))?))
}

fn properties(kind: &Kind) -> Result<Arc<WriterProperties>> {
    let mut builder = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(ZSTD_LEVEL)?))
        .set_created_by("wayline".into());
    for name in std::iter::once("period").chain(kind.fields.iter().map(|(name, _)| *name)) {
        builder = builder
            .set_column_dictionary_enabled(ColumnPath::from(name), false)
            .set_column_encoding(ColumnPath::from(name), Encoding::DELTA_BINARY_PACKED);
    }
    Ok(Arc::new(builder.build()))
}

fn encode(kind: &Kind, chunk: &Chunk) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut writer = SerializedFileWriter::new(&mut out, schema(kind)?, properties(kind)?)?;
    let mut group = writer.next_row_group()?;
    let names: Vec<ByteArray> = chunk
        .keys()
        .map(|(series, _)| ByteArray::from(series.as_str()))
        .collect();
    let periods = chunk
        .keys()
        .map(|(_, period)| i64::try_from(*period))
        .collect::<Result<Vec<_>, _>>()?;
    let mut column = group
        .next_column()?
        .context("The chunk schema has no series.")?;
    column
        .typed::<ByteArrayType>()
        .write_batch(&names, None, None)?;
    column.close()?;
    let mut column = group
        .next_column()?
        .context("The chunk schema has no period.")?;
    column
        .typed::<Int64Type>()
        .write_batch(&periods, None, None)?;
    column.close()?;
    for index in 0..kind.fields.len() {
        let cells = chunk.values().map(|row| row.get(index).copied().flatten());
        let values: Vec<i64> = cells.clone().flatten().collect();
        let levels: Vec<i16> = cells.map(|cell| i16::from(cell.is_some())).collect();
        let mut column = group
            .next_column()?
            .context("The chunk schema is missing a column.")?;
        column
            .typed::<Int64Type>()
            .write_batch(&values, Some(&levels), None)?;
        column.close()?;
    }
    group.close()?;
    writer.close()?;
    Ok(out)
}

fn integers(reader: ColumnReader, rows: usize, optional: bool) -> Result<Vec<Option<i64>>> {
    let ColumnReader::Int64ColumnReader(mut reader) = reader else {
        anyhow::bail!("The history chunk has a column of the wrong type.");
    };
    let (mut values, mut levels) = (Vec::with_capacity(rows), Vec::with_capacity(rows));
    let mut read = 0;
    while read < rows {
        let (records, _, _) = reader.read_records(
            rows - read,
            optional.then_some(&mut levels),
            None,
            &mut values,
        )?;
        ensure!(records > 0, "The history chunk ended early.");
        read += records;
    }
    if !optional {
        return Ok(values.into_iter().map(Some).collect());
    }
    ensure!(
        levels.len() == rows,
        "The history chunk has missing levels."
    );
    let mut present = values.into_iter();
    Ok(levels
        .into_iter()
        .map(|level| if level > 0 { present.next() } else { None })
        .collect())
}

fn names(reader: ColumnReader, rows: usize) -> Result<Vec<String>> {
    let ColumnReader::ByteArrayColumnReader(mut reader) = reader else {
        anyhow::bail!("The history chunk has no series names.");
    };
    let mut values = Vec::with_capacity(rows);
    let mut read = 0;
    while read < rows {
        let (records, _, _) = reader.read_records(rows - read, None, None, &mut values)?;
        ensure!(records > 0, "The history chunk ended early.");
        read += records;
    }
    values
        .iter()
        .map(|value: &ByteArray| Ok(value.as_utf8()?.to_owned()))
        .collect()
}

fn decode(kind: &Kind, bytes: Vec<u8>) -> Result<Chunk> {
    let reader = SerializedFileReader::new(Bytes::from(bytes))?;
    let expected: Vec<&str> = ["series", "period"]
        .into_iter()
        .chain(kind.fields.iter().map(|(name, _)| *name))
        .collect();
    let found: Vec<String> = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect();
    ensure!(
        found == expected,
        "The history chunk has an unexpected schema."
    );
    let mut chunk = Chunk::new();
    for index in 0..reader.num_row_groups() {
        let group = reader.get_row_group(index)?;
        let rows = usize::try_from(group.metadata().num_rows())?;
        let series = names(group.get_column_reader(0)?, rows)?;
        let periods = integers(group.get_column_reader(1)?, rows, false)?;
        let columns = (0..kind.fields.len())
            .map(|field| integers(group.get_column_reader(field + 2)?, rows, true))
            .collect::<Result<Vec<_>>>()?;
        for (row, (name, period)) in series.into_iter().zip(periods).enumerate() {
            let period =
                u64::try_from(period.context("The history chunk has a row without a key.")?)?;
            let cells = columns.iter().map(|column| column[row]).collect();
            chunk.insert((name, period), cells);
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
        let chunk = dir.join(format!("{}.parquet", at / 15 / 240 * 240));
        fs::write(&chunk, b"not arrow")?;
        let read = series(root, &TEST, 15, (at / 15, at / 15), &[])?;
        ensure!(read.is_empty(), "A damaged chunk was read.");
        ensure!(
            dir.join(format!("{}.parquet.broken", at / 15 / 240 * 240))
                .exists()
        );
        let events = fs::read_to_string(root.join("events.json"))?;
        ensure!(events.contains("damaged"));
        store(root, 7, &TEST, &[entry("a", at, 3)], now)?;
        ensure!(!series(root, &TEST, 15, (at / 15, at / 15), &[])?.is_empty());
        Ok(())
    }
}
