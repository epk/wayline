#![forbid(unsafe_code)]

#[cfg(not(target_os = "linux"))]
compile_error!("waylined requires Linux. Cross-compile the ARM64 musl executable through devenv.");

mod config;
mod nl80211;
mod probe;
mod route;
mod sqm;
mod telemetry;
mod wifi;

use probe::Probe;
use rustix::event::{self, PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wayline_protocol::{Config, MAX_STATUS, Status};

const MAX_CLIENTS: usize = 4;
const MAX_REQUEST: usize = 64;
const REQUEST_WINDOW: Duration = Duration::from_millis(200);
const REPLY_WINDOW: Duration = Duration::from_secs(2);
const IDLE_WAIT: Duration = Duration::from_secs(1);
const CONFIG_CHECK: Duration = Duration::from_secs(1);

struct Endpoint {
    listener: UnixListener,
    path: PathBuf,
    _lock: File,
}
impl Endpoint {
    fn bind(path: PathBuf) -> io::Result<Self> {
        if !path.is_absolute() || path.as_os_str().len() > 100 {
            return Err(io::Error::other(
                "socket path must be absolute and at most 100 bytes",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("socket needs a parent directory"))?;
        match fs::create_dir(parent) {
            Ok(()) => fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        let uid = rustix::process::geteuid().as_raw();
        let meta = fs::symlink_metadata(parent)?;
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "socket directory must be owned by this user with mode 0700",
            ));
        }
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(lock_path)?;
        lock.try_lock()
            .map_err(|_| io::Error::other("another monitor holds the socket lock"))?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_socket() && meta.uid() == uid => {
                fs::remove_file(&path)?;
            }
            Ok(_) => return Err(io::Error::other("refusing to replace a non-socket file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            _lock: lock,
        })
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

enum Phase {
    Request {
        line: [u8; MAX_REQUEST],
        length: usize,
    },
    Reply {
        data: Vec<u8>,
        offset: usize,
    },
}

struct Client {
    stream: UnixStream,
    phase: Phase,
    deadline: Instant,
}

struct Monitor {
    config: Config,
    probes: Vec<Probe>,
    started: Instant,
    nonce: [u8; 16],
    run_id: String,
    boot_id: String,
    reply_hint: usize,
    wifi: Option<wifi::Wifi>,
    telemetry: Option<telemetry::Telemetry>,
    sqm: Option<sqm::Controller<sqm::LinuxShaper>>,
    sqm_state: PathBuf,
}

impl Monitor {
    fn try_new(config: Config, sqm_state: PathBuf) -> io::Result<Self> {
        let started = Instant::now();
        let interval = Duration::from_millis(config.interval_ms);
        let timeout = Duration::from_millis(config.timeout_ms);
        let step = interval / u32::try_from(config.tunnels.len()).unwrap_or(1).max(1);
        let probes = config
            .tunnels
            .iter()
            .zip(0_u32..)
            .map(|(tunnel, slot)| {
                Probe::new(
                    tunnel.clone(),
                    config.target,
                    started + step * slot,
                    interval,
                    timeout,
                )
            })
            .collect();
        let mut nonce = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut nonce)?;
        let wifi = config.wifi.as_ref().map(wifi::Wifi::start).transpose()?;
        let telemetry = config
            .telemetry
            .as_ref()
            .map(telemetry::Telemetry::start)
            .transpose()?;
        let sqm = config
            .sqm
            .clone()
            .map(|sqm| sqm::Controller::new(sqm, sqm::LinuxShaper::new(sqm_state.clone())));
        Ok(Self {
            config,
            probes,
            started,
            nonce,
            run_id: nonce
                .iter()
                .fold(String::with_capacity(32), |mut out, byte| {
                    let _ = write!(out, "{byte:02x}");
                    out
                }),
            boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .map(|id| id.trim().chars().filter(char::is_ascii_hexdigit).collect())
                .unwrap_or_default(),
            reply_hint: 0,
            wifi,
            telemetry,
            sqm,
            sqm_state,
        })
    }

    fn reload(&mut self, config: Config) -> io::Result<()> {
        let now = Instant::now();
        let interval = Duration::from_millis(config.interval_ms);
        let timeout = Duration::from_millis(config.timeout_ms);
        let same_schedule = config.interval_ms == self.config.interval_ms
            && config.timeout_ms == self.config.timeout_ms;
        let step = interval / u32::try_from(config.tunnels.len()).unwrap_or(1).max(1);
        let mut old = std::mem::take(&mut self.probes);
        self.probes = config
            .tunnels
            .iter()
            .zip(0_u32..)
            .map(|(tunnel, slot)| {
                let found = old
                    .iter()
                    .position(|probe| probe.id() == tunnel.id)
                    .filter(|_| same_schedule);
                if let Some(index) = found {
                    let mut probe = old.swap_remove(index);
                    probe.retarget(tunnel.clone(), config.target);
                    probe
                } else {
                    Probe::new(
                        tunnel.clone(),
                        config.target,
                        now + step * slot,
                        interval,
                        timeout,
                    )
                }
            })
            .collect();
        let running = self.wifi.take();
        self.wifi = match (&config.wifi, running) {
            (Some(wanted), Some(running)) if running.interval_ms() == wanted.interval_ms => {
                Some(running)
            }
            (wanted, running) => {
                if let Some(running) = running {
                    running.stop();
                }
                wanted.as_ref().map(wifi::Wifi::start).transpose()?
            }
        };
        let running = self.telemetry.take();
        self.telemetry = match (&config.telemetry, running) {
            (Some(wanted), Some(running)) if running.interval_ms() == wanted.interval_ms => {
                running.retrack(&wanted.interfaces);
                Some(running)
            }
            (wanted, running) => {
                if let Some(running) = running {
                    running.stop();
                }
                wanted
                    .as_ref()
                    .map(telemetry::Telemetry::start)
                    .transpose()?
            }
        };
        let kept = self
            .sqm
            .take()
            .filter(|running| config.sqm.as_ref() == Some(running.config()));
        self.sqm = kept.or_else(|| {
            config
                .sqm
                .clone()
                .map(|sqm| sqm::Controller::new(sqm, sqm::LinuxShaper::new(self.sqm_state.clone())))
        });
        self.config = config;
        Ok(())
    }

    fn snapshot(&mut self, since: Option<u64>) -> io::Result<Vec<u8>> {
        let now = Instant::now();
        let mut status = Status {
            schema: 4,
            run_id: self.run_id.clone(),
            target: self.config.target,
            uptime_ms: probe::millis(now.duration_since(self.started)),
            interval_ms: self.config.interval_ms,
            boot_id: self.boot_id.clone(),
            boot_ms: probe::boot_millis(),
            wall_ms: probe::unix_millis(),
            tunnels: self
                .probes
                .iter()
                .map(|probe| probe.series(now, since))
                .collect(),
            wifi: self.wifi.as_ref().map(|wifi| wifi.status(now, since)),
            telemetry: self
                .telemetry
                .as_ref()
                .map(|telemetry| telemetry.status(now, since)),
            sqm: self
                .sqm
                .as_ref()
                .map(|controller| controller.status(now, since)),
        };
        let data = fit(&mut status, self.reply_hint)?;
        self.reply_hint = data.len();
        Ok(data)
    }
}

fn fit(status: &mut Status, hint: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity(hint);
    loop {
        data.clear();
        serde_json::to_writer(&mut data, &*status)?;
        data.push(b'\n');
        if data.len() <= MAX_STATUS {
            return Ok(data);
        }
        let mut times: Vec<u64> = status
            .tunnels
            .iter()
            .flat_map(|series| series.buckets.iter().map(|bucket| bucket.at))
            .chain(
                status
                    .wifi
                    .iter()
                    .flat_map(|wifi| wifi.buckets.iter().map(|bucket| bucket.at)),
            )
            .chain(status.telemetry.iter().flat_map(|telemetry| {
                telemetry
                    .system_buckets
                    .iter()
                    .map(|bucket| bucket.at)
                    .chain(
                        telemetry
                            .link_buckets
                            .iter()
                            .flat_map(|link| link.buckets.iter().map(|bucket| bucket.at)),
                    )
            }))
            .chain(status.sqm.iter().flat_map(|sqm| {
                sqm.buckets
                    .iter()
                    .flat_map(|link| link.buckets.iter().map(|bucket| bucket.at))
            }))
            .collect();
        times.sort_unstable();
        let excess = data.len() - MAX_STATUS;
        let count = times.len().saturating_mul(excess) / data.len() + 1;
        let Some(&cut) = count
            .min(times.len())
            .checked_sub(1)
            .and_then(|last| times.get(last))
        else {
            return Err(io::Error::other("status exceeds size limit"));
        };
        for series in &mut status.tunnels {
            series.buckets.retain(|bucket| bucket.at > cut);
        }
        if let Some(wifi) = &mut status.wifi {
            wifi.buckets.retain(|bucket| bucket.at > cut);
        }
        if let Some(telemetry) = &mut status.telemetry {
            telemetry.system_buckets.retain(|bucket| bucket.at > cut);
            for link in &mut telemetry.link_buckets {
                link.buckets.retain(|bucket| bucket.at > cut);
            }
        }
        if let Some(sqm) = &mut status.sqm {
            for link in &mut sqm.buckets {
                link.buckets.retain(|bucket| bucket.at > cut);
            }
        }
    }
}

fn parse_since(line: &[u8]) -> Option<u64> {
    let first = line.split(|byte| *byte == b'\n').next()?;
    std::str::from_utf8(first.strip_prefix(b"since=")?)
        .ok()?
        .parse()
        .ok()
}

fn advance(client: &mut Client, now: Instant, monitor: &mut Monitor) -> bool {
    if let Phase::Request { line, length } = &mut client.phase {
        let Some(room) = line.get_mut(*length..) else {
            return false;
        };
        let done = match client.stream.read(room) {
            Ok(0) => true,
            Ok(size) => {
                *length += size;
                line[..*length].contains(&b'\n') || *length >= MAX_REQUEST || now >= client.deadline
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => now >= client.deadline,
            Err(_) => return false,
        };
        if !done {
            return true;
        }
        let Ok(data) = monitor.snapshot(parse_since(&line[..*length])) else {
            return false;
        };
        client.phase = Phase::Reply { data, offset: 0 };
        client.deadline = now + REPLY_WINDOW;
    }
    if now >= client.deadline {
        return false;
    }
    let Phase::Reply { data, offset } = &mut client.phase else {
        return false;
    };
    match client.stream.write(&data[*offset..]) {
        Ok(0) => false,
        Ok(size) => {
            *offset += size;
            *offset < data.len()
        }
        Err(error) => error.kind() == io::ErrorKind::WouldBlock,
    }
}

enum Ready {
    Listener,
    Probe(usize),
    Other,
}

fn wait_ready(
    endpoint: &Endpoint,
    probes: &[Probe],
    clients: &[Client],
    wait: Duration,
) -> io::Result<Vec<Ready>> {
    let mut fds = Vec::with_capacity(1 + probes.len() + clients.len());
    let mut kinds = Vec::with_capacity(fds.capacity());
    fds.push(PollFd::new(&endpoint.listener, PollFlags::IN));
    kinds.push(Ready::Listener);
    for (index, probe) in probes.iter().enumerate() {
        if let Some(socket) = probe.listening() {
            fds.push(PollFd::new(socket, PollFlags::IN));
            kinds.push(Ready::Probe(index));
        }
    }
    for client in clients {
        let flags = if matches!(client.phase, Phase::Request { .. }) {
            PollFlags::IN
        } else {
            PollFlags::OUT
        };
        fds.push(PollFd::new(&client.stream, flags));
        kinds.push(Ready::Other);
    }
    let timeout = Timespec {
        tv_sec: i64::try_from(wait.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(wait.subsec_nanos()),
    };
    match event::poll(&mut fds, Some(&timeout)) {
        Ok(_) => (),
        Err(Errno::INTR) => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    }
    Ok(fds
        .iter()
        .zip(kinds)
        .filter(|(fd, _)| !fd.revents().is_empty())
        .map(|(_, kind)| kind)
        .collect())
}

fn accept_clients(endpoint: &Endpoint, clients: &mut Vec<Client>, now: Instant) -> io::Result<()> {
    for _ in 0..MAX_CLIENTS {
        match endpoint.listener.accept() {
            Ok((stream, _)) => {
                if clients.len() == MAX_CLIENTS {
                    drop(stream);
                    continue;
                }
                stream.set_nonblocking(true)?;
                clients.push(Client {
                    stream,
                    phase: Phase::Request {
                        line: [0; MAX_REQUEST],
                        length: 0,
                    },
                    deadline: now + REQUEST_WINDOW,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn signature(path: &Path) -> Option<(u64, u64, i64, i64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.ino(), meta.len(), meta.mtime(), meta.mtime_nsec()))
}

fn serve(
    config: Config,
    config_path: &Path,
    socket_path: PathBuf,
    run_for: Option<Duration>,
) -> io::Result<()> {
    let endpoint = Endpoint::bind(socket_path)?;
    let routes = route::RouteLookup::try_new()?;
    let mut seen = signature(config_path);
    let mut checked = Instant::now();
    let sqm_state = endpoint.path.with_file_name(sqm::ORIGINALS);
    let mut monitor = Monitor::try_new(config, sqm_state)?;
    let stop_at = run_for.map(|duration| monitor.started + duration);
    let mut clients: Vec<Client> = Vec::with_capacity(MAX_CLIENTS);
    let mut sequence = 0_u64;
    let mut next_probe = 0;
    loop {
        let now = Instant::now();
        if stop_at.is_some_and(|at| now >= at) {
            return Ok(());
        }
        if now.saturating_duration_since(checked) >= CONFIG_CHECK {
            checked = now;
            let current = signature(config_path);
            if current != seen {
                seen = current;
                match config::read(config_path) {
                    Ok(config) => monitor.reload(config)?,
                    Err(error) => eprintln!("waylined: keeping the running configuration: {error}"),
                }
            }
            if let Some(sqm) = &mut monitor.sqm {
                let rtts: Vec<(String, u32)> = monitor
                    .probes
                    .iter_mut()
                    .filter_map(|probe| {
                        let us = probe.take_fresh_us()?;
                        Some((probe.interface().to_owned(), us))
                    })
                    .collect();
                sqm.step(now, &rtts);
            }
        }
        let wall_ms = probe::unix_millis();
        for probe in &mut monitor.probes {
            probe.expire(now, wall_ms);
        }
        let count = monitor.probes.len();
        let first = next_probe;
        for index in (0..count).map(|offset| (first + offset) % count) {
            let Some(probe) = monitor.probes.get_mut(index) else {
                continue;
            };
            if !probe.due(now) {
                continue;
            }
            if probe.saturated() {
                probe.skip_slot(now);
                continue;
            }
            next_probe = (index + 1) % count;
            sequence = sequence.wrapping_add(1);
            probe.query_route(now, &routes, sequence, &monitor.nonce);
        }

        clients.retain_mut(|client| advance(client, now, &mut monitor));
        let deadline = monitor
            .probes
            .iter()
            .map(Probe::deadline)
            .chain(clients.iter().map(|client| client.deadline))
            .chain(stop_at)
            .chain([checked + CONFIG_CHECK])
            .min()
            .unwrap_or(now + IDLE_WAIT);
        let wait = deadline.saturating_duration_since(Instant::now());
        for ready in wait_ready(&endpoint, &monitor.probes, &clients, wait)? {
            match ready {
                Ready::Listener => accept_clients(&endpoint, &mut clients, Instant::now())?,
                Ready::Probe(index) => {
                    if let Some(probe) = monitor.probes.get_mut(index) {
                        probe.drain(Instant::now());
                    }
                }
                Ready::Other => (),
            }
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("waylined: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    if command == "--help" || command == "-h" || command.is_empty() {
        println!(
            "waylined monitor --config FILE --socket ABSOLUTE_PATH [--run-for SECONDS]\nwaylined status --socket ABSOLUTE_PATH [--since UNIX_SECONDS]\nIPv4 ICMP, upstream Wi-Fi and router telemetry monitor. The monitor does not change routes. Socket clients receive one JSON snapshot."
        );
        return Ok(());
    }
    if command == "--version" {
        println!("waylined {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut config_path = None;
    let mut socket_path = None;
    let mut run_for = None;
    let mut since = None;
    while let Some(arg) = args.next() {
        let value = args.next().ok_or("option needs a value")?;
        match arg.as_str() {
            "--config" if config_path.is_none() => config_path = Some(PathBuf::from(value)),
            "--socket" if socket_path.is_none() => socket_path = Some(PathBuf::from(value)),
            "--run-for" if run_for.is_none() => {
                let seconds: u64 = value.parse()?;
                if seconds == 0 || seconds > 86_400 {
                    return Err("run-for must be 1..86400 seconds".into());
                }
                run_for = Some(Duration::from_secs(seconds));
            }
            "--since" if since.is_none() => since = Some(value.parse::<u64>()?),
            _ => return Err("unknown or repeated option".into()),
        }
    }
    let socket_path = socket_path.ok_or("--socket is required")?;
    if since.is_some() && command != "status" {
        return Err("--since is only valid for status".into());
    }
    match command.as_str() {
        "monitor" => {
            let path = config_path.ok_or("--config is required")?;
            let config = config::read(&path)?;
            serve(config, &path, socket_path, run_for)?;
        }
        "status" if config_path.is_none() && run_for.is_none() => {
            let mut stream = UnixStream::connect(socket_path)?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let request = since.map_or_else(String::new, |cursor| format!("since={cursor}"));
            stream.write_all(format!("{request}\n").as_bytes())?;
            let mut data = Vec::with_capacity(MAX_STATUS);
            stream
                .take((MAX_STATUS + 1) as u64)
                .read_to_end(&mut data)?;
            if data.len() > MAX_STATUS || data.last() != Some(&b'\n') {
                return Err("invalid or oversized status".into());
            }
            io::stdout().write_all(&data)?;
        }
        _ => return Err("Use monitor or status. Use --help for details.".into()),
    }
    Ok(())
}
