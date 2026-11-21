use crate::runtime::{self, RUNTIME};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use wayline_protocol::{BUCKET_SECONDS, Bucket, LinkBucket, SystemBucket, WifiBucket};

const STEP_MS: i64 = 60_000;
const DRIFT_MS: i64 = 30_000;
const SETTLE_MS: u64 = 15 * 60_000;
const MARKER: &str = "clock-synced";

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
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Clock {
    pub boot: String,
    pub synced: bool,
    pub source: Option<Source>,
    pub offset_ms: i64,
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

fn decide(state: &mut State, observation: &Observation, marker: Option<&str>) -> Clock {
    let offset = signed(observation.wall_ms) - signed(observation.boot_ms);
    if state.boot != observation.boot {
        *state = State {
            boot: observation.boot.to_owned(),
            first_offset_ms: offset,
            synced: None,
        };
    }
    if state.synced.is_none() {
        state.synced = if marker.is_some_and(|boot| boot.trim() == observation.boot) {
            Some(Source::Ntp)
        } else if (offset - state.first_offset_ms).abs() > STEP_MS {
            Some(Source::Step)
        } else if observation.boot_ms >= SETTLE_MS && observation.reachable {
            Some(Source::Connectivity)
        } else {
            None
        };
    }
    Clock {
        boot: state.boot.clone(),
        synced: state.synced.is_some() && !observation.boot.is_empty(),
        source: state.synced,
        offset_ms: offset,
    }
}

pub fn observe(observation: &Observation) -> Result<Clock> {
    observe_in(Path::new(RUNTIME), observation)
}

fn observe_in(directory: &Path, observation: &Observation) -> Result<Clock> {
    let path = directory.join("clock.json");
    let mut state: State = runtime::load(&path);
    let before = serde_json::to_vec(&state)?;
    let marker = std::fs::read_to_string(directory.join(MARKER)).ok();
    let clock = decide(&mut state, observation, marker.as_deref());
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
    use super::{Observation, Source, State, corrected, decide, observe_in, restamp};
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
            None,
        );
        assert!(!early.synced);
        let ntp = decide(
            &mut state,
            &seen("b1", 40_000, 1_700_000_040_000, false),
            Some("b1\n"),
        );
        assert!(ntp.synced && ntp.source == Some(Source::Ntp));
        let mut state = State::default();
        decide(
            &mut state,
            &seen("b2", 30_000, 1_700_000_030_000, false),
            Some("old-boot"),
        );
        let stepped = decide(
            &mut state,
            &seen("b2", 90_000, 1_800_000_000_000, false),
            None,
        );
        assert!(stepped.synced && stepped.source == Some(Source::Step));
        let mut state = State::default();
        decide(
            &mut state,
            &seen("b3", 60_000, 1_700_000_060_000, true),
            None,
        );
        let settled = decide(
            &mut state,
            &seen("b3", 16 * 60_000, 1_700_000_960_000, true),
            None,
        );
        assert!(settled.synced && settled.source == Some(Source::Connectivity));
        let fresh = decide(
            &mut state,
            &seen("b4", 1_000, 1_700_000_001_000, false),
            None,
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

    fn hotplug(directory: &std::path::Path, action: &str, stratum: &str) -> anyhow::Result<()> {
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../package/files/etc/hotplug.d/ntp/95-wayline"
        );
        let status = std::process::Command::new("sh")
            .arg(script)
            .env("ACTION", action)
            .env("stratum", stratum)
            .env("WAYLINE_RUNTIME", directory)
            .env("WAYLINE_BOOT_ID", directory.join("boot_id"))
            .status()?;
        anyhow::ensure!(status.success(), "The hotplug script failed.");
        Ok(())
    }

    #[test]
    fn the_ntp_hotplug_script_marks_this_boot_synced() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        std::fs::write(root.join("boot_id"), "boot-a\n")?;
        let early = seen("boot-a", 20_000, 1_700_000_020_000, false);
        anyhow::ensure!(!observe_in(root, &early)?.synced);
        hotplug(root, "stratum", "16")?;
        hotplug(root, "periodic", "2")?;
        anyhow::ensure!(
            !observe_in(root, &early)?.synced,
            "An unsynced event marked sync."
        );
        hotplug(root, "stratum", "2")?;
        let clock = observe_in(root, &seen("boot-a", 25_000, 1_700_000_025_000, false))?;
        anyhow::ensure!(clock.synced && clock.source == Some(Source::Ntp));
        let next = observe_in(root, &seen("boot-b", 5_000, 1_700_000_005_000, false))?;
        anyhow::ensure!(!next.synced, "A marker from an earlier boot counted.");
        Ok(())
    }
}
