use crate::{
    native::{self, List, MAX_TUNNELS},
    resolve::Resolved,
    runtime::SETTINGS,
    settings::{self, Settings, Target},
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::Ipv4Addr,
    path::Path,
};
use wayline_protocol::{Config, MAX_SERIES, Tunnel};
use wayline_support::{digest, read};

const IGNORED: [&str; 7] = [
    ".name",
    ".anonymous",
    "peer_id",
    "group_id",
    "client_id",
    "profiles",
    "options_in_used",
];

#[derive(Deserialize)]
pub struct Rule {
    #[serde(rename = ".index")]
    index: Option<u64>,
    enabled: Option<String>,
    via: Option<String>,
    via_type: Option<String>,
    tunnel_id: Option<String>,
    name: Option<String>,
    #[serde(flatten)]
    rest: BTreeMap<String, Value>,
}

#[derive(Default, Deserialize)]
struct Section {
    proto: Option<String>,
    disabled: Option<String>,
    config: Option<String>,
    mark: Option<String>,
}

#[derive(Default, Deserialize)]
struct Status {
    #[serde(default)]
    up: bool,
    l3_device: Option<String>,
    #[serde(rename = "ipv4-address", default)]
    addresses: Vec<Address>,
}

#[derive(Deserialize)]
struct Address {
    address: Ipv4Addr,
}

pub struct PathInfo {
    pub id: String,
    pub key: String,
    pub name: String,
    pub interface: String,
    pub source: Ipv4Addr,
    pub mark: u32,
    pub identity: String,
    pub endpoint: Option<Target>,
}

pub struct TunnelInfo {
    pub id: String,
    pub key: String,
    pub name: String,
    pub enabled: bool,
}

pub struct Inventory {
    pub tunnels: Vec<TunnelInfo>,
    pub paths: Vec<PathInfo>,
    pub wan: Option<PathInfo>,
    pub rules: Vec<(String, Rule)>,
}

impl Rule {
    fn signature(&self) -> Option<BTreeMap<&str, &Value>> {
        let fields: BTreeMap<_, _> = self
            .rest
            .iter()
            .filter(|(key, _)| !IGNORED.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), value))
            .collect();
        fields
            .values()
            .all(|value| {
                value.is_string()
                    || value.as_array().is_some_and(|items| {
                        items.len() <= 64 && items.iter().all(Value::is_string)
                    })
            })
            .then_some(fields)
    }
}

impl PathInfo {
    pub fn series(&self, target: &Target) -> String {
        series_id(self.prefix(), target)
    }

    pub fn prefix(&self) -> &str {
        &self.identity[..48]
    }
}

pub fn series_id(prefix: &str, target: &Target) -> String {
    let label = match target {
        Target::Address(address) => address.to_string().replace('.', "_"),
        Target::Host(host) => digest(host.as_bytes())[..14].to_owned(),
    };
    format!("{prefix}_{label}")
}

pub struct Endpoint {
    pub tunnel: String,
    pub name: String,
    pub target: Target,
}

impl Inventory {
    fn rule(&self, id: &str) -> Result<&Rule> {
        self.rules
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, rule)| rule)
            .context("The tunnel rule is unavailable.")
    }

    pub fn has_path(&self, id: &str) -> bool {
        self.paths.iter().any(|path| path.id == id)
    }
}

fn uci(name: &str) -> Result<Map<String, Value>> {
    let mut response: Map<String, Value> = native::bus("uci", "get", &json!({"config":name}))?;
    match response.remove("values") {
        Some(Value::Object(values)) => Ok(values),
        _ => anyhow::bail!("Unknown native configuration schema."),
    }
}

fn section(network: &Map<String, Value>, name: &str) -> Section {
    network
        .get(name)
        .and_then(|value| Section::deserialize(value).ok())
        .unwrap_or_default()
}

fn number(value: &str) -> Option<u32> {
    value.strip_prefix("0x").map_or_else(
        || value.parse().ok(),
        |hex| u32::from_str_radix(hex, 16).ok(),
    )
}

fn path(
    section_id: &str,
    key: &str,
    name: &str,
    network: &Map<String, Value>,
    wan: bool,
) -> Option<PathInfo> {
    let status: Status =
        native::bus(&format!("network.interface.{name}"), "status", &json!({})).ok()?;
    let device = status
        .l3_device
        .filter(|device| device.len() <= 15 && settings::id(device))?;
    let source = status.addresses.first()?.address;
    if !status.up || !settings::ip(source) {
        return None;
    }
    let mark = if wan {
        0
    } else {
        let raw = section(network, &format!("rt_{name}")).mark?;
        let (mark_text, scope) = raw.split_once('/')?;
        if number(scope) != Some(0xf000) {
            return None;
        }
        number(mark_text)?
    };
    let profile = section(network, name).config.unwrap_or_default();
    let endpoint = if wan {
        None
    } else {
        let kind = format!("wireguard_{name}");
        network
            .values()
            .filter(|value| value[".type"] == kind.as_str())
            .find_map(|value| value["endpoint_host"].as_str()?.parse().ok())
    };
    let identity = digest(format!("{key}|{name}|{device}|{source}|{mark}|{profile}").as_bytes());
    Some(PathInfo {
        id: section_id.into(),
        key: key.into(),
        name: section_id.into(),
        interface: device,
        source,
        mark,
        identity,
        endpoint,
    })
}

pub fn inventory() -> Result<Inventory> {
    let network = uci("network")?;
    let mut rules = uci("route_policy")?
        .into_iter()
        .filter(|(_, value)| value[".type"] == "rule")
        .map(|(id, value)| Ok((id, Rule::deserialize(value)?)))
        .collect::<Result<Vec<_>>>()?;
    rules.sort_by_key(|(_, rule)| rule.index.unwrap_or(u64::MAX));
    let mut paths = Vec::new();
    let mut tunnels: Vec<TunnelInfo> = Vec::new();
    for (id, rule) in &rules {
        let enabled = match rule.enabled.as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => continue,
        };
        if !settings::id(id) {
            continue;
        }
        let name = rule.via.as_deref().unwrap_or_default();
        let interface = section(&network, name);
        let Some(number) = rule
            .tunnel_id
            .as_deref()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0)
        else {
            continue;
        };
        let key = format!("vpn:{number}");
        ensure!(
            !tunnels.iter().any(|tunnel| tunnel.key == key),
            "Duplicate native tunnel number."
        );
        if !matches!(rule.via_type.as_deref(), Some("wireguard" | "openvpn"))
            && !matches!(interface.proto.as_deref(), Some("wgclient" | "ovpnclient"))
        {
            continue;
        }
        let label = rule
            .name
            .as_deref()
            .filter(|text| {
                !text.is_empty() && text.len() <= 80 && !text.chars().any(char::is_control)
            })
            .unwrap_or(id);
        tunnels.push(TunnelInfo {
            id: id.clone(),
            key: key.clone(),
            name: label.into(),
            enabled,
        });
        if !enabled || interface.disabled.as_deref() == Some("1") {
            continue;
        }
        if let Some(mut item) = path(id, &key, name, &network, false) {
            item.name = label.into();
            paths.push(item);
        }
    }
    ensure!(
        tunnels.len() <= MAX_TUNNELS,
        "More than eight VPN tunnels are unsupported."
    );
    let wan = path("wan", "wan", "wan", &network, true);
    Ok(Inventory {
        tunnels,
        paths,
        wan,
        rules,
    })
}

pub fn normalize(settings: &mut Settings, inventory: &Inventory) {
    for id in &mut settings.tunnels {
        if let Some(tunnel) = inventory.tunnels.iter().find(|item| item.id == *id) {
            id.clone_from(&tunnel.key);
        }
    }
}

pub fn native_tunnel<'a>(
    native: &'a List,
    inventory: &Inventory,
    selected: &str,
) -> Result<&'a native::Tunnel> {
    ensure!(
        native.tunnels.len() <= MAX_TUNNELS,
        "More than eight VPN tunnels are unsupported."
    );
    let mut ids = std::collections::BTreeSet::new();
    let mut numbers = std::collections::BTreeSet::new();
    for tunnel in &native.tunnels {
        ensure!(
            settings::id(&tunnel.id)
                && tunnel.number > 0
                && ids.insert(tunnel.id.as_str())
                && numbers.insert(tunnel.number),
            "Ambiguous native tunnel identity."
        );
    }
    let tunnel = native
        .tunnels
        .iter()
        .find(|item| item.id == selected)
        .context("The tunnel is unavailable.")?;
    let configured = inventory
        .tunnels
        .iter()
        .find(|item| item.id == selected)
        .context("The VPN tunnel is unavailable.")?;
    let rule = inventory.rule(selected)?;
    ensure!(
        matches!(tunnel.via.kind.as_str(), "wireguard" | "openvpn")
            && tunnel.enabled == configured.enabled
            && rule
                .tunnel_id
                .as_deref()
                .and_then(|id| id.parse::<u32>().ok())
                == Some(tunnel.number),
        "The native tunnel state differs from its configuration."
    );
    Ok(tunnel)
}

fn monitored<'a>(
    settings: &'a Settings,
    inventory: &'a Inventory,
) -> impl Iterator<Item = &'a PathInfo> {
    inventory
        .paths
        .iter()
        .filter(|item| settings.tunnels.contains(&item.key) || settings.tunnels.contains(&item.id))
}

pub fn endpoints(settings: &Settings, inventory: &Inventory) -> Vec<Endpoint> {
    if !(settings.diagnostics && settings.direct_wan && settings.endpoint_diagnostics) {
        return Vec::new();
    }
    monitored(settings, inventory)
        .filter_map(|item| {
            Some(Endpoint {
                tunnel: item.id.clone(),
                name: item.name.clone(),
                target: item.endpoint.clone()?,
            })
        })
        .collect()
}

pub fn hosts(settings: &Settings, inventory: &Inventory) -> Vec<String> {
    let mut hosts = BTreeSet::new();
    for target in std::iter::once(&settings.target)
        .chain(&settings.targets)
        .chain(
            endpoints(settings, inventory)
                .iter()
                .map(|endpoint| &endpoint.target),
        )
    {
        if let Target::Host(host) = target {
            hosts.insert(host.clone());
        }
    }
    hosts.into_iter().collect()
}

pub fn monitor(settings: &Settings, inventory: &Inventory, resolved: &Resolved) -> Config {
    let mut targets = vec![settings.target.clone()];
    if settings.diagnostics {
        for target in &settings.targets {
            if !targets.contains(target) {
                targets.push(target.clone());
            }
        }
    }
    let mut wanted = Vec::new();
    for item in monitored(settings, inventory) {
        wanted.extend(targets.iter().map(|target| (item, target.clone())));
    }
    if settings.diagnostics
        && settings.direct_wan
        && let Some(wan) = &inventory.wan
    {
        let extra = endpoints(settings, inventory)
            .into_iter()
            .map(|endpoint| endpoint.target);
        wanted.extend(
            settings
                .targets
                .iter()
                .cloned()
                .chain(extra)
                .map(|target| (wan, target)),
        );
    }
    let mut seen = BTreeSet::new();
    let tunnels: Vec<_> = wanted
        .into_iter()
        .filter_map(|(path, target)| {
            let address = resolved.address(&target)?;
            seen.insert((path.interface.clone(), address))
                .then(|| measurement(path, &target, address))
        })
        .take(MAX_SERIES)
        .collect();
    let target = resolved
        .address(&settings.target)
        .or_else(|| tunnels.first().and_then(|tunnel| tunnel.target))
        .unwrap_or(Ipv4Addr::new(1, 1, 1, 1));
    Config {
        target,
        interval_ms: u64::from(settings.interval) * 1000,
        timeout_ms: 2000,
        tunnels,
    }
}

fn measurement(path: &PathInfo, target: &Target, address: Ipv4Addr) -> Tunnel {
    Tunnel {
        id: path.series(target),
        name: path.name.clone(),
        interface: path.interface.clone(),
        source: path.source,
        mark: path.mark,
        target: Some(address),
        path_id: path.id.clone(),
    }
}

pub fn revision() -> Result<String> {
    let data = [
        read(Path::new(SETTINGS), 32_768).unwrap_or_default(),
        read(Path::new("/etc/config/route_policy"), 131_072)?,
        read(Path::new("/etc/config/network"), 131_072)?,
    ]
    .concat();
    Ok(digest(&data))
}

pub fn safe_order(
    native: &List,
    inventory: &Inventory,
    selected: &str,
) -> Result<(Vec<String>, bool)> {
    ensure!(
        !native.tunnels.is_empty() && native.tunnels.len() <= MAX_TUNNELS,
        "The tunnel group is not eligible."
    );
    let mut signature = None;
    let mut ids: Vec<String> = Vec::new();
    for tunnel in &native.tunnels {
        let id = tunnel.id.as_str();
        native_tunnel(native, inventory, id)?;
        ensure!(
            settings::id(id)
                && !ids.iter().any(|other| other == id)
                && inventory
                    .tunnels
                    .iter()
                    .any(|item| item.id == id && (!item.enabled || inventory.has_path(id))),
            "The tunnel group is not eligible."
        );
        let fields = inventory
            .rule(id)?
            .signature()
            .context("Unsupported rule fields prevent selection.")?;
        match &signature {
            Some(previous) => ensure!(
                &fields == previous,
                "Different traffic rules prevent selection."
            ),
            None => signature = Some(fields),
        }
        ids.push(id.to_owned());
    }
    ensure!(
        inventory.has_path(selected),
        "The selected tunnel is not connected."
    );
    let index = ids
        .iter()
        .position(|id| id == selected)
        .context("The selected tunnel is unavailable.")?;
    let changed = index != 0;
    if changed {
        let id = ids.remove(index);
        ids.insert(0, id);
    }
    Ok((ids, changed))
}

#[cfg(test)]
mod tests {
    use super::{Inventory, PathInfo, monitor, series_id};
    use crate::{
        resolve::Resolved,
        settings::{Settings, Target},
    };
    use anyhow::Result;
    use std::net::Ipv4Addr;

    fn path(id: &str, interface: &str, endpoint: Option<&str>) -> PathInfo {
        PathInfo {
            id: id.into(),
            key: format!("vpn:{id}"),
            name: id.into(),
            interface: interface.into(),
            source: Ipv4Addr::new(10, 0, 0, 2),
            mark: 4096,
            identity: "ab".repeat(32),
            endpoint: endpoint.and_then(|value| value.parse().ok()),
        }
    }

    fn inventory() -> Inventory {
        Inventory {
            tunnels: Vec::new(),
            paths: vec![path("a", "wg1", Some("vpn.example.com"))],
            wan: Some(path("wan", "eth1", None)),
            rules: Vec::new(),
        }
    }

    fn resolved() -> Result<Resolved> {
        Ok(serde_json::from_str(
            r#"{"at":0,"hosts":{"dns.example.com":["1.1.1.1"],"vpn.example.com":["203.0.113.9"]}}"#,
        )?)
    }

    #[test]
    fn host_series_keep_one_identity_per_name() -> Result<()> {
        let host: Target = "dns.example.com".parse()?;
        let address: Target = "9.9.9.9".parse()?;
        assert_eq!(series_id("p", &address), "p_9_9_9_9");
        assert_eq!(series_id("p", &host), series_id("p", &host));
        assert!(series_id(&"x".repeat(48), &host).len() <= 64);
        Ok(())
    }

    #[test]
    fn monitor_skips_duplicates_and_unresolved_names() -> Result<()> {
        let mut settings = Settings {
            enabled: true,
            diagnostics: true,
            direct_wan: true,
            endpoint_diagnostics: true,
            tunnels: vec!["vpn:a".into()],
            ..Settings::default()
        };
        settings.targets = vec![
            "dns.example.com".parse()?,
            "1.1.1.1".parse()?,
            "missing.example.com".parse()?,
        ];
        let config = monitor(&settings, &inventory(), &resolved()?);
        let on_tunnel = config
            .tunnels
            .iter()
            .filter(|tunnel| tunnel.interface == "wg1")
            .count();
        assert_eq!(on_tunnel, 1);
        let on_wan: Vec<_> = config
            .tunnels
            .iter()
            .filter(|tunnel| tunnel.interface == "eth1")
            .filter_map(|tunnel| tunnel.target)
            .collect();
        assert_eq!(
            on_wan,
            [Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(203, 0, 113, 9)]
        );
        Ok(())
    }
}
