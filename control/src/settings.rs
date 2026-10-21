use crate::runtime::SETTINGS;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fmt, net::Ipv4Addr, path::Path, str::FromStr};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Address(Ipv4Addr),
    Host(String),
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field represents an independent UI switch."
)]
pub struct Settings {
    pub enabled: bool,
    pub target: Target,
    pub interval: u32,
    #[serde(deserialize_with = "tunnel_list")]
    pub tunnels: Vec<String>,
    pub diagnostics: bool,
    pub targets: Vec<Target>,
    pub direct_wan: bool,
    pub history_hours: u32,
    pub auto_updates: bool,
    #[serde(default)]
    pub auto_exit: bool,
    #[serde(default)]
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
            target: Target::Address(Ipv4Addr::new(1, 1, 1, 1)),
            interval: 15,
            tunnels: Vec::new(),
            diagnostics: false,
            targets: vec![
                Target::Address(Ipv4Addr::new(1, 1, 1, 1)),
                Target::Address(Ipv4Addr::new(8, 8, 8, 8)),
                Target::Address(Ipv4Addr::new(9, 9, 9, 9)),
            ],
            direct_wan: false,
            history_hours: 24,
            auto_updates: false,
            auto_exit: false,
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

    pub fn validate(&self) -> Result<()> {
        ensure!([15, 30, 60].contains(&self.interval), "Invalid interval.");
        ensure!(
            [0, 24].contains(&self.history_hours),
            "Invalid history retention."
        );
        ensure!(
            self.tunnels.len() <= 8 && (!self.enabled || !self.tunnels.is_empty()),
            "Choose one to eight tunnels."
        );
        ensure!(
            (1..=4).contains(&self.targets.len()),
            "Use one to four targets."
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Settings, Target};
    use anyhow::Result;

    #[test]
    fn targets() -> Result<()> {
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
            assert!(bad.parse::<Target>().is_err(), "{bad}");
        }
        Ok(())
    }

    #[test]
    fn legacy_settings_load() -> Result<()> {
        let old = r#"{"enabled":false,"target":"1.1.1.1","interval":15,"tunnels":{},"diagnostics":false,"targets":["8.8.8.8"],"directWan":false,"historyHours":24,"autoUpdates":false}"#;
        let settings: Settings = serde_json::from_str(old)?;
        settings.validate()?;
        assert!(!settings.auto_exit && !settings.endpoint_diagnostics);
        Ok(())
    }
}
