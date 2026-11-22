use anyhow::{Context, Result, bail, ensure};
use rustix::fs::{Mode, OFlags};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wayline_protocol::{MAX_STATUS, Series, Status, TelemetryStatus, WifiStatus};

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
        .unwrap_or_else(|_| "0.1.0-alpha.24".into())
        .trim()
        .into()
}

fn lock_file(name: &str) -> Result<fs::File> {
    Ok(fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(format!("{RUNTIME}/{name}.lock"))?)
}

pub fn locked<T>(name: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock = lock_file(name)?;
    lock.try_lock().context("Another operation is active.")?;
    action()
}

const STORE_WAIT: Duration = Duration::from_secs(5);

pub fn reading<T>(name: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock = lock_file(name)?;
    wait_for(|| lock.try_lock_shared())?;
    action()
}

pub fn writing<T>(name: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock = lock_file(name)?;
    wait_for(|| lock.try_lock())?;
    action()
}

fn wait_for(mut attempt: impl FnMut() -> Result<(), fs::TryLockError>) -> Result<()> {
    let deadline = Instant::now() + STORE_WAIT;
    loop {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(fs::TryLockError::WouldBlock) => bail!("Another operation is active."),
            Err(fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

pub fn directory(path: &Path) -> Result<()> {
    private(path, 0)
}

fn private(path: &Path, owner: u32) -> Result<()> {
    if !path.exists() {
        fs::create_dir(path)?;
    }
    let folder = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .context("Private directory is not secure.")?;
    let stat = rustix::fs::fstat(&folder)?;
    ensure!(stat.st_uid == owner, "Private directory is not secure.");
    if stat.st_mode & 0o7777 != 0o700 {
        rustix::fs::fchmod(&folder, Mode::from_raw_mode(0o700))?;
    }
    Ok(())
}

#[derive(Default)]
pub struct Parts {
    pub series: Vec<Series>,
    pub wifi: Option<WifiStatus>,
    pub telemetry: Option<TelemetryStatus>,
    pub boot_id: String,
    pub boot_ms: u64,
    pub wall_ms: u64,
}

pub fn current() -> Parts {
    parts(full(Path::new(STATUS_SOCKET), ""))
}

pub fn ring() -> Parts {
    parts(full(Path::new(STATUS_SOCKET), "since=0"))
}

fn parts(status: Option<Status>) -> Parts {
    status.map_or_else(Parts::default, |status| Parts {
        series: status.tunnels,
        wifi: status.wifi,
        telemetry: status.telemetry,
        boot_id: status.boot_id,
        boot_ms: status.boot_ms,
        wall_ms: status.wall_ms,
    })
}

fn full(socket: &Path, request: &str) -> Option<Status> {
    fetch(socket, request)
        .ok()
        .and_then(|data| serde_json::from_slice::<Status>(&data).ok())
}

#[cfg(test)]
fn status(socket: &Path, request: &str) -> Vec<Series> {
    parts(full(socket, request)).series
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
    use super::{fetch, private, status, wait_for};
    use anyhow::{Result, ensure};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;
    use std::thread;

    const REPLY: &str = r#"{"schema":1,"run_id":"r","target":"1.1.1.1","uptime_ms":0,"interval_ms":5000,"tunnels":[]}"#;

    #[test]
    fn store_readers_share_and_wait_for_a_writer() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("history.lock");
        let open = || {
            fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
        };
        let (first, second, writer) = (open()?, open()?, open()?);
        wait_for(|| first.try_lock_shared())?;
        wait_for(|| second.try_lock_shared())?;
        first.unlock()?;
        second.unlock()?;
        writer.lock()?;
        let release = thread::spawn(move || -> Result<()> {
            thread::sleep(std::time::Duration::from_millis(100));
            writer.unlock()?;
            Ok(())
        });
        let reader = open()?;
        wait_for(|| reader.try_lock_shared())?;
        release
            .join()
            .map_err(|_| anyhow::anyhow!("Writer thread panicked."))??;
        Ok(())
    }

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

    #[test]
    fn private_directories_are_created_and_tightened() -> Result<()> {
        let owner = rustix::process::getuid().as_raw();
        let root = tempfile::tempdir()?;
        let mode = |path: &std::path::Path| -> Result<u32> {
            Ok(fs::symlink_metadata(path)?.permissions().mode() & 0o7777)
        };
        let fresh = root.path().join("fresh");
        private(&fresh, owner)?;
        ensure!(mode(&fresh)? == 0o700);
        let restored = root.path().join("restored");
        fs::create_dir(&restored)?;
        fs::set_permissions(&restored, fs::Permissions::from_mode(0o755))?;
        private(&restored, owner)?;
        ensure!(mode(&restored)? == 0o700);
        ensure!(private(&restored, owner + 1).is_err());
        let link = root.path().join("link");
        symlink(&restored, &link)?;
        ensure!(private(&link, owner).is_err());
        let file = root.path().join("file");
        fs::write(&file, b"")?;
        ensure!(private(&file, owner).is_err());
        Ok(())
    }
}
