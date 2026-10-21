#![forbid(unsafe_code)]

#[cfg(not(target_os = "linux"))]
compile_error!("waylined requires Linux. Cross-compile the ARM64 musl executable through devenv.");

mod config;
mod probe;
mod readiness;
mod route;

use futures_util::future::select_all;
use probe::{Links, MAX_PENDING, Probe};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    time::{Duration, Instant},
};
use std::{future::Future, pin::Pin};
use wayline_protocol::{Config, MAX_SERIES, MAX_STATUS, Series, Status};

const MAX_CLIENTS: usize = 4;
const MAX_INFLIGHT: usize = MAX_SERIES * MAX_PENDING + 24;
const MAX_REQUEST: usize = 64;
const REQUEST_WINDOW: Duration = Duration::from_millis(200);
const REPLY_WINDOW: Duration = Duration::from_secs(2);

struct Endpoint {
    listener: UnixListener,
    ready: monoio::net::UnixListener,
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
        let meta = fs::symlink_metadata(parent)?;
        if !meta.is_dir()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
        {
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
            Ok(meta)
                if meta.file_type().is_socket()
                    && meta.uid() == rustix::process::geteuid().as_raw() =>
            {
                fs::remove_file(&path)?;
            }
            Ok(_) => return Err(io::Error::other("refusing to replace a non-socket file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let ready = monoio::net::UnixListener::from_std(listener.try_clone()?)?;
        Ok(Self {
            listener,
            ready,
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
    Request(Vec<u8>),
    Reply { data: Vec<u8>, offset: usize },
}

struct Client {
    stream: UnixStream,
    ready: monoio::net::UnixStream,
    phase: Phase,
    deadline: Instant,
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn snapshot(
    config: &Config,
    probes: &[Probe],
    started: Instant,
    nonce: &[u8; 16],
    since: Option<u64>,
) -> io::Result<Vec<u8>> {
    let now = Instant::now();
    let tunnels = config
        .tunnels
        .iter()
        .zip(probes)
        .map(|(t, p)| Series {
            id: t.id.clone(),
            name: t.name.clone(),
            interface: t.interface.clone(),
            path_id: if t.path_id.is_empty() {
                t.id.clone()
            } else {
                t.path_id.clone()
            },
            target: t.target.unwrap_or(config.target),
            state: p.state,
            median_us: p.median_us(),
            sent: p.sent,
            received: p.received,
            success_age_ms: p.last_success.map(|at| millis(now.duration_since(at))),
            history: p.history(),
            buckets: p.buckets(since),
        })
        .collect();
    let mut status = Status {
        schema: 3,
        run_id: nonce
            .iter()
            .fold(String::with_capacity(32), |mut out, byte| {
                use std::fmt::Write as _;
                let _ = write!(out, "{byte:02x}");
                out
            }),
        target: config.target,
        uptime_ms: millis(now.duration_since(started)),
        interval_ms: config.interval_ms,
        tunnels,
    };
    fit(&mut status)
}

fn fit(status: &mut Status) -> io::Result<Vec<u8>> {
    loop {
        let mut data = serde_json::to_vec(&*status)?;
        data.push(b'\n');
        if data.len() <= MAX_STATUS {
            return Ok(data);
        }
        let mut times: Vec<u64> = status
            .tunnels
            .iter()
            .flat_map(|series| series.buckets.iter().map(|bucket| bucket.at))
            .collect();
        if times.is_empty() {
            return Err(io::Error::other("status exceeds size limit"));
        }
        times.sort_unstable();
        let excess = data.len() - MAX_STATUS;
        let count = times.len().saturating_mul(excess) / data.len() + 1;
        let Some(&cut) = times.get(count.min(times.len()) - 1) else {
            return Err(io::Error::other("status exceeds size limit"));
        };
        for series in &mut status.tunnels {
            series.buckets.retain(|bucket| bucket.at > cut);
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

fn advance(
    client: &mut Client,
    now: Instant,
    build: &dyn Fn(Option<u64>) -> io::Result<Vec<u8>>,
) -> bool {
    if let Phase::Request(line) = &mut client.phase {
        let mut chunk = [0_u8; MAX_REQUEST];
        let done = match client.stream.read(&mut chunk) {
            Ok(0) => true,
            Ok(size) => {
                line.extend_from_slice(&chunk[..size]);
                line.contains(&b'\n') || line.len() >= MAX_REQUEST || now >= client.deadline
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => now >= client.deadline,
            Err(_) => return false,
        };
        if !done {
            return true;
        }
        let Ok(data) = build(parse_since(line)) else {
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

fn service_clients(
    clients: &mut Vec<Client>,
    config: &Config,
    probes: &[Probe],
    started: Instant,
    nonce: &[u8; 16],
    now: Instant,
) {
    clients.retain_mut(|client| {
        advance(client, now, &|since| {
            snapshot(config, probes, started, nonce, since)
        })
    });
}

enum Ready {
    Listener,
    Route,
    Probe(usize),
    Client,
    Timer,
}

type ReadinessFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Ready>> + 'a>>;

#[expect(
    clippy::future_not_send,
    reason = "Monoio keeps each future on one local thread."
)]
async fn wait_ready(
    endpoint: &Endpoint,
    routes: &route::RouteLookup,
    probes: &[Probe],
    clients: &[Client],
    wait: Duration,
    round: usize,
) -> io::Result<(bool, bool, [bool; MAX_SERIES])> {
    let mut pending: Vec<ReadinessFuture<'_>> = Vec::with_capacity(3 + MAX_SERIES + MAX_CLIENTS);
    pending.push(Box::pin(async {
        endpoint.ready.readable(false).await?;
        Ok(Ready::Listener)
    }));
    pending.push(Box::pin(async {
        routes.socket.ready.readable(false).await?;
        Ok(Ready::Route)
    }));
    for (index, probe) in probes.iter().enumerate() {
        if !probe.pending.is_empty()
            && let Some(socket) = &probe.socket
        {
            pending.push(Box::pin(async move {
                socket.ready.readable(false).await?;
                Ok(Ready::Probe(index))
            }));
        }
    }
    for client in clients {
        let reading = matches!(client.phase, Phase::Request(_));
        pending.push(Box::pin(async move {
            if reading {
                client.ready.readable(false).await?;
            } else {
                client.ready.writable(false).await?;
            }
            Ok(Ready::Client)
        }));
    }
    pending.push(Box::pin(async move {
        monoio::time::sleep(wait).await;
        Ok(Ready::Timer)
    }));
    let start = round % pending.len();
    pending.rotate_left(start);
    let (event, _, _) = select_all(pending).await;
    let mut ready = [false; MAX_SERIES];
    let event = event?;
    if let Ready::Probe(index) = event {
        ready[index] = true;
    }
    Ok((
        matches!(event, Ready::Listener),
        matches!(event, Ready::Route),
        ready,
    ))
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
                    ready: monoio::net::UnixStream::from_std(stream.try_clone()?)?,
                    stream,
                    phase: Phase::Request(Vec::with_capacity(MAX_REQUEST)),
                    deadline: now + REQUEST_WINDOW,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn drain_routes(
    routes: &route::RouteLookup,
    probes: &mut [Probe],
    config: &Config,
    timeout: Duration,
    nonce: &[u8; 16],
    links: &mut Links,
) -> io::Result<()> {
    for _ in 0..16 {
        let Some(reply) = routes.receive()? else {
            break;
        };
        if let Some((probe, tunnel)) = probes.iter_mut().zip(&config.tunnels).find(|(probe, _)| {
            probe
                .route_pending
                .as_ref()
                .is_some_and(|p| p.sequence == reply.sequence)
        }) {
            probe.route_reply(
                reply.interface,
                tunnel,
                tunnel.target.unwrap_or(config.target),
                timeout,
                nonce,
                links,
            );
        }
    }
    Ok(())
}

#[expect(
    clippy::future_not_send,
    reason = "Monoio keeps each future on one local thread."
)]
async fn serve(config: &Config, socket_path: PathBuf, run_for: Option<Duration>) -> io::Result<()> {
    let endpoint = Endpoint::bind(socket_path)?;
    let routes = route::RouteLookup::new()?;
    let started = Instant::now();
    let stop_at = run_for.map(|duration| started + duration);
    let interval = Duration::from_millis(config.interval_ms);
    let timeout = Duration::from_millis(config.timeout_ms);
    let step = interval / u32::try_from(config.tunnels.len()).unwrap_or(1);
    let mut probes: Vec<_> = (0..config.tunnels.len())
        .map(|i| {
            Probe::new(
                started + step * u32::try_from(i).unwrap_or_default(),
                interval,
            )
        })
        .collect();
    let mut clients: Vec<Client> = Vec::with_capacity(MAX_CLIENTS);
    let mut nonce = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let mut sequence = 0_u64;
    let mut next_probe = 0;
    let mut ready_round = 0_usize;
    loop {
        let now = Instant::now();
        if stop_at.is_some_and(|at| now >= at) {
            return Ok(());
        }
        for probe in &mut probes {
            probe.expire(now, timeout);
        }
        let mut attempts: usize = probes.iter().map(Probe::attempts).sum();
        let mut links = Links::default();
        let first_probe = next_probe;
        for offset in 0..probes.len() {
            let index = (first_probe + offset) % probes.len();
            let probe = &mut probes[index];
            if now < probe.next_due || probe.route_pending.is_some() {
                continue;
            }
            if probe.pending.len() >= MAX_PENDING {
                probe.skip_slot(now);
                continue;
            }
            if attempts >= MAX_INFLIGHT {
                break;
            }
            let tunnel = &config.tunnels[index];
            next_probe = (index + 1) % config.tunnels.len();
            sequence = sequence.wrapping_add(1);
            probe.query_route(
                tunnel,
                tunnel.target.unwrap_or(config.target),
                now,
                &routes,
                sequence,
                &mut links,
            );
            attempts += usize::from(probe.route_pending.is_some());
        }

        service_clients(&mut clients, config, &probes, started, &nonce, now);
        let mut deadline = now + Duration::from_secs(1);
        if let Some(at) = stop_at {
            deadline = deadline.min(at);
        }
        for probe in &probes {
            for pending in &probe.pending {
                deadline = deadline.min(pending.deadline);
            }
            if let Some(route) = &probe.route_pending {
                deadline = deadline.min(route.deadline);
            } else if attempts < MAX_INFLIGHT {
                deadline = deadline.min(probe.next_due);
            }
        }
        for client in &clients {
            deadline = deadline.min(client.deadline);
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        let (accept_ready, route_ready, ready) =
            wait_ready(&endpoint, &routes, &probes, &clients, wait, ready_round).await?;
        ready_round = ready_round.wrapping_add(1);
        links.clear();
        if route_ready {
            drain_routes(&routes, &mut probes, config, timeout, &nonce, &mut links)?;
        }
        for (i, probe) in probes.iter_mut().enumerate() {
            if ready[i] {
                probe.drain(
                    config.tunnels[i].target.unwrap_or(config.target),
                    Instant::now(),
                );
            }
        }
        if accept_ready {
            accept_clients(&endpoint, &mut clients, Instant::now())?;
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
            "waylined monitor --config FILE --socket ABSOLUTE_PATH [--run-for SECONDS]\nwaylined status --socket ABSOLUTE_PATH [--since UNIX_SECONDS]\nIPv4 ICMP monitor. The monitor does not change routes. Socket clients receive one JSON snapshot."
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
            let config = config::read(&config_path.ok_or("--config is required")?)?;
            monoio::RuntimeBuilder::<monoio::LegacyDriver>::new()
                .with_entries(32)
                .enable_timer()
                .build()?
                .block_on(serve(&config, socket_path, run_for))?;
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
