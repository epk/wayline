use crate::runtime::SETTINGS;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::Path;
use std::str::FromStr;
use std::{fmt, io};

const MAX_TARGETS: usize = 5;
static FALLBACK: Target = Target::Address(Ipv4Addr::new(1, 1, 1, 1));

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Target {
    Address(Ipv4Addr),
    Host(String),
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field represents an independent UI switch."
)]
pub struct Settings {
    pub enabled: bool,
    pub interval: u32,
    #[serde(deserialize_with = "tunnel_list")]
    pub tunnels: Vec<String>,
    pub targets: Vec<Target>,
    pub direct_wan: bool,
    pub history_days: u32,
    pub auto_updates: bool,
    pub auto_exit: bool,
    #[serde(deserialize_with = "tunnel_list")]
    pub auto_tunnels: Vec<String>,
    pub endpoint_diagnostics: bool,
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

impl TryFrom<String> for Target {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        value.parse()
    }
}

impl From<Target> for String {
    fn from(target: Target) -> Self {
        target.to_string()
    }
}

impl Settings {
    pub fn read() -> Result<Self> {
        let value: Self = match wayline_support::read(Path::new(SETTINGS), 32768) {
            Ok(data) => serde_json::from_slice(&data)?,
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                Self::default()
            }
            Err(error) => return Err(error),
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
        ensure!(unique(&self.tunnels), "Duplicate tunnel identity.");
        ensure!(unique(&self.targets), "Duplicate target.");
        ensure!(
            self.auto_tunnels.len() <= 8 && (!self.auto_exit || self.auto_tunnels.len() >= 2),
            "Choose at least two monitored tunnels for automatic switching."
        );
        ensure!(
            self.auto_tunnels.iter().all(|id| self.tunnels.contains(id)),
            "Automatic switching only permits monitored tunnels."
        );
        ensure!(unique(&self.auto_tunnels), "Duplicate automatic tunnel.");
        Ok(())
    }
}

fn unique<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .all(|(index, item)| !items[..index].contains(item))
}

#[cfg(test)]
mod tests {
    use super::{Settings, Target};
    use anyhow::{Result, ensure};

    fn targets(values: &[&str]) -> Result<Vec<Target>> {
        values.iter().map(|value| value.parse::<Target>()).collect()
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
    fn new_format_round_trips() -> Result<()> {
        let settings = Settings::default();
        let json = serde_json::to_string(&settings)?;
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
    fn stored_files_ignore_unknown_keys() -> Result<()> {
        let old = r#"{"target":"1.1.1.1","diagnostics":true,"historyHours":24,"tunnels":{},"autoTunnels":{}}"#;
        let settings: Settings = serde_json::from_str(old)?;
        settings.validate()?;
        ensure!(settings == Settings::default());
        Ok(())
    }

    #[test]
    fn history_retention_is_validated() -> Result<()> {
        let json = serde_json::to_string(&Settings::default())?;
        ensure!(json.contains("\"historyDays\":7"));
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
