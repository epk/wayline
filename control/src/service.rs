use crate::resolve::Resolved;
use crate::router::{self, Inventory};
use crate::runtime;
use crate::runtime::{DAEMON, MONITOR_CONFIG, RUNTIME, STATUS_SOCKET};
use crate::settings::Settings;
use crate::{auto, changes, dimensions, egress, history, native, update};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::PermissionsExt;
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

const CRON_ENTRY: &[u8] = b"* * * * * /usr/sbin/wayline-ctl tick >/dev/null 2>&1";
const CRON_MARK: &[u8] = b"/usr/sbin/wayline-ctl tick";

fn crontab(existing: &[u8], lifecycle: Lifecycle) -> Vec<u8> {
    let body = existing.strip_suffix(b"\n").unwrap_or(existing);
    let mut updated = Vec::with_capacity(existing.len() + CRON_ENTRY.len() + 1);
    if !body.is_empty() {
        for line in body.split(|&byte| byte == b'\n') {
            if !line
                .windows(CRON_MARK.len())
                .any(|window| window == CRON_MARK)
            {
                updated.extend_from_slice(line);
                updated.push(b'\n');
            }
        }
    }
    if lifecycle == Lifecycle::Enable {
        updated.extend_from_slice(CRON_ENTRY);
        updated.push(b'\n');
    }
    updated
}

fn schedule(cron: &Path, lifecycle: Lifecycle) -> Result<()> {
    let (existing, current) = match fs::read(cron) {
        Ok(data) => {
            let target = fs::canonicalize(cron)?;
            let mode = fs::metadata(&target)?.permissions().mode() & 0o7777;
            (data, Some((target, mode)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::ensure!(
                fs::symlink_metadata(cron).is_err(),
                "The root crontab is a broken link."
            );
            (Vec::new(), None)
        }
        Err(error) => {
            return Err(anyhow::Error::new(error).context("Could not read the root crontab."));
        }
    };
    let updated = crontab(&existing, lifecycle);
    if current.is_some() && updated == existing {
        return Ok(());
    }
    let (target, mode) = current.unwrap_or_else(|| (cron.to_path_buf(), 0o600));
    let parent = target
        .parent()
        .context("The crontab needs a parent directory.")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&updated)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    file.as_file().sync_all()?;
    file.persist(&target)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn lifecycle(lifecycle: Lifecycle) -> Result<()> {
    let cron = Path::new("/etc/crontabs/root");
    let startup = Path::new("/etc/rc.d/S95wayline");
    if lifecycle == Lifecycle::Enable {
        if !startup.exists() {
            unix_fs::symlink("/usr/sbin/wayline-ctl", startup)?;
        }
    } else {
        let _ = fs::remove_file(startup);
        let _ = delete();
    }
    schedule(cron, lifecycle)?;
    let _ = wayline_support::text("/etc/init.d/cron", &["reload"], 8192, 10);
    if lifecycle == Lifecycle::Enable {
        let _ = delete();
        runtime::writing("reconcile", reconcile_inner)?;
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
    let resolved = monitored
        .filter(|_| config.enabled)
        .map_or(Ok(()), |inventory| {
            let hosts = router::hosts(&config, inventory);
            let mut resolved = Resolved::read();
            if resolved.stale(&hosts) {
                resolved.refresh(&hosts)
            } else {
                Ok(())
            }
        });
    let applied = if failure.is_none()
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
        })
    } else {
        Ok(())
    };
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
    let collected = if config.monitoring() {
        let interval = if troubled(&config) { 60 } else { 300 };
        collect_due(
            Path::new(&format!("{RUNTIME}/last-collect")),
            runtime::now(),
            interval,
            || history::collect(&config),
        )
    } else {
        Ok(())
    };
    let _ = auto::step(&config);
    let outcome = resolved
        .and(applied)
        .and(refreshed)
        .and(collected)
        .and(recorded)
        .and(described);
    failure.map_or(outcome, Err)
}

fn collect_due(
    stamp: &Path,
    now: u64,
    interval: u64,
    collect: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let last = fs::read_to_string(stamp)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if now >= last && now.saturating_sub(last) < interval {
        return Ok(());
    }
    collect()?;
    wayline_support::atomic(stamp, now.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{Lifecycle, collect_due, crontab, schedule};
    use anyhow::{Result, bail, ensure};
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const ENTRY: &str = "* * * * * /usr/sbin/wayline-ctl tick >/dev/null 2>&1\n";

    #[test]
    fn crontab_lines_are_filtered_as_bytes() {
        let existing =
            b"0 4 * * * /sbin/reboot\n\xff\xfe odd\n* * * * * /usr/sbin/wayline-ctl tick\n";
        let enabled = crontab(existing, Lifecycle::Enable);
        let mut expected = b"0 4 * * * /sbin/reboot\n\xff\xfe odd\n".to_vec();
        expected.extend_from_slice(ENTRY.as_bytes());
        assert_eq!(enabled, expected);
        assert_eq!(crontab(&enabled, Lifecycle::Enable), enabled);
        assert_eq!(
            crontab(&enabled, Lifecycle::Disable),
            b"0 4 * * * /sbin/reboot\n\xff\xfe odd\n"
        );
        assert_eq!(crontab(b"", Lifecycle::Enable), ENTRY.as_bytes());
        assert_eq!(crontab(b"", Lifecycle::Disable), b"");
        assert_eq!(crontab(b"a\n\nb", Lifecycle::Disable), b"a\n\nb\n");
    }

    #[test]
    fn scheduling_keeps_mode_link_and_other_jobs() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let real = folder.path().join("real");
        fs::write(&real, b"0 4 * * * /sbin/reboot\n\xff odd\n")?;
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640))?;
        let cron = folder.path().join("root");
        symlink(&real, &cron)?;
        schedule(&cron, Lifecycle::Enable)?;
        ensure!(fs::symlink_metadata(&cron)?.file_type().is_symlink());
        ensure!(fs::metadata(&real)?.permissions().mode() & 0o7777 == 0o640);
        let mut expected = b"0 4 * * * /sbin/reboot\n\xff odd\n".to_vec();
        expected.extend_from_slice(ENTRY.as_bytes());
        ensure!(fs::read(&real)? == expected);
        schedule(&cron, Lifecycle::Disable)?;
        ensure!(fs::read(&real)? == b"0 4 * * * /sbin/reboot\n\xff odd\n");
        let fresh = folder.path().join("crontabs").join("root");
        schedule(&fresh, Lifecycle::Enable)?;
        ensure!(fs::read(&fresh)? == ENTRY.as_bytes());
        ensure!(fs::metadata(&fresh)?.permissions().mode() & 0o7777 == 0o600);
        Ok(())
    }

    #[test]
    fn an_unreadable_crontab_is_left_alone() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let cron = folder.path().join("root");
        fs::create_dir(&cron)?;
        ensure!(schedule(&cron, Lifecycle::Enable).is_err());
        ensure!(fs::metadata(&cron)?.is_dir());
        let dangling = folder.path().join("dangling");
        symlink(folder.path().join("missing"), &dangling)?;
        ensure!(schedule(&dangling, Lifecycle::Enable).is_err());
        ensure!(fs::symlink_metadata(&dangling)?.file_type().is_symlink());
        Ok(())
    }

    #[test]
    fn a_failed_collection_is_retried_on_the_next_tick() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let stamp = folder.path().join("last-collect");
        let runs = Cell::new(0);
        let fail = || -> Result<()> {
            runs.set(runs.get() + 1);
            bail!("The store is busy.")
        };
        ensure!(collect_due(&stamp, 1000, 300, fail).is_err());
        ensure!(!stamp.exists());
        ensure!(collect_due(&stamp, 1060, 300, fail).is_err());
        ensure!(runs.get() == 2);
        let succeed = || -> Result<()> {
            runs.set(runs.get() + 1);
            Ok(())
        };
        collect_due(&stamp, 1120, 300, succeed)?;
        ensure!(fs::read_to_string(&stamp)? == "1120");
        collect_due(&stamp, 1180, 300, succeed)?;
        ensure!(runs.get() == 3);
        collect_due(&stamp, 1420, 300, succeed)?;
        collect_due(&stamp, 100, 300, succeed)?;
        ensure!(runs.get() == 5);
        Ok(())
    }
}
