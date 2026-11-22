use crate::columnar;
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use parquet::basic::{Compression, ZstdLevel};
use parquet::column::reader::ColumnReader;
use parquet::data_type::{ByteArray, ByteArrayType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DAY_MS: u64 = 86_400_000;
const LIMIT: usize = 8 * 1024 * 1024;
const EXTENSION: &str = "parquet";

#[derive(Clone, Copy)]
pub enum Type {
    Text,
    Integer,
    Time,
    OptionalTime,
}

pub struct Field {
    pub name: &'static str,
    pub kind: Type,
    pub description: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    Text(String),
    Number(Option<i64>),
}

impl Cell {
    pub fn text(self) -> Option<String> {
        match self {
            Self::Text(value) => Some(value),
            Self::Number(_) => None,
        }
    }

    pub fn number(self) -> Option<i64> {
        match self {
            Self::Number(value) => value,
            Self::Text(_) => None,
        }
    }

    pub fn millis(self) -> Option<u64> {
        self.number().and_then(|value| u64::try_from(value).ok())
    }
}

pub fn millis(value: u64) -> Cell {
    Cell::Number(i64::try_from(value).ok())
}

pub trait Record: Sized {
    const KIND: &'static str;
    const DESCRIPTION: &'static str;
    const FIELDS: &'static [Field];

    fn at(&self) -> u64;
    fn cells(&self) -> Vec<Cell>;
    fn from_cells(cells: Vec<Cell>) -> Option<Self>;
}

pub fn dir<R: Record>(root: &Path) -> PathBuf {
    root.join(format!("kind={}", R::KIND))
}

fn day_path<R: Record>(root: &Path, day: u64) -> PathBuf {
    let stamp = columnar::stamp(day * 86_400);
    dir::<R>(root).join(format!("{}.{EXTENSION}", &stamp[..8]))
}

fn days<R: Record>(root: &Path) -> Vec<u64> {
    let mut found: Vec<u64> = fs::read_dir(dir::<R>(root))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let date = name.strip_suffix(&format!(".{EXTENSION}"))?;
            Some(columnar::unstamp(&format!("{date}T000000Z"))? / 86_400)
        })
        .collect();
    found.sort_unstable();
    found
}

fn schema<R: Record>() -> String {
    let fields: Vec<String> = R::FIELDS
        .iter()
        .map(|field| match field.kind {
            Type::Text => format!("required binary {} (UTF8);", field.name),
            Type::Integer => format!("required int64 {};", field.name),
            Type::Time => format!("required int64 {} (TIMESTAMP(MILLIS,true));", field.name),
            Type::OptionalTime => {
                format!("optional int64 {} (TIMESTAMP(MILLIS,true));", field.name)
            }
        })
        .collect();
    format!("message {} {{ {} }}", R::KIND, fields.join(" "))
}

fn encode<R: Record>(rows: &[R]) -> Result<Vec<u8>> {
    let columns = serde_json::Value::Array(
        R::FIELDS
            .iter()
            .map(|field| {
                let unit = match field.kind {
                    Type::Text => "text",
                    Type::Integer => "count",
                    Type::Time | Type::OptionalTime => "timestamp",
                };
                serde_json::json!({"name": field.name, "unit": unit, "description": field.description})
            })
            .collect(),
    );
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
        .set_created_by("wayline".into())
        .set_key_value_metadata(Some(columnar::metadata(
            R::KIND,
            &columns,
            vec![("wayline.description", R::DESCRIPTION.to_owned())],
        )))
        .build();
    let mut out = Vec::new();
    let mut writer = SerializedFileWriter::new(
        &mut out,
        Arc::new(parse_message_type(&schema::<R>())?),
        Arc::new(properties),
    )?;
    let table: Vec<Vec<Cell>> = rows.iter().map(Record::cells).collect();
    ensure!(
        table.iter().all(|cells| cells.len() == R::FIELDS.len()),
        "A {} row has the wrong width.",
        R::KIND
    );
    let mut group = writer.next_row_group()?;
    for (index, field) in R::FIELDS.iter().enumerate() {
        let mut column = group
            .next_column()?
            .with_context(|| format!("The {} table is missing a column.", R::KIND))?;
        let cells = table.iter().map(|cells| cells[index].clone());
        match field.kind {
            Type::Text => {
                let values: Vec<ByteArray> = cells
                    .map(|cell| ByteArray::from(cell.text().unwrap_or_default().as_str()))
                    .collect();
                column
                    .typed::<ByteArrayType>()
                    .write_batch(&values, None, None)?;
            }
            Type::Integer | Type::Time => {
                let values: Vec<i64> = cells.map(|cell| cell.number().unwrap_or(0)).collect();
                column
                    .typed::<Int64Type>()
                    .write_batch(&values, None, None)?;
            }
            Type::OptionalTime => {
                let cells: Vec<Option<i64>> = cells.map(Cell::number).collect();
                let values: Vec<i64> = cells.iter().flatten().copied().collect();
                let levels: Vec<i16> = cells.iter().map(|cell| i16::from(cell.is_some())).collect();
                column
                    .typed::<Int64Type>()
                    .write_batch(&values, Some(&levels), None)?;
            }
        }
        column.close()?;
    }
    group.close()?;
    writer.close()?;
    Ok(out)
}

fn decode<R: Record>(bytes: Vec<u8>) -> Result<Vec<R>> {
    let reader = SerializedFileReader::new(Bytes::from(bytes))?;
    let names: Vec<String> = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect();
    let mut found = Vec::new();
    for index in 0..reader.num_row_groups() {
        let group = reader.get_row_group(index)?;
        let rows = usize::try_from(group.metadata().num_rows())?;
        let mut columns: Vec<Vec<Cell>> = Vec::new();
        for field in R::FIELDS {
            let position = names.iter().position(|name| name == field.name);
            let column = match (position, field.kind) {
                (None, Type::Text) => vec![Cell::Text(String::new()); rows],
                (None, _) => vec![Cell::Number(None); rows],
                (Some(at), Type::Text) => columnar::strings(group.get_column_reader(at)?, rows)?
                    .into_iter()
                    .map(Cell::Text)
                    .collect(),
                (Some(at), kind) => {
                    let reader: ColumnReader = group.get_column_reader(at)?;
                    columnar::integers(reader, rows, matches!(kind, Type::OptionalTime))?
                        .into_iter()
                        .map(Cell::Number)
                        .collect()
                }
            };
            columns.push(column);
        }
        for row in 0..rows {
            let cells = columns.iter().map(|column| column[row].clone()).collect();
            found.extend(R::from_cells(cells));
        }
    }
    Ok(found)
}

fn load<R: Record>(path: &Path) -> Result<Vec<R>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = wayline_support::read(path, LIMIT)
        .with_context(|| format!("Cannot read history table {}.", path.display()))?;
    if let Ok(rows) = decode::<R>(bytes) {
        return Ok(rows);
    }
    columnar::set_aside(path)?;
    Ok(Vec::new())
}

fn save<R: Record>(root: &Path, day: u64, rows: &[R]) -> Result<()> {
    let path = day_path::<R>(root, day);
    if rows.is_empty() {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    columnar::write(&path, &encode(rows)?)
}

pub fn read<R: Record>(root: &Path, (start, end): (u64, u64)) -> Result<Vec<R>> {
    let (first, last) = (start.saturating_sub(DAY_MS) / DAY_MS, end / DAY_MS);
    let mut found = Vec::new();
    for day in days::<R>(root) {
        if (first..=last).contains(&day) {
            found.extend(load::<R>(&day_path::<R>(root, day))?);
        }
    }
    found.sort_by_key(Record::at);
    Ok(found)
}

pub fn all<R: Record>(root: &Path) -> Result<Vec<R>> {
    read(root, (0, u64::MAX - DAY_MS))
}

pub fn newest<R: Record>(root: &Path, count: usize) -> Result<Vec<R>> {
    let mut found: Vec<R> = Vec::new();
    for day in days::<R>(root).into_iter().rev() {
        if found.len() >= count {
            break;
        }
        found.extend(load::<R>(&day_path::<R>(root, day))?);
    }
    found.sort_by_key(Record::at);
    let excess = found.len().saturating_sub(count);
    found.drain(..excess);
    Ok(found)
}

fn by_day<R: Record>(rows: Vec<R>) -> BTreeMap<u64, Vec<R>> {
    let mut grouped: BTreeMap<u64, Vec<R>> = BTreeMap::new();
    for row in rows {
        grouped.entry(row.at() / DAY_MS).or_default().push(row);
    }
    grouped
}

pub fn append<R: Record>(root: &Path, rows: Vec<R>) -> Result<()> {
    for (day, rows) in by_day(rows) {
        let mut stored = load::<R>(&day_path::<R>(root, day))?;
        stored.extend(rows);
        stored.sort_by_key(Record::at);
        save(root, day, &stored)?;
    }
    Ok(())
}

pub fn update<R: Record + Clone + PartialEq>(
    root: &Path,
    change: impl FnOnce(&mut Vec<R>),
) -> Result<()> {
    let before = by_day(all::<R>(root)?);
    let mut rows: Vec<R> = before.values().flatten().cloned().collect();
    change(&mut rows);
    let mut after = by_day(rows);
    let touched: BTreeSet<u64> = before.keys().chain(after.keys()).copied().collect();
    for day in touched {
        let new = after.remove(&day).unwrap_or_default();
        if before.get(&day).map_or(new.is_empty(), |old| *old == new) {
            continue;
        }
        save(root, day, &new)?;
    }
    Ok(())
}

pub fn expire<R: Record>(root: &Path, keep_seconds: u64, now: u64) -> Result<()> {
    let cutoff = now.saturating_sub(keep_seconds) * 1_000 / DAY_MS;
    for day in days::<R>(root) {
        if day < cutoff {
            fs::remove_file(day_path::<R>(root, day))?;
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
pub struct Summary {
    pub kind: &'static str,
    pub files: usize,
    pub rows: usize,
    pub bytes: u64,
    pub first: Option<u64>,
    pub last: Option<u64>,
}

pub fn summary<R: Record>(root: &Path) -> Summary {
    let found = days::<R>(root);
    let bytes = found
        .iter()
        .filter_map(|day| fs::metadata(day_path::<R>(root, *day)).ok())
        .map(|meta| meta.len())
        .sum();
    let rows = all::<R>(root).unwrap_or_default();
    Summary {
        kind: R::KIND,
        files: found.len(),
        rows: rows.len(),
        bytes,
        first: rows.first().map(Record::at),
        last: rows.last().map(Record::at),
    }
}

pub fn report<R: Record>(root: &Path) -> String {
    let found = days::<R>(root);
    let bytes: u64 = found
        .iter()
        .filter_map(|day| fs::metadata(day_path::<R>(root, *day)).ok())
        .map(|meta| meta.len())
        .sum();
    let rows = all::<R>(root).map_or(0, |rows| rows.len());
    format!(
        "history kind={}: {} daily files, {rows} rows, {bytes} bytes",
        R::KIND,
        found.len()
    )
}

#[cfg(test)]
mod tests {
    use super::{Cell, Field, Record, Type, all, append, expire, millis, newest, read, update};
    use anyhow::{Result, ensure};

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Note {
        at: u64,
        text: String,
        end: Option<u64>,
    }

    impl Record for Note {
        const KIND: &'static str = "note";
        const DESCRIPTION: &'static str = "Test notes";
        const FIELDS: &'static [Field] = &[
            Field {
                name: "ts",
                kind: Type::Time,
                description: "When",
            },
            Field {
                name: "text",
                kind: Type::Text,
                description: "What",
            },
            Field {
                name: "end",
                kind: Type::OptionalTime,
                description: "Until",
            },
        ];

        fn at(&self) -> u64 {
            self.at
        }

        fn cells(&self) -> Vec<Cell> {
            vec![
                millis(self.at),
                Cell::Text(self.text.clone()),
                Cell::Number(self.end.and_then(|end| i64::try_from(end).ok())),
            ]
        }

        fn from_cells(cells: Vec<Cell>) -> Option<Self> {
            let mut cells = cells.into_iter();
            Some(Self {
                at: cells.next()?.millis()?,
                text: cells.next()?.text()?,
                end: cells.next()?.millis(),
            })
        }
    }

    const DAY: u64 = 86_400_000;

    fn note(at: u64, text: &str) -> Note {
        Note {
            at,
            text: text.into(),
            end: None,
        }
    }

    #[test]
    fn records_round_trip_by_day_and_expire() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let base = 20_000 * DAY;
        append(
            root,
            vec![note(base + 5, "one"), note(base + DAY + 5, "two")],
        )?;
        append(root, vec![note(base + 1, "zero")])?;
        let rows: Vec<Note> = all(root)?;
        ensure!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>() == ["zero", "one", "two"],
            "Unexpected rows: {rows:?}"
        );
        ensure!(std::fs::read_dir(root.join("kind=note"))?.count() == 2);
        let later: Vec<Note> = read(root, (base + DAY + 10, base + DAY + 20))?;
        ensure!(later.iter().any(|row| row.text == "two"));
        let last: Vec<Note> = newest(root, 1)?;
        ensure!(last.len() == 1 && last[0].text == "two");
        update::<Note>(root, |rows| {
            for row in rows.iter_mut() {
                row.end = Some(row.at + 1);
            }
        })?;
        let rows: Vec<Note> = all(root)?;
        ensure!(rows.iter().all(|row| row.end == Some(row.at + 1)));
        expire::<Note>(root, 86_400, (base + 2 * DAY) / 1_000 + 10)?;
        let rows: Vec<Note> = all(root)?;
        ensure!(
            rows.len() == 1 && rows[0].text == "two",
            "Expiry kept {rows:?}"
        );
        Ok(())
    }

    #[test]
    fn damaged_files_are_moved_aside() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        append(root, vec![note(20_000 * DAY, "one")])?;
        let file = std::fs::read_dir(root.join("kind=note"))?
            .next()
            .transpose()?
            .map(|entry| entry.path());
        let file = file.ok_or_else(|| anyhow::anyhow!("No file written."))?;
        std::fs::write(&file, b"not parquet")?;
        let rows: Vec<Note> = all(root)?;
        ensure!(rows.is_empty());
        ensure!(file.with_extension("parquet.broken").exists());
        Ok(())
    }

    #[test]
    fn unreadable_files_are_errors_and_stay() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        append(root, vec![note(20_000 * DAY, "one")])?;
        let file = root.join("kind=note").join("20241005.parquet");
        std::fs::create_dir(&file)?;
        ensure!(all::<Note>(root).is_err(), "An unreadable day was ignored.");
        ensure!(file.is_dir() && !file.with_extension("parquet.broken").exists());
        Ok(())
    }
}
