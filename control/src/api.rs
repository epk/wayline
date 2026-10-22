use crate::{
    auto, changes, egress, history, native,
    resolve::Resolved,
    router::{self, Inventory, PathInfo, TunnelInfo},
    runtime::{SETTINGS, locked, milliseconds, snapshot, store, version},
    service::reconcile,
    settings::{self, Settings, Target},
    update,
};
use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, net::Ipv4Addr};
use wayline_protocol::{Series, State as Measurement};

const RECENT: usize = 8;
const MAX_RECENT_LOSS: usize = 2;
const STALE: &str = "State changed. Refresh first.";
const SCOPE: &str = "Equivalent native traffic rules";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    version: String,
    config: Settings,
    history: history::Availability,
    tunnels: Vec<Tunnel>,
    diagnostics: BTreeMap<String, BTreeMap<String, Option<f64>>>,
    endpoints: Vec<EndpointView>,
    wan: WanView,
    uplinks: Vec<UplinkView>,
    events: Vec<changes::Event>,
    resolved: BTreeMap<String, Option<Ipv4Addr>>,
    auto: AutoView,
    egress: egress::Cache,
    revision: String,
    scope: &'static str,
    capabilities: Capabilities,
    compat: native::Compat,
    monitoring_issue: Option<String>,
    update: update::Status,
}

#[derive(Serialize)]
struct EndpointView {
    tunnel: String,
    name: String,
    address: Option<String>,
    reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WanView {
    available: bool,
    interface: Option<String>,
    reason: String,
}

#[derive(Serialize)]
struct UplinkView {
    id: String,
    interface: String,
    device: String,
    address: Ipv4Addr,
    active: bool,
    state: Option<Measurement>,
    median: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoView {
    enabled: bool,
    last: Option<auto::Switch>,
    needs_attention: bool,
    paused: bool,
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
    selectable: bool,
    select_reason: String,
    pub eligible: bool,
    reason: String,
    state_label: &'static str,
    measurement_state: Option<Measurement>,
    age: String,
    pub median: Option<f64>,
    samples: usize,
    lost: usize,
}

pub struct Exits {
    pub revision: String,
    pub tunnels: Vec<Tunnel>,
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

struct Discovery {
    revision: String,
    config: Settings,
    inventory: Inventory,
    snapshot: Vec<Series>,
    native: Result<native::List>,
    compat: native::Compat,
}

impl Discovery {
    fn read(mut compat: native::Compat, endpoints: bool) -> Result<Self> {
        let revision = router::revision()?;
        let mut config = Settings::read()?;
        let inventory = router::inventory_with(endpoints && config.endpoint_diagnostics)?;
        router::normalize(&mut config, &inventory);
        let snapshot = snapshot();
        let native = native::List::read();
        if compat.level == native::Level::Full && native.is_err() {
            compat.monitoring_only();
        }
        Ok(Self {
            revision,
            config,
            inventory,
            snapshot,
            native,
            compat,
        })
    }

    fn compatible(&self) -> bool {
        self.compat.level == native::Level::Full
    }

    fn confirm(&self) -> Result<()> {
        ensure!(
            router::revision()? == self.revision,
            "State changed during discovery. Refresh again."
        );
        Ok(())
    }

    fn tunnels(&self) -> Vec<Tunnel> {
        let Self {
            config,
            inventory,
            snapshot,
            native,
            ..
        } = self;
        let compatible = self.compatible();
        let preferred = native
            .as_ref()
            .ok()
            .and_then(|list| list.tunnels.first())
            .map(|tunnel| tunnel.id.as_str());
        let order = native
            .as_ref()
            .map_err(|error| anyhow!("Cannot read native tunnel state: {error}"))
            .and_then(|list| router::native_order(list, inventory));
        inventory
            .tunnels
            .iter()
            .map(|tunnel| {
                let path = inventory.paths.iter().find(|path| path.id == tunnel.id);
                let item = path.and_then(|path| measurement(snapshot, path, config.primary()));
                let selection = selection(tunnel, path.is_some(), compatible, &order, inventory);
                let health = health(config, tunnel, item);
                Tunnel {
                    id: tunnel.id.clone(),
                    monitor_id: tunnel.key.clone(),
                    name: tunnel.name.clone(),
                    enabled: tunnel.enabled,
                    connected: path.is_some(),
                    history_available: path.is_some(),
                    controllable: compatible
                        && native.as_ref().is_ok_and(|list| {
                            router::native_tunnel(list, inventory, &tunnel.id)
                                .is_ok_and(|native| tunnel.enabled || native.startable())
                        }),
                    preferred: preferred == Some(tunnel.id.as_str()),
                    select_reason: selection
                        .as_ref()
                        .err()
                        .map_or_else(String::new, ToString::to_string),
                    selectable: selection.is_ok(),
                    eligible: selection.is_ok() && health.is_ok(),
                    reason: selection
                        .and(health)
                        .err()
                        .map_or_else(String::new, |error| error.to_string()),
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
                }
            })
            .collect()
    }
}

fn selection(
    tunnel: &TunnelInfo,
    connected: bool,
    compatible: bool,
    order: &Result<Vec<&str>>,
    inventory: &Inventory,
) -> Result<()> {
    ensure!(compatible, "Firmware support prevents exit changes.");
    ensure!(tunnel.enabled, "The tunnel is stopped.");
    ensure!(connected, "The tunnel is enabled but not connected.");
    let order = order.as_ref().map_err(|error| anyhow!("{error}"))?;
    router::position(order, inventory, &tunnel.id)?;
    Ok(())
}

fn health(config: &Settings, tunnel: &TunnelInfo, item: Option<&Series>) -> Result<()> {
    ensure!(
        config.tunnels.contains(&tunnel.key),
        "Not monitored, so automatic switching ignores it."
    );
    let item = item.context("No measurement to the primary destination yet.")?;
    ensure!(
        item.state.live(),
        "The primary destination is not reachable through this tunnel."
    );
    let limit = u64::from(config.interval).saturating_mul(3).max(15) * 1000;
    ensure!(
        item.success_age_ms.is_some_and(|age| age <= limit),
        "The last successful probe is too old."
    );
    let (probes, lost) = item
        .history
        .iter()
        .rev()
        .take(RECENT)
        .fold((0, 0), |(probes, lost), sample| {
            (probes + 1, lost + usize::from(sample.us.is_none()))
        });
    ensure!(
        lost <= MAX_RECENT_LOSS,
        "{lost} of the last {probes} probes were lost."
    );
    Ok(())
}

fn measurement<'a>(snapshot: &'a [Series], path: &PathInfo, target: &Target) -> Option<&'a Series> {
    let id = path.series(target);
    snapshot.iter().find(|series| series.id == id)
}

fn diagnostics(
    config: &Settings,
    inventory: &Inventory,
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
                .chain(inventory.standby_uplinks())
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
        for (key, target) in router::endpoints(config, inventory) {
            result.entry(target.to_string()).or_insert_with(|| {
                let id = router::endpoint_series(key);
                let latency = snapshot
                    .iter()
                    .find(|series| series.id == id)
                    .or_else(|| measurement(snapshot, wan, target))
                    .and_then(|item| item.median_us)
                    .map(milliseconds);
                BTreeMap::from([(wan.id.clone(), latency)])
            });
        }
    }
    result
}

fn resolved(config: &Settings, inventory: &Inventory) -> BTreeMap<String, Option<Ipv4Addr>> {
    let hosts = router::hosts(config, inventory);
    if hosts.is_empty() {
        return BTreeMap::new();
    }
    let cache = Resolved::read();
    hosts
        .into_iter()
        .map(|host| {
            let address = cache.host(&host);
            (host, address)
        })
        .collect()
}

fn endpoint_views(config: &Settings, inventory: &Inventory) -> Vec<EndpointView> {
    if !router::endpoints_enabled(config) {
        return Vec::new();
    }
    router::monitored(config, inventory)
        .map(|item| EndpointView {
            tunnel: item.id.clone(),
            name: item.name.clone(),
            address: item.endpoint.as_ref().map(ToString::to_string),
            reason: item.endpoint_reason.clone(),
        })
        .collect()
}

fn uplink_views(config: &Settings, inventory: &Inventory, snapshot: &[Series]) -> Vec<UplinkView> {
    inventory
        .uplinks
        .iter()
        .map(|uplink| {
            let active = inventory
                .wan
                .as_ref()
                .is_some_and(|wan| wan.name == uplink.name);
            let path = inventory.wan.as_ref().filter(|_| active).unwrap_or(uplink);
            let item = measurement(snapshot, path, config.primary());
            UplinkView {
                id: uplink.id.clone(),
                interface: uplink.name.clone(),
                device: uplink.interface.clone(),
                address: uplink.source,
                active,
                state: item.map(|item| item.state),
                median: item.and_then(|item| item.median_us).map(milliseconds),
            }
        })
        .collect()
}

fn wan_view(inventory: &Inventory) -> WanView {
    WanView {
        available: inventory.wan.is_some(),
        interface: inventory.wan.as_ref().map(|wan| wan.name.clone()),
        reason: inventory.wan_reason.clone(),
    }
}

fn unsupported(compat: native::Compat) -> State {
    let config = Settings::read().unwrap_or_default();
    State {
        version: version(),
        history: history::availability(&config),
        config,
        tunnels: Vec::new(),
        diagnostics: BTreeMap::new(),
        endpoints: Vec::new(),
        wan: WanView {
            available: false,
            interface: None,
            reason: compat.message.clone(),
        },
        uplinks: Vec::new(),
        events: Vec::new(),
        resolved: BTreeMap::new(),
        auto: AutoView {
            enabled: false,
            last: None,
            needs_attention: false,
            paused: false,
        },
        egress: egress::Cache::default(),
        revision: String::new(),
        scope: SCOPE,
        capabilities: Capabilities {
            configure: false,
            select: false,
            control: false,
            update: true,
            reason: compat.message.clone(),
        },
        compat,
        monitoring_issue: None,
        update: update::view(),
    }
}

pub fn state() -> Result<State> {
    let compat = native::compat();
    if compat.level == native::Level::Unsupported {
        return Ok(unsupported(compat));
    }
    let discovery = Discovery::read(compat, true)?;
    let mut tunnels = discovery.tunnels();
    let compatible = discovery.compatible();
    let Discovery {
        revision,
        config,
        inventory,
        snapshot,
        compat,
        ..
    } = discovery;
    let known = history::known_prefixes();
    for tunnel in tunnels
        .iter_mut()
        .filter(|tunnel| !tunnel.history_available)
    {
        tunnel.history_available = known.contains(&router::tunnel_prefix(&tunnel.monitor_id));
    }
    let auto_paused = config.auto_exit
        && !tunnels.iter().any(|tunnel| {
            tunnel.preferred && tunnel.enabled && config.auto_tunnels.contains(&tunnel.monitor_id)
        });
    let auto_record = auto::Record::read();
    let events = changes::read(&config)
        .into_iter()
        .rev()
        .take(changes::RECENT)
        .collect();
    let state = State {
        version: version(),
        history: history::availability(&config),
        diagnostics: diagnostics(&config, &inventory, &snapshot),
        resolved: resolved(&config, &inventory),
        endpoints: endpoint_views(&config, &inventory),
        wan: wan_view(&inventory),
        uplinks: uplink_views(&config, &inventory, &snapshot),
        egress: egress::Cache::view(&config, &inventory),
        monitoring_issue: router::check_capacity(&config, &inventory)
            .err()
            .map(|error| error.to_string()),
        tunnels,
        events,
        auto: AutoView {
            enabled: config.auto_exit,
            last: auto_record.last,
            needs_attention: auto_record.needs_attention,
            paused: auto_paused,
        },
        revision,
        scope: SCOPE,
        capabilities: Capabilities {
            configure: compat.level >= native::Level::Monitoring,
            select: compatible,
            control: compatible,
            update: true,
            reason: compat.message.clone(),
        },
        compat,
        config,
        update: update::view(),
    };
    ensure!(
        router::revision()? == state.revision,
        "State changed during discovery. Refresh again."
    );
    Ok(state)
}

pub fn exits() -> Result<Exits> {
    let discovery = Discovery::read(native::compat(), false)?;
    let tunnels = discovery.tunnels();
    discovery.confirm()?;
    Ok(Exits {
        revision: discovery.revision,
        tunnels,
    })
}

pub fn history(query: &history::Query) -> Result<history::Response> {
    history::query(&Settings::read()?, &router::inventory_with(false)?, query)
}

pub fn set_config(request: SetConfig) -> Result<Value> {
    locked("selection", || {
        locked("settings", || {
            native::require(native::Level::Monitoring)?;
            ensure!(request.expected_revision == router::revision()?, STALE);
            let mut config = request.config;
            let inventory = router::inventory()?;
            router::normalize(&mut config, &inventory);
            config.validate()?;
            router::check_capacity(&config, &inventory)?;
            ensure!(
                config
                    .tunnels
                    .iter()
                    .all(|id| inventory.tunnels.iter().any(|tunnel| &tunnel.key == id)),
                "A selected tunnel is unavailable."
            );
            let previous = Settings::read()?;
            if previous != config {
                auto::Record::touch(previous.auto_exit != config.auto_exit)?;
                store(SETTINGS, &config)?;
                let hosts = router::hosts(&config, &inventory);
                let mut resolved = Resolved::read();
                if resolved.stale(&hosts) {
                    resolved.refresh(&hosts)?;
                }
                reconcile()?;
            }
            Ok(json!({"applied":true}))
        })
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
            let inventory = router::inventory_with(false)?;
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
            let stored = Settings::read()?;
            let mut config = stored.clone();
            router::normalize(&mut config, &inventory);
            if config != stored {
                store(SETTINGS, &config)?;
            }
            native::call::<Value>("set_tunnel", &json!({"tunnel_id":number,"enabled":enabled}))
                .context(UNKNOWN)?;
            let after = native::List::read().context(UNKNOWN)?;
            let actual = after
                .tunnels
                .iter()
                .find(|item| item.number == number)
                .context("Tunnel outcome is unknown. Refresh first.")?;
            router::forget();
            let actual =
                router::native_tunnel(&after, &router::inventory_with(false)?, &actual.id)?;
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
        let result = apply(&request.section_id, &request.expected_revision, false)?;
        if result["unchanged"] != true {
            auto::Record::touch(true)?;
        }
        Ok(result)
    })
}

pub fn apply(id: &str, expected_revision: &str, require_health: bool) -> Result<Value> {
    native::require(native::Level::Full)?;
    let inventory = router::inventory_with(false)?;
    let list = native::List::read()?;
    let mut desired = router::native_order(&list, &inventory)?;
    let index = router::position(&desired, &inventory, id)?;
    if index == 0 {
        return Ok(json!({"applied":true,"unchanged":true}));
    }
    desired[..=index].rotate_right(1);
    ensure!(expected_revision == router::revision()?, STALE);
    if require_health {
        ensure!(
            exits()?
                .tunnels
                .iter()
                .any(|item| item.id == id && item.eligible),
            "The selected tunnel has no recent healthy measurements."
        );
    }
    ensure!(
        expected_revision == router::revision()?,
        "State changed before selection. Refresh first."
    );
    native::call::<Value>("order_tunnel", &json!({"id_list":desired}))?;
    let after = native::List::read().context("Selection outcome is unknown. Refresh first.")?;
    ensure!(
        after
            .tunnels
            .iter()
            .map(|tunnel| tunnel.id.as_str())
            .eq(desired.iter().copied()),
        "Selection changed concurrently. Refresh first."
    );
    Ok(json!({"applied":true}))
}

pub fn start_update(method: &str) -> Result<Value> {
    let started = update::start(method)?;
    Ok(json!({"started":started,"update":update::view()}))
}
