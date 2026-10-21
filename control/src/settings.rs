use crate::runtime::SETTINGS;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fmt, net::Ipv4Addr, path::Path, str::FromStr};

const MAX_TARGETS: usize = 5;
static FALLBACK: Target = Target::Address(Ipv4Addr::new(1, 1, 1, 1));

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Address(Ipv4Addr),
    Host(String),
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field represents an independent UI switch."
)]
pub struct Settings {
    pub enabled: bool,
    pub interval: u32,
    pub tunnels: Vec<String>,
    pub targets: Vec<Target>,
    pub direct_wan: bool,
    pub history_days: u32,
    pub auto_updates: bool,
    pub auto_exit: bool,
    pub auto_tunnels: Vec<String>,
    pub endpoint_diagnostics: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Stored files mirror the independent UI switches."
)]
struct Stored {
    enabled: bool,
    interval: u32,
    #[serde(deserialize_with = "tunnel_list")]
    tunnels: Vec<String>,
    targets: Vec<Target>,
    target: Option<Target>,
    diagnostics: Option<bool>,
    direct_wan: bool,
    history_hours: Option<u32>,
    history_days: Option<u32>,
    auto_updates: bool,
    #[serde(default)]
    auto_exit: bool,
    #[serde(default, deserialize_with = "optional_tunnels")]
    auto_tunnels: Option<Vec<String>>,
    #[serde(default)]
    endpoint_diagnostics: bool,
}

fn tunnel_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(Vec::new());
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}

fn optional_tunnels<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<String>>, D::Error> {
    tunnel_list(deserializer).map(Some)
}

impl TryFrom<Stored> for Settings {
    type Error = anyhow::Error;

    fn try_from(stored: Stored) -> Result<Self> {
        ensure!(
            stored
                .history_hours
                .is_none_or(|hours| [0, 24].contains(&hours)),
            "Invalid history retention."
        );
        let history_days = stored.history_days.unwrap_or(match stored.history_hours {
            Some(0) => 0,
            _ => 7,
        });
        let targets = if let Some(primary) = stored.target {
            let mut targets = vec![primary];
            if stored.diagnostics.unwrap_or(false) {
                for target in stored.targets {
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
            targets.truncate(MAX_TARGETS);
            targets
        } else {
            ensure!(stored.diagnostics.is_none(), "Unknown field diagnostics.");
            stored.targets
        };
        Ok(Self {
            enabled: stored.enabled,
            interval: stored.interval,
            tunnels: stored.tunnels,
            targets,
            direct_wan: stored.direct_wan,
            history_days,
            auto_updates: stored.auto_updates,
            auto_exit: stored.auto_exit && stored.auto_tunnels.is_some(),
            auto_tunnels: stored.auto_tunnels.unwrap_or_default(),
            endpoint_diagnostics: stored.endpoint_diagnostics,
        })
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::try_from(Stored::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: 5,
            tunnels: Vec::new(),
            targets: vec![
                Target::Address(Ipv4Addr::new(1, 1, 1, 1)),
                Target::Address(Ipv4Addr::new(8, 8, 8, 8)),
                Target::Address(Ipv4Addr::new(9, 9, 9, 9)),
            ],
            direct_wan: false,
            history_days: 7,
            auto_updates: false,
            auto_exit: false,
            auto_tunnels: Vec::new(),
            endpoint_diagnostics: false,
        }
    }
}

pub fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

pub const fn ip(value: Ipv4Addr) -> bool {
    let first = value.octets()[0];
    first > 0 && first < 224 && !value.is_loopback()
}

impl FromStr for Target {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
        if let Ok(address) = value.parse::<Ipv4Addr>() {
            ensure!(ip(address), "Use a unicast IPv4 address.");
            return Ok(Self::Address(address));
        }
        let label = |label: &str| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        };
        ensure!(
            value.len() <= 253
                && value.split('.').all(label)
                && value
                    .rsplit('.')
                    .next()
                    .is_some_and(|last| last.bytes().any(|byte| byte.is_ascii_alphabetic())),
            "Use an IPv4 address or a host name."
        );
        Ok(Self::Host(value))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Address(address) => address.fmt(formatter),
            Self::Host(host) => formatter.write_str(host),
        }
    }
}

impl Serialize for Target {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl Settings {
    pub fn read() -> Result<Self> {
        let path = Path::new(SETTINGS);
        let value: Self = if path.exists() {
            serde_json::from_slice(&wayline_support::read(path, 32768)?)?
        } else {
            Self::default()
        };
        value.validate()?;
        Ok(value)
    }

    pub fn primary(&self) -> &Target {
        self.targets.first().unwrap_or(&FALLBACK)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            [1, 5, 15, 30, 60].contains(&self.interval),
            "Invalid interval."
        );
        ensure!(
            [0, 1, 7, 30].contains(&self.history_days),
            "Invalid history retention."
        );
        ensure!(
            self.tunnels.len() <= 8 && (!self.enabled || !self.tunnels.is_empty()),
            "Choose one to eight tunnels."
        );
        ensure!(
            (1..=MAX_TARGETS).contains(&self.targets.len()),
            "Use one to five targets."
        );
        ensure!(
            self.tunnels.iter().all(|value| id(value)
                || value
                    .strip_prefix("vpn:")
                    .and_then(|number| number.parse::<u32>().ok())
                    .is_some_and(|number| number > 0)),
            "Invalid tunnel identity."
        );
        for (index, value) in self.tunnels.iter().enumerate() {
            ensure!(
                !self.tunnels[..index].contains(value),
                "Duplicate tunnel identity."
            );
        }
        for (index, value) in self.targets.iter().enumerate() {
            ensure!(!self.targets[..index].contains(value), "Duplicate target.");
        }
        ensure!(
            self.auto_tunnels.len() <= 8 && (!self.auto_exit || self.auto_tunnels.len() >= 2),
            "Choose at least two monitored tunnels for automatic switching."
        );
        for (index, id) in self.auto_tunnels.iter().enumerate() {
            ensure!(
                self.tunnels.contains(id),
                "Automatic switching only permits monitored tunnels."
            );
            ensure!(
                !self.auto_tunnels[..index].contains(id),
                "Duplicate automatic tunnel."
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Settings, Target};
    use anyhow::{Result, ensure};

    fn targets(values: &[&str]) -> Result<Vec<Target>> {
        values.iter().map(|value| value.parse::<Target>()).collect()
    }

    fn listed(settings: &Settings) -> Vec<String> {
        settings.targets.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn targets_parse() -> Result<()> {
        assert!(matches!("1.1.1.1".parse::<Target>()?, Target::Address(_)));
        assert_eq!("Example.COM.".parse::<Target>()?.to_string(), "example.com");
        for bad in [
            "",
            "999.1.1.1",
            "127.0.0.1",
            "a b",
            "-a.com",
            "a..com",
            "1.2.3",
            "::1",
        ] {
            ensure!(bad.parse::<Target>().is_err(), "{bad}");
        }
        Ok(())
    }

    #[test]
    fn legacy_with_diagnostics_merges_targets() -> Result<()> {
        let old = r#"{"enabled":false,"target":"1.1.1.1","interval":15,"tunnels":{},"diagnostics":true,"targets":["8.8.8.8","1.1.1.1","9.9.9.9"],"directWan":false,"historyHours":24,"autoUpdates":false}"#;
        let settings: Settings = serde_json::from_str(old)?;
        settings.validate()?;
        assert_eq!(listed(&settings), ["1.1.1.1", "8.8.8.8", "9.9.9.9"]);
        assert!(!settings.auto_exit && !settings.endpoint_diagnostics);
        Ok(())
    }

    #[test]
    fn legacy_without_diagnostics_keeps_only_target() -> Result<()> {
        let old = r#"{"enabled":false,"target":"1.1.1.1","interval":15,"tunnels":{},"diagnostics":false,"targets":["8.8.8.8"],"directWan":false,"historyHours":24,"autoUpdates":false}"#;
        let settings: Settings = serde_json::from_str(old)?;
        settings.validate()?;
        assert_eq!(listed(&settings), ["1.1.1.1"]);
        assert_eq!(settings.primary().to_string(), "1.1.1.1");
        Ok(())
    }

    #[test]
    fn legacy_merge_is_capped_at_five() -> Result<()> {
        let old = r#"{"enabled":false,"target":"9.9.9.9","interval":15,"tunnels":{},"diagnostics":true,"targets":["1.1.1.1","8.8.8.8","4.4.4.4","5.5.5.5","6.6.6.6"],"directWan":false,"historyHours":24,"autoUpdates":false}"#;
        let settings: Settings = serde_json::from_str(old)?;
        settings.validate()?;
        assert_eq!(
            listed(&settings),
            ["9.9.9.9", "1.1.1.1", "8.8.8.8", "4.4.4.4", "5.5.5.5"]
        );
        Ok(())
    }

    #[test]
    fn new_format_round_trips() -> Result<()> {
        let settings = Settings::default();
        let json = serde_json::to_string(&settings)?;
        ensure!(!json.contains("\"target\"") && !json.contains("diagnostics"));
        let loaded: Settings = serde_json::from_str(&json)?;
        ensure!(loaded == settings, "The round trip changed settings.");
        loaded.validate()?;
        Ok(())
    }

    #[test]
    fn automatic_pool_requires_explicit_monitored_members() -> Result<()> {
        let mut settings = Settings {
            tunnels: vec![
                "vpn:1".into(),
                "vpn:2".into(),
                "vpn:3".into(),
                "vpn:4".into(),
            ],
            auto_exit: true,
            auto_tunnels: vec!["vpn:1".into(), "vpn:2".into()],
            ..Settings::default()
        };
        settings.validate()?;
        let mut old = serde_json::to_value(&settings)?;
        old.as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Missing settings object."))?
            .remove("autoTunnels");
        let migrated: Settings = serde_json::from_value(old)?;
        assert!(!migrated.auto_exit && migrated.auto_tunnels.is_empty());
        migrated.validate()?;
        for pool in [
            vec!["vpn:1"],
            vec!["vpn:1", "vpn:1"],
            vec!["vpn:1", "vpn:5"],
        ] {
            settings.auto_tunnels = pool.into_iter().map(str::to_owned).collect();
            assert!(settings.validate().is_err());
        }
        Ok(())
    }

    #[test]
    fn validation_rejects_bad_target_lists() -> Result<()> {
        let with = |values: &[&str]| -> Result<Settings> {
            Ok(Settings {
                targets: targets(values)?,
                ..Settings::default()
            })
        };
        ensure!(with(&[])?.validate().is_err(), "Zero targets accepted.");
        ensure!(
            with(&[
                "1.1.1.1", "8.8.8.8", "9.9.9.9", "4.4.4.4", "5.5.5.5", "6.6.6.6",
            ])?
            .validate()
            .is_err(),
            "Six targets accepted."
        );
        with(&["1.1.1.1", "8.8.8.8", "9.9.9.9", "4.4.4.4", "5.5.5.5"])?.validate()?;
        ensure!(
            with(&["1.1.1.1", "8.8.8.8", "1.1.1.1"])?
                .validate()
                .is_err(),
            "Duplicate targets accepted."
        );
        Ok(())
    }

    #[test]
    fn history_retention_maps_legacy_hours() -> Result<()> {
        let legacy = |hours: u32| {
            format!(
                r#"{{"enabled":false,"interval":5,"tunnels":[],"targets":["1.1.1.1"],"directWan":false,"historyHours":{hours},"autoUpdates":false}}"#
            )
        };
        ensure!(serde_json::from_str::<Settings>(&legacy(24))?.history_days == 7);
        ensure!(serde_json::from_str::<Settings>(&legacy(0))?.history_days == 0);
        ensure!(serde_json::from_str::<Settings>(&legacy(12)).is_err());
        let json = serde_json::to_string(&Settings::default())?;
        ensure!(json.contains("\"historyDays\":7") && !json.contains("historyHours"));
        ensure!(Settings::default().interval == 5);
        Settings {
            history_days: 30,
            interval: 1,
            ..Settings::default()
        }
        .validate()?;
        ensure!(
            Settings {
                history_days: 2,
                ..Settings::default()
            }
            .validate()
            .is_err()
        );
        Ok(())
    }
}
