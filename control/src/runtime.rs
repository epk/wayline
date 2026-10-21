use anyhow::{Context, Result, ensure};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use wayline_protocol::{MAX_STATUS, Series, Status};
use wayline_support::output;

pub const RUNTIME: &str = "/var/run/wayline";
pub const MONITOR_CONFIG: &str = "/var/run/wayline/monitor.json";
pub const STATUS_SOCKET: &str = "/var/run/wayline/status.sock";
pub const SETTINGS: &str = "/etc/wayline/settings.json";
pub const DAEMON: &str = "/usr/sbin/waylined";

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn version() -> String {
    fs::read_to_string("/usr/share/wayline/version")
        .unwrap_or_else(|_| "0.1.0-alpha.3".into())
        .trim()
        .into()
}

pub fn locked<T>(name: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(format!("{RUNTIME}/{name}.lock"))?;
    lock.try_lock().context("Another operation is active.")?;
    action()
}

pub fn directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o777 == 0o700,
        "Private directory is not secure."
    );
    Ok(())
}

pub fn snapshot() -> Vec<Series> {
    output(
        DAEMON,
        &["status", "--socket", STATUS_SOCKET],
        MAX_STATUS,
        3,
    )
    .ok()
    .and_then(|data| serde_json::from_slice::<Status>(&data).ok())
    .map_or_else(Vec::new, |status| status.tunnels)
}

pub fn milliseconds(micros: u32) -> f64 {
    f64::from(micros) / 1000.0
}
