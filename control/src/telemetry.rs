use crate::columnar::{self, Entry, Fold, Kind, Row};
use crate::history;
use crate::router::Inventory;
use crate::runtime;
use crate::settings::Settings;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wayline_protocol::{Counter, LinkBucket, SystemBucket, TelemetryStatus};

pub const INTERVAL_MS: u64 = 5_000;
const SYSTEM_LINK: &str = "system";

pub static SYSTEM: Kind = Kind {
    name: "system",
    fields: &[
        ("samples", Fold::Sum),
        ("cpu_n", Fold::Sum),
        ("cpu_sum", Fold::Sum),
        ("cpu_max", Fold::Max),
        ("load_sum", Fold::Sum),
        ("mem_n", Fold::Sum),
        ("mem_used_sum", Fold::Sum),
        ("mem_available_min", Fold::Min),
        ("temp_n", Fold::Sum),
        ("temp_sum", Fold::Sum),
        ("temp_max", Fold::Max),
        ("conntrack_max", Fold::Max),
        ("conntrack_limit", Fold::Max),
    ],
};

pub static INTERFACE: Kind = Kind {
    name: "interface",
    fields: &[
        ("samples", Fold::Sum),
        ("seconds_ms", Fold::Sum),
        ("rx_bytes", Fold::Sum),
        ("tx_bytes", Fold::Sum),
        ("rx_packets", Fold::Sum),
        ("tx_packets", Fold::Sum),
        ("errors", Fold::Sum),
        ("drops", Fold::Sum),
        ("rx_peak_bps", Fold::Max),
        ("tx_peak_bps", Fold::Max),
        ("handshake_age_max", Fold::Max),
    ],
};

fn value(value: impl TryInto<i64>) -> Option<i64> {
    value.try_into().ok().or(Some(i64::MAX))
}

fn optional(value: Option<impl TryInto<i64>>) -> Option<i64> {
    value.and_then(|value| value.try_into().ok())
}

fn system_row(bucket: &SystemBucket) -> Row {
    vec![
        value(bucket.samples),
        value(bucket.cpu_n),
        value(bucket.cpu_sum),
        optional(bucket.cpu_max),
        value(bucket.load_sum),
        value(bucket.mem_n),
        value(bucket.mem_used_sum),
        optional(bucket.mem_available_min),
        value(bucket.temp_n),
        value(bucket.temp_sum),
        optional(bucket.temp_max),
        optional(bucket.conntrack_max),
        optional(bucket.conntrack_limit),
    ]
}

fn link_row(bucket: &LinkBucket) -> Row {
    vec![
        value(bucket.samples),
        value(bucket.seconds_ms),
        value(bucket.rx_bytes),
        value(bucket.tx_bytes),
        value(bucket.rx_packets),
        value(bucket.tx_packets),
        value(bucket.errors),
        value(bucket.drops),
        optional(bucket.rx_peak_bps),
        optional(bucket.tx_peak_bps),
        optional(bucket.handshake_age_max),
    ]
}

pub fn system_entries(status: &TelemetryStatus) -> Vec<Entry> {
    status
        .system_buckets
        .iter()
        .map(|bucket| Entry {
            link: SYSTEM_LINK.into(),
            at: bucket.at,
            row: system_row(bucket),
        })
        .collect()
}

pub fn link_entries(status: &TelemetryStatus) -> Vec<Entry> {
    status
        .link_buckets
        .iter()
        .flat_map(|link| {
            link.buckets.iter().map(|bucket| Entry {
                link: link.id.clone(),
                at: bucket.at,
                row: link_row(bucket),
            })
        })
        .collect()
}

pub fn counters(inventory: &Inventory) -> Vec<Counter> {
    let mut seen = std::collections::BTreeSet::new();
    inventory
        .uplinks
        .iter()
        .map(|uplink| Counter {
            id: uplink.id.clone(),
            name: uplink.name.clone(),
            device: uplink.interface.clone(),
            kind: "uplink".into(),
        })
        .chain(inventory.paths.iter().map(|path| Counter {
            id: path.id.clone(),
            name: path.name.clone(),
            device: path.interface.clone(),
            kind: "tunnel".into(),
        }))
        .filter(|counter| seen.insert(counter.device.clone()))
        .take(32)
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemView {
    cpu: Option<f64>,
    load: [f64; 3],
    mem_total: u64,
    mem_available: u64,
    temperature: Option<f64>,
    conntrack: Option<u32>,
    conntrack_max: Option<u32>,
    uptime: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceView {
    id: String,
    name: String,
    device: String,
    kind: String,
    present: bool,
    rx_bps: Option<u64>,
    tx_bps: Option<u64>,
    rx_bytes: u64,
    tx_bytes: u64,
    errors: u64,
    drops: u64,
    handshake_age: Option<u64>,
    endpoint: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    enabled: bool,
    sample_age_ms: Option<u64>,
    interval: u64,
    system: Option<SystemView>,
    interfaces: Vec<InterfaceView>,
}

fn tenths(permille: u32) -> f64 {
    f64::from(permille) / 10.0
}

fn hundredths(centi: u32) -> f64 {
    f64::from(centi) / 100.0
}

pub fn view(config: &Settings, status: Option<TelemetryStatus>) -> View {
    let status = status.filter(|_| config.telemetry);
    View {
        enabled: config.telemetry,
        sample_age_ms: status.as_ref().and_then(|status| status.sample_age_ms),
        interval: INTERVAL_MS / 1_000,
        system: status
            .as_ref()
            .and_then(|status| status.system)
            .map(|system| SystemView {
                cpu: system.cpu_permille.map(tenths),
                load: system.load_centi.map(hundredths),
                mem_total: system.mem_total,
                mem_available: system.mem_available,
                temperature: system
                    .temperature_milli
                    .map(|milli| f64::from(milli) / 1_000.0),
                conntrack: system.conntrack,
                conntrack_max: system.conntrack_max,
                uptime: system.uptime,
            }),
        interfaces: status
            .map(|status| {
                status
                    .links
                    .into_iter()
                    .map(|link| InterfaceView {
                        id: link.id,
                        name: link.name,
                        device: link.device,
                        kind: link.kind,
                        present: link.present,
                        rx_bps: link.rx_bps,
                        tx_bps: link.tx_bps,
                        rx_bytes: link.rx_bytes,
                        tx_bytes: link.tx_bytes,
                        errors: link.errors,
                        drops: link.drops,
                        handshake_age: link.handshake_age,
                        endpoint: link.endpoint,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

pub const fn disabled() -> View {
    View {
        enabled: false,
        sample_age_ms: None,
        interval: INTERVAL_MS / 1_000,
        system: None,
        interfaces: Vec::new(),
    }
}

#[derive(Deserialize)]
pub struct Query {
    start: u64,
    end: u64,
    step: u64,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemColumns {
    cpu: Vec<Option<f64>>,
    cpu_max: Vec<Option<f64>>,
    load: Vec<Option<f64>>,
    mem_used: Vec<Option<f64>>,
    mem_available_min: Vec<Option<i64>>,
    temperature: Vec<Option<f64>>,
    temperature_max: Vec<Option<f64>>,
    conntrack: Vec<Option<i64>>,
    conntrack_max: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceColumns {
    id: String,
    name: String,
    device: String,
    kind: String,
    rx_bps: Vec<Option<i64>>,
    tx_bps: Vec<Option<i64>>,
    rx_peak_bps: Vec<Option<i64>>,
    tx_peak_bps: Vec<Option<i64>>,
    rx_bytes: Vec<Option<i64>>,
    tx_bytes: Vec<Option<i64>>,
    errors: Vec<Option<i64>>,
    drops: Vec<Option<i64>>,
    handshake_age_max: Vec<Option<i64>>,
}

#[derive(Serialize)]
pub struct Response {
    start: u64,
    step: u64,
    count: usize,
    system: SystemColumns,
    interfaces: Vec<InterfaceColumns>,
}

impl crate::encoding::Document for Response {
    fn frame(&self) -> Result<crate::encoding::Frame> {
        use crate::encoding::Column;
        let mut meta = serde_json::Map::new();
        meta.insert("start".into(), self.start.into());
        meta.insert("step".into(), self.step.into());
        meta.insert("count".into(), self.count.into());
        meta.insert(
            "interfaces".into(),
            self.interfaces
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "id": item.id, "name": item.name, "device": item.device, "kind": item.kind
                    })
                })
                .collect(),
        );
        meta.insert(
            "conntrackMax".into(),
            serde_json::to_value(self.system.conntrack_max)?,
        );
        let s = &self.system;
        let mut columns = vec![
            ("system.cpu".to_owned(), Column::F64(s.cpu.clone())),
            ("system.cpuMax".to_owned(), Column::F64(s.cpu_max.clone())),
            ("system.load".to_owned(), Column::F64(s.load.clone())),
            ("system.memUsed".to_owned(), Column::F64(s.mem_used.clone())),
            (
                "system.temperature".to_owned(),
                Column::F64(s.temperature.clone()),
            ),
            (
                "system.temperatureMax".to_owned(),
                Column::F64(s.temperature_max.clone()),
            ),
            (
                "system.memAvailableMin".to_owned(),
                Column::I64(s.mem_available_min.clone()),
            ),
            (
                "system.conntrack".to_owned(),
                Column::I64(s.conntrack.clone()),
            ),
        ];
        for (index, item) in self.interfaces.iter().enumerate() {
            for (name, values) in [
                ("rxBps", &item.rx_bps),
                ("txBps", &item.tx_bps),
                ("rxPeakBps", &item.rx_peak_bps),
                ("txPeakBps", &item.tx_peak_bps),
                ("rxBytes", &item.rx_bytes),
                ("txBytes", &item.tx_bytes),
                ("errors", &item.errors),
                ("drops", &item.drops),
                ("handshakeAgeMax", &item.handshake_age_max),
            ] {
                columns.push((format!("{index}.{name}"), Column::I64(values.clone())));
            }
        }
        Ok(crate::encoding::Frame { meta, columns })
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Telemetry sums stay far below the 53-bit mantissa."
)]
const fn float(value: i64) -> f64 {
    value as f64
}

fn ratio(sum: Option<i64>, count: Option<i64>, scale: f64) -> Option<f64> {
    let count = count.filter(|count| *count > 0)?;
    Some((float(sum?) / float(count) / scale * 100.0).round() / 100.0)
}

fn system_columns(rows: &BTreeMap<u64, Row>, first: u64, last: u64) -> SystemColumns {
    let mut columns = SystemColumns::default();
    for period in first..=last {
        let row = rows.get(&period);
        let cell = |index: usize| row.and_then(|row| row.get(index).copied().flatten());
        columns.cpu.push(ratio(cell(2), cell(1), 10.0));
        columns
            .cpu_max
            .push(cell(3).map(|value| float(value) / 10.0));
        columns.load.push(ratio(cell(4), cell(0), 100.0));
        columns.mem_used.push(ratio(cell(6), cell(5), 10.0));
        columns.mem_available_min.push(cell(7));
        columns.temperature.push(ratio(cell(9), cell(8), 1_000.0));
        columns
            .temperature_max
            .push(cell(10).map(|value| float(value) / 1_000.0));
        columns.conntrack.push(cell(11));
        if let Some(limit) = cell(12) {
            columns.conntrack_max = Some(limit);
        }
    }
    columns
}

fn rate(bytes: Option<i64>, milliseconds: Option<i64>) -> Option<i64> {
    let milliseconds = milliseconds.filter(|value| *value > 0)?;
    Some(bytes?.saturating_mul(8_000) / milliseconds)
}

fn interface_columns(
    counter: Counter,
    rows: Option<&BTreeMap<u64, Row>>,
    first: u64,
    last: u64,
) -> InterfaceColumns {
    let mut columns = InterfaceColumns {
        id: counter.id,
        name: counter.name,
        device: counter.device,
        kind: counter.kind,
        rx_bps: Vec::new(),
        tx_bps: Vec::new(),
        rx_peak_bps: Vec::new(),
        tx_peak_bps: Vec::new(),
        rx_bytes: Vec::new(),
        tx_bytes: Vec::new(),
        errors: Vec::new(),
        drops: Vec::new(),
        handshake_age_max: Vec::new(),
    };
    for period in first..=last {
        let record = rows.and_then(|rows| rows.get(&period));
        let cell = |index: usize| record.and_then(|record| record.get(index).copied().flatten());
        let sampled = cell(1).is_some_and(|value| value > 0);
        let tallied = |index: usize| cell(index).filter(|_| sampled);
        columns.rx_bps.push(rate(cell(2), cell(1)));
        columns.tx_bps.push(rate(cell(3), cell(1)));
        columns.rx_peak_bps.push(cell(8));
        columns.tx_peak_bps.push(cell(9));
        columns.rx_bytes.push(tallied(2));
        columns.tx_bytes.push(tallied(3));
        columns.errors.push(tallied(6));
        columns.drops.push(tallied(7));
        columns.handshake_age_max.push(cell(10));
    }
    columns
}

pub fn query(config: &Settings, inventory: &Inventory, query: &Query) -> Result<Response> {
    runtime::locked("history", || query_inner(config, inventory, query))
}

fn query_inner(config: &Settings, inventory: &Inventory, query: &Query) -> Result<Response> {
    let step = history::resolution(
        config.history_days,
        query.step,
        query.start,
        query.end,
        runtime::now() * 1_000,
    )?;
    let (first, last) = (query.start / 1_000 / step, query.end / 1_000 / step);
    let path = history::root(config);
    let (_, _, status) = history::fresh_all()?;
    let system_live = status.as_ref().map(system_entries).unwrap_or_default();
    let link_live = status.as_ref().map(link_entries).unwrap_or_default();
    let mut system = columnar::series(&path, &SYSTEM, step, (first, last), &system_live)?;
    let mut links = columnar::series(&path, &INTERFACE, step, (first, last), &link_live)?;
    let mut known = counters(inventory);
    for id in links.keys() {
        if !known.iter().any(|counter| &counter.id == id) {
            known.push(Counter {
                id: id.clone(),
                name: id.clone(),
                device: String::new(),
                kind: if id.starts_with("uplink:") {
                    "uplink".into()
                } else {
                    "tunnel".into()
                },
            });
        }
    }
    let interfaces = known
        .into_iter()
        .filter_map(|counter| {
            let rows = links.remove(&counter.id)?;
            Some(interface_columns(counter, Some(&rows), first, last))
        })
        .collect();
    Ok(Response {
        start: first * step * 1_000,
        step,
        count: usize::try_from(last - first + 1)?,
        system: system_columns(&system.remove(SYSTEM_LINK).unwrap_or_default(), first, last),
        interfaces,
    })
}

#[cfg(test)]
mod tests {
    use super::{INTERFACE, SYSTEM, link_row, rate, system_columns, system_row};
    use crate::columnar;
    use anyhow::{Result, ensure};
    use std::collections::BTreeMap;
    use wayline_protocol::{LinkBucket, SystemBucket};

    #[test]
    fn rows_match_their_kinds() {
        assert_eq!(
            system_row(&SystemBucket::default()).len(),
            SYSTEM.fields.len()
        );
        assert_eq!(
            link_row(&LinkBucket::default()).len(),
            INTERFACE.fields.len()
        );
        assert_eq!(rate(Some(125_000), Some(5_000)), Some(200_000));
        assert_eq!(rate(Some(1), Some(0)), None);
    }

    #[test]
    fn system_rows_store_and_render() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().to_path_buf();
        let now = crate::runtime::now() / 60 * 60 - 120;
        let bucket = SystemBucket {
            at: now,
            samples: 3,
            cpu_n: 2,
            cpu_sum: 500,
            cpu_max: Some(400),
            load_sum: 63,
            mem_n: 3,
            mem_used_sum: 2_250,
            mem_available_min: Some(250_000_000),
            temp_n: 3,
            temp_sum: 165_000,
            temp_max: Some(58_000),
            conntrack_max: Some(400),
            conntrack_limit: Some(16_384),
        };
        let entries = vec![columnar::Entry {
            link: "system".into(),
            at: now,
            row: system_row(&bucket),
        }];
        columnar::store(&database, 7, &SYSTEM, &entries, crate::runtime::now())?;
        let found = columnar::series(&database, &SYSTEM, 60, (now / 60, now / 60), &[])?;
        let rows = found.get("system").cloned().unwrap_or_else(BTreeMap::new);
        let columns = system_columns(&rows, now / 60, now / 60);
        ensure!(columns.cpu == [Some(25.0)], "{:?}", columns.cpu);
        ensure!(columns.cpu_max == [Some(40.0)]);
        ensure!(columns.load == [Some(0.21)]);
        ensure!(columns.mem_used == [Some(75.0)]);
        ensure!(columns.temperature == [Some(55.0)]);
        ensure!(columns.conntrack_max == Some(16_384));
        Ok(())
    }
}
