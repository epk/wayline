use crate::{
    router::{Inventory, PathInfo},
    runtime::{RUNTIME, now},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const LIMIT: Duration = Duration::from_secs(6);
const MAX_AGE: u64 = 300;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cache {
    pub checked_at: u64,
    pub paths: Vec<Egress>,
}

impl Cache {
    fn path() -> String {
        format!("{RUNTIME}/egress.json")
    }

    pub fn read() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    fn matches(&self, inventory: &Inventory) -> bool {
        let wanted: Vec<_> = inventory
            .paths
            .iter()
            .chain(inventory.wan.as_ref())
            .map(|path| (path.id.as_str(), path.interface.as_str()))
            .collect();
        let cached: Vec<_> = self
            .paths
            .iter()
            .map(|path| (path.id.as_str(), path.interface.as_str()))
            .collect();
        wanted == cached
    }

    pub fn refresh(inventory: &Inventory) -> Result<()> {
        let cache = Self::read();
        if now().saturating_sub(cache.checked_at) < MAX_AGE && cache.matches(inventory) {
            return Ok(());
        }
        let fresh = Self {
            checked_at: now(),
            paths: probe(inventory),
        };
        wayline_support::atomic(
            std::path::Path::new(&Self::path()),
            &serde_json::to_vec(&fresh)?,
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Egress {
    pub id: String,
    pub name: String,
    pub interface: String,
    pub ip: Option<String>,
    pub location: Option<String>,
    pub colo: Option<String>,
    pub error: Option<String>,
}

struct Trace {
    ip: String,
    location: Option<String>,
    colo: Option<String>,
}

#[must_use]
pub fn probe(inventory: &Inventory) -> Vec<Egress> {
    let deadline = Instant::now() + LIMIT;
    let paths: Vec<&PathInfo> = inventory
        .paths
        .iter()
        .chain(inventory.wan.as_ref())
        .collect();
    let pending: Vec<_> = paths.iter().map(|path| start(path)).collect();
    paths
        .into_iter()
        .zip(pending)
        .map(|(path, pending)| {
            let mut egress = Egress {
                id: path.id.clone(),
                name: path.name.clone(),
                interface: path.interface.clone(),
                ip: None,
                location: None,
                colo: None,
                error: None,
            };
            match pending.and_then(|receiver| receive(&receiver, deadline)) {
                Ok(trace) => {
                    egress.ip = Some(trace.ip);
                    egress.location = trace.location;
                    egress.colo = trace.colo;
                }
                Err(error) => egress.error = Some(format!("{error:#}")),
            }
            egress
        })
        .collect()
}

fn start(path: &PathInfo) -> Result<Receiver<Result<Trace>>> {
    let (sender, receiver) = mpsc::channel();
    let (interface, source, mark) = (path.interface.clone(), path.source, path.mark);
    thread::Builder::new()
        .name("egress-probe".into())
        .spawn(move || {
            sender.send(trace(&interface, source, mark)).ok();
        })
        .context("Could not start the egress probe.")?;
    Ok(receiver)
}

fn receive(receiver: &Receiver<Result<Trace>>, deadline: Instant) -> Result<Trace> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .context("The egress probe timed out.")?
}

fn trace(interface: &str, source: Ipv4Addr, mark: u32) -> Result<Trace> {
    let response = fetch(interface, source, mark)?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .context("The egress response is malformed.")?;
    let status = head.lines().next().unwrap_or_default();
    if !status.starts_with("HTTP/") || status.split_whitespace().nth(1) != Some("200") {
        bail!("The egress endpoint did not return HTTP 200.");
    }
    let fields: HashMap<&str, &str> = body
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    let ip = fields
        .get("ip")
        .context("The egress response has no IP address.")?;
    let ip = ip
        .parse::<IpAddr>()
        .context("The egress IP address is invalid.")?
        .to_string();
    let text_field = |key: &str| {
        fields
            .get(key)
            .filter(|value| !value.is_empty())
            .map(|value| (*value).to_owned())
    };
    Ok(Trace {
        ip,
        location: text_field("loc"),
        colo: text_field("colo"),
    })
}

#[cfg(target_os = "linux")]
fn fetch(interface: &str, source: Ipv4Addr, mark: u32) -> Result<Vec<u8>> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    use std::{
        io::{Read, Write},
        net::{SocketAddrV4, TcpStream},
    };

    const TARGET: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 80);
    const STEP: Duration = Duration::from_secs(4);
    const LIMIT_BYTES: u64 = 8192;
    const REQUEST: &[u8] = b"GET /cdn-cgi/trace HTTP/1.1\r\nHost: cloudflare.com\r\nUser-Agent: wayline\r\nConnection: close\r\n\r\n";

    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
        .context("Could not create the egress socket.")?;
    socket
        .bind_device(Some(interface.as_bytes()))
        .context("Could not bind the egress socket to the interface.")?;
    if mark != 0 {
        socket
            .set_mark(mark)
            .context("Could not set the egress socket mark.")?;
    }
    socket
        .bind(&SockAddr::from(SocketAddrV4::new(source, 0)))
        .context("Could not bind the egress source address.")?;
    socket
        .connect_timeout(&SockAddr::from(TARGET), STEP)
        .context("Could not connect to the egress endpoint.")?;
    socket.set_read_timeout(Some(STEP))?;
    socket.set_write_timeout(Some(STEP))?;
    let mut stream = TcpStream::from(socket);
    stream
        .write_all(REQUEST)
        .context("Could not send the egress request.")?;
    let mut response = Vec::new();
    stream
        .take(LIMIT_BYTES)
        .read_to_end(&mut response)
        .context("Could not read the egress response.")?;
    Ok(response)
}

#[cfg(not(target_os = "linux"))]
fn fetch(_interface: &str, _source: Ipv4Addr, _mark: u32) -> Result<Vec<u8>> {
    bail!("Egress probes need Linux.")
}

#[must_use]
pub fn render(results: &[Egress]) -> String {
    results
        .iter()
        .map(|egress| {
            let head = format!("{} ({}):", egress.name, egress.interface);
            match (&egress.ip, &egress.error) {
                (Some(ip), _) => {
                    let details: Vec<&str> = [&egress.location, &egress.colo]
                        .into_iter()
                        .flatten()
                        .map(String::as_str)
                        .collect();
                    if details.is_empty() {
                        format!("{head} {ip}")
                    } else {
                        format!("{head} {ip}  {}", details.join(" "))
                    }
                }
                (None, Some(error)) => format!("{head} {error}"),
                (None, None) => format!("{head} no result"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
