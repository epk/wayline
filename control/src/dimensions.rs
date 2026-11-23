use crate::columnar;
use crate::history;
use crate::router::{self, Inventory};
use crate::runtime;
use crate::settings::Settings;
use crate::telemetry;
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use parquet::basic::{Compression, ZstdLevel};
use parquet::column::reader::ColumnReader;
use parquet::data_type::{ByteArray, ByteArrayType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const FILE: &str = "dimensions.parquet";
pub const ACCESS_POINT: &str = "access_point";
const LIMIT: usize = 4 * 1024 * 1024;
const SCHEMA: &str = "message dimensions { required binary table (UTF8); required binary series (UTF8); required binary kind (UTF8); required binary subject (UTF8); required binary name (UTF8); optional binary target (UTF8); optional binary device (UTF8); required int64 valid_from (TIMESTAMP(MILLIS,true)); optional int64 valid_to (TIMESTAMP(MILLIS,true)); }";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dimension {
    pub table: String,
    pub series: String,
    pub kind: String,
    pub subject: String,
    pub name: String,
    pub target: Option<String>,
    pub device: Option<String>,
    pub valid_from: u64,
    pub valid_to: Option<u64>,
}

impl Dimension {
    fn same_labels(&self, other: &Self) -> bool {
        (
            &self.kind,
            &self.subject,
            &self.name,
            &self.target,
            &self.device,
        ) == (
            &other.kind,
            &other.subject,
            &other.name,
            &other.target,
            &other.device,
        )
    }

    fn key(&self) -> (String, String) {
        (self.table.clone(), self.series.clone())
    }
}

fn row(
    table: &str,
    series: String,
    kind: &str,
    (subject, name): (&str, &str),
    target: Option<String>,
    device: Option<&str>,
) -> Dimension {
    Dimension {
        table: table.into(),
        series,
        kind: kind.into(),
        subject: subject.into(),
        name: name.into(),
        target,
        device: device
            .filter(|device| !device.is_empty())
            .map(str::to_owned),
        valid_from: 0,
        valid_to: None,
    }
}

pub fn current(config: &Settings, inventory: &Inventory) -> Vec<Dimension> {
    let table = history::LATENCY.name;
    let mut found = Vec::new();
    for target in &config.targets {
        let paths = inventory
            .paths
            .iter()
            .map(|path| ("tunnel", path))
            .chain(inventory.wan.iter().map(|path| ("wan", path)))
            .chain(inventory.uplinks.iter().map(|path| ("uplink", path)));
        for (kind, path) in paths {
            found.push(row(
                table,
                path.series(target),
                kind,
                (&path.id, &path.name),
                Some(target.to_string()),
                Some(&path.interface),
            ));
        }
    }
    for path in &inventory.paths {
        found.push(row(
            table,
            router::endpoint_series(&path.key),
            "endpoint",
            (
                &format!("endpoint:{}", path.id),
                &format!("{} server", path.name),
            ),
            path.endpoint.as_ref().map(ToString::to_string),
            inventory.wan.as_ref().map(|wan| wan.interface.as_str()),
        ));
    }
    found.push(row(
        crate::wifi::KIND.name,
        crate::wifi::UPSTREAM.into(),
        "wifi",
        (crate::wifi::SUBJECT, "Upstream Wi-Fi"),
        None,
        None,
    ));
    found.push(row(
        telemetry::SYSTEM.name,
        telemetry::SYSTEM_LINK.into(),
        "system",
        ("system", "Router"),
        None,
        None,
    ));
    for counter in telemetry::counters(inventory) {
        found.push(row(
            telemetry::INTERFACE.name,
            counter.id.clone(),
            &counter.kind,
            (&counter.id, &counter.name),
            None,
            Some(&counter.device),
        ));
    }
    let mut seen = BTreeSet::new();
    found.retain(|item| seen.insert(item.key()));
    found
}

pub fn access_points(status: Option<&wayline_protocol::WifiStatus>) -> Vec<Dimension> {
    let Some(status) = status else {
        return Vec::new();
    };
    let mut found: Vec<Dimension> = status
        .changes
        .iter()
        .map(|change| change.to.clone())
        .chain(status.link.iter().map(wayline_protocol::WifiIdentity::of))
        .filter_map(|identity| {
            let ap = wayline_protocol::access_point_id(&identity.bssid);
            (ap != 0).then(|| {
                let name = if identity.ssid.is_empty() {
                    "a hidden network".to_owned()
                } else {
                    identity.ssid.clone()
                };
                row(
                    crate::wifi::KIND.name,
                    crate::wifi::series_key(ap),
                    ACCESS_POINT,
                    (&identity.bssid, &name),
                    Some(crate::wifi::channel_of(&identity)),
                    None,
                )
            })
        })
        .collect();
    found.reverse();
    let mut seen = BTreeSet::new();
    found.retain(|item| seen.insert(item.key()));
    found
}

fn optional_strings(reader: ColumnReader, rows: usize) -> Result<Vec<Option<String>>> {
    let ColumnReader::ByteArrayColumnReader(mut reader) = reader else {
        anyhow::bail!("The dimension table has a column of the wrong type.");
    };
    let (mut values, mut levels) = (Vec::with_capacity(rows), Vec::with_capacity(rows));
    let mut read = 0;
    while read < rows {
        let (records, _, _) =
            reader.read_records(rows - read, Some(&mut levels), None, &mut values)?;
        ensure!(records > 0, "The dimension table ended early.");
        read += records;
    }
    let mut present = values.into_iter();
    levels
        .into_iter()
        .map(|level| {
            if level > 0 {
                let value: ByteArray = present.next().context("Missing dimension value.")?;
                Ok(Some(value.as_utf8()?.to_owned()))
            } else {
                Ok(None)
            }
        })
        .collect()
}

pub fn read(path: &Path) -> Result<Vec<Dimension>> {
    let reader = SerializedFileReader::new(Bytes::from(wayline_support::read(path, LIMIT)?))?;
    let mut found = Vec::new();
    let millis = |value: Option<i64>| value.and_then(|value| u64::try_from(value).ok());
    for index in 0..reader.num_row_groups() {
        let group = reader.get_row_group(index)?;
        let rows = usize::try_from(group.metadata().num_rows())?;
        let text = |column: usize| columnar::strings(group.get_column_reader(column)?, rows);
        let (tables, series, kinds, subjects, names) =
            (text(0)?, text(1)?, text(2)?, text(3)?, text(4)?);
        let targets = optional_strings(group.get_column_reader(5)?, rows)?;
        let devices = optional_strings(group.get_column_reader(6)?, rows)?;
        let from = columnar::integers(group.get_column_reader(7)?, rows, false)?;
        let to = columnar::integers(group.get_column_reader(8)?, rows, true)?;
        for index in 0..rows {
            found.push(Dimension {
                table: tables[index].clone(),
                series: series[index].clone(),
                kind: kinds[index].clone(),
                subject: subjects[index].clone(),
                name: names[index].clone(),
                target: targets[index].clone(),
                device: devices[index].clone(),
                valid_from: millis(from[index]).unwrap_or(0),
                valid_to: millis(to[index]),
            });
        }
    }
    Ok(found)
}

fn encode(rows: &[Dimension]) -> Result<Vec<u8>> {
    let columns = serde_json::json!([
        {"name": "table", "unit": "text", "description": "Kind directory the series belongs to"},
        {"name": "series", "unit": "text", "description": "Series key in that kind's files"},
        {"name": "kind", "unit": "text", "description": "tunnel, wan, uplink, endpoint, wifi or system"},
        {"name": "subject", "unit": "text", "description": "Identifier used by the page and API"},
        {"name": "name", "unit": "text", "description": "Display name"},
        {"name": "target", "unit": "text", "description": "Probe destination"},
        {"name": "device", "unit": "text", "description": "Network device"},
        {"name": "valid_from", "unit": "timestamp", "description": "UTC time this version took effect"},
        {"name": "valid_to", "unit": "timestamp", "description": "UTC time this version ended; null while current"}
    ]);
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
        .set_created_by("wayline".into())
        .set_key_value_metadata(Some(columnar::metadata("dimensions", &columns, Vec::new())))
        .build();
    let mut out = Vec::new();
    let mut writer = SerializedFileWriter::new(
        &mut out,
        Arc::new(parse_message_type(SCHEMA)?),
        Arc::new(properties),
    )?;
    let mut group = writer.next_row_group()?;
    let required: [fn(&Dimension) -> &str; 5] = [
        |item| &item.table,
        |item| &item.series,
        |item| &item.kind,
        |item| &item.subject,
        |item| &item.name,
    ];
    for read in required {
        let values: Vec<ByteArray> = rows
            .iter()
            .map(|item| ByteArray::from(read(item)))
            .collect();
        let mut column = group.next_column()?.context("Missing dimension column.")?;
        column
            .typed::<ByteArrayType>()
            .write_batch(&values, None, None)?;
        column.close()?;
    }
    let optional: [fn(&Dimension) -> Option<&String>; 2] =
        [|item| item.target.as_ref(), |item| item.device.as_ref()];
    for read in optional {
        let values: Vec<ByteArray> = rows
            .iter()
            .filter_map(read)
            .map(|value| ByteArray::from(value.as_str()))
            .collect();
        let levels: Vec<i16> = rows
            .iter()
            .map(|item| i16::from(read(item).is_some()))
            .collect();
        let mut column = group.next_column()?.context("Missing dimension column.")?;
        column
            .typed::<ByteArrayType>()
            .write_batch(&values, Some(&levels), None)?;
        column.close()?;
    }
    let from = rows
        .iter()
        .map(|item| i64::try_from(item.valid_from))
        .collect::<Result<Vec<_>, _>>()?;
    let mut column = group.next_column()?.context("Missing valid_from column.")?;
    column.typed::<Int64Type>().write_batch(&from, None, None)?;
    column.close()?;
    let to = rows
        .iter()
        .filter_map(|item| item.valid_to)
        .map(i64::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let levels: Vec<i16> = rows
        .iter()
        .map(|item| i16::from(item.valid_to.is_some()))
        .collect();
    let mut column = group.next_column()?.context("Missing valid_to column.")?;
    column
        .typed::<Int64Type>()
        .write_batch(&to, Some(&levels), None)?;
    column.close()?;
    group.close()?;
    writer.close()?;
    Ok(out)
}

pub fn merge(previous: Vec<Dimension>, current: Vec<Dimension>, now: u64) -> Vec<Dimension> {
    let mut rows = previous;
    let mut seen = BTreeSet::new();
    for mut item in current {
        seen.insert(item.key());
        let open = rows
            .iter_mut()
            .find(|row| row.valid_to.is_none() && row.key() == item.key());
        match open {
            Some(row) if row.same_labels(&item) => {}
            Some(row) => {
                row.valid_to = Some(now);
                item.valid_from = now;
                rows.push(item);
            }
            None => {
                item.valid_from = now;
                rows.push(item);
            }
        }
    }
    for row in &mut rows {
        if row.valid_to.is_none() && !seen.contains(&row.key()) && row.kind != ACCESS_POINT {
            row.valid_to = Some(now);
        }
    }
    rows.sort_by(|left, right| {
        (&left.table, &left.series, left.valid_from).cmp(&(
            &right.table,
            &right.series,
            right.valid_from,
        ))
    });
    rows
}

pub fn path(config: &Settings) -> PathBuf {
    history::root(config).join(FILE)
}

pub fn record(config: &Settings, inventory: &Inventory) -> Result<()> {
    runtime::writing("history", || {
        let path = path(config);
        let previous = if path.exists() {
            read(&path).unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut labels = current(config, inventory);
        if config.upstream_wifi {
            labels.extend(access_points(runtime::current().wifi.as_ref()));
        }
        let rows = merge(previous.clone(), labels, runtime::now() * 1_000);
        if rows == previous {
            return Ok(());
        }
        runtime::directory(&history::root(config))?;
        columnar::write(&path, &encode(&rows)?)
    })
}

#[cfg(test)]
mod tests {
    use super::{Dimension, encode, merge, read};
    use anyhow::{Result, ensure};

    fn item(series: &str, name: &str, target: Option<&str>) -> Dimension {
        Dimension {
            table: "latency".into(),
            series: series.into(),
            kind: "tunnel".into(),
            subject: "home".into(),
            name: name.into(),
            target: target.map(str::to_owned),
            device: None,
            valid_from: 0,
            valid_to: None,
        }
    }

    #[test]
    fn access_points_stay_open_out_of_range_and_take_the_newest_label() -> Result<()> {
        use wayline_protocol::{WifiChange, WifiIdentity, WifiState, WifiStatus};
        let identity = |ssid: &str, bssid: &str, channel: u16| WifiIdentity {
            ssid: ssid.into(),
            bssid: bssid.into(),
            channel: Some(channel),
            frequency: Some(5_200),
            width: Some(80),
        };
        let status = WifiStatus {
            state: WifiState::Connected,
            interface: None,
            candidates: Vec::new(),
            sample_age_ms: None,
            link: None,
            recent: Vec::new(),
            buckets: Vec::new(),
            changes: vec![
                WifiChange {
                    at_ms: 1,
                    from: None,
                    to: identity("A", "aa:00:00:00:00:01", 36),
                },
                WifiChange {
                    at_ms: 2,
                    from: None,
                    to: identity("B", "bb:00:00:00:00:01", 40),
                },
                WifiChange {
                    at_ms: 3,
                    from: None,
                    to: identity("A", "aa:00:00:00:00:01", 44),
                },
            ],
        };
        let found = super::access_points(Some(&status));
        ensure!(found.len() == 2, "Expected two access points: {found:?}");
        let a = found
            .iter()
            .find(|row| row.series == "ap:aa0000000001")
            .ok_or_else(|| anyhow::anyhow!("missing A"))?;
        ensure!(a.name == "A" && a.target.as_deref() == Some("44 · 5 GHz · 80 MHz"));
        let first = merge(Vec::new(), found, 1_000);
        let later = merge(first.clone(), Vec::new(), 2_000);
        ensure!(later == first, "Out-of-range access points were closed.");
        Ok(())
    }

    #[test]
    fn renames_close_old_versions_and_removals_end_them() -> Result<()> {
        let initial = || {
            vec![
                item("old", "Old tunnel", None),
                item("a", "Before", Some("1.1.1.1")),
            ]
        };
        let first = merge(Vec::new(), initial(), 1_000);
        ensure!(
            first
                .iter()
                .all(|row| row.valid_from == 1_000 && row.valid_to.is_none())
        );
        ensure!(
            merge(first.clone(), initial(), 2_000) == first,
            "Unchanged labels created a new version."
        );
        let rows = merge(
            first,
            vec![item("a", "After", Some("1.1.1.1")), item("b", "New", None)],
            3_000,
        );
        ensure!(rows.len() == 4, "Unexpected rows: {}", rows.len());
        let versions: Vec<&Dimension> = rows.iter().filter(|row| row.series == "a").collect();
        ensure!(versions[0].name == "Before" && versions[0].valid_to == Some(3_000));
        ensure!(
            versions[1].name == "After"
                && versions[1].valid_from == 3_000
                && versions[1].valid_to.is_none()
        );
        ensure!(
            rows.iter()
                .any(|row| row.series == "old" && row.valid_to == Some(3_000))
        );
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("dimensions.parquet");
        std::fs::write(&path, encode(&rows)?)?;
        ensure!(
            read(&path)? == rows,
            "The dimension table did not round trip."
        );
        Ok(())
    }
}
