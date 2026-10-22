use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wayline_protocol::{MAX_STATUS, Series, Status};

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

pub fn load<T: Default + DeserializeOwned>(path: impl AsRef<Path>) -> T {
    fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

pub fn store(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    wayline_support::atomic(path.as_ref(), &serde_json::to_vec(value)?)
}

pub fn version() -> String {
    fs::read_to_string("/usr/share/wayline/version")
        .unwrap_or_else(|_| "0.1.0-alpha.17".into())
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
    status(Path::new(STATUS_SOCKET), "")
}

pub fn ring() -> Vec<Series> {
    status(Path::new(STATUS_SOCKET), "since=0")
}

fn status(socket: &Path, request: &str) -> Vec<Series> {
    fetch(socket, request)
        .ok()
        .and_then(|data| serde_json::from_slice::<Status>(&data).ok())
        .map_or_else(Vec::new, |status| status.tunnels)
}

fn fetch(socket: &Path, request: &str) -> Result<Vec<u8>> {
    let mut stream = UnixStream::connect(socket)?;
    let window = Some(Duration::from_secs(2));
    stream.set_read_timeout(window)?;
    stream.set_write_timeout(window)?;
    writeln!(stream, "{request}")?;
    let mut data = Vec::new();
    stream
        .take(u64::try_from(MAX_STATUS)? + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() <= MAX_STATUS && data.last() == Some(&b'\n'),
        "Invalid or oversized status."
    );
    Ok(data)
}

pub fn milliseconds(micros: u32) -> f64 {
    f64::from(micros) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::{fetch, status};
    use anyhow::{Result, ensure};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;

    const REPLY: &str = r#"{"schema":1,"run_id":"r","target":"1.1.1.1","uptime_ms":0,"interval_ms":5000,"tunnels":[]}"#;

    #[test]
    fn status_requests_use_the_daemon_line_protocol() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let socket = directory.path().join("s");
        let listener = UnixListener::bind(&socket)?;
        let server = thread::spawn(move || -> Result<Vec<String>> {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept()?;
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line)?;
                requests.push(line);
                writeln!(stream, "{REPLY}")?;
            }
            Ok(requests)
        });
        ensure!(fetch(&socket, "since=0")?.starts_with(b"{\"schema\""));
        ensure!(status(&socket, "").is_empty());
        let requests = server
            .join()
            .map_err(|_| anyhow::anyhow!("Server thread panicked."))??;
        ensure!(requests == ["since=0\n", "\n"], "Unexpected: {requests:?}");
        Ok(())
    }
}
