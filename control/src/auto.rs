use crate::{
    api::{self, State},
    native,
    runtime::{RUNTIME, locked, now},
    settings::Settings,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use wayline_support::atomic;

const HOLD_SECONDS: u64 = 300;
const RECOVERY_SECONDS: u64 = 60;
const MIN_RATIO: f64 = 0.8;
const MIN_GAIN_MS: f64 = 10.0;

#[derive(Default, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub last: Option<Switch>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Switch {
    pub at: u64,
    pub from: String,
    pub to: String,
    pub reason: String,
}

pub struct View<'a> {
    pub id: &'a str,
    pub eligible: bool,
    pub median_ms: Option<f64>,
}

pub enum Reason {
    Faster,
    Unhealthy,
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
        fs::read(Self::path())
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    fn write(&self) -> Result<()> {
        atomic(Path::new(&Self::path()), &serde_json::to_vec(self)?)
    }

    pub fn touch() -> Result<()> {
        Self {
            at: now(),
            ..Self::read()
        }
        .write()
    }
}

pub fn decide(views: &[View], current: &str, idle: u64) -> Option<(String, Reason)> {
    let current = views.iter().find(|view| view.id == current)?;
    let best = views
        .iter()
        .filter(|view| view.id != current.id && view.eligible)
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
        None if idle >= RECOVERY_SECONDS => Some((best.0.id.to_owned(), Reason::Unhealthy)),
        _ => None,
    }
}

pub fn step(config: &Settings) -> Result<()> {
    if !config.enabled || !config.auto_exit || !native::contract() {
        return Ok(());
    }
    locked("selection", || {
        let revision = crate::router::revision()?;
        let state = api::state()?;
        let record = Record::read();
        let Some(current) = state.tunnels.iter().find(|tunnel| tunnel.preferred) else {
            return Ok(());
        };
        if !current.enabled || !config.tunnels.contains(&current.monitor_id) {
            return Ok(());
        }
        let views: Vec<_> = state
            .tunnels
            .iter()
            .map(|tunnel| View {
                id: &tunnel.id,
                eligible: tunnel.eligible && config.tunnels.contains(&tunnel.monitor_id),
                median_ms: tunnel.median,
            })
            .collect();
        let Some((target, reason)) = decide(&views, &current.id, now().saturating_sub(record.at))
        else {
            return Ok(());
        };
        let name = |id: &str| state_name(&state, id);
        let switch = Switch {
            at: now() * 1000,
            from: current.name.clone(),
            to: name(&target),
            reason: reason.text().into(),
        };
        api::apply(&target, &revision)?;
        Record {
            at: now(),
            last: Some(switch),
        }
        .write()
    })
}

fn state_name(state: &State, id: &str) -> String {
    state
        .tunnels
        .iter()
        .find(|tunnel| tunnel.id == id)
        .map_or_else(|| id.to_owned(), |tunnel| tunnel.name.clone())
}

#[cfg(test)]
mod tests {
    use super::{Reason, View, decide};

    fn view(id: &str, eligible: bool, median: Option<f64>) -> View<'_> {
        View {
            id,
            eligible,
            median_ms: median,
        }
    }

    #[test]
    fn holds_until_time_and_gain() {
        let views = [view("a", true, Some(50.0)), view("b", true, Some(30.0))];
        assert!(decide(&views, "a", 100).is_none());
        assert!(matches!(
            decide(&views, "a", 300),
            Some((id, Reason::Faster)) if id == "b"
        ));
        let small = [view("a", true, Some(50.0)), view("b", true, Some(45.0))];
        assert!(decide(&small, "a", 900).is_none());
        let tiny = [view("a", true, Some(8.0)), view("b", true, Some(2.0))];
        assert!(decide(&tiny, "a", 900).is_none());
    }

    #[test]
    fn recovers_from_unhealthy_exit() {
        let views = [
            view("a", false, None),
            view("b", true, Some(90.0)),
            view("c", true, Some(40.0)),
        ];
        assert!(decide(&views, "a", 30).is_none());
        assert!(matches!(
            decide(&views, "a", 60),
            Some((id, Reason::Unhealthy)) if id == "c"
        ));
    }

    #[test]
    fn never_picks_unhealthy_or_unknown() {
        let views = [view("a", false, None), view("b", false, Some(5.0))];
        assert!(decide(&views, "a", 900).is_none());
        assert!(decide(&views, "missing", 900).is_none());
    }
}
