use crate::changes::{self, Event};
use crate::history::{self, LEVELS};
use crate::identity;
use crate::runtime::{self, RUNTIME};
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::column::reader::ColumnReader;
use parquet::data_type::{ByteArray, ByteArrayType, Int64Type};
use parquet::file::metadata::KeyValue;
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

pub use wayline_protocol::schema::{Fold, Kind};

pub type Row = Vec<Option<i64>>;

pub struct Entry {
    pub link: String,
    pub at: u64,
    pub row: Row,
}

pub type Series = BTreeMap<String, BTreeMap<u64, Row>>;

type Chunk = BTreeMap<(String, u64), Row>;

struct Tier {
    step: u64,
    span: u64,
}

fn tiers() -> impl Iterator<Item = Tier> {
    LEVELS
        .iter()
        .zip(SPANS)
        .map(|((step, _), span)| Tier { step: *step, span })
}

fn tier(step: u64) -> Tier {
    tiers().find(|tier| tier.step == step).unwrap_or(Tier {
        step: LEVELS[0].0,
        span: SPANS[0],
    })
}

pub fn kind_dir(root: &Path, kind: &Kind) -> PathBuf {
    root.join(format!("kind={}", kind.name))
}

fn tier_dir(root: &Path, kind: &Kind, step: u64) -> PathBuf {
    kind_dir(root, kind).join(format!("step={step}"))
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_cycle - 719_468
}

const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_cycle = shifted - era * 146_097;
    let of_era = (of_cycle - of_cycle / 1_460 + of_cycle / 36_524 - of_cycle / 146_096) / 365;
    let of_year = of_cycle - (365 * of_era + of_era / 4 - of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

pub fn stamp(seconds: u64) -> String {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let clock = seconds.rem_euclid(86_400);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        clock / 3_600,
        clock % 3_600 / 60,
        clock % 60
    )
}

pub fn unstamp(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> { text.get(range)?.parse().ok() };
    let (year, month, day) = (number(0..4)?, number(4..6)?, number(6..8)?);
    let (hour, minute, second) = (number(9..11)?, number(11..13)?, number(13..15)?);
    let seconds = u64::try_from(
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second,
    )
    .ok()?;
    (stamp(seconds) == text).then_some(seconds)
}

fn chunk_path(dir: &Path, start: u64, step: u64) -> PathBuf {
    dir.join(format!("{}.{EXTENSION}", stamp(start * step)))
}

fn schema(kind: &Kind) -> Result<Arc<Type>> {
    let fields = kind
        .fields
        .iter()
        .map(|field| format!("optional int64 {};", field.name))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Arc::new(parse_message_type(&format!(
        "message {} {{ required binary series (UTF8); required int64 ts (TIMESTAMP(MILLIS,true)); {fields} }}",
        kind.name
    ))?))
}

pub fn metadata(
    kind: &str,
    columns: &serde_json::Value,
    extra: Vec<(&'static str, String)>,
) -> Vec<KeyValue> {
    [
        (
            "wayline.schema",
            wayline_protocol::schema::VERSION.to_owned(),
        ),
        ("wayline.kind", kind.to_owned()),
        ("wayline.device", identity::device()),
        ("wayline.columns", columns.to_string()),
    ]
    .into_iter()
    .chain(extra)
    .map(|(key, value)| KeyValue::new(key.to_owned(), value))
    .collect()
}

fn properties(kind: &Kind, tier: &Tier) -> Result<Arc<WriterProperties>> {
    let columns = serde_json::to_value(kind.fields)?;
    let extra = vec![
        ("wayline.description", kind.description.to_owned()),
        ("wayline.step_seconds", tier.step.to_string()),
        ("wayline.span_seconds", (tier.span * tier.step).to_string()),
    ];
    let mut builder = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(ZSTD_LEVEL)?))
        .set_created_by("wayline".into())
        .set_key_value_metadata(Some(metadata(kind.name, &columns, extra)));
    for name in std::iter::once("ts").chain(kind.fields.iter().map(|field| field.name)) {
        builder = builder
            .set_column_dictionary_enabled(ColumnPath::from(name), false)
            .set_column_encoding(ColumnPath::from(name), Encoding::DELTA_BINARY_PACKED);
    }
    Ok(Arc::new(builder.build()))
}

fn encode(kind: &Kind, tier: &Tier, chunk: &Chunk) -> Result<Vec<u8>> {
    let step = tier.step;
    let mut out = Vec::new();
    let mut writer = SerializedFileWriter::new(&mut out, schema(kind)?, properties(kind, tier)?)?;
    let mut group = writer.next_row_group()?;
    let names: Vec<ByteArray> = chunk
        .keys()
        .map(|(series, _)| ByteArray::from(series.as_str()))
        .collect();
    let times = chunk
        .keys()
        .map(|(_, period)| i64::try_from(period * step * 1_000))
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
        .context("The chunk schema has no time.")?;
    column
        .typed::<Int64Type>()
        .write_batch(&times, None, None)?;
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

pub fn integers(reader: ColumnReader, rows: usize, optional: bool) -> Result<Vec<Option<i64>>> {
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

pub fn strings(reader: ColumnReader, rows: usize) -> Result<Vec<String>> {
    let ColumnReader::ByteArrayColumnReader(mut reader) = reader else {
        anyhow::bail!("The Parquet file has no text column here.");
    };
    let mut values = Vec::with_capacity(rows);
    let mut read = 0;
    while read < rows {
        let (records, _, _) = reader.read_records(rows - read, None, None, &mut values)?;
        ensure!(records > 0, "The Parquet file ended early.");
        read += records;
    }
    values
        .iter()
        .map(|value: &ByteArray| Ok(value.as_utf8()?.to_owned()))
        .collect()
}

fn decode(kind: &Kind, step: u64, bytes: Vec<u8>) -> Result<Chunk> {
    let reader = SerializedFileReader::new(Bytes::from(bytes))?;
    let names: Vec<String> = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect();
    let position = |name: &str| names.iter().position(|found| found == name);
    let series_at = position("series").context("The history chunk has no series column.")?;
    let time_at = position("ts").context("The history chunk has no ts column.")?;
    let fields: Vec<Option<usize>> = kind
        .fields
        .iter()
        .map(|field| position(field.name))
        .collect();
    let mut chunk = Chunk::new();
    for index in 0..reader.num_row_groups() {
        let group = reader.get_row_group(index)?;
        let rows = usize::try_from(group.metadata().num_rows())?;
        let series = strings(group.get_column_reader(series_at)?, rows)?;
        let times = integers(group.get_column_reader(time_at)?, rows, false)?;
        let columns = fields
            .iter()
            .map(|column| match column {
                Some(column) => integers(group.get_column_reader(*column)?, rows, true),
                None => Ok(vec![None; rows]),
            })
            .collect::<Result<Vec<_>>>()?;
        for (row, (name, at)) in series.into_iter().zip(times).enumerate() {
            let at = u64::try_from(at.context("The history chunk has a row without a time.")?)?;
            ensure!(
                at % (step * 1_000) == 0,
                "The history chunk has a time off its step."
            );
            let cells = columns.iter().map(|column| column[row]).collect();
            chunk.insert((name, at / 1_000 / step), cells);
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
            seq: 0,
            subject: "history".into(),
            kind: "history".into(),
            text: DAMAGED.into(),
        }],
    )
}

fn load(root: &Path, kind: &Kind, step: u64, path: &Path) -> Result<Chunk> {
    if !path.exists() {
        return Ok(Chunk::new());
    }
    if let Ok(chunk) =
        wayline_support::read(path, CHUNK_LIMIT).and_then(|bytes| decode(kind, step, bytes))
    {
        return Ok(chunk);
    }
    damaged(root, path)?;
    Ok(Chunk::new())
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    wayline_support::atomic(path, bytes)
}

fn save(kind: &Kind, tier: &Tier, path: &Path, chunk: &Chunk) -> Result<()> {
    if chunk.is_empty() {
        return match fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    write(path, &encode(kind, tier, chunk)?)
}

fn starts(dir: &Path, step: u64) -> Vec<u64> {
    let mut found: Vec<u64> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let seconds = unstamp(name.strip_suffix(&format!(".{EXTENSION}"))?)?;
            (seconds % step == 0).then_some(seconds / step)
        })
        .collect();
    found.sort_unstable();
    found
}

pub fn fold(kind: &Kind, rows: &[&Row]) -> Row {
    kind.fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let mut values: Vec<i64> = rows
                .iter()
                .filter_map(|row| row.get(index).copied().flatten())
                .collect();
            if values.is_empty() {
                return None;
            }
            match field.fold {
                Fold::Sum => Some(
                    values
                        .iter()
                        .fold(0_i64, |sum, value| sum.saturating_add(*value)),
                ),
                Fold::Min => values.iter().min().copied(),
                Fold::Max => values.iter().max().copied(),
                Fold::MedianOfMedians => {
                    values.sort_unstable();
                    values.get((values.len() - 1) / 2).copied()
                }
            }
        })
        .collect()
}

fn rows_in(root: &Path, kind: &Kind, tier: &Tier, (first, last): (u64, u64)) -> Result<Chunk> {
    let dir = tier_dir(root, kind, tier.step);
    let mut found = Chunk::new();
    for start in starts(&dir, tier.step) {
        if start > last || start + tier.span <= first {
            continue;
        }
        found.extend(
            load(root, kind, tier.step, &chunk_path(&dir, start, tier.step))?
                .into_iter()
                .filter(|((_, period), _)| (first..=last).contains(period)),
        );
    }
    Ok(found)
}

fn merge(root: &Path, kind: &Kind, tier: &Tier, rows: Chunk, replace: bool) -> Result<()> {
    let dir = tier_dir(root, kind, tier.step);
    let mut grouped: BTreeMap<u64, Chunk> = BTreeMap::new();
    for (key, row) in rows {
        grouped
            .entry(key.1 / tier.span * tier.span)
            .or_default()
            .insert(key, row);
    }
    for (start, rows) in grouped {
        let path = chunk_path(&dir, start, tier.step);
        let mut chunk = load(root, kind, tier.step, &path)?;
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
            save(kind, tier, &path, &chunk)?;
        }
    }
    Ok(())
}

fn coarsen(kind: &Kind, rows: &Chunk, ratio: u64, keep: Option<&BTreeSet<u64>>) -> Chunk {
    let mut groups: BTreeMap<(String, u64), Vec<&Row>> = BTreeMap::new();
    for ((series, period), row) in rows {
        let parent = period / ratio;
        if keep.is_none_or(|keep| keep.contains(&parent)) {
            groups
                .entry((series.clone(), parent))
                .or_default()
                .push(row);
        }
    }
    groups
        .into_iter()
        .map(|(key, rows)| (key, fold(kind, &rows)))
        .collect()
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
    merge(
        root,
        kind,
        parent,
        coarsen(kind, &children, ratio, Some(&parents)),
        true,
    )?;
    Ok(parents)
}

fn expire(root: &Path, kind: &Kind, days: u32, now: u64) -> Result<()> {
    for tier in tiers() {
        let kept = history::retained(days, tier.step);
        let dir = tier_dir(root, kind, tier.step);
        for start in starts(&dir, tier.step) {
            if kept == 0 || (start + tier.span) * tier.step <= now.saturating_sub(kept) {
                fs::remove_file(chunk_path(&dir, start, tier.step))?;
            }
        }
    }
    Ok(())
}

pub fn kinds(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("kind="))
        .map(|entry| entry.path())
        .collect()
}

fn files(root: &Path) -> Vec<(u64, u64, PathBuf)> {
    let mut found = Vec::new();
    for kind in kinds(root) {
        for tier in tiers() {
            let dir = kind.join(format!("step={}", tier.step));
            for start in starts(&dir, tier.step) {
                let path = chunk_path(&dir, start, tier.step);
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

pub fn bytes(root: &Path) -> u64 {
    files(root).iter().map(|(_, bytes, _)| bytes).sum()
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
        let bounds = history::aligned((low, high), step / BUCKET_SECONDS);
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
            fine = coarsen(kind, &fine, parent / child, None);
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
        "history chunks: {} Parquet files, {total} bytes of {} bytes cap",
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
    use super::{Entry, Fold, Kind, fold, series, stamp, store, unstamp};
    use crate::history::{HOUR, LEVELS};
    use crate::runtime;
    use anyhow::{Context, Result, ensure};
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use std::fs;
    use wayline_protocol::schema::{Measure, measure};

    static FIELDS: [Measure; 4] = [
        measure("total", Fold::Sum, "count", "Total"),
        measure("low", Fold::Min, "value", "Lowest"),
        measure("high", Fold::Max, "value", "Highest"),
        measure("middle", Fold::MedianOfMedians, "value", "Middle"),
    ];

    static TEST: Kind = Kind {
        name: "test",
        description: "Test kind",
        fields: &FIELDS,
    };

    static NARROW: Kind = Kind {
        name: "test",
        description: "Test kind before a column was added",
        fields: &[
            measure("total", Fold::Sum, "count", "Total"),
            measure(
                "retired",
                Fold::Max,
                "value",
                "A column a later version dropped",
            ),
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
    fn stamps_are_utc_and_reversible() {
        assert_eq!(stamp(0), "19700101T000000Z");
        assert_eq!(stamp(951_782_400), "20000229T000000Z");
        assert_eq!(stamp(1_795_291_800), "20261121T201000Z");
        assert_eq!(unstamp("20261121T201000Z"), Some(1_795_291_800));
        assert_eq!(stamp(4_102_444_800), "21000101T000000Z");
        for seconds in [0, 951_782_399, 4_102_444_800, 1_795_291_815] {
            assert_eq!(unstamp(&stamp(seconds)), Some(seconds));
        }
        for bad in [
            "20261321T000000Z",
            "20260230T000000Z",
            "2026112T1830000Z",
            "x",
        ] {
            assert_eq!(unstamp(bad), None, "{bad}");
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
    fn chunks_are_self_describing() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let now = runtime::now();
        let at = now / 900 * 900 - 900;
        store(directory.path(), 7, &TEST, &[entry("a", at, 3)], now)?;
        let dir = directory.path().join("kind=test").join("step=15");
        let path = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .next()
            .context("No chunk was written.")?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        ensure!(name.ends_with("Z.parquet"), "Unexpected chunk name {name}.");
        let reader = SerializedFileReader::new(fs::File::open(&path)?)?;
        let meta = reader.metadata().file_metadata();
        let pairs: Vec<(String, String)> = meta
            .key_value_metadata()
            .into_iter()
            .flatten()
            .map(|pair| (pair.key.clone(), pair.value.clone().unwrap_or_default()))
            .collect();
        ensure!(pairs.contains(&("wayline.kind".into(), "test".into())));
        ensure!(pairs.contains(&("wayline.step_seconds".into(), "15".into())));
        let columns = pairs
            .iter()
            .find(|(key, _)| key == "wayline.columns")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        ensure!(
            columns.contains("\"rollup\":\"median_of_medians\""),
            "{columns}"
        );
        ensure!(columns.contains("\"description\":\"Middle\""), "{columns}");
        ensure!(pairs.contains(&("wayline.span_seconds".into(), "3600".into())));
        ensure!(
            pairs
                .iter()
                .any(|(key, value)| key == "wayline.device" && !value.is_empty())
        );
        let time = meta.schema_descr().column(1);
        ensure!(time.name() == "ts");
        ensure!(
            format!("{:?}", time.logical_type_ref()).contains("is_adjusted_to_u_t_c: true"),
            "{:?}",
            time.logical_type_ref()
        );
        Ok(())
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
        for (step, _) in LEVELS {
            let dir = root.join("kind=test").join(format!("step={step}"));
            ensure!(dir.is_dir(), "Step {step} was not written.");
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
        let old_hour = stamp(start / 3_600 * 3_600);
        let remaining = super::files(root)
            .iter()
            .filter(|(_, _, path)| path.to_string_lossy().contains(&old_hour))
            .count();
        ensure!(remaining == 0, "Expired chunks remain: {remaining}");
        Ok(())
    }

    #[test]
    fn older_and_newer_schemas_stay_readable() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let now = runtime::now();
        let at = now / 900 * 900 - 900;
        let narrow = Entry {
            link: "a".into(),
            at,
            row: vec![Some(7), Some(99)],
        };
        store(root, 7, &NARROW, &[narrow], now)?;
        let read = series(root, &TEST, 15, (at / 15, at / 15), &[])?;
        let row = read
            .get("a")
            .and_then(|rows| rows.get(&(at / 15)))
            .cloned()
            .unwrap_or_default();
        ensure!(
            row == vec![Some(7), None, None, None],
            "Older file read as {row:?}"
        );
        let events = root.join("events.json");
        ensure!(!events.exists(), "A readable file was treated as damaged.");
        store(root, 7, &TEST, &[entry("a", at + 15, 3)], now)?;
        let back = series(root, &NARROW, 15, (at / 15, at / 15 + 1), &[])?;
        ensure!(back.get("a").map_or(0, std::collections::BTreeMap::len) == 2);
        Ok(())
    }

    #[test]
    fn damaged_chunks_are_moved_aside() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let now = runtime::now();
        let at = now / 15 * 15 - 600;
        store(root, 7, &TEST, &[entry("a", at, 3)], now)?;
        let dir = root.join("kind=test").join("step=15");
        let name = stamp(at / 15 / 240 * 240 * 15);
        fs::write(dir.join(format!("{name}.parquet")), b"not parquet")?;
        let read = series(root, &TEST, 15, (at / 15, at / 15), &[])?;
        ensure!(read.is_empty(), "A damaged chunk was read.");
        ensure!(dir.join(format!("{name}.parquet.broken")).exists());
        let events = fs::read_to_string(root.join("events.json"))?;
        ensure!(events.contains("damaged"));
        store(root, 7, &TEST, &[entry("a", at, 3)], now)?;
        ensure!(!series(root, &TEST, 15, (at / 15, at / 15), &[])?.is_empty());
        Ok(())
    }
}
