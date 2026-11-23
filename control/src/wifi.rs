use crate::changes::{self, Event};
use crate::columnar::{self, Entry, Row};
use crate::history;
use crate::router::Inventory;
use crate::runtime;
use crate::settings::Settings;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wayline_protocol::{WifiBucket, WifiLink, WifiSample, WifiState, WifiStatus};

pub const UPSTREAM: &str = "upstream";
pub const SUBJECT: &str = "wifi";

pub use wayline_protocol::schema::WIFI as KIND;

#[derive(Deserialize)]
pub struct Query {
    start: u64,
    end: u64,
    step: u64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    samples: u32,
    uptime: Option<f64>,
    signal: Option<f64>,
    signal_min: Option<i64>,
    signal_max: Option<i64>,
    noise: Option<f64>,
    snr: Option<f64>,
    tx_rate: Option<f64>,
    tx_rate_min: Option<f64>,
    rx_rate: Option<f64>,
    expected: Option<f64>,
    retries: Option<f64>,
    failures: Option<f64>,
    busy: Option<f64>,
    rx_air: Option<f64>,
    tx_air: Option<f64>,
    upload: Option<f64>,
    download: Option<f64>,
    tx_bytes: u64,
    rx_bytes: u64,
    disconnects: u32,
    roams: u32,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Columns {
    samples: Vec<u32>,
    uptime: Vec<Option<f64>>,
    signal: Vec<Option<f64>>,
    signal_min: Vec<Option<i64>>,
    signal_max: Vec<Option<i64>>,
    noise: Vec<Option<f64>>,
    snr: Vec<Option<f64>>,
    tx_rate: Vec<Option<f64>>,
    tx_rate_min: Vec<Option<f64>>,
    rx_rate: Vec<Option<f64>>,
    expected: Vec<Option<f64>>,
    retries: Vec<Option<f64>>,
    failures: Vec<Option<f64>>,
    busy: Vec<Option<f64>>,
    rx_air: Vec<Option<f64>>,
    tx_air: Vec<Option<f64>>,
    upload: Vec<Option<f64>>,
    download: Vec<Option<f64>>,
    tx_bytes: Vec<u64>,
    rx_bytes: Vec<u64>,
    disconnects: Vec<u32>,
    roams: Vec<u32>,
}

impl Columns {
    fn push(&mut self, point: &Point) {
        self.samples.push(point.samples);
        self.uptime.push(point.uptime);
        self.signal.push(point.signal);
        self.signal_min.push(point.signal_min);
        self.signal_max.push(point.signal_max);
        self.noise.push(point.noise);
        self.snr.push(point.snr);
        self.tx_rate.push(point.tx_rate);
        self.tx_rate_min.push(point.tx_rate_min);
        self.rx_rate.push(point.rx_rate);
        self.expected.push(point.expected);
        self.retries.push(point.retries);
        self.failures.push(point.failures);
        self.busy.push(point.busy);
        self.rx_air.push(point.rx_air);
        self.tx_air.push(point.tx_air);
        self.upload.push(point.upload);
        self.download.push(point.download);
        self.tx_bytes.push(point.tx_bytes);
        self.rx_bytes.push(point.rx_bytes);
        self.disconnects.push(point.disconnects);
        self.roams.push(point.roams);
    }
}

impl crate::encoding::Document for Response {
    fn frame(&self) -> Result<crate::encoding::Frame> {
        use crate::encoding::Column;
        let mut meta = serde_json::Map::new();
        meta.insert("start".into(), self.start.into());
        meta.insert("step".into(), self.step.into());
        meta.insert("count".into(), self.count.into());
        meta.insert("summary".into(), serde_json::to_value(&self.summary)?);
        meta.insert("events".into(), serde_json::to_value(&self.events)?);
        meta.insert("networks".into(), serde_json::to_value(&self.networks)?);
        let c = &self.columns;
        let float =
            |name: &str, values: &[Option<f64>]| (name.to_owned(), Column::F64(values.to_vec()));
        let level = |name: &str, values: &[Option<i64>]| {
            (
                name.to_owned(),
                Column::I32(
                    values
                        .iter()
                        .map(|value| value.and_then(|value| i32::try_from(value).ok()))
                        .collect(),
                ),
            )
        };
        let bytes = |name: &str, values: &[u64]| {
            (
                name.to_owned(),
                Column::I64(
                    values
                        .iter()
                        .map(|value| i64::try_from(*value).ok())
                        .collect(),
                ),
            )
        };
        Ok(crate::encoding::Frame {
            meta,
            columns: vec![
                ("samples".into(), Column::U32(c.samples.clone())),
                float("uptime", &c.uptime),
                float("signal", &c.signal),
                level("signalMin", &c.signal_min),
                level("signalMax", &c.signal_max),
                float("noise", &c.noise),
                float("snr", &c.snr),
                float("txRate", &c.tx_rate),
                float("txRateMin", &c.tx_rate_min),
                float("rxRate", &c.rx_rate),
                float("expected", &c.expected),
                float("retries", &c.retries),
                float("failures", &c.failures),
                float("busy", &c.busy),
                float("rxAir", &c.rx_air),
                float("txAir", &c.tx_air),
                float("upload", &c.upload),
                float("download", &c.download),
                bytes("txBytes", &c.tx_bytes),
                bytes("rxBytes", &c.rx_bytes),
                ("disconnects".into(), Column::U32(c.disconnects.clone())),
                ("roams".into(), Column::U32(c.roams.clone())),
            ],
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    id: String,
    bssid: Option<String>,
    ssid: Option<String>,
    channel: Option<String>,
    connected_seconds: u64,
    first: u64,
    last: u64,
    summary: Point,
}

#[derive(Serialize)]
pub struct Response {
    start: u64,
    step: u64,
    count: usize,
    summary: Point,
    columns: Columns,
    events: Vec<Event>,
    networks: Vec<Network>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    enabled: bool,
    state: Option<WifiState>,
    reason: String,
    interface: Option<String>,
    candidates: Vec<String>,
    sample_age_ms: Option<u64>,
    interval: u64,
    link: Option<WifiLink>,
    recent: Vec<WifiSample>,
    uplink: Option<String>,
    history_available: bool,
}

fn value(value: impl TryInto<i64>) -> Option<i64> {
    value.try_into().ok().or(Some(i64::MAX))
}

pub fn row(bucket: &WifiBucket) -> Row {
    vec![
        value(bucket.samples),
        value(bucket.connected),
        value(bucket.signal_n),
        value(bucket.signal_sum),
        bucket.signal_min.map(i64::from),
        bucket.signal_max.map(i64::from),
        value(bucket.noise_n),
        value(bucket.noise_sum),
        value(bucket.tx_rate_n),
        value(bucket.tx_rate_sum),
        bucket.tx_rate_min.map(i64::from),
        value(bucket.rx_rate_n),
        value(bucket.rx_rate_sum),
        value(bucket.expected_n),
        value(bucket.expected_sum),
        value(bucket.frames),
        value(bucket.retries),
        value(bucket.failed),
        value(bucket.tx_bytes),
        value(bucket.rx_bytes),
        value(bucket.active_ms),
        value(bucket.busy_ms),
        value(bucket.rx_ms),
        value(bucket.tx_ms),
        value(bucket.disconnects),
        value(bucket.roams),
    ]
}

fn bucket(at: u64, row: &[Option<i64>]) -> WifiBucket {
    let cell = |index: usize| row.get(index).copied().flatten();
    let big = |index: usize| {
        cell(index)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0)
    };
    let small = |index: usize| u32::try_from(big(index)).unwrap_or(u32::MAX);
    WifiBucket {
        samples: small(0),
        connected: small(1),
        signal_n: small(2),
        signal_sum: cell(3).unwrap_or(0),
        signal_min: cell(4).and_then(|value| i16::try_from(value).ok()),
        signal_max: cell(5).and_then(|value| i16::try_from(value).ok()),
        noise_n: small(6),
        noise_sum: cell(7).unwrap_or(0),
        tx_rate_n: small(8),
        tx_rate_sum: big(9),
        tx_rate_min: cell(10).and_then(|value| u32::try_from(value).ok()),
        rx_rate_n: small(11),
        rx_rate_sum: big(12),
        expected_n: small(13),
        expected_sum: big(14),
        frames: big(15),
        retries: big(16),
        failed: big(17),
        tx_bytes: big(18),
        rx_bytes: big(19),
        active_ms: big(20),
        busy_ms: big(21),
        rx_ms: big(22),
        tx_ms: big(23),
        disconnects: small(24),
        roams: small(25),
        ..WifiBucket::empty(at)
    }
}

pub fn series_key(ap: u64) -> String {
    if ap == 0 {
        UPSTREAM.into()
    } else {
        format!("ap:{ap:012x}")
    }
}

pub fn bssid_of(key: &str) -> Option<String> {
    let hex = key.strip_prefix("ap:")?;
    (hex.len() == 12 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| {
        hex.as_bytes()
            .chunks(2)
            .map(|pair| String::from_utf8_lossy(pair).into_owned())
            .collect::<Vec<_>>()
            .join(":")
    })
}

pub fn entries(buckets: &[WifiBucket]) -> Vec<Entry> {
    buckets
        .iter()
        .map(|item| Entry {
            link: series_key(item.ap),
            at: item.at,
            row: row(item),
        })
        .collect()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Wi-Fi counters stay far below the 53-bit mantissa."
)]
const fn float(value: u64) -> f64 {
    value as f64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Signal sums stay far below the 53-bit mantissa."
)]
const fn signed(value: i64) -> f64 {
    value as f64
}

fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

fn average(sum: f64, count: u32) -> Option<f64> {
    (count > 0).then(|| sum / f64::from(count))
}

fn share(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| rounded(float(part) * 100.0 / float(whole), 2))
}

fn megabits(kbps: f64) -> f64 {
    rounded(kbps / 1_000.0, 1)
}

fn point(item: Option<&WifiBucket>, seconds: u64) -> Point {
    let Some(item) = item.filter(|item| item.samples > 0) else {
        return Point::default();
    };
    let signal = average(signed(item.signal_sum), item.signal_n);
    let noise = average(signed(item.noise_sum), item.noise_n);
    let traffic = |bytes: u64| {
        (item.connected > 0 && seconds > 0)
            .then(|| rounded(float(bytes) * 8.0 / float(seconds) / 1_000_000.0, 3))
    };
    Point {
        samples: item.samples,
        uptime: share(u64::from(item.connected), u64::from(item.samples)),
        signal: signal.map(|value| rounded(value, 1)),
        signal_min: item.signal_min.map(i64::from),
        signal_max: item.signal_max.map(i64::from),
        noise: noise.map(|value| rounded(value, 1)),
        snr: signal
            .zip(noise)
            .map(|(signal, noise)| rounded(signal - noise, 1)),
        tx_rate: average(float(item.tx_rate_sum), item.tx_rate_n).map(megabits),
        tx_rate_min: item.tx_rate_min.map(f64::from).map(megabits),
        rx_rate: average(float(item.rx_rate_sum), item.rx_rate_n).map(megabits),
        expected: average(float(item.expected_sum), item.expected_n).map(megabits),
        retries: share(item.retries, item.frames.saturating_add(item.retries)),
        failures: share(item.failed, item.frames),
        busy: share(item.busy_ms, item.active_ms),
        rx_air: share(item.rx_ms, item.active_ms),
        tx_air: share(item.tx_ms, item.active_ms),
        upload: traffic(item.tx_bytes),
        download: traffic(item.rx_bytes),
        tx_bytes: item.tx_bytes,
        rx_bytes: item.rx_bytes,
        disconnects: item.disconnects,
        roams: item.roams,
    }
}

fn periods(
    config: &Settings,
    step: u64,
    first: u64,
    last: u64,
    live: &[WifiBucket],
) -> Result<BTreeMap<String, BTreeMap<u64, WifiBucket>>> {
    let found = columnar::series(
        &history::root(config),
        &KIND,
        step,
        (first, last),
        &entries(live),
    )?;
    Ok(found
        .into_iter()
        .map(|(key, rows)| {
            let buckets = rows
                .into_iter()
                .map(|(period, row)| (period, bucket(period, &row)))
                .collect();
            (key, buckets)
        })
        .collect())
}

struct Label {
    ssid: String,
    channel: Option<String>,
    bssid: String,
    from: u64,
}

fn access_point_names(config: &Settings) -> BTreeMap<String, Label> {
    let path = crate::dimensions::path(config);
    let rows = if path.exists() {
        crate::dimensions::read(&path).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut names: BTreeMap<String, Label> = BTreeMap::new();
    for row in rows
        .into_iter()
        .filter(|row| row.table == KIND.name && row.kind == crate::dimensions::ACCESS_POINT)
    {
        if names
            .get(&row.series)
            .is_none_or(|label| row.valid_from >= label.from)
        {
            names.insert(
                row.series,
                Label {
                    ssid: row.name,
                    channel: row.target,
                    bssid: row.subject,
                    from: row.valid_from,
                },
            );
        }
    }
    names
}

fn networks(
    config: &Settings,
    step: u64,
    by_series: &BTreeMap<String, BTreeMap<u64, WifiBucket>>,
) -> Vec<Network> {
    let names = access_point_names(config);
    let mut found: Vec<Network> = by_series
        .iter()
        .filter_map(|(key, buckets)| {
            let used: Vec<(&u64, &WifiBucket)> = buckets
                .iter()
                .filter(|(_, item)| item.samples > 0)
                .collect();
            let (first, last) = (*used.first()?.0, *used.last()?.0);
            let mut total = WifiBucket::empty(first);
            for (_, item) in &used {
                total.merge(item);
            }
            let seconds = u64::from(total.connected) * crate::router::WIFI_INTERVAL_MS / 1_000;
            let label = names.get(key);
            Some(Network {
                id: key.clone(),
                bssid: label
                    .map(|label| label.bssid.clone())
                    .or_else(|| bssid_of(key)),
                ssid: label.map(|label| label.ssid.clone()),
                channel: label.and_then(|label| label.channel.clone()),
                connected_seconds: seconds,
                first: first * step * 1_000,
                last: last * step * 1_000,
                summary: point(Some(&total), seconds.max(1)),
            })
        })
        .collect();
    found.sort_by(|left, right| {
        right
            .connected_seconds
            .cmp(&left.connected_seconds)
            .then_with(|| left.id.cmp(&right.id))
    });
    found
}

pub fn query(config: &Settings, query: &Query) -> Result<Response> {
    runtime::reading("history", || query_inner(config, query))
}

fn query_inner(config: &Settings, query: &Query) -> Result<Response> {
    let step = history::resolution(
        config.history_days,
        query.step,
        query.start,
        query.end,
        runtime::now() * 1_000,
    )?;
    let (first, last) = (query.start / 1_000 / step, query.end / 1_000 / step);
    let (_, live) = history::fresh()?;
    let by_series = periods(config, step, first, last, &live)?;
    let mut periods: BTreeMap<u64, WifiBucket> = BTreeMap::new();
    for buckets in by_series.values() {
        for (period, item) in buckets {
            periods
                .entry(*period)
                .or_insert_with(|| WifiBucket::empty(*period))
                .merge(item);
        }
    }
    let mut total = WifiBucket::empty(first);
    for item in periods.values() {
        total.merge(item);
    }
    let mut columns = Columns::default();
    for period in first..=last {
        columns.push(&point(periods.get(&period), step));
    }
    let events = changes::read(config, (query.start, query.end))
        .into_iter()
        .filter(|event| event.subject == SUBJECT)
        .collect();
    Ok(Response {
        start: first * step * 1_000,
        step,
        count: columns.samples.len(),
        summary: point(Some(&total), (last - first + 1) * step),
        columns,
        events,
        networks: networks(config, step, &by_series),
    })
}

pub fn down(buckets: &[WifiBucket]) -> impl Iterator<Item = (u64, Option<bool>)> + '_ {
    buckets
        .iter()
        .map(|item| (item.at, (item.samples > 0).then_some(item.connected == 0)))
}

const fn band(frequency: u32) -> &'static str {
    match frequency {
        0..3_000 => "2.4 GHz",
        3_000..5_925 => "5 GHz",
        _ => "6 GHz",
    }
}

pub fn channel(link: &WifiLink) -> String {
    channel_of(&wayline_protocol::WifiIdentity::of(link))
}

pub fn channel_of(link: &wayline_protocol::WifiIdentity) -> String {
    let mut parts = Vec::new();
    if let Some(channel) = link.channel {
        parts.push(channel.to_string());
    }
    if let Some(frequency) = link.frequency {
        parts.push(band(frequency).into());
    }
    if let Some(width) = link.width {
        parts.push(format!("{width} MHz"));
    }
    if parts.is_empty() {
        "unknown".into()
    } else {
        parts.join(" · ")
    }
}

fn reason(enabled: bool, status: Option<&WifiStatus>) -> String {
    if !enabled {
        return "Upstream Wi-Fi monitoring is off.".into();
    }
    let Some(status) = status else {
        return "The monitor is not running yet.".into();
    };
    match status.state {
        WifiState::Starting => "The monitor is starting.".into(),
        WifiState::QueryFailed => "The kernel's Wi-Fi interface (nl80211) did not answer.".into(),
        WifiState::NoClientInterface => {
            "The router has no Wi-Fi client interface. Connect to a Wi-Fi network as a repeater in the router's Internet page.".into()
        }
        WifiState::Disconnected => format!(
            "The repeater interface {} is not connected to a Wi-Fi network.",
            status.interface.as_deref().unwrap_or("unknown")
        ),
        WifiState::Connected => String::new(),
    }
}

pub fn view(config: &Settings, inventory: Option<&Inventory>, status: Option<WifiStatus>) -> View {
    let enabled = config.upstream_wifi;
    let status = status.filter(|_| enabled);
    let interface = status.as_ref().and_then(|status| status.interface.clone());
    let uplink = inventory.and_then(|inventory| {
        inventory
            .uplinks
            .iter()
            .find(|uplink| Some(&uplink.interface) == interface.as_ref())
            .map(|uplink| uplink.id.clone())
    });
    let history_available = columnar::kind_dir(&history::root(config), &KIND).exists()
        || status
            .as_ref()
            .is_some_and(|status| !status.buckets.is_empty());
    View {
        enabled,
        reason: reason(enabled, status.as_ref()),
        state: status.as_ref().map(|status| status.state),
        candidates: status
            .as_ref()
            .map(|status| status.candidates.clone())
            .unwrap_or_default(),
        sample_age_ms: status.as_ref().and_then(|status| status.sample_age_ms),
        interval: crate::router::WIFI_INTERVAL_MS / 1_000,
        link: status.as_ref().and_then(|status| status.link.clone()),
        recent: status.map(|status| status.recent).unwrap_or_default(),
        interface,
        uplink,
        history_available,
    }
}

pub const fn disabled() -> View {
    View {
        enabled: false,
        state: None,
        reason: String::new(),
        interface: None,
        candidates: Vec::new(),
        sample_age_ms: None,
        interval: crate::router::WIFI_INTERVAL_MS / 1_000,
        link: None,
        recent: Vec::new(),
        uplink: None,
        history_available: false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn access_point_series_keys_round_trip() {
        let ap = wayline_protocol::access_point_id("76:fa:ce:8a:c4:b3");
        assert_eq!(super::series_key(ap), "ap:76face8ac4b3");
        assert_eq!(
            super::bssid_of(&super::series_key(ap)).as_deref(),
            Some("76:fa:ce:8a:c4:b3")
        );
        assert_eq!(super::series_key(0), super::UPSTREAM);
        assert_eq!(super::bssid_of(super::UPSTREAM), None);
    }

    use super::{KIND, entries, point};
    use crate::columnar;
    use crate::history::{HOUR, LEVELS};
    use crate::runtime;
    use anyhow::{Result, ensure};
    use wayline_protocol::{BUCKET_SECONDS, WifiBucket};

    fn sample(at: u64, index: u64) -> WifiBucket {
        let signal = -50 - i16::try_from(index % 20).unwrap_or(0);
        WifiBucket {
            samples: 3,
            connected: 3,
            signal_n: 3,
            signal_sum: i64::from(signal) * 3,
            signal_min: Some(signal - 2),
            signal_max: Some(signal + 2),
            noise_n: 3,
            noise_sum: -95 * 3,
            tx_rate_n: 3,
            tx_rate_sum: 3 * 600_000,
            tx_rate_min: Some(400_000),
            rx_rate_n: 3,
            rx_rate_sum: 3 * 800_000,
            frames: 100,
            retries: 10,
            failed: 1,
            tx_bytes: 1_000_000,
            rx_bytes: 4_000_000,
            active_ms: 15_000,
            busy_ms: 3_000,
            rx_ms: 1_500,
            tx_ms: 500,
            disconnects: u32::from(index == 7),
            roams: u32::from(index == 9),
            ..WifiBucket::empty(at)
        }
    }

    fn hour(start: u64) -> Vec<WifiBucket> {
        (0..240)
            .map(|index| sample(start + index * BUCKET_SECONDS, index))
            .collect()
    }

    #[test]
    fn rows_round_trip() {
        let item = sample(1_800_000_000, 7);
        let row = super::row(&item);
        assert_eq!(row.len(), KIND.fields.len());
        assert_eq!(super::bucket(item.at, &row), item);
    }

    #[test]
    fn buckets_store_and_roll_up_exactly() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().to_path_buf();
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        let buckets = hour(start);
        columnar::store(
            &database,
            30,
            &KIND,
            &entries(&buckets),
            crate::runtime::now(),
        )?;
        columnar::store(
            &database,
            30,
            &KIND,
            &entries(&buckets),
            crate::runtime::now(),
        )?;
        for ((step, _), expected) in LEVELS.iter().zip([240, 60, 12, 1]) {
            let first = start / step;
            let rows = columnar::series(&database, &KIND, *step, (first, first + 239), &[])?
                .get("upstream")
                .map_or(0, std::collections::BTreeMap::len);
            ensure!(rows == expected, "Step {step} has {rows} rows.");
        }
        let found = columnar::series(&database, &KIND, 3_600, (start / HOUR, start / HOUR), &[])?;
        let row = found
            .get("upstream")
            .and_then(|rows| rows.get(&(start / HOUR)))
            .cloned()
            .unwrap_or_default();
        let total = super::bucket(start / HOUR, &row);
        let mut expected = WifiBucket::empty(start / HOUR);
        for item in &buckets {
            expected.merge(item);
        }
        ensure!(total == expected, "Rolled-up hour differs: {total:?}");
        let shown = point(Some(&total), 3_600);
        ensure!(shown.disconnects == 1 && shown.roams == 1);
        ensure!(shown.signal_min == Some(-71) && shown.signal_max == Some(-48));
        ensure!(shown.snr.is_some_and(|snr| snr > 30.0));
        ensure!(shown.busy == Some(20.0) && shown.retries.is_some());
        ensure!(shown.tx_rate == Some(600.0) && shown.tx_rate_min == Some(400.0));
        Ok(())
    }

    #[test]
    fn live_buckets_fill_recent_periods() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().to_path_buf();
        let start = runtime::now() / HOUR * HOUR - HOUR;
        let buckets = hour(start);
        let (old, live) = buckets.split_at(200);
        columnar::store(&database, 7, &KIND, &entries(old), crate::runtime::now())?;
        let found = columnar::series(
            &database,
            &KIND,
            60,
            (start / 60, start / 60 + 59),
            &entries(live),
        )?;
        let minutes = found
            .get("upstream")
            .map_or(0, std::collections::BTreeMap::len);
        ensure!(minutes == 60, "Missing minutes: {minutes}");
        let samples: i64 = found
            .get("upstream")
            .into_iter()
            .flat_map(|rows| rows.values())
            .filter_map(|row| row[0])
            .sum();
        ensure!(samples == 720, "Unexpected samples {samples}.");
        Ok(())
    }
}
