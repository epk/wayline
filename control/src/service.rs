use crate::{
    auto, changes, egress, history, native,
    resolve::Resolved,
    router,
    runtime::{DAEMON, MONITOR_CONFIG, RUNTIME, STATUS_SOCKET, locked, now, snapshot},
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
        let _ = history::collect(&config, &snapshot());
    }
}

fn delete() -> Result<Value> {
    flush();
    native::bus("service", "delete", &json!({"name":"wayline"}))
}

fn halt() {
    let _ = fs::remove_file(MONITOR_CONFIG);
    let _ = delete();
}

pub fn stop() -> Result<()> {
    delete().map(drop)
}

pub fn running() -> Result<()> {
    let list = native::bus::<Value>("service", "list", &json!({"name":"wayline"}))?;
    anyhow::ensure!(
        list["wayline"]["instances"]["monitor"]["running"] == true,
        "The monitor is not running."
    );
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

pub fn restart() -> Result<()> {
    locked("reconcile", || {
        delete()?;
        reconcile_inner()
    })
}

fn reconcile_inner() -> Result<()> {
    let config = Settings::read()?;
    let desired = if config.enabled && native::supported() {
        Some(router::monitor(
            &config,
            &router::inventory()?,
            &Resolved::read(),
        ))
    } else {
        None
    };
    let Some(desired) = desired.filter(|desired| !desired.tunnels.is_empty()) else {
        halt();
        return Ok(());
    };
    let path = Path::new(MONITOR_CONFIG);
    let encoded = serde_json::to_vec(&desired)?;
    let changed = fs::read(path).ok().as_deref() != Some(&encoded);
    let running = native::bus::<Value>("service", "list", &json!({"name":"wayline"}))
        .is_ok_and(|value| value["wayline"]["instances"]["monitor"]["running"] == true);
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
            "respawn":["3600","5","3"],"limits":{"core":"0","nofile":"128 128"},"stderr":true,"file":[path]}}}),
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
    if config.enabled && native::supported() {
        let hosts = router::hosts(&config, &router::inventory()?);
        let mut resolved = Resolved::read();
        if resolved.stale(&hosts) {
            resolved.refresh(&hosts)?;
        }
    }
    reconcile()?;
    let recorded = if config.enabled {
        router::inventory().and_then(|inventory| changes::record(&config, &inventory))
    } else {
        Ok(())
    };
    if config.enabled && native::supported() {
        egress::Cache::refresh(&router::inventory()?)?;
    }
    let collected = format!("{RUNTIME}/last-collect");
    let last = fs::read_to_string(&collected)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let now = now();
    if config.enabled && (now.saturating_sub(last) >= 300 || now < last) {
        let result = history::collect(&config, &snapshot())
            .and_then(|()| router::inventory())
            .and_then(|inventory| history::migrate(&config, &inventory));
        atomic(Path::new(&collected), now.to_string().as_bytes())?;
        result?;
    }
    let _ = auto::step(&config);
    recorded
}
