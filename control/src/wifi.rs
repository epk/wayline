use crate::changes::{self, Event};
use crate::history::{self, Access, HOUR, LEVELS, MIN_CLOCK};
use crate::router::Inventory;
use crate::runtime::{self, RUNTIME};
use crate::settings::Settings;
use anyhow::{Context, Error, Result, ensure};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, Row, TransactionBehavior, params_from_iter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use wayline_protocol::{BUCKET_SECONDS, WifiBucket, WifiLink, WifiSample, WifiState, WifiStatus};

const VERSION: u32 = 1;
const UPSTREAM: &str = "upstream";
const MEMORY_PAGES: u64 = 256;
const PAGES: u64 = 2_048;
const DAMAGED: &str = "The Wi-Fi history database was damaged and was reset.";
pub const SUBJECT: &str = "wifi";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fold {
    Sum,
    Min,
    Max,
}

const FIELDS: [(&str, Fold); 26] = [
    ("samples", Fold::Sum),
    ("connected", Fold::Sum),
    ("signal_n", Fold::Sum),
    ("signal_sum", Fold::Sum),
    ("signal_min", Fold::Min),
    ("signal_max", Fold::Max),
    ("noise_n", Fold::Sum),
    ("noise_sum", Fold::Sum),
    ("tx_rate_n", Fold::Sum),
    ("tx_rate_sum", Fold::Sum),
    ("tx_rate_min", Fold::Min),
    ("rx_rate_n", Fold::Sum),
    ("rx_rate_sum", Fold::Sum),
    ("expected_n", Fold::Sum),
    ("expected_sum", Fold::Sum),
    ("frames", Fold::Sum),
    ("retries", Fold::Sum),
    ("failed", Fold::Sum),
    ("tx_bytes", Fold::Sum),
    ("rx_bytes", Fold::Sum),
    ("active_ms", Fold::Sum),
    ("busy_ms", Fold::Sum),
    ("rx_ms", Fold::Sum),
    ("tx_ms", Fold::Sum),
    ("disconnects", Fold::Sum),
    ("roams", Fold::Sum),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    start: u64,
    end: u64,
    step: u64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    at: u64,
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

#[derive(Serialize)]
pub struct Response {
    step: u64,
    summary: Point,
    points: Vec<Point>,
    events: Vec<Event>,
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

pub fn database(config: &Settings) -> PathBuf {
    history::database(config).with_file_name("wifi.sqlite")
}

fn cap(path: &Path) -> u64 {
    if path.starts_with(RUNTIME) {
        MEMORY_PAGES
    } else {
        PAGES
    }
}

fn open(path: &Path, access: Access) -> Result<Connection> {
    history::open_capped(path, access, cap(path))
}

fn columns() -> String {
    FIELDS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",")
}

fn folds() -> String {
    FIELDS
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
    let fields = FIELDS.iter().fold(String::new(), |mut out, (name, _)| {
        let _ = write!(out, "{name} INTEGER,");
        out
    });
    let tiers = LEVELS.iter().fold(String::new(), |mut out, (_, table)| {
        let _ = write!(
            out,
            "CREATE TABLE {table}(period INTEGER NOT NULL,link INTEGER NOT NULL,{fields}PRIMARY KEY(period,link)) WITHOUT ROWID;"
        );
        out
    });
    format!(
        "CREATE TABLE link(id INTEGER PRIMARY KEY,name TEXT NOT NULL UNIQUE,seen INTEGER NOT NULL);{tiers}"
    )
}

fn number(value: impl TryInto<i64>) -> Sql {
    Sql::Integer(value.try_into().unwrap_or(i64::MAX))
}

fn optional(value: Option<impl Into<i64>>) -> Sql {
    value.map_or(Sql::Null, |value| Sql::Integer(value.into()))
}

fn values(bucket: &WifiBucket) -> Vec<Sql> {
    vec![
        number(bucket.samples),
        number(bucket.connected),
        number(bucket.signal_n),
        number(bucket.signal_sum),
        optional(bucket.signal_min),
        optional(bucket.signal_max),
        number(bucket.noise_n),
        number(bucket.noise_sum),
        number(bucket.tx_rate_n),
        number(bucket.tx_rate_sum),
        optional(bucket.tx_rate_min),
        number(bucket.rx_rate_n),
        number(bucket.rx_rate_sum),
        number(bucket.expected_n),
        number(bucket.expected_sum),
        number(bucket.frames),
        number(bucket.retries),
        number(bucket.failed),
        number(bucket.tx_bytes),
        number(bucket.rx_bytes),
        number(bucket.active_ms),
        number(bucket.busy_ms),
        number(bucket.rx_ms),
        number(bucket.tx_ms),
        number(bucket.disconnects),
        number(bucket.roams),
    ]
}

struct Cursor<'row, 'statement> {
    row: &'row Row<'statement>,
    index: usize,
}

impl Cursor<'_, '_> {
    fn raw(&mut self) -> rusqlite::Result<Option<i64>> {
        self.index += 1;
        self.row.get(self.index)
    }

    fn big(&mut self) -> rusqlite::Result<u64> {
        Ok(self
            .raw()?
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0))
    }

    fn small(&mut self) -> rusqlite::Result<u32> {
        Ok(u32::try_from(self.big()?).unwrap_or(u32::MAX))
    }

    fn signed(&mut self) -> rusqlite::Result<i64> {
        Ok(self.raw()?.unwrap_or(0))
    }

    fn level(&mut self) -> rusqlite::Result<Option<i16>> {
        Ok(self.raw()?.and_then(|value| i16::try_from(value).ok()))
    }

    fn rate(&mut self) -> rusqlite::Result<Option<u32>> {
        Ok(self.raw()?.and_then(|value| u32::try_from(value).ok()))
    }
}

fn bucket(row: &Row, at: u64) -> rusqlite::Result<WifiBucket> {
    let mut cursor = Cursor { row, index: 0 };
    Ok(WifiBucket {
        at,
        samples: cursor.small()?,
        connected: cursor.small()?,
        signal_n: cursor.small()?,
        signal_sum: cursor.signed()?,
        signal_min: cursor.level()?,
        signal_max: cursor.level()?,
        noise_n: cursor.small()?,
        noise_sum: cursor.signed()?,
        tx_rate_n: cursor.small()?,
        tx_rate_sum: cursor.big()?,
        tx_rate_min: cursor.rate()?,
        rx_rate_n: cursor.small()?,
        rx_rate_sum: cursor.big()?,
        expected_n: cursor.small()?,
        expected_sum: cursor.big()?,
        frames: cursor.big()?,
        retries: cursor.big()?,
        failed: cursor.big()?,
        tx_bytes: cursor.big()?,
        rx_bytes: cursor.big()?,
        active_ms: cursor.big()?,
        busy_ms: cursor.big()?,
        rx_ms: cursor.big()?,
        tx_ms: cursor.big()?,
        disconnects: cursor.small()?,
        roams: cursor.small()?,
    })
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

fn span(buckets: &[WifiBucket]) -> Option<(u64, u64)> {
    let periods = buckets.iter().map(|bucket| bucket.at / BUCKET_SECONDS);
    Some((periods.clone().min()?, periods.max()?))
}

fn append(path: &Path, days: u32, now: u64, buckets: &[WifiBucket]) -> Result<()> {
    let mut connection = open(path, Access::ReadWrite)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO link(name,seen) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET seen=excluded.seen WHERE seen<excluded.seen",
        (UPSTREAM, now / HOUR),
    )?;
    let link: i64 =
        transaction.query_row("SELECT id FROM link WHERE name=?1", [UPSTREAM], |row| {
            row.get(0)
        })?;
    let marks = vec!["?"; FIELDS.len()].join(",");
    {
        let mut insert = transaction.prepare(&format!(
            "INSERT OR IGNORE INTO {}(period,link,{}) VALUES(?,?,{marks})",
            LEVELS[0].1,
            columns()
        ))?;
        for item in buckets {
            let mut arguments = vec![number(item.at / BUCKET_SECONDS), Sql::Integer(link)];
            arguments.extend(values(item));
            insert.execute(params_from_iter(arguments))?;
        }
    }
    if let Some(mut range) = span(buckets) {
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
                    "INSERT OR REPLACE INTO {parent}(period,link,{}) SELECT period/?1,link,{} FROM {child} WHERE period BETWEEN ?2 AND ?3 GROUP BY period/?1,link",
                    columns(),
                    folds()
                ),
                [ratio, low, high],
            )?;
            range = (range.0 / ratio, range.1 / ratio);
        }
    }
    for (step, table) in LEVELS {
        let kept = history::retained(days, step);
        transaction.execute(
            &format!("DELETE FROM {table} WHERE period<?1"),
            [now.saturating_sub(kept) / step + u64::from(kept == 0)],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn shrink(path: &Path) -> Result<()> {
    let mut connection = open(path, Access::ReadWrite)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (_, table) in LEVELS.iter().rev() {
        transaction.execute(
            &format!(
                "DELETE FROM {table} WHERE period<=(SELECT period FROM {table} ORDER BY period LIMIT 1 OFFSET (SELECT count(*)/10 FROM {table}))"
            ),
            [],
        )?;
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

pub fn store(path: &Path, days: u32, buckets: &[WifiBucket]) -> Result<()> {
    let now = runtime::now();
    append(path, days, now, buckets).or_else(|error| {
        recover(path, error)?;
        append(path, days, now, buckets)
    })
}

fn stored(
    path: &Path,
    table: &str,
    first: u64,
    last: u64,
    into: &mut BTreeMap<u64, WifiBucket>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let connection = open(path, Access::ReadOnly)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < VERSION {
        return Ok(());
    }
    let mut statement = connection.prepare(&format!(
        "SELECT t.period,{} FROM {table} t JOIN link l ON l.id=t.link WHERE l.name=?1 AND t.period BETWEEN ?2 AND ?3",
        columns()
            .split(',')
            .map(|name| format!("t.{name}"))
            .collect::<Vec<_>>()
            .join(",")
    ))?;
    let mut rows = statement.query((UPSTREAM, first, last))?;
    while let Some(row) = rows.next()? {
        let period: u64 = row.get(0)?;
        into.insert(period, bucket(row, period)?);
    }
    Ok(())
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

fn point(at: u64, item: Option<&WifiBucket>, seconds: u64) -> Point {
    let Some(item) = item.filter(|item| item.samples > 0) else {
        return Point {
            at,
            ..Point::default()
        };
    };
    let signal = average(signed(item.signal_sum), item.signal_n);
    let noise = average(signed(item.noise_sum), item.noise_n);
    let traffic = |bytes: u64| {
        (item.connected > 0 && seconds > 0)
            .then(|| rounded(float(bytes) * 8.0 / float(seconds) / 1_000_000.0, 3))
    };
    Point {
        at,
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

fn series(
    path: &Path,
    step: u64,
    first: u64,
    last: u64,
    live: &[WifiBucket],
) -> Result<BTreeMap<u64, WifiBucket>> {
    let mut periods = BTreeMap::new();
    stored(path, history::table(step), first, last, &mut periods)?;
    if let Some(range) = span(live) {
        let ratio = step / BUCKET_SECONDS;
        let (low, high) = history::aligned(range, ratio);
        let mut fine = BTreeMap::new();
        stored(path, LEVELS[0].1, low, high, &mut fine)?;
        for item in live {
            fine.insert(item.at / BUCKET_SECONDS, *item);
        }
        let mut coarse: BTreeMap<u64, WifiBucket> = BTreeMap::new();
        for (period, item) in fine {
            let parent = period / ratio;
            coarse
                .entry(parent)
                .or_insert_with(|| WifiBucket::empty(parent))
                .merge(&item);
        }
        periods.extend(
            coarse
                .into_iter()
                .filter(|(period, _)| (first..=last).contains(period)),
        );
    }
    Ok(periods)
}

pub fn query(config: &Settings, query: &Query) -> Result<Response> {
    runtime::locked("history", || query_inner(config, query))
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
    let periods = series(&database(config), step, first, last, &live)?;
    let mut total = WifiBucket::empty(first);
    for item in periods.values() {
        total.merge(item);
    }
    let points = (first..=last)
        .map(|period| point(period * step * 1_000, periods.get(&period), step))
        .collect();
    let events = changes::read(config)
        .into_iter()
        .filter(|event| event.subject == SUBJECT && (query.start..=query.end).contains(&event.at))
        .collect();
    Ok(Response {
        step,
        summary: point(query.start, Some(&total), (last - first + 1) * step),
        points,
        events,
    })
}

const fn band(frequency: u32) -> &'static str {
    match frequency {
        0..3_000 => "2.4 GHz",
        3_000..5_925 => "5 GHz",
        _ => "6 GHz",
    }
}

pub fn channel(link: &WifiLink) -> String {
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
        WifiState::QueryFailed => "The router's Wi-Fi status service (iwinfo) did not answer.".into(),
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
    let history_available = database(config).exists()
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

pub fn report(config: &Settings) -> Vec<String> {
    let path = database(config);
    let mut lines = vec![format!(
        "Wi-Fi history database: {} bytes of {} bytes cap",
        fs::metadata(&path).map_or(0, |meta| meta.len()),
        cap(&path) * history::PAGE_BYTES
    )];
    if let Ok(connection) = open(&path, Access::ReadOnly).context("unreadable") {
        for (_, table) in LEVELS {
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
                    "Wi-Fi {table}: {rows} rows, oldest period {}, newest period {}",
                    oldest.map_or_else(String::new, |value| value.to_string()),
                    newest.map_or_else(String::new, |value| value.to_string())
                ));
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{BUCKET_SECONDS, HOUR, LEVELS, append, open, point, series, store};
    use crate::history::Access;
    use crate::runtime;
    use anyhow::{Result, ensure};
    use wayline_protocol::WifiBucket;

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
    fn buckets_store_and_roll_up_exactly() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("wifi.sqlite");
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        let buckets = hour(start);
        store(&database, 30, &buckets)?;
        store(&database, 30, &buckets)?;
        let connection = open(&database, Access::ReadOnly)?;
        for ((_, table), expected) in LEVELS.iter().zip([240, 60, 12, 1]) {
            let rows: i64 =
                connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            ensure!(rows == expected, "{table} has {rows} rows.");
        }
        let periods = series(&database, 3_600, start / HOUR, start / HOUR, &[])?;
        let total = periods.get(&(start / HOUR)).copied().unwrap_or_default();
        let mut expected = WifiBucket::empty(start / HOUR);
        for item in &buckets {
            expected.merge(item);
        }
        expected.at = total.at;
        ensure!(total == expected, "Rolled-up hour differs: {total:?}");
        let shown = point(start * 1_000, Some(&total), 3_600);
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
        let database = directory.path().join("wifi.sqlite");
        let start = runtime::now() / HOUR * HOUR - HOUR;
        let buckets = hour(start);
        let (old, live) = buckets.split_at(200);
        store(&database, 7, old)?;
        let periods = series(&database, 60, start / 60, start / 60 + 59, live)?;
        ensure!(periods.len() == 60, "Missing minutes: {}", periods.len());
        let samples: u32 = periods.values().map(|item| item.samples).sum();
        ensure!(samples == 720, "Unexpected samples {samples}.");
        Ok(())
    }

    #[test]
    fn expired_rows_are_pruned() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("wifi.sqlite");
        let now = runtime::now();
        let old = now - 40 * 86_400;
        store(&database, 30, &[sample(old, 0)])?;
        append(&database, 30, now, &[sample(now / 15 * 15 - 60, 1)])?;
        let connection = open(&database, Access::ReadOnly)?;
        let oldest: i64 =
            connection.query_row("SELECT min(period) FROM s1h", [], |row| row.get(0))?;
        ensure!(u64::try_from(oldest)? * 3_600 >= now - 31 * 86_400);
        Ok(())
    }
}
