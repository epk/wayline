use crate::{
    auto, history, native,
    resolve::Resolved,
    router,
    runtime::{RUNTIME, SETTINGS, locked, milliseconds, snapshot, version},
    service::reconcile,
    settings::{self, Settings, Target},
    update,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, net::Ipv4Addr, path::Path};
use wayline_protocol::{Series, State as Measurement};
use wayline_support::atomic;

const STALE: &str = "State changed. Refresh first.";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    version: String,
    config: Settings,
    pub tunnels: Vec<Tunnel>,
    diagnostics: BTreeMap<String, BTreeMap<String, Option<f64>>>,
    endpoints: Vec<EndpointView>,
    resolved: BTreeMap<String, Option<Ipv4Addr>>,
    auto: AutoView,
    revision: String,
    scope: &'static str,
    capabilities: Capabilities,
    compat: native::Compat,
    update: Option<update::Status>,
}

#[derive(Serialize)]
struct EndpointView {
    tunnel: String,
    name: String,
    address: String,
}

#[derive(Serialize)]
struct AutoView {
    enabled: bool,
    last: Option<auto::Switch>,
}

#[derive(Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field represents an independent capability."
)]
struct Capabilities {
    configure: bool,
    select: bool,
    control: bool,
    update: bool,
    reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each field represents an independent UI flag."
)]
pub struct Tunnel {
    pub id: String,
    pub monitor_id: String,
    pub name: String,
    pub enabled: bool,
    connected: bool,
    history_available: bool,
    controllable: bool,
    pub preferred: bool,
    pub eligible: bool,
    reason: &'static str,
    state_label: &'static str,
    measurement_state: Option<Measurement>,
    age: String,
    pub median: Option<f64>,
    samples: usize,
    lost: usize,
    recent: Vec<Option<f64>>,
}

#[derive(Deserialize)]
pub struct SetConfig {
    expected_revision: String,
    config: Settings,
}

#[derive(Deserialize)]
pub struct Select {
    section_id: String,
    expected_revision: String,
}

#[derive(Deserialize)]
pub struct SetEnabled {
    section_id: String,
    enabled: bool,
    expected_revision: String,
}

fn measurement<'a>(
    snapshot: &'a [Series],
    path: &router::PathInfo,
    target: &Target,
) -> Option<&'a Series> {
    let id = path.series(target);
    snapshot.iter().find(|series| series.id == id)
}

fn diagnostics(
    config: &Settings,
    inventory: &router::Inventory,
    endpoints: &[router::Endpoint],
    snapshot: &[Series],
) -> BTreeMap<String, BTreeMap<String, Option<f64>>> {
    let mut result: BTreeMap<_, _> = config
        .targets
        .iter()
        .map(|target| {
            let values = inventory
                .paths
                .iter()
                .chain(inventory.wan.iter())
                .map(|path| {
                    let latency = measurement(snapshot, path, target)
                        .and_then(|item| item.median_us)
                        .map(milliseconds);
                    (path.id.clone(), latency)
                })
                .collect();
            (target.to_string(), values)
        })
        .collect();
    if let Some(wan) = &inventory.wan {
        for endpoint in endpoints {
            result
                .entry(endpoint.target.to_string())
                .or_insert_with(|| {
                    let latency = measurement(snapshot, wan, &endpoint.target)
                        .and_then(|item| item.median_us)
                        .map(milliseconds);
                    BTreeMap::from([(wan.id.clone(), latency)])
                });
        }
    }
    result
}

fn resolved(
    config: &Settings,
    inventory: &router::Inventory,
) -> BTreeMap<String, Option<Ipv4Addr>> {
    let cache = Resolved::read();
    router::hosts(config, inventory)
        .into_iter()
        .map(|host| {
            let address = cache.address(&Target::Host(host.clone()));
            (host, address)
        })
        .collect()
}

fn tunnels(
    config: &Settings,
    inventory: &router::Inventory,
    snapshot: &[Series],
    native: &Result<native::List>,
    compatible: bool,
) -> Vec<Tunnel> {
    let known = history::Known::read(config);
    let preferred = native
        .as_ref()
        .ok()
        .and_then(|list| list.tunnels.first())
        .map(|tunnel| tunnel.id.as_str());
    let mut tunnels = Vec::new();
    for tunnel in &inventory.tunnels {
        let path = inventory.paths.iter().find(|path| path.id == tunnel.id);
        let item = path.and_then(|path| measurement(snapshot, path, &config.target));
        let eligible = compatible
            && item.is_some_and(|item| {
                item.state.live()
                    && item.lost() == 0
                    && item
                        .success_age_ms
                        .is_some_and(|age| age <= u64::from(config.interval) * 3000)
            })
            && native
                .as_ref()
                .is_ok_and(|list| router::safe_order(list, inventory, &tunnel.id).is_ok());
        tunnels.push(Tunnel {
            id: tunnel.id.clone(),
            monitor_id: tunnel.key.clone(),
            name: tunnel.name.clone(),
            enabled: tunnel.enabled,
            connected: path.is_some(),
            history_available: path.is_some() || known.contains(&tunnel.key),
            controllable: compatible
                && native.as_ref().is_ok_and(|list| {
                    router::native_tunnel(list, inventory, &tunnel.id)
                        .is_ok_and(|native| tunnel.enabled || native.startable())
                }),
            preferred: preferred == Some(tunnel.id.as_str()),
            eligible,
            reason: if eligible {
                ""
            } else {
                "The traffic rules or recent measurements prevent selection."
            },
            state_label: if !tunnel.enabled {
                "Stopped"
            } else if path.is_none() {
                "Enabled, offline"
            } else {
                "Connected"
            },
            measurement_state: item.map(|item| item.state),
            age: item
                .and_then(|item| item.success_age_ms)
                .map_or_else(|| "unknown".into(), |age| format!("{}s", age / 1000)),
            median: item.and_then(|item| item.median_us).map(milliseconds),
            samples: item.map_or(0, |item| item.history.len()),
            lost: item.map_or(0, Series::lost),
            recent: item.map_or_else(Vec::new, |item| {
                item.history
                    .iter()
                    .map(|sample| sample.us.map(milliseconds))
                    .collect()
            }),
        });
    }
    tunnels
}

fn unsupported(compat: native::Compat) -> State {
    State {
        version: version(),
        config: Settings::read().unwrap_or_default(),
        tunnels: Vec::new(),
        diagnostics: BTreeMap::new(),
        endpoints: Vec::new(),
        resolved: BTreeMap::new(),
        auto: AutoView {
            enabled: false,
            last: None,
        },
        revision: String::new(),
        scope: "Equivalent native traffic rules",
        capabilities: Capabilities {
            configure: false,
            select: false,
            control: false,
            update: true,
            reason: compat.message.clone(),
        },
        compat,
        update: update::Status::read(),
    }
}

pub fn state() -> Result<State> {
    let mut compat = native::compat();
    if compat.level == native::Level::Unsupported {
        return Ok(unsupported(compat));
    }
    let revision = router::revision()?;
    let mut config = Settings::read()?;
    let inventory = router::inventory()?;
    router::normalize(&mut config, &inventory);
    let snapshot = snapshot();
    let native = native::List::read();
    if compat.level == native::Level::Full && native.is_err() {
        compat.level = native::Level::Monitoring;
        compat.verified = false;
        compat.features = native::features(compat.level);
        compat.message = format!(
            "Firmware {} replies in a format that Wayline does not know. Monitoring and history work. Exit selection and Start and Stop are off.",
            compat.firmware
        );
    }
    let compatible = compat.level == native::Level::Full;
    let tunnels = tunnels(&config, &inventory, &snapshot, &native, compatible);
    let auto_enabled = config.auto_exit;
    let resolved = resolved(&config, &inventory);
    let endpoints = router::endpoints(&config, &inventory);
    let diagnostics = diagnostics(&config, &inventory, &endpoints, &snapshot);
    ensure!(
        router::revision()? == revision,
        "State changed during discovery. Refresh again."
    );
    Ok(State {
        version: version(),
        config,
        tunnels,
        diagnostics,
        resolved,
        endpoints: endpoints
            .into_iter()
            .map(|endpoint| EndpointView {
                tunnel: endpoint.tunnel,
                name: endpoint.name,
                address: endpoint.target.to_string(),
            })
            .collect(),
        auto: AutoView {
            enabled: auto_enabled,
            last: auto::Record::read().last,
        },
        revision,
        scope: "Equivalent native traffic rules",
        capabilities: Capabilities {
            configure: compat.level >= native::Level::Monitoring,
            select: compatible,
            control: compatible,
            update: true,
            reason: compat.message.clone(),
        },
        compat,
        update: update::Status::read(),
    })
}

pub fn history(query: &history::Query) -> Result<history::Response> {
    history::query(
        &Settings::read()?,
        &snapshot(),
        &router::inventory()?,
        query,
    )
}

pub fn set_config(request: SetConfig) -> Result<Value> {
    locked("settings", || {
        native::require(native::Level::Monitoring)?;
        ensure!(request.expected_revision == router::revision()?, STALE);
        let mut config = request.config;
        let inventory = router::inventory()?;
        router::normalize(&mut config, &inventory);
        config.validate()?;
        ensure!(
            config
                .tunnels
                .iter()
                .all(|id| inventory.tunnels.iter().any(|tunnel| &tunnel.key == id)),
            "A selected tunnel is unavailable."
        );
        if Settings::read()? != config {
            atomic(Path::new(SETTINGS), &serde_json::to_vec(&config)?)?;
            let hosts = router::hosts(&config, &inventory);
            let mut resolved = Resolved::read();
            if resolved.stale(&hosts) {
                resolved.refresh(&hosts)?;
            }
            reconcile()?;
        }
        Ok(json!({"applied":true}))
    })
}

pub fn set_tunnel_enabled(request: &SetEnabled) -> Result<Value> {
    const UNKNOWN: &str = "Tunnel outcome is unknown. Refresh before another change.";
    let enabled = request.enabled;
    locked("selection", || {
        locked("settings", || {
            native::require(native::Level::Full)?;
            ensure!(settings::id(&request.section_id), "Invalid tunnel section.");
            let revision = router::revision()?;
            ensure!(request.expected_revision == revision, STALE);
            let inventory = router::inventory()?;
            let before = native::List::read()?;
            let tunnel = router::native_tunnel(&before, &inventory, &request.section_id)?;
            if tunnel.enabled == enabled {
                return Ok(json!({"applied":true,"unchanged":true,"enabled":enabled}));
            }
            ensure!(
                !enabled || tunnel.startable(),
                "Assign a VPN profile in the native dashboard before starting this tunnel."
            );
            let number = tunnel.number;
            ensure!(
                router::revision()? == revision,
                "State changed before the request. Refresh first."
            );
            let mut config = Settings::read()?;
            router::normalize(&mut config, &inventory);
            if config != Settings::read()? {
                atomic(Path::new(SETTINGS), &serde_json::to_vec(&config)?)?;
            }
            native::call::<Value>("set_tunnel", &json!({"tunnel_id":number,"enabled":enabled}))
                .context(UNKNOWN)?;
            let after = native::List::read().context(UNKNOWN)?;
            let actual = after
                .tunnels
                .iter()
                .find(|item| item.number == number)
                .context("Tunnel outcome is unknown. Refresh first.")?;
            let actual = router::native_tunnel(&after, &router::inventory()?, &actual.id)?;
            ensure!(
                actual.number == number && actual.enabled == enabled,
                "Tunnel outcome differs from the request. Refresh first."
            );
            ensure!(
                before.only_changed(&after, number),
                "The VPN state changed concurrently. Refresh first."
            );
            let monitor_pending = reconcile().is_err();
            Ok(json!({"applied":true,"enabled":enabled,"monitor_pending":monitor_pending}))
        })
    })
}

pub fn select(request: &Select) -> Result<Value> {
    locked("selection", || {
        let result = apply(&request.section_id, &request.expected_revision)?;
        if result["unchanged"] != true {
            auto::Record::touch()?;
        }
        Ok(result)
    })
}

pub fn apply(id: &str, expected_revision: &str) -> Result<Value> {
    native::require(native::Level::Full)?;
    let inventory = router::inventory()?;
    let list = native::List::read()?;
    let (desired, changed) = router::safe_order(&list, &inventory, id)?;
    if !changed {
        return Ok(json!({"applied":true,"unchanged":true}));
    }
    ensure!(expected_revision == router::revision()?, STALE);
    ensure!(
        state()?
            .tunnels
            .iter()
            .any(|item| item.id == id && item.eligible),
        "The selected tunnel has no recent healthy measurements."
    );
    ensure!(
        expected_revision == router::revision()?,
        "State changed before selection. Refresh first."
    );
    native::call::<Value>("order_tunnel", &json!({"id_list":desired}))?;
    let order: Vec<_> = native::List::read()
        .context("Selection outcome is unknown. Refresh first.")?
        .tunnels
        .into_iter()
        .map(|tunnel| tunnel.id)
        .collect();
    ensure!(
        order == desired,
        "Selection changed concurrently. Refresh first."
    );
    Ok(json!({"applied":true}))
}

pub fn queue_update(method: &str) -> Result<Value> {
    atomic(
        Path::new(&format!("{RUNTIME}/update-request")),
        method.as_bytes(),
    )?;
    Ok(json!({"message":"The request is queued. Its status appears within one minute."}))
}
