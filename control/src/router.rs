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
use wayline_support::{digest, read, text};

const VPN_PROTOCOLS: [&str; 4] = ["wgclient", "ovpnclient", "wireguard", "openvpn"];
const NO_PEER: &str = "The tunnel has no active WireGuard peer endpoint yet.";
const OPENVPN: &str = "OpenVPN endpoint discovery is not supported.";
const IPV6: &str = "The endpoint is an IPv6 address, which Wayline does not measure.";

const IGNORED: [&str; 8] = [
    ".name",
    ".anonymous",
    "peer_id",
    "group_id",
    "client_id",
    "profiles",
    "options_in_used",
    "mark",
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

#[derive(Deserialize)]
struct Dump {
    #[serde(default)]
    interface: Vec<Interface>,
}

#[derive(Deserialize)]
struct Interface {
    #[serde(rename = "interface", default)]
    name: String,
    #[serde(default)]
    up: bool,
    #[serde(default)]
    metric: u32,
    #[serde(default)]
    proto: String,
    #[serde(default)]
    l3_device: String,
    ip4table: Option<Value>,
    #[serde(rename = "ipv4-address", default)]
    addresses: Vec<Address>,
    #[serde(default)]
    route: Vec<Route>,
}

#[derive(Deserialize)]
struct Route {
    #[serde(default)]
    target: String,
    #[serde(default)]
    mask: u8,
    table: Option<Value>,
    #[serde(rename = "type")]
    kind: Option<u32>,
}

pub struct PathInfo {
    pub id: String,
    pub key: String,
    pub name: String,
    pub interface: String,
    pub source: Ipv4Addr,
    pub mark: u32,
    pub profile: String,
    pub series_prefix: String,
    pub legacy: Option<String>,
    pub endpoint: Option<Target>,
    pub endpoint_reason: String,
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
    pub wan_reason: String,
    pub uplinks: Vec<PathInfo>,
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

impl Interface {
    fn uplink(&self) -> Option<(String, Ipv4Addr)> {
        let source = self.addresses.first()?.address;
        let default_route = self.route.iter().any(|route| {
            route.target == "0.0.0.0"
                && route.mask == 0
                && route.table.is_none()
                && route.kind != Some(6)
        });
        (self.up
            && self.l3_device.len() <= 15
            && settings::id(&self.l3_device)
            && settings::ip(source)
            && !VPN_PROTOCOLS.contains(&self.proto.as_str())
            && self.ip4table.is_none()
            && default_route)
            .then(|| (self.l3_device.clone(), source))
    }
}

impl PathInfo {
    pub fn series(&self, target: &Target) -> String {
        series_id(self.prefix(), target)
    }

    pub fn prefix(&self) -> &str {
        &self.series_prefix
    }
}

fn digest_prefix(seed: &str) -> String {
    digest(seed.as_bytes())[..48].to_owned()
}

pub fn tunnel_prefix(key: &str) -> String {
    digest_prefix(&format!("tunnel|{key}"))
}

pub fn series_id(prefix: &str, target: &Target) -> String {
    let label = match target {
        Target::Address(address) => address.to_string().replace('.', "_"),
        Target::Host(host) => digest(host.as_bytes())[..14].to_owned(),
    };
    format!("{prefix}_{label}")
}

pub struct Endpoint {
    pub key: String,
    pub target: Target,
}

pub fn endpoint_series(key: &str) -> String {
    format!("{}_server", digest_prefix(&format!("endpoint|{key}")))
}

impl Inventory {
    pub fn standby_uplinks(&self) -> impl Iterator<Item = &PathInfo> {
        self.uplinks
            .iter()
            .filter(|uplink| self.wan.as_ref().is_none_or(|wan| wan.name != uplink.name))
    }

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

fn endpoint(device: &str) -> (Option<Target>, String) {
    let output = match text("wg", &["show", device, "endpoints"], 4096, 2) {
        Ok(output) => output,
        Err(error) => return (None, format!("The wg tool failed: {error}.")),
    };
    let value = output
        .lines()
        .next()
        .and_then(|line| line.split('\t').nth(1))
        .unwrap_or("(none)");
    if value.starts_with('[') || value.matches(':').count() > 1 {
        return (None, IPV6.into());
    }
    match value
        .rsplit_once(':')
        .map(|(host, _)| host.parse::<Ipv4Addr>())
    {
        Some(Ok(address)) => (Some(Target::Address(address)), String::new()),
        _ => (None, NO_PEER.into()),
    }
}

fn path(
    section_id: &str,
    key: &str,
    name: &str,
    network: &Map<String, Value>,
    wireguard: bool,
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
    let raw = section(network, &format!("rt_{name}")).mark?;
    let (mark_text, scope) = raw.split_once('/')?;
    if number(scope) != Some(0xf000) {
        return None;
    }
    let mark = number(mark_text)?;
    let profile = section(network, name).config.unwrap_or_default();
    let (endpoint, endpoint_reason) = if wireguard {
        endpoint(&device)
    } else {
        (None, OPENVPN.into())
    };
    let legacy = digest_prefix(&format!("{key}|{name}|{device}|{source}|{mark}|{profile}"));
    Some(PathInfo {
        id: section_id.into(),
        key: key.into(),
        name: section_id.into(),
        interface: device,
        source,
        mark,
        profile,
        series_prefix: tunnel_prefix(key),
        legacy: Some(legacy),
        endpoint,
        endpoint_reason,
    })
}

fn discover_uplinks() -> Result<Vec<PathInfo>> {
    let dump: Dump = native::bus("network.interface", "dump", &json!({}))?;
    let mut candidates: Vec<_> = dump
        .interface
        .into_iter()
        .filter_map(|item| {
            let (device, source) = item.uplink()?;
            Some((item.metric, item.name, device, source))
        })
        .collect();
    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(candidates
        .into_iter()
        .map(|(_, name, device, source)| PathInfo {
            id: format!("uplink:{name}"),
            key: format!("uplink:{name}"),
            interface: device,
            source,
            mark: 0,
            profile: String::new(),
            series_prefix: digest_prefix(&format!("uplink|{name}")),
            legacy: None,
            endpoint: None,
            endpoint_reason: String::new(),
            name,
        })
        .collect())
}

fn direct(uplink: &PathInfo) -> PathInfo {
    PathInfo {
        id: "wan".into(),
        key: "wan".into(),
        name: uplink.name.clone(),
        interface: uplink.interface.clone(),
        source: uplink.source,
        mark: 0,
        profile: String::new(),
        series_prefix: digest_prefix("direct-wan"),
        legacy: Some(digest_prefix(&format!(
            "wan|{}|{}|{}|0|",
            uplink.name, uplink.interface, uplink.source
        ))),
        endpoint: None,
        endpoint_reason: String::new(),
    }
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
        let wireguard = rule.via_type.as_deref() == Some("wireguard")
            || interface.proto.as_deref() == Some("wgclient");
        if let Some(mut item) = path(id, &key, name, &network, wireguard) {
            item.name = label.into();
            paths.push(item);
        }
    }
    ensure!(
        tunnels.len() <= MAX_TUNNELS,
        "More than eight VPN tunnels are unsupported."
    );
    let (uplinks, wan, wan_reason) = match discover_uplinks() {
        Ok(list) => {
            let wan = list.first().map(direct);
            let reason = if wan.is_some() {
                String::new()
            } else {
                "No uplink interface has an IPv4 default route.".into()
            };
            (list, wan, reason)
        }
        Err(error) => (
            Vec::new(),
            None,
            format!("The uplink interface list could not be read: {error}."),
        ),
    };
    Ok(Inventory {
        tunnels,
        paths,
        wan,
        wan_reason,
        uplinks,
        rules,
    })
}

pub fn normalize(settings: &mut Settings, inventory: &Inventory) {
    for id in settings
        .tunnels
        .iter_mut()
        .chain(&mut settings.auto_tunnels)
    {
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

pub fn monitored<'a>(
    settings: &'a Settings,
    inventory: &'a Inventory,
) -> impl Iterator<Item = &'a PathInfo> {
    inventory
        .paths
        .iter()
        .filter(|item| settings.tunnels.contains(&item.key) || settings.tunnels.contains(&item.id))
}

pub const fn endpoints_enabled(settings: &Settings) -> bool {
    settings.endpoint_diagnostics
}

pub fn endpoints(settings: &Settings, inventory: &Inventory) -> Vec<Endpoint> {
    if !endpoints_enabled(settings) {
        return Vec::new();
    }
    monitored(settings, inventory)
        .filter_map(|item| {
            Some(Endpoint {
                key: item.key.clone(),
                target: item.endpoint.clone()?,
            })
        })
        .collect()
}

pub fn hosts(settings: &Settings, inventory: &Inventory) -> Vec<String> {
    let mut hosts = BTreeSet::new();
    for target in settings.targets.iter().chain(
        endpoints(settings, inventory)
            .iter()
            .map(|endpoint| &endpoint.target),
    ) {
        if let Target::Host(host) = target {
            hosts.insert(host.clone());
        }
    }
    hosts.into_iter().collect()
}

pub fn check_capacity(settings: &Settings, inventory: &Inventory) -> Result<()> {
    if !settings.enabled {
        return Ok(());
    }
    let uplinks = if settings.direct_wan {
        inventory.uplinks.len().max(1)
    } else {
        0
    };
    let endpoints = if settings.endpoint_diagnostics {
        settings.tunnels.len()
    } else {
        0
    };
    let wanted = (settings.tunnels.len() + uplinks) * settings.targets.len() + endpoints;
    ensure!(
        wanted <= MAX_SERIES,
        "This configuration needs {wanted} measurements. The limit is {MAX_SERIES}. Remove a destination, tunnel, or optional check."
    );
    Ok(())
}

pub fn monitor(settings: &Settings, inventory: &Inventory, resolved: &Resolved) -> Result<Config> {
    check_capacity(settings, inventory)?;
    let mut wanted = Vec::new();
    for item in monitored(settings, inventory) {
        wanted.extend(
            settings
                .targets
                .iter()
                .map(|target| (item, target.clone(), None)),
        );
    }
    if let Some(wan) = &inventory.wan {
        if settings.direct_wan {
            wanted.extend(
                settings
                    .targets
                    .iter()
                    .map(|target| (wan, target.clone(), None)),
            );
        }
        wanted.extend(endpoints(settings, inventory).into_iter().map(|endpoint| {
            let id = endpoint_series(&endpoint.key);
            (wan, endpoint.target, Some(id))
        }));
    }
    if settings.direct_wan {
        for uplink in inventory.standby_uplinks() {
            wanted.extend(
                settings
                    .targets
                    .iter()
                    .map(|target| (uplink, target.clone(), None)),
            );
        }
    }
    let mut seen = BTreeSet::new();
    let tunnels: Vec<_> = wanted
        .into_iter()
        .filter_map(|(path, target, id)| {
            let address = resolved.address(&target)?;
            seen.insert((path.interface.clone(), address)).then(|| {
                let id = id.unwrap_or_else(|| path.series(&target));
                measurement(path, id, address)
            })
        })
        .collect();
    ensure!(
        tunnels.len() <= MAX_SERIES,
        "The measurement limit was exceeded."
    );
    let target = resolved
        .address(settings.primary())
        .or_else(|| tunnels.first().and_then(|tunnel| tunnel.target))
        .unwrap_or(Ipv4Addr::new(1, 1, 1, 1));
    Ok(Config {
        target,
        interval_ms: u64::from(settings.interval) * 1000,
        timeout_ms: 2000,
        tunnels,
    })
}

fn measurement(path: &PathInfo, id: String, address: Ipv4Addr) -> Tunnel {
    Tunnel {
        id,
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
                && inventory.tunnels.iter().any(|item| item.id == id),
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
    use super::{
        Inventory, PathInfo, Rule, TunnelInfo, check_capacity, endpoint_series, monitor,
        safe_order, series_id,
    };
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
            profile: String::new(),
            series_prefix: "ab".repeat(24),
            legacy: None,
            endpoint: endpoint.and_then(|value| value.parse().ok()),
            endpoint_reason: String::new(),
        }
    }

    fn inventory() -> Inventory {
        Inventory {
            tunnels: Vec::new(),
            paths: vec![path("a", "wg1", Some("vpn.example.com"))],
            wan: Some(path("wan", "eth1", None)),
            wan_reason: String::new(),
            uplinks: vec![path("uplink:wwan", "wwan0", None)],
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
        let config = monitor(&settings, &inventory(), &resolved()?)?;
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
        let expected = endpoint_series("vpn:a");
        assert!(config.tunnels.iter().any(|tunnel| tunnel.id == expected));
        assert!(
            config
                .tunnels
                .iter()
                .any(|tunnel| tunnel.interface == "wwan0")
        );
        assert_eq!(
            crate::egress::scheduled_paths(&settings, &inventory()).len(),
            2
        );
        settings.direct_wan = false;
        assert_eq!(
            crate::egress::scheduled_paths(&settings, &inventory()).len(),
            1
        );
        settings.enabled = false;
        assert!(crate::egress::scheduled_paths(&settings, &inventory()).is_empty());
        settings.enabled = true;
        settings.direct_wan = true;
        settings.tunnels = (1..=8).map(|number| format!("vpn:{number}")).collect();
        settings.targets = ["1.1.1.1", "8.8.8.8", "9.9.9.9", "4.4.4.4", "5.5.5.5"]
            .iter()
            .map(|target| target.parse())
            .collect::<Result<Vec<_>>>()?;
        assert!(check_capacity(&settings, &inventory()).is_err());
        assert!(monitor(&settings, &inventory(), &resolved()?).is_err());
        settings.endpoint_diagnostics = false;
        check_capacity(&settings, &inventory())?;
        Ok(())
    }

    #[test]
    fn traffic_signatures_keep_policy_and_exclude_routing_identity() -> Result<()> {
        let left: Rule =
            serde_json::from_str(r#"{"mark":"0x1000","killswitch":"1","from":["client-a"]}"#)?;
        let right: Rule =
            serde_json::from_str(r#"{"mark":"0x2000","killswitch":"1","from":["client-a"]}"#)?;
        assert_eq!(left.signature(), right.signature());
        for changed in [
            r#"{"mark":"0x2000","killswitch":"0","from":["client-a"]}"#,
            r#"{"mark":"0x2000","killswitch":"1","from":["client-b"]}"#,
        ] {
            let changed: Rule = serde_json::from_str(changed)?;
            assert_ne!(left.signature(), changed.signature());
        }
        Ok(())
    }

    #[test]
    fn priority_can_recover_past_an_offline_tunnel() -> Result<()> {
        let mut inventory = inventory();
        inventory.paths = vec![path("b", "wg2", None)];
        inventory.tunnels = [("a", "vpn:1"), ("b", "vpn:2")]
            .into_iter()
            .map(|(id, key)| TunnelInfo {
                id: id.into(),
                key: key.into(),
                name: id.into(),
                enabled: true,
            })
            .collect();
        inventory.rules = [("a", 1, "0x1000"), ("b", 2, "0x2000")]
            .into_iter()
            .map(|(id, number, mark)| {
                let rule = serde_json::json!({
                    "tunnel_id": number.to_string(),
                    "enabled": "1",
                    "killswitch": "1",
                    "mark": mark
                });
                Ok((id.into(), serde_json::from_value(rule)?))
            })
            .collect::<Result<Vec<_>>>()?;
        let native = serde_json::from_value(serde_json::json!({"tunnels":[
            {"id":"a","tunnel_id":1,"enabled":true,"via":{"type":"wireguard"}},
            {"id":"b","tunnel_id":2,"enabled":true,"via":{"type":"wireguard"}}
        ]}))?;
        assert_eq!(
            safe_order(&native, &inventory, "b")?,
            (vec!["b".into(), "a".into()], true)
        );
        assert!(safe_order(&native, &inventory, "a").is_err());
        Ok(())
    }
}
