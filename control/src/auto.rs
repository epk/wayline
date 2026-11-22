use crate::api::{self, Exits};
use crate::runtime;
use crate::runtime::RUNTIME;
use crate::settings::Settings;
use crate::{native, router};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

const HOLD_SECONDS: u64 = 300;
const RECOVERY_SECONDS: u64 = 60;
const OBSERVATION_GAP: u64 = 120;
const MIN_RATIO: f64 = 0.8;
const MIN_GAIN_MS: f64 = 10.0;

#[derive(Default, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub last: Option<Switch>,
    #[serde(default)]
    pub needs_attention: bool,
    #[serde(default)]
    observed_exit: Option<String>,
    #[serde(default)]
    unhealthy_since: Option<u64>,
    #[serde(default)]
    observed_at: u64,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Switch {
    pub at: u64,
    pub from: String,
    pub to: String,
    pub reason: String,
}

pub struct View<'tunnel> {
    pub id: &'tunnel str,
    pub key: &'tunnel str,
    pub eligible: bool,
    pub median_ms: Option<f64>,
}

pub enum Reason {
    Faster,
    Unhealthy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    Preserve,
    Clear,
}

impl Reason {
    const fn text(&self) -> &'static str {
        match self {
            Self::Faster => "Lower latency",
            Self::Unhealthy => "Previous exit unhealthy",
        }
    }
}

impl Record {
    fn path() -> String {
        format!("{RUNTIME}/auto.json")
    }

    pub fn read() -> Self {
        let fresh = |needs_attention| Self {
            at: runtime::now(),
            needs_attention,
            ..Self::default()
        };
        match fs::read(Self::path()) {
            Ok(data) => serde_json::from_slice(&data).unwrap_or_else(|_| fresh(true)),
            Err(error) => fresh(error.kind() != std::io::ErrorKind::NotFound),
        }
    }

    fn write(&self) -> Result<()> {
        runtime::store(Self::path(), self)
    }

    pub fn touch(attention: Attention) -> Result<()> {
        let previous = Self::read();
        Self {
            at: runtime::now(),
            needs_attention: previous.needs_attention && attention == Attention::Preserve,
            observed_exit: None,
            unhealthy_since: None,
            observed_at: 0,
            ..previous
        }
        .write()
    }

    fn observe(&mut self, id: &str, healthy: bool, at: u64) -> u64 {
        if self.observed_exit.as_deref() != Some(id)
            || at < self.observed_at
            || at.saturating_sub(self.observed_at) > OBSERVATION_GAP
        {
            self.observed_exit = Some(id.to_owned());
            self.unhealthy_since = None;
        }
        self.observed_at = at;
        if healthy {
            self.unhealthy_since = None;
            return 0;
        }
        let since = self.unhealthy_since.get_or_insert(at);
        if at < *since {
            *since = at;
        }
        at.saturating_sub(*since)
    }
}

pub fn decide(
    views: &[View],
    current: &str,
    pool: &[String],
    idle: u64,
    unhealthy_for: u64,
) -> Option<(String, Reason)> {
    let current = views.iter().find(|view| view.id == current)?;
    if !pool.iter().any(|key| key == current.key) {
        return None;
    }
    let best = views
        .iter()
        .filter(|view| {
            view.id != current.id && view.eligible && pool.iter().any(|key| key == view.key)
        })
        .filter_map(|view| Some((view, view.median_ms?)))
        .min_by(|left, right| left.1.total_cmp(&right.1))?;
    match current.median_ms.filter(|_| current.eligible) {
        Some(median)
            if idle >= HOLD_SECONDS
                && best.1 <= median * MIN_RATIO
                && median - best.1 >= MIN_GAIN_MS =>
        {
            Some((best.0.id.to_owned(), Reason::Faster))
        }
        None if idle >= RECOVERY_SECONDS && unhealthy_for >= RECOVERY_SECONDS => {
            Some((best.0.id.to_owned(), Reason::Unhealthy))
        }
        _ => None,
    }
}

pub fn step(config: &Settings) -> Result<()> {
    if !config.enabled || !config.auto_exit || !native::contract() {
        return Ok(());
    }
    runtime::locked("selection", || {
        if Settings::read()? != *config {
            return Ok(());
        }
        let mut record = Record::read();
        if record.needs_attention {
            return Ok(());
        }
        router::forget();
        let Exits { revision, tunnels } = api::exits()?;
        let Some(current) = tunnels.iter().find(|tunnel| tunnel.preferred) else {
            return Ok(());
        };
        if !current.enabled || !config.auto_tunnels.contains(&current.monitor_id) {
            record.observed_exit = None;
            record.unhealthy_since = None;
            record.write()?;
            return Ok(());
        }
        let views: Vec<_> = tunnels
            .iter()
            .map(|tunnel| View {
                id: &tunnel.id,
                key: &tunnel.monitor_id,
                eligible: tunnel.eligible && config.auto_tunnels.contains(&tunnel.monitor_id),
                median_ms: tunnel.median,
            })
            .collect();
        let at = runtime::now();
        let unhealthy_for = record.observe(
            &current.monitor_id,
            current.eligible && current.median.is_some(),
            at,
        );
        let decision = decide(
            &views,
            &current.id,
            &config.auto_tunnels,
            at.saturating_sub(record.at),
            unhealthy_for,
        );
        record.write()?;
        let Some((target, reason)) = decision else {
            return Ok(());
        };
        let switch = Switch {
            at: runtime::now() * 1000,
            from: current.name.clone(),
            to: tunnels
                .iter()
                .find(|tunnel| tunnel.id == target)
                .map_or_else(|| target.clone(), |tunnel| tunnel.name.clone()),
            reason: reason.text().into(),
        };
        let before = match Snapshot::take() {
            Ok(before) => before,
            Err(error) => {
                record.last_error = Some(format!("{error:#}"));
                return record.write();
            }
        };
        let idle_since = record.at;
        record.at = runtime::now();
        record.needs_attention = true;
        record.write()?;
        match api::apply(&target, &revision, api::Selection::Automatic) {
            Ok(_) => Record {
                at: runtime::now(),
                last: Some(switch),
                ..Record::default()
            }
            .write(),
            Err(error) => {
                let after = Snapshot::take().ok();
                record.failed(idle_since, &before, after.as_ref(), &error);
                record.write()?;
                Err(error)
            }
        }
    })
}

#[derive(PartialEq)]
struct Snapshot {
    revision: String,
    order: Vec<(u32, bool)>,
    defaults: serde_json::Value,
}

impl Snapshot {
    fn take() -> Result<Self> {
        let list = native::List::read()?;
        Ok(Self {
            revision: router::revision()?,
            order: list
                .tunnels
                .iter()
                .map(|tunnel| (tunnel.number, tunnel.enabled))
                .collect(),
            defaults: list.default_tunnels,
        })
    }
}

impl Record {
    fn failed(
        &mut self,
        idle_since: u64,
        before: &Snapshot,
        after: Option<&Snapshot>,
        error: &anyhow::Error,
    ) {
        self.last_error = Some(format!("{error:#}"));
        if after == Some(before) {
            self.at = idle_since;
            self.needs_attention = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Reason, Record, Snapshot, View, decide};

    fn snapshot(order: &[(u32, bool)]) -> Snapshot {
        Snapshot {
            revision: "abc".into(),
            order: order.to_vec(),
            defaults: serde_json::json!([1]),
        }
    }

    fn attempted() -> Record {
        Record {
            at: 2000,
            needs_attention: true,
            ..Record::default()
        }
    }

    #[test]
    fn a_failure_that_left_the_router_unchanged_retries() {
        let before = snapshot(&[(1, true), (2, true)]);
        let mut record = attempted();
        record.failed(
            1000,
            &before,
            Some(&snapshot(&[(1, true), (2, true)])),
            &anyhow::anyhow!("State changed before selection. Refresh first."),
        );
        assert!(!record.needs_attention);
        assert_eq!(record.at, 1000);
        assert_eq!(
            record.last_error.as_deref(),
            Some("State changed before selection. Refresh first.")
        );
    }

    #[test]
    fn an_unverifiable_outcome_latches() {
        let before = snapshot(&[(1, true), (2, true)]);
        let error = anyhow::anyhow!("Selection changed concurrently. Refresh first.");
        let mut changed = attempted();
        changed.failed(
            1000,
            &before,
            Some(&snapshot(&[(2, true), (1, true)])),
            &error,
        );
        assert!(changed.needs_attention);
        assert_eq!(changed.at, 2000);
        let mut moved = attempted();
        let mut revised = snapshot(&[(1, true), (2, true)]);
        revised.revision = "def".into();
        moved.failed(1000, &before, Some(&revised), &error);
        assert!(moved.needs_attention);
        let mut unknown = attempted();
        unknown.failed(1000, &before, None, &error);
        assert!(unknown.needs_attention);
        assert!(unknown.last_error.is_some());
    }

    fn view(id: &str, eligible: bool, median: Option<f64>) -> View<'_> {
        View {
            id,
            key: id,
            eligible,
            median_ms: median,
        }
    }

    fn pool() -> [String; 2] {
        ["a".into(), "b".into()]
    }

    #[test]
    fn holds_until_time_and_gain() {
        let views = [view("a", true, Some(50.0)), view("b", true, Some(30.0))];
        assert!(decide(&views, "a", &pool(), 100, 0).is_none());
        assert!(matches!(
            decide(&views, "a", &pool(), 300, 0),
            Some((id, Reason::Faster)) if id == "b"
        ));
        let small = [view("a", true, Some(50.0)), view("b", true, Some(45.0))];
        assert!(decide(&small, "a", &pool(), 900, 0).is_none());
        let tiny = [view("a", true, Some(8.0)), view("b", true, Some(2.0))];
        assert!(decide(&tiny, "a", &pool(), 900, 0).is_none());
    }

    #[test]
    fn recovers_from_unhealthy_exit() {
        let views = [
            view("a", false, None),
            view("b", true, Some(90.0)),
            view("c", true, Some(40.0)),
        ];
        let mut record = Record::default();
        let allowed = ["a".into(), "b".into(), "c".into()];
        assert_eq!(record.observe("a", false, 1000), 0);
        assert!(decide(&views, "a", &allowed, 900, 0).is_none());
        assert!(decide(&views, "a", &allowed, 900, record.observe("a", false, 1059)).is_none());
        assert!(matches!(
            decide(&views, "a", &allowed, 900, record.observe("a", false, 1060)),
            Some((id, Reason::Unhealthy)) if id == "c"
        ));
        assert_eq!(record.observe("a", true, 1061), 0);
        assert_eq!(record.observe("a", false, 1062), 0);
        assert_eq!(record.observe("b", false, 1200), 0);
        assert_eq!(record.observe("b", false, 1100), 0);
        assert_eq!(record.observe("b", false, 1160), 60);
        assert_eq!(record.observe("b", false, 1281), 0);
    }

    #[test]
    fn never_picks_unhealthy_or_unknown() {
        let views = [view("a", false, None), view("b", false, Some(5.0))];
        assert!(decide(&views, "a", &pool(), 900, 900).is_none());
        assert!(decide(&views, "missing", &pool(), 900, 900).is_none());
        let four = [
            view("a", true, Some(50.0)),
            view("b", true, Some(30.0)),
            view("c", true, Some(1.0)),
            view("d", true, Some(2.0)),
        ];
        assert!(
            matches!(decide(&four, "a", &pool(), 900, 0), Some((id, Reason::Faster)) if id == "b")
        );
        assert!(decide(&four, "c", &pool(), 900, 0).is_none());
    }
}
