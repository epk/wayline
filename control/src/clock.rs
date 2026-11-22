use crate::runtime::{self, RUNTIME};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use wayline_protocol::{BUCKET_SECONDS, Bucket, LinkBucket, SystemBucket, WifiBucket};

const STEP_MS: i64 = 60_000;
const DRIFT_MS: i64 = 30_000;
const SETTLE_MS: u64 = 15 * 60_000;
const NTP_VALID: &str = "/var/state/ntp-valid";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Ntp,
    Step,
    Connectivity,
}

#[derive(Default, Serialize, Deserialize, PartialEq, Eq)]
struct State {
    boot: String,
    first_offset_ms: i64,
    synced: Option<Source>,
    #[serde(default)]
    trusted_offset_ms: Option<i64>,
    #[serde(default)]
    moved_ms: Option<i64>,
    #[serde(default)]
    announced: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Clock {
    pub boot: String,
    pub synced: bool,
    pub source: Option<Source>,
    pub offset_ms: i64,
    pub moved_ms: Option<i64>,
}

pub struct Observation<'a> {
    pub boot: &'a str,
    pub boot_ms: u64,
    pub wall_ms: u64,
    pub reachable: bool,
}

fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn decide(state: &mut State, observation: &Observation, ntp_valid: bool) -> Clock {
    let offset = signed(observation.wall_ms) - signed(observation.boot_ms);
    if state.boot != observation.boot {
        *state = State {
            boot: observation.boot.to_owned(),
            first_offset_ms: offset,
            ..State::default()
        };
    }
    if state.synced.is_none() {
        state.synced = if ntp_valid {
            Some(Source::Ntp)
        } else if (offset - state.first_offset_ms).abs() > STEP_MS {
            Some(Source::Step)
        } else if observation.boot_ms >= SETTLE_MS && observation.reachable {
            Some(Source::Connectivity)
        } else {
            None
        };
    }
    if state.synced == Some(Source::Connectivity) {
        let trusted = *state.trusted_offset_ms.get_or_insert(offset);
        if (offset - trusted).abs() > STEP_MS {
            state.synced = Some(Source::Step);
            state.moved_ms = Some(offset - trusted);
        } else if ntp_valid {
            state.synced = Some(Source::Ntp);
        }
    }
    Clock {
        boot: state.boot.clone(),
        synced: state.synced.is_some() && !observation.boot.is_empty(),
        source: state.synced,
        offset_ms: offset,
        moved_ms: state.moved_ms.filter(|_| !state.announced),
    }
}

pub fn moved_note(moved_ms: i64) -> String {
    let minutes = moved_ms.unsigned_abs().div_ceil(60_000);
    let direction = if moved_ms > 0 { "forward" } else { "back" };
    format!(
        "The router clock moved {direction} by about {minutes} min after Wayline had trusted it from connectivity alone. History saved earlier in this boot may be shown at the wrong time."
    )
}

pub fn announce() -> Result<()> {
    announce_in(Path::new(RUNTIME))
}

fn announce_in(directory: &Path) -> Result<()> {
    let path = directory.join("clock.json");
    let mut state: State = runtime::load(&path);
    if state.moved_ms.is_some() && !state.announced {
        state.announced = true;
        runtime::store(&path, &state)?;
    }
    Ok(())
}

pub fn of(parts: &runtime::Parts) -> Result<Option<Clock>> {
    if parts.boot_id.is_empty() {
        return Ok(None);
    }
    observe(&Observation {
        boot: &parts.boot_id,
        boot_ms: parts.boot_ms,
        wall_ms: parts.wall_ms,
        reachable: parts
            .series
            .iter()
            .any(|series| series.state.live() && series.received > 0),
    })
    .map(Some)
}

pub fn observe(observation: &Observation) -> Result<Clock> {
    observe_in(Path::new(RUNTIME), Path::new(NTP_VALID), observation)
}

fn observe_in(directory: &Path, ntp_valid: &Path, observation: &Observation) -> Result<Clock> {
    let path = directory.join("clock.json");
    let mut state: State = runtime::load(&path);
    let before = serde_json::to_vec(&state)?;
    let clock = decide(&mut state, observation, ntp_valid.exists());
    if serde_json::to_vec(&state)? != before {
        runtime::store(&path, &state)?;
    }
    Ok(clock)
}

pub fn corrected(at: u64, uptime_ms: u64, offset_ms: i64) -> Option<u64> {
    if uptime_ms == 0 {
        return None;
    }
    let expected = signed(uptime_ms) + offset_ms;
    let recorded = signed(at.saturating_mul(1_000));
    if (recorded - expected).abs() <= DRIFT_MS || expected < 0 {
        return None;
    }
    let seconds = u64::try_from(expected / 1_000).ok()?;
    Some(seconds - seconds % BUCKET_SECONDS)
}

pub trait Stamped {
    fn stamp(&self) -> (u64, u64);
    fn restamp(&mut self, at: u64);
}

macro_rules! stamped {
    ($($kind:ty),*) => {$(
        impl Stamped for $kind {
            fn stamp(&self) -> (u64, u64) {
                (self.at, self.uptime_ms)
            }
            fn restamp(&mut self, at: u64) {
                self.at = at;
            }
        }
    )*};
}

stamped!(Bucket, WifiBucket, SystemBucket, LinkBucket);

pub fn restamp<T: Stamped>(items: &mut [T], offset_ms: i64) -> usize {
    let mut changed = 0;
    for item in items {
        let (at, uptime) = item.stamp();
        if let Some(fixed) = corrected(at, uptime, offset_ms) {
            item.restamp(fixed);
            changed += 1;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::{
        Observation, Source, State, announce_in, corrected, decide, moved_note, observe_in, restamp,
    };
    use wayline_protocol::WifiBucket;

    fn seen(boot: &str, boot_ms: u64, wall_ms: u64, reachable: bool) -> Observation<'_> {
        Observation {
            boot,
            boot_ms,
            wall_ms,
            reachable,
        }
    }

    #[test]
    fn sync_is_detected_from_ntp_step_or_settled_connectivity() {
        let mut state = State::default();
        let early = decide(
            &mut state,
            &seen("b1", 30_000, 1_700_000_030_000, true),
            false,
        );
        assert!(!early.synced);
        let ntp = decide(
            &mut state,
            &seen("b1", 40_000, 1_700_000_040_000, false),
            true,
        );
        assert!(ntp.synced && ntp.source == Some(Source::Ntp));
        let mut state = State::default();
        decide(
            &mut state,
            &seen("b2", 30_000, 1_700_000_030_000, false),
            false,
        );
        let stepped = decide(
            &mut state,
            &seen("b2", 90_000, 1_800_000_000_000, false),
            false,
        );
        assert!(stepped.synced && stepped.source == Some(Source::Step));
        let mut state = State::default();
        decide(
            &mut state,
            &seen("b3", 60_000, 1_700_000_060_000, true),
            false,
        );
        let settled = decide(
            &mut state,
            &seen("b3", 16 * 60_000, 1_700_000_960_000, true),
            false,
        );
        assert!(settled.synced && settled.source == Some(Source::Connectivity));
        let fresh = decide(
            &mut state,
            &seen("b4", 1_000, 1_700_000_001_000, false),
            false,
        );
        assert!(!fresh.synced, "A new boot keeps the old sync.");
    }

    #[test]
    fn buckets_recorded_before_a_step_move_to_their_real_time() {
        let offset = 1_800_000_000_000_i64;
        assert_eq!(
            corrected(1_700_000_000, 60_000, offset),
            Some(1_800_000_060)
        );
        assert_eq!(corrected(1_800_000_060, 60_000, offset), None);
        assert_eq!(corrected(1_700_000_000, 0, offset), None);
        let mut buckets = vec![
            WifiBucket {
                uptime_ms: 30_000,
                ..WifiBucket::empty(1_700_000_010)
            },
            WifiBucket {
                uptime_ms: 45_000,
                ..WifiBucket::empty(1_700_000_025)
            },
        ];
        assert_eq!(restamp(&mut buckets, offset), 2);
        assert_eq!(buckets[1].at - buckets[0].at, 15);
        assert_eq!(buckets[0].at % 15, 0);
    }

    #[test]
    fn firmware_ntp_validity_marks_the_clock_synced() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let valid = root.join("ntp-valid");
        let early = seen("boot-a", 20_000, 1_700_000_020_000, false);
        anyhow::ensure!(!observe_in(root, &valid, &early)?.synced);
        std::fs::write(&valid, "kmwan set success\n")?;
        let clock = observe_in(
            root,
            &valid,
            &seen("boot-a", 25_000, 1_700_000_025_000, false),
        )?;
        anyhow::ensure!(clock.synced && clock.source == Some(Source::Ntp));
        std::fs::remove_file(&valid)?;
        let kept = observe_in(
            root,
            &valid,
            &seen("boot-a", 30_000, 1_700_000_030_000, false),
        )?;
        anyhow::ensure!(kept.synced, "Sync for a boot is kept once seen.");
        let next = observe_in(
            root,
            &valid,
            &seen("boot-b", 5_000, 1_700_000_005_000, false),
        )?;
        anyhow::ensure!(!next.synced, "A new boot starts unsynced.");
        Ok(())
    }

    #[test]
    fn connectivity_trust_is_provisional_until_the_clock_steps() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        let valid = root.join("ntp-valid");
        let base = 1_700_000_000_000;
        observe_in(root, &valid, &seen("b", 60_000, base + 60_000, true))?;
        let trusted = observe_in(root, &valid, &seen("b", 960_000, base + 960_000, true))?;
        anyhow::ensure!(trusted.synced && trusted.source == Some(Source::Connectivity));
        anyhow::ensure!(trusted.moved_ms.is_none());
        let drift = observe_in(root, &valid, &seen("b", 990_000, base + 1_010_000, true))?;
        anyhow::ensure!(drift.source == Some(Source::Connectivity) && drift.moved_ms.is_none());
        let hour: u64 = 3_600_000;
        let stepped = observe_in(
            root,
            &valid,
            &seen("b", 1_020_000, base + 1_020_000 + hour, true),
        )?;
        anyhow::ensure!(stepped.synced && stepped.source == Some(Source::Step));
        anyhow::ensure!(
            stepped.moved_ms == Some(3_600_000),
            "{:?}",
            stepped.moved_ms
        );
        anyhow::ensure!(moved_note(3_600_000).contains("forward by about 60 min"));
        anyhow::ensure!(
            observe_in(
                root,
                &valid,
                &seen("b", 1_030_000, base + 1_030_000 + hour, true)
            )?
            .moved_ms
            .is_some(),
            "The move is reported until it is announced."
        );
        announce_in(root)?;
        let later = observe_in(
            root,
            &valid,
            &seen("b", 1_040_000, base + 1_040_000 + hour, true),
        )?;
        anyhow::ensure!(later.synced && later.moved_ms.is_none());
        let mut state = State {
            boot: "c".into(),
            first_offset_ms: 5,
            synced: Some(Source::Connectivity),
            ..State::default()
        };
        let upgraded = decide(
            &mut state,
            &seen("c", 2_000_000, base + 2_000_000, true),
            false,
        );
        anyhow::ensure!(
            upgraded.moved_ms.is_none(),
            "A state saved before this field must not look like a step."
        );
        let confirmed = decide(
            &mut state,
            &seen("c", 2_010_000, base + 2_010_000, true),
            true,
        );
        anyhow::ensure!(confirmed.source == Some(Source::Ntp));
        Ok(())
    }
}
