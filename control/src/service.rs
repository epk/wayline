use crate::{
    auto, changes, egress, history, native,
    resolve::Resolved,
    router::{self, Inventory},
    runtime::{DAEMON, MONITOR_CONFIG, RUNTIME, STATUS_SOCKET, locked, now},
    settings::Settings,
    update,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::symlink, path::Path};
use wayline_support::{atomic, text};

fn flush() {
    if let Ok(config) = Settings::read()
        && config.enabled
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
    if monitor_running().unwrap_or(true) {
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
    locked("reconcile", reconcile_inner)
}

fn reconcile_inner() -> Result<()> {
    let config = Settings::read()?;
    let inventory = (config.enabled && native::supported())
        .then(router::inventory)
        .transpose()?;
    apply(&config, inventory.as_ref())
}

pub fn restart() -> Result<()> {
    locked("reconcile", || {
        delete()?;
        reconcile_inner()
    })
}

fn apply(config: &Settings, inventory: Option<&Inventory>) -> Result<()> {
    let desired = inventory
        .map(|inventory| router::monitor(config, inventory, &Resolved::read()))
        .filter(|desired| !desired.tunnels.is_empty());
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
            atomic(path, &encoded)?;
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

pub fn lifecycle(install: bool) -> Result<()> {
    let cron = Path::new("/etc/crontabs/root");
    let mut lines: Vec<_> = fs::read_to_string(cron)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.contains("/usr/sbin/wayline-ctl tick"))
        .map(str::to_owned)
        .collect();
    let startup = Path::new("/etc/rc.d/S95wayline");
    if install {
        lines.push("* * * * * /usr/sbin/wayline-ctl tick >/dev/null 2>&1".into());
        if !startup.exists() {
            symlink("/usr/sbin/wayline-ctl", startup)?;
        }
    } else {
        let _ = fs::remove_file(startup);
        let _ = delete();
    }
    fs::create_dir_all("/etc/crontabs")?;
    atomic(cron, format!("{}\n", lines.join("\n")).as_bytes())?;
    let _ = text("/etc/init.d/cron", &["reload"], 8192, 10);
    if install {
        let _ = delete();
        reconcile()?;
    }
    Ok(())
}

pub fn tick() -> Result<()> {
    locked("tick", || {
        let routine = routine();
        let config = Settings::read().unwrap_or_default();
        let updated = update::scheduled(&config);
        routine.and(updated)
    })
}

fn routine() -> Result<()> {
    let config = Settings::read()?;
    let (inventory, failure) = match config.enabled.then(router::inventory) {
        Some(Ok(inventory)) => (Some(inventory), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    let monitored = inventory.as_ref().filter(|_| native::supported());
    if let Some(inventory) = monitored {
        let hosts = router::hosts(&config, inventory);
        let mut resolved = Resolved::read();
        if resolved.stale(&hosts) {
            resolved.refresh(&hosts)?;
        }
    }
    if failure.is_none() {
        locked("reconcile", || apply(&config, monitored))?;
    }
    let recorded = inventory
        .as_ref()
        .map_or(Ok(()), |inventory| changes::record(&config, inventory));
    if let Some(inventory) = monitored {
        egress::Cache::refresh(&config, inventory)?;
    }
    let collected = format!("{RUNTIME}/last-collect");
    let last = fs::read_to_string(&collected)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let now = now();
    if config.enabled && (now.saturating_sub(last) >= 300 || now < last) {
        let result = history::collect(&config).and_then(|()| {
            inventory
                .as_ref()
                .map_or(Ok(()), |inventory| history::migrate(&config, inventory))
        });
        atomic(Path::new(&collected), now.to_string().as_bytes())?;
        result?;
    }
    let _ = auto::step(&config);
    failure.map_or(recorded, Err)
}
