use crate::columnar::{Entry, Row};
use crate::runtime;
use crate::settings::Settings;
use serde::Serialize;
use wayline_protocol::{SqmAction, SqmBucket, SqmStatus};

pub use wayline_protocol::schema::SQM as KIND;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqmLinkView {
    pub id: String,
    pub name: String,
    pub egress_device: String,
    pub ingress_device: String,
    pub dl_kbps: u32,
    pub ul_kbps: u32,
    pub rx_kbps: u32,
    pub tx_kbps: u32,
    pub baseline_ms: Option<f64>,
    pub delta_ms: Option<f64>,
    pub dl_action: SqmAction,
    pub ul_action: SqmAction,
    pub adjustments: u64,
    pub updated_age_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqmView {
    pub enabled: bool,
    pub active: bool,
    pub links: Vec<SqmLinkView>,
}

fn value(value: impl TryInto<i64>) -> Option<i64> {
    value.try_into().ok().or(Some(i64::MAX))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "RTT deltas in microseconds stay far below the 53-bit mantissa."
)]
const fn signed(value: i64) -> f64 {
    value as f64
}

pub fn row(bucket: &SqmBucket) -> Row {
    vec![
        value(bucket.samples),
        value(bucket.dl_kbps_sum),
        bucket.dl_kbps_min.map(i64::from),
        bucket.dl_kbps_max.map(i64::from),
        value(bucket.ul_kbps_sum),
        bucket.ul_kbps_min.map(i64::from),
        bucket.ul_kbps_max.map(i64::from),
        value(bucket.baseline_us_n),
        value(bucket.baseline_us_sum),
        bucket.delta_us_max,
        value(bucket.increases),
        value(bucket.decreases),
    ]
}

#[cfg(test)]
#[must_use]
fn bucket(at: u64, row: &[Option<i64>]) -> SqmBucket {
    let cell = |index: usize| row.get(index).copied().flatten();
    let big = |index: usize| {
        cell(index)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0)
    };
    let small = |index: usize| u16::try_from(big(index)).unwrap_or(u16::MAX);
    let rate = |index: usize| cell(index).and_then(|value| u32::try_from(value).ok());
    SqmBucket {
        at,
        samples: small(0),
        dl_kbps_sum: big(1),
        dl_kbps_min: rate(2),
        dl_kbps_max: rate(3),
        ul_kbps_sum: big(4),
        ul_kbps_min: rate(5),
        ul_kbps_max: rate(6),
        baseline_us_n: small(7),
        baseline_us_sum: big(8),
        delta_us_max: cell(9),
        increases: small(10),
        decreases: small(11),
    }
}

pub fn entries(status: &SqmStatus) -> Vec<Entry> {
    status
        .buckets
        .iter()
        .flat_map(|history| {
            history.buckets.iter().map(|item| Entry {
                link: history.id.clone(),
                at: item.at,
                row: row(item),
            })
        })
        .collect()
}

pub fn view(config: &Settings, status: Option<SqmStatus>) -> SqmView {
    let enabled = config.sqm_autorate && config.enabled;
    let status = status.filter(|_| enabled);
    SqmView {
        enabled,
        active: status.as_ref().is_some_and(|status| status.active),
        links: status
            .map(|status| {
                status
                    .links
                    .into_iter()
                    .map(|link| SqmLinkView {
                        id: link.id,
                        name: link.name,
                        egress_device: link.egress_device,
                        ingress_device: link.ingress_device,
                        dl_kbps: link.dl_kbps,
                        ul_kbps: link.ul_kbps,
                        rx_kbps: link.rx_kbps,
                        tx_kbps: link.tx_kbps,
                        baseline_ms: link.baseline_us.map(runtime::milliseconds),
                        delta_ms: link.delta_us.map(|us| signed(us) / 1_000.0),
                        dl_action: link.dl_action,
                        ul_action: link.ul_action,
                        adjustments: link.adjustments,
                        updated_age_ms: link.updated_age_ms,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

pub const fn disabled() -> SqmView {
    SqmView {
        enabled: false,
        active: false,
        links: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{KIND, bucket, entries, row, view};
    use crate::columnar;
    use crate::history::{HOUR, LEVELS};
    use crate::runtime;
    use crate::settings::Settings;
    use anyhow::{Result, ensure};
    use wayline_protocol::{
        BUCKET_SECONDS, SqmAction, SqmBucket, SqmHistory, SqmLinkStatus, SqmStatus,
    };

    fn sample(at: u64, index: u64) -> SqmBucket {
        let dl = u32::try_from(40_000 + index * 100).unwrap_or(40_000);
        let ul = u32::try_from(20_000 + index * 50).unwrap_or(20_000);
        SqmBucket {
            at,
            samples: 15,
            dl_kbps_sum: u64::from(dl) * 15,
            dl_kbps_min: Some(dl - 500),
            dl_kbps_max: Some(dl + 500),
            ul_kbps_sum: u64::from(ul) * 15,
            ul_kbps_min: Some(ul - 250),
            ul_kbps_max: Some(ul + 250),
            baseline_us_n: 15,
            baseline_us_sum: 15 * 18_000,
            delta_us_max: Some(i64::try_from(index * 100).unwrap_or(0)),
            increases: u16::from(index == 3),
            decreases: u16::from(index == 7),
        }
    }

    #[test]
    fn rows_round_trip() {
        let item = sample(1_800_000_000, 7);
        let encoded = row(&item);
        assert_eq!(encoded.len(), KIND.fields.len());
        assert_eq!(bucket(item.at, &encoded), item);
    }

    #[test]
    fn buckets_store_and_roll_up_to_parquet() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path();
        let start = runtime::now() / HOUR * HOUR - 2 * HOUR;
        let buckets: Vec<SqmBucket> = (0..240)
            .map(|index| sample(start + index * BUCKET_SECONDS, index))
            .collect();
        let status = SqmStatus {
            active: true,
            links: Vec::new(),
            buckets: vec![SqmHistory {
                id: "wan".into(),
                buckets,
            }],
        };
        let rows = entries(&status);
        columnar::store(database, 30, &KIND, &rows, runtime::now())?;
        for ((step, _), expected) in LEVELS.iter().zip([240, 60, 12, 1]) {
            let first = start / step;
            let count = columnar::series(database, &KIND, *step, (first, first + 239), &[])?
                .get("wan")
                .map_or(0, std::collections::BTreeMap::len);
            ensure!(count == expected, "Step {step} has {count} rows.");
        }
        let hour = columnar::series(database, &KIND, 3_600, (start / HOUR, start / HOUR), &[])?;
        let row = hour
            .get("wan")
            .and_then(|rows| rows.get(&(start / HOUR)))
            .cloned()
            .unwrap_or_default();
        let rolled = bucket(start / HOUR, &row);
        ensure!(rolled.samples == 240 * 15);
        ensure!(rolled.dl_kbps_min == Some(39_500));
        ensure!(rolled.dl_kbps_max == Some(40_000 + 239 * 100 + 500));
        ensure!(rolled.ul_kbps_min == Some(19_750));
        ensure!(rolled.ul_kbps_max == Some(20_000 + 239 * 50 + 250));
        ensure!(rolled.baseline_us_n == 240 * 15);
        ensure!(rolled.baseline_us_sum == 240 * 15 * 18_000);
        ensure!(rolled.delta_us_max == Some(23_900));
        ensure!(rolled.increases == 1 && rolled.decreases == 1);
        Ok(())
    }

    #[test]
    fn view_formats_status_fields() {
        let settings = Settings {
            enabled: true,
            sqm_autorate: true,
            ..Settings::default()
        };
        let status = SqmStatus {
            active: true,
            links: vec![SqmLinkStatus {
                id: "tunnel1".into(),
                name: "WireGuard".into(),
                egress_device: "wg0".into(),
                ingress_device: "ifb4wg0".into(),
                dl_kbps: 100_000,
                ul_kbps: 40_000,
                rx_kbps: 25_000,
                tx_kbps: 12_000,
                baseline_us: Some(18_500),
                delta_us: Some(-500),
                dl_action: SqmAction::Increase,
                ul_action: SqmAction::Hold,
                adjustments: 4,
                updated_age_ms: Some(250),
            }],
            buckets: Vec::new(),
        };
        let rendered = view(&settings, Some(status));
        assert!(rendered.enabled && rendered.active);
        assert_eq!(rendered.links.len(), 1);
        assert_eq!(rendered.links[0].baseline_ms, Some(18.5));
        assert_eq!(rendered.links[0].delta_ms, Some(-0.5));
    }
}
