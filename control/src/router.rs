use crate::settings::{self, Settings};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
use wayline_support::{digest, read, text};

const CONTRACT: &str = "9c8a9102c046ff12ba95e4c9a03b4e5392c461584c5d4d8ab9c5591ab6ec9237";

pub struct PathInfo {
    pub id: String,
    pub name: String,
    pub interface: String,
    pub source: String,
    pub mark: u32,
    pub identity: String,
}

pub struct Inventory {
    pub paths: Vec<PathInfo>,
    pub wan: Option<PathInfo>,
    pub rules: Vec<(String, Value)>,
}

pub fn contract() -> bool {
    read(Path::new("/usr/lib/oui-httpd/rpc/vpn-client"), 131_072)
        .is_ok_and(|data| digest(&data) == CONTRACT)
        && fs::read_to_string("/etc/glversion").is_ok_and(|value| value.trim() == "4.9.0")
}

pub fn bus(object: &str, method: &str, params: &Value) -> Result<Value> {
    let encoded = serde_json::to_string(params)?;
    Ok(serde_json::from_str(&text(
        "ubus",
        &["-t", "3", "call", object, method, &encoded],
        262_144,
        4,
    )?)?)
}

pub fn native(method: &str, params: &Value) -> Result<Value> {
    let response = bus(
        "gl-session",
        "call",
        &json!({"module":"vpn-client","func":method,"params":params}),
    )?;
    response
        .get("result")
        .cloned()
        .context("Unknown native RPC response.")
}

fn config(name: &str) -> Result<Value> {
    let response = bus("uci", "get", &json!({"config":name}))?;
    response
        .get("values")
        .cloned()
        .context("Unknown native configuration schema.")
}

fn number(value: &str) -> Option<u32> {
    value.strip_prefix("0x").map_or_else(
        || value.parse().ok(),
        |hex| u32::from_str_radix(hex, 16).ok(),
    )
}

fn path(
    section: &str,
    name: &str,
    interface: &Value,
    network: &Value,
    wan: bool,
) -> Option<PathInfo> {
    let status = bus(&format!("network.interface.{name}"), "status", &json!({})).ok()?;
    if status["up"] != true {
        return None;
    }
    let device = status["l3_device"].as_str()?;
    let source = status["ipv4-address"][0]["address"].as_str()?;
    if device.len() > 15 || !settings::id(device) || !settings::ip(source.parse().ok()?) {
        return None;
    }
    let mark = if wan {
        0
    } else {
        let raw = network[format!("rt_{name}")]["mark"].as_str()?;
        let (mark_text, scope) = raw.split_once('/')?;
        if number(scope) != Some(0xf000) {
            return None;
        }
        number(mark_text)?
    };
    let profile = interface["config"].as_str().unwrap_or_default();
    let identity =
        digest(format!("{section}|{name}|{device}|{source}|{mark}|{profile}").as_bytes());
    Some(PathInfo {
        id: section.into(),
        name: section.into(),
        interface: device.into(),
        source: source.into(),
        mark,
        identity,
    })
}

pub fn inventory() -> Result<Inventory> {
    let network = config("network")?;
    let policy = config("route_policy")?;
    let mut rules: Vec<_> = policy
        .as_object()
        .context("Unknown policy schema.")?
        .iter()
        .filter(|(_, value)| value[".type"] == "rule")
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect();
    rules.sort_by_key(|(_, value)| value[".index"].as_u64().unwrap_or(u64::MAX));
    let mut paths = Vec::new();
    for (id, rule) in &rules {
        if rule["enabled"] != "1" || !settings::id(id) {
            continue;
        }
        let Some(name) = rule["via"].as_str() else {
            continue;
        };
        let interface = &network[name];
        if ![Some("wgclient"), Some("ovpnclient")].contains(&interface["proto"].as_str())
            || interface["disabled"] == "1"
        {
            continue;
        }
        if let Some(mut item) = path(id, name, interface, &network, false) {
            if let Some(label) = rule["name"].as_str().filter(|text| {
                !text.is_empty() && text.len() <= 80 && !text.chars().any(char::is_control)
            }) {
                item.name = label.into();
            }
            paths.push(item);
        }
    }
    ensure!(
        paths.len() <= 8,
        "More than eight active tunnels are unsupported."
    );
    let wan = path("wan", "wan", &network["wan"], &network, true);
    Ok(Inventory { paths, wan, rules })
}

impl PathInfo {
    pub fn series(&self, target: &str) -> String {
        format!("{}_{}", &self.identity[..48], target.replace('.', "_"))
    }
}

pub fn monitor(settings: &Settings, inventory: &Inventory) -> Value {
    let mut targets = vec![settings.target];
    if settings.diagnostics {
        for target in &settings.targets {
            if !targets.contains(target) {
                targets.push(*target);
            }
        }
    }
    let mut series = Vec::new();
    for item in &inventory.paths {
        if settings.tunnels.contains(&item.id) {
            for target in &targets {
                series.push(measurement(item, &target.to_string()));
            }
        }
    }
    if settings.diagnostics
        && settings.direct_wan
        && let Some(wan) = &inventory.wan
    {
        for target in &settings.targets {
            series.push(measurement(wan, &target.to_string()));
        }
    }
    json!({"target":settings.target,"interval_ms":settings.interval*1000,"timeout_ms":2000,"tunnels":series})
}

fn measurement(path: &PathInfo, target: &str) -> Value {
    json!({"id":path.series(target),"path_id":path.id,"name":path.name,"interface":path.interface,"source":path.source,"mark":path.mark,"target":target})
}

pub fn revision() -> Result<String> {
    let data = [
        read(Path::new(settings::SETTINGS), 32_768).unwrap_or_default(),
        read(Path::new("/etc/config/route_policy"), 131_072)?,
        read(Path::new("/etc/config/network"), 131_072)?,
    ]
    .concat();
    Ok(digest(&data))
}

pub fn safe_order(
    native: &Value,
    inventory: &Inventory,
    selected: &str,
) -> Result<(Vec<String>, bool)> {
    let tunnels = native["tunnels"]
        .as_array()
        .context("Unknown native tunnel schema.")?;
    ensure!(
        !tunnels.is_empty() && tunnels.len() <= 8,
        "The tunnel group is not eligible."
    );
    let ignored = [
        ".name",
        ".index",
        ".anonymous",
        "tunnel_id",
        "via",
        "via_type",
        "peer_id",
        "group_id",
        "client_id",
        "name",
        "options_in_used",
    ];
    let mut signature = None;
    let mut ids = Vec::new();
    for tunnel in tunnels {
        let id = tunnel["id"]
            .as_str()
            .context("Unknown native tunnel identity.")?;
        ensure!(
            settings::id(id)
                && !ids.iter().any(|other| other == id)
                && inventory.paths.iter().any(|item| item.id == id),
            "The tunnel group is not eligible."
        );
        let rule = &inventory
            .rules
            .iter()
            .find(|(name, _)| name == id)
            .context("The tunnel rule is unavailable.")?
            .1;
        let fields: BTreeMap<_, _> = rule
            .as_object()
            .context("Unknown rule schema.")?
            .iter()
            .filter(|(key, _)| !ignored.contains(&key.as_str()))
            .collect();
        ensure!(
            fields.values().all(|value| value.is_string()
                || value
                    .as_array()
                    .is_some_and(|items| items.len() <= 64 && items.iter().all(Value::is_string))),
            "Unsupported rule fields prevent selection."
        );
        if let Some(previous) = &signature {
            ensure!(
                &fields == previous,
                "Different traffic rules prevent selection."
            );
        } else {
            signature = Some(fields);
        }
        ids.push(id.to_owned());
    }
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
