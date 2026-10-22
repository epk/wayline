use crate::{
    native::{self, List, MAX_TUNNELS},
    resolve::Resolved,
    runtime::SETTINGS,
    settings::{self, Settings, Target},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::{Map, Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashSet},
    net::Ipv4Addr,
    path::Path,
    sync::Arc,
};
use wayline_protocol::{Config, MAX_SERIES, Tunnel};
use wayline_support::{digest, read, text};

const VPN_PROTOCOLS: [&str; 4] = ["wgclient", "ovpnclient", "wireguard", "openvpn"];
const NO_PEER: &str = "The tunnel has no active WireGuard peer endpoint yet.";
const OPENVPN: &str = "OpenVPN endpoint discovery is not supported.";
const IPV6: &str = "The endpoint is an IPv6 address, which Wayline does not measure.";
const TOO_MANY: &str = "More than eight VPN tunnels are unsupported.";

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

type Found = (PathInfo, bool);

thread_local! {
    static CACHE: RefCell<Option<Inventory>> = const { RefCell::new(None) };
}

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
struct Section<'a> {
    proto: Option<&'a str>,
    disabled: Option<&'a str>,
    config: Option<&'a str>,
    mark: Option<&'a str>,
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
    ip4table: Option<IgnoredAny>,
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
    table: Option<IgnoredAny>,
    #[serde(rename = "type")]
    kind: Option<u32>,
}

#[derive(Clone)]
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

#[derive(Clone)]
pub struct TunnelInfo {
    pub id: String,
    pub key: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone)]
pub struct Inventory {
    pub tunnels: Vec<TunnelInfo>,
    pub paths: Vec<PathInfo>,
    pub wan: Option<PathInfo>,
    pub wan_reason: String,
    pub uplinks: Vec<PathInfo>,
    pub rules: Arc<[(String, Rule)]>,
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
    fn source(&self) -> Option<Ipv4Addr> {
        let source = self.addresses.first()?.address;
        (self.up
            && self.l3_device.len() <= 15
            && settings::id(&self.l3_device)
            && settings::ip(source))
        .then_some(source)
    }

    fn uplink(&self) -> Option<Ipv4Addr> {
        let source = self.source()?;
        let default_route = self.route.iter().any(|route| {
            route.target == "0.0.0.0"
                && route.mask == 0
                && route.table.is_none()
                && route.kind != Some(6)
        });
        (!VPN_PROTOCOLS.contains(&self.proto.as_str()) && self.ip4table.is_none() && default_route)
            .then_some(source)
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

fn section<'a>(network: &'a Map<String, Value>, name: &str) -> Section<'a> {
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

fn mark(raw: &str) -> Option<u32> {
    let (value, scope) = raw.split_once('/')?;
    if number(scope) != Some(0xf000) {
        return None;
    }
    number(value)
}

fn endpoint(listing: &Result<String>, device: &str) -> (Option<Target>, String) {
    let output = match listing {
        Ok(output) => output,
        Err(error) => return (None, format!("The wg tool failed: {error}.")),
    };
    let value = output
        .lines()
        .find_map(|line| {
            let mut columns = line.split('\t');
            (columns.next() == Some(device))
                .then(|| columns.nth(1))
                .flatten()
        })
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
    (id, key, label): (&str, &str, &str),
    name: &str,
    network: &Map<String, Value>,
    interfaces: &[Interface],
) -> Option<PathInfo> {
    let interface = interfaces.iter().find(|item| item.name == name)?;
    let source = interface.source()?;
    let mark = mark(section(network, &format!("rt_{name}")).mark?)?;
    let profile = section(network, name).config.unwrap_or_default();
    let device = &interface.l3_device;
    Some(PathInfo {
        id: id.into(),
        key: key.into(),
        name: label.into(),
        interface: device.clone(),
        source,
        mark,
        profile: profile.into(),
        series_prefix: tunnel_prefix(key),
        legacy: Some(digest_prefix(&format!(
            "{key}|{name}|{device}|{source}|{mark}|{profile}"
        ))),
        endpoint: None,
        endpoint_reason: String::new(),
    })
}

fn native_tunnels(
    network: &Map<String, Value>,
    rules: &[(String, Rule)],
    interfaces: &[Interface],
) -> Result<(Vec<TunnelInfo>, Vec<Found>)> {
    let mut tunnels: Vec<TunnelInfo> = Vec::new();
    let mut found = Vec::new();
    for (id, rule) in rules {
        let enabled = match rule.enabled.as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => continue,
        };
        if !settings::id(id) {
            continue;
        }
        let name = rule.via.as_deref().unwrap_or_default();
        let interface = section(network, name);
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
            && !matches!(interface.proto, Some("wgclient" | "ovpnclient"))
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
        if !enabled || interface.disabled == Some("1") {
            continue;
        }
        let wireguard =
            rule.via_type.as_deref() == Some("wireguard") || interface.proto == Some("wgclient");
        if let Some(item) = path((id, &key, label), name, network, interfaces) {
            found.push((item, wireguard));
        }
    }
    ensure!(tunnels.len() <= MAX_TUNNELS, TOO_MANY);
    Ok((tunnels, found))
}

fn with_endpoints(found: Vec<Found>, wanted: bool) -> Vec<PathInfo> {
    let listing = (wanted && found.iter().any(|(_, wireguard)| *wireguard))
        .then(|| text("wg", &["show", "all", "endpoints"], 16_384, 2));
    found
        .into_iter()
        .map(|(mut path, wireguard)| {
            (path.endpoint, path.endpoint_reason) = match (wireguard, &listing) {
                (false, _) => (None, OPENVPN.into()),
                (true, None) => (None, String::new()),
                (true, Some(listing)) => endpoint(listing, &path.interface),
            };
            path
        })
        .collect()
}

fn uplinks(interfaces: &[Interface]) -> Vec<PathInfo> {
    let mut candidates: Vec<_> = interfaces
        .iter()
        .filter_map(|item| Some((item, item.uplink()?)))
        .collect();
    candidates.sort_by(|(left, _), (right, _)| {
        (left.metric, &left.name).cmp(&(right.metric, &right.name))
    });
    candidates
        .into_iter()
        .map(|(item, source)| {
            let id = format!("uplink:{}", item.name);
            PathInfo {
                key: id.clone(),
                id,
                name: item.name.clone(),
                interface: item.l3_device.clone(),
                source,
                mark: 0,
                profile: String::new(),
                series_prefix: digest_prefix(&format!("uplink|{}", item.name)),
                legacy: None,
                endpoint: None,
                endpoint_reason: String::new(),
            }
        })
        .collect()
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

fn discover(endpoints: bool) -> Result<Inventory> {
    let network = uci("network")?;
    let mut rules = uci("route_policy")?
        .into_iter()
        .filter(|(_, value)| value[".type"] == "rule")
        .map(|(id, value)| Ok((id, Rule::deserialize(value)?)))
        .collect::<Result<Vec<_>>>()?;
    rules.sort_by_key(|(_, rule)| rule.index.unwrap_or(u64::MAX));
    let dump = native::bus::<Dump>("network.interface", "dump", &json!({}));
    let interfaces = dump.as_ref().map_or(&[][..], |dump| &dump.interface);
    let (tunnels, found) = native_tunnels(&network, &rules, interfaces)?;
    if let Err(error) = &dump
        && tunnels.iter().any(|tunnel| tunnel.enabled)
    {
        anyhow::bail!("The interface list could not be read: {error}.");
    }
    let (uplinks, wan, wan_reason) = match &dump {
        Ok(_) => {
            let uplinks = uplinks(interfaces);
            let wan = uplinks.first().map(direct);
            let reason = if wan.is_some() {
                String::new()
            } else {
                "No uplink interface has an IPv4 default route.".into()
            };
            (uplinks, wan, reason)
        }
        Err(error) => (
            Vec::new(),
            None,
            format!("The uplink interface list could not be read: {error}."),
        ),
    };
    Ok(Inventory {
        tunnels,
        paths: with_endpoints(found, endpoints),
        wan,
        wan_reason,
        uplinks,
        rules: rules.into(),
    })
}

pub fn inventory() -> Result<Inventory> {
    inventory_with(true)
}

pub fn inventory_with(endpoints: bool) -> Result<Inventory> {
    if let Some(cached) = CACHE.with_borrow(Clone::clone) {
        return Ok(cached);
    }
    let fresh = discover(endpoints)?;
    if endpoints {
        CACHE.set(Some(fresh.clone()));
    }
    Ok(fresh)
}

pub fn forget() {
    CACHE.take();
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

fn unambiguous(native: &List) -> Result<()> {
    ensure!(native.tunnels.len() <= MAX_TUNNELS, TOO_MANY);
    let mut ids = BTreeSet::new();
    let mut numbers = BTreeSet::new();
    ensure!(
        native.tunnels.iter().all(|tunnel| settings::id(&tunnel.id)
            && tunnel.number > 0
            && ids.insert(tunnel.id.as_str())
            && numbers.insert(tunnel.number)),
        "Ambiguous native tunnel identity."
    );
    Ok(())
}

fn consistent(tunnel: &native::Tunnel, inventory: &Inventory) -> Result<()> {
    let configured = inventory
        .tunnels
        .iter()
        .find(|item| item.id == tunnel.id)
        .context("The VPN tunnel is unavailable.")?;
    let rule = inventory.rule(&tunnel.id)?;
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
    Ok(())
}

pub fn native_tunnel<'a>(
    native: &'a List,
    inventory: &Inventory,
    selected: &str,
) -> Result<&'a native::Tunnel> {
    unambiguous(native)?;
    let tunnel = native
        .tunnels
        .iter()
        .find(|item| item.id == selected)
        .context("The tunnel is unavailable.")?;
    consistent(tunnel, inventory)?;
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

pub fn endpoints<'a>(
    settings: &'a Settings,
    inventory: &'a Inventory,
) -> impl Iterator<Item = (&'a str, &'a Target)> {
    monitored(settings, inventory)
        .filter(|_| endpoints_enabled(settings))
        .filter_map(|item| Some((item.key.as_str(), item.endpoint.as_ref()?)))
}

pub fn hosts(settings: &Settings, inventory: &Inventory) -> Vec<String> {
    settings
        .targets
        .iter()
        .chain(endpoints(settings, inventory).map(|(_, target)| target))
        .filter_map(|target| match target {
            Target::Host(host) => Some(host.as_str()),
            Target::Address(_) => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
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
        "This configuration needs {wanted} measurements, and the limit is {MAX_SERIES}. The lowest-priority measurements are skipped. Remove a destination, tunnel, or optional check to measure everything."
    );
    Ok(())
}

pub fn monitor<'a>(
    settings: &'a Settings,
    inventory: &'a Inventory,
    resolved: &Resolved,
) -> Config {
    let per_target = |path: &'a PathInfo| {
        settings
            .targets
            .iter()
            .map(move |target| (path, target, None))
    };
    let wan = || inventory.wan.iter();
    let wanted = monitored(settings, inventory)
        .flat_map(per_target)
        .chain(wan().filter(|_| settings.direct_wan).flat_map(per_target))
        .chain(wan().flat_map(|wan| {
            endpoints(settings, inventory).map(move |(key, target)| (wan, target, Some(key)))
        }))
        .chain(
            inventory
                .standby_uplinks()
                .filter(|_| settings.direct_wan)
                .flat_map(per_target),
        );
    let mut seen = HashSet::new();
    let tunnels: Vec<_> = wanted
        .filter_map(|(path, target, endpoint)| {
            let address = resolved.address(target)?;
            seen.insert((path.interface.as_str(), address)).then(|| {
                let id = endpoint.map_or_else(|| path.series(target), endpoint_series);
                measurement(path, id, address)
            })
        })
        .take(MAX_SERIES)
        .collect();
    let target = resolved
        .address(settings.primary())
        .or_else(|| tunnels.first().and_then(|tunnel| tunnel.target))
        .unwrap_or(Ipv4Addr::new(1, 1, 1, 1));
    Config {
        target,
        interval_ms: u64::from(settings.interval) * 1000,
        timeout_ms: 2000,
        tunnels,
    }
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
    let mut data = read(Path::new(SETTINGS), 32_768).unwrap_or_default();
    for path in ["/etc/config/route_policy", "/etc/config/network"] {
        data.extend(read(Path::new(path), 131_072)?);
    }
    Ok(digest(&data))
}

pub fn native_order<'a>(native: &'a List, inventory: &Inventory) -> Result<Vec<&'a str>> {
    ensure!(
        !native.tunnels.is_empty() && native.tunnels.len() <= MAX_TUNNELS,
        "The tunnel group is not eligible."
    );
    unambiguous(native)?;
    let mut signature = None;
    for tunnel in &native.tunnels {
        consistent(tunnel, inventory)?;
        let fields = inventory
            .rule(&tunnel.id)?
            .signature()
            .context("Unsupported rule fields prevent selection.")?;
        match &signature {
            Some(previous) => ensure!(
                &fields == previous,
                "Different traffic rules prevent selection."
            ),
            None => signature = Some(fields),
        }
    }
    Ok(native
        .tunnels
        .iter()
        .map(|tunnel| tunnel.id.as_str())
        .collect())
}

pub fn position(order: &[&str], inventory: &Inventory, selected: &str) -> Result<usize> {
    ensure!(
        inventory.has_path(selected),
        "The selected tunnel is not connected."
    );
    order
        .iter()
        .position(|id| *id == selected)
        .context("The selected tunnel is unavailable.")
}

#[cfg(test)]
mod tests {
    use super::{
        Inventory, PathInfo, Rule, TunnelInfo, check_capacity, endpoint, endpoint_series, mark,
        monitor, native_order, position, series_id,
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
            rules: Vec::new().into(),
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
        check_capacity(&settings, &inventory())?;
        let mut crowded = inventory();
        crowded.uplinks = (0..9)
            .map(|index| path(&format!("uplink:u{index}"), &format!("eth{index}"), None))
            .collect();
        assert!(check_capacity(&settings, &crowded).is_err());
        assert!(
            monitor(&settings, &crowded, &resolved()?).tunnels.len()
                <= wayline_protocol::MAX_SERIES
        );
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
            .collect::<Result<Vec<_>>>()?
            .into();
        let native = serde_json::from_value(serde_json::json!({"tunnels":[
            {"id":"a","tunnel_id":1,"enabled":true,"via":{"type":"wireguard"}},
            {"id":"b","tunnel_id":2,"enabled":true,"via":{"type":"wireguard"}}
        ]}))?;
        let order = native_order(&native, &inventory)?;
        assert_eq!(order, ["a", "b"]);
        assert_eq!(position(&order, &inventory, "b")?, 1);
        assert!(position(&order, &inventory, "a").is_err());
        Ok(())
    }

    #[test]
    fn marks_and_endpoints_parse() -> Result<()> {
        assert_eq!(mark("0x1000/0xf000"), Some(0x1000));
        assert_eq!(mark("0x1000/0xff00"), None);
        assert_eq!(mark("4096"), None);
        let listing =
            Ok("wg0\tkey\t203.0.113.9:51820\nwg1\tkey\t(none)\nwg2\tkey\t[::1]:1\n".into());
        let address: Target = "203.0.113.9".parse()?;
        assert!(endpoint(&listing, "wg0").0 == Some(address));
        assert!(endpoint(&listing, "wg1").0.is_none());
        assert!(endpoint(&listing, "wg2").1.contains("IPv6"));
        assert!(endpoint(&listing, "wg3").0.is_none());
        Ok(())
    }
}
