use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    net::Ipv4Addr,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub const RUNTIME: &str = "/var/run/wayline";
pub const SETTINGS: &str = "/etc/wayline/settings.json";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field represents an independent UI switch."
)]
pub struct Settings {
    pub enabled: bool,
    pub target: Ipv4Addr,
    pub interval: u32,
    #[serde(deserialize_with = "tunnel_list")]
    pub tunnels: Vec<String>,
    pub diagnostics: bool,
    pub targets: Vec<Ipv4Addr>,
    pub direct_wan: bool,
    pub history_hours: u32,
    pub auto_updates: bool,
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
            target: Ipv4Addr::new(1, 1, 1, 1),
            interval: 15,
            tunnels: Vec::new(),
            diagnostics: false,
            targets: vec![
                Ipv4Addr::new(1, 1, 1, 1),
                Ipv4Addr::new(8, 8, 8, 8),
                Ipv4Addr::new(9, 9, 9, 9),
            ],
            direct_wan: false,
            history_hours: 24,
            auto_updates: false,
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

pub fn directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o777 == 0o700,
        "Private directory is not secure."
    );
    Ok(())
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
        ensure!(
            ip(self.target) && [15, 30, 60].contains(&self.interval),
            "Invalid target or interval."
        );
        ensure!(
            [0, 24].contains(&self.history_hours),
            "Invalid history retention."
        );
        ensure!(
            self.tunnels.len() <= 8 && (!self.enabled || !self.tunnels.is_empty()),
            "Choose one to eight tunnels."
        );
        ensure!(
            (1..=4).contains(&self.targets.len()) && self.targets.iter().all(|value| ip(*value)),
            "Use one to four unicast IPv4 targets."
        );
        ensure!(
            self.tunnels.iter().all(|value| id(value)),
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
