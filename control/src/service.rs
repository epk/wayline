use crate::resolve::Resolved;
use crate::router::{self, Inventory};
use crate::runtime;
use crate::runtime::{DAEMON, MONITOR_CONFIG, RUNTIME, STATUS_SOCKET};
use crate::settings::Settings;
use crate::{auto, changes, dimensions, egress, history, native, update};
use anyhow::Result;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::Path;

fn flush() {
    if let Ok(config) = Settings::read()
        && config.monitoring()
    {
        let _ = history::collect(&config);
    }
}

fn delete() -> Result<Value> {
    flush();
    native::bus("service", "delete", &json!({"name":"wayline"}))
}

fn halt() {
    let _ = fs::remove_file(MONITOR_CONFIG);
    let registered = native::bus::<Value>("service", "list", &json!({"name":"wayline"}))
        .map_or(true, |list| !list["wayline"].is_null());
    if registered {
        let _ = delete();
    }
}

pub fn stop() -> Result<()> {
    delete().map(drop)
}

fn monitor_running() -> Result<bool> {
    let list = native::bus::<Value>("service", "list", &json!({"name":"wayline"}))?;
    Ok(list["wayline"]["instances"]["monitor"]["running"] == true)
}

pub fn running() -> Result<()> {
    anyhow::ensure!(monitor_running()?, "The monitor is not running.");
    Ok(())
}

pub fn enabled() -> Result<()> {
    anyhow::ensure!(
        Path::new("/etc/rc.d/S95wayline").exists(),
        "Wayline is not enabled at boot."
    );
    Ok(())
}

pub fn reconcile() -> Result<()> {
    runtime::locked("reconcile", reconcile_inner)
}

fn reconcile_inner() -> Result<()> {
    let config = Settings::read()?;
    let inventory = (config.monitoring() && native::supported())
        .then(router::inventory)
        .transpose()?;
    apply(&config, inventory.as_ref())
}

pub fn restart() -> Result<()> {
    runtime::locked("reconcile", || {
        delete()?;
        reconcile_inner()
    })
}

fn apply(config: &Settings, inventory: Option<&Inventory>) -> Result<()> {
    let desired = inventory
        .map(|inventory| router::monitor(config, inventory, &Resolved::read()))
        .filter(|desired| {
            !desired.tunnels.is_empty() || desired.wifi.is_some() || desired.telemetry.is_some()
        });
    let Some(desired) = desired else {
        halt();
        return Ok(());
    };
    let path = Path::new(MONITOR_CONFIG);
    let encoded = serde_json::to_vec(&desired)?;
    let changed = fs::read(path).ok().as_deref() != Some(&encoded);
    let running = monitor_running().unwrap_or(false);
    if changed || !running {
        if changed {
            if running {
                flush();
            }
            wayline_support::atomic(path, &encoded)?;
        }
        native::bus::<Value>(
            "service",
            "set",
            &json!({"name":"wayline","instances":{"monitor":{
            "command":[DAEMON,"monitor","--config",path,"--socket",STATUS_SOCKET],
            "respawn":["3600","5","3"],"limits":{"core":"0","nofile":"256 256"},"stderr":true,"file":[path]}}}),
        )?;
    }
    Ok(())
}

fn troubled(config: &Settings) -> bool {
    let parts = runtime::current();
    let lossy = parts.series.iter().any(|series| {
        series
            .buckets
            .last()
            .is_some_and(|bucket| bucket.sent > 0 && bucket.lost == bucket.sent)
    });
    let disconnected = config.upstream_wifi
        && parts.wifi.as_ref().is_some_and(|wifi| {
            matches!(
                wifi.state,
                wayline_protocol::WifiState::Disconnected
                    | wayline_protocol::WifiState::NoClientInterface
            )
        });
    lossy || disconnected
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Enable,
    Disable,
}

pub fn lifecycle(lifecycle: Lifecycle) -> Result<()> {
    let cron = Path::new("/etc/crontabs/root");
    let mut lines: Vec<_> = fs::read_to_string(cron)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.contains("/usr/sbin/wayline-ctl tick"))
        .map(str::to_owned)
        .collect();
    let startup = Path::new("/etc/rc.d/S95wayline");
    if lifecycle == Lifecycle::Enable {
        lines.push("* * * * * /usr/sbin/wayline-ctl tick >/dev/null 2>&1".into());
        if !startup.exists() {
            unix_fs::symlink("/usr/sbin/wayline-ctl", startup)?;
        }
    } else {
        let _ = fs::remove_file(startup);
        let _ = delete();
    }
    fs::create_dir_all("/etc/crontabs")?;
    wayline_support::atomic(cron, format!("{}\n", lines.join("\n")).as_bytes())?;
    let _ = wayline_support::text("/etc/init.d/cron", &["reload"], 8192, 10);
    if lifecycle == Lifecycle::Enable {
        let _ = delete();
        reconcile()?;
    }
    Ok(())
}

pub fn tick() -> Result<()> {
    runtime::locked("tick", || {
        let routine = routine();
        let config = Settings::read().unwrap_or_default();
        let updated = update::scheduled(&config);
        routine.and(updated)
    })
}

fn routine() -> Result<()> {
    let config = Settings::read()?;
    let (inventory, failure) = match config.monitoring().then(router::inventory) {
        Some(Ok(inventory)) => (Some(inventory), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    let monitored = inventory.as_ref().filter(|_| native::supported());
    if let Some(inventory) = monitored.filter(|_| config.enabled) {
        let hosts = router::hosts(&config, inventory);
        let mut resolved = Resolved::read();
        if resolved.stale(&hosts) {
            resolved.refresh(&hosts)?;
        }
    }
    if failure.is_none()
        && !inventory
            .as_ref()
            .is_some_and(|inventory| inventory.partial)
    {
        runtime::locked("reconcile", || {
            if Settings::read()? == config {
                apply(&config, monitored)
            } else {
                Ok(())
            }
        })?;
    }
    let refreshed = monitored.map_or(Ok(()), |inventory| {
        egress::Cache::refresh(&config, inventory)
    });
    let recorded = inventory
        .as_ref()
        .map_or(Ok(()), |inventory| changes::record(&config, inventory));
    let described = inventory
        .as_ref()
        .filter(|_| config.monitoring())
        .map_or(Ok(()), |inventory| dimensions::record(&config, inventory));
    refreshed?;
    let collected = format!("{RUNTIME}/last-collect");
    let last = fs::read_to_string(&collected)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let now = runtime::now();
    let interval = if config.monitoring() && troubled(&config) {
        60
    } else {
        300
    };
    if config.monitoring() && (now.saturating_sub(last) >= interval || now < last) {
        let result = history::collect(&config);
        wayline_support::atomic(Path::new(&collected), now.to_string().as_bytes())?;
        result?;
    }
    let _ = auto::step(&config);
    let outcome = recorded.and(described);
    failure.map_or(outcome, Err)
}
