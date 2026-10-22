use crate::router;
use crate::router::{Inventory, PathInfo};
use crate::runtime;
use crate::runtime::RUNTIME;
use crate::settings::Settings;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

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
        runtime::load(Self::path())
    }

    pub fn view(config: &Settings, inventory: &Inventory) -> Self {
        let mut cache = Self::read();
        let paths = scheduled_paths(config, inventory);
        cache.paths.retain(|cached| {
            paths
                .iter()
                .any(|path| path.id == cached.id && path.interface == cached.interface)
        });
        cache
    }

    fn matches(&self, paths: &[&PathInfo]) -> bool {
        paths.len() == self.paths.len()
            && paths.iter().zip(&self.paths).all(|(wanted, cached)| {
                wanted.id == cached.id && wanted.interface == cached.interface
            })
    }

    pub fn refresh(config: &Settings, inventory: &Inventory) -> Result<()> {
        let cache = Self::read();
        let paths = scheduled_paths(config, inventory);
        if runtime::now().saturating_sub(cache.checked_at) < MAX_AGE && cache.matches(&paths) {
            return Ok(());
        }
        let fresh = Self {
            checked_at: runtime::now(),
            paths: probe_paths(&paths),
        };
        runtime::store(Self::path(), &fresh)
    }
}

pub fn scheduled_paths<'a>(config: &'a Settings, inventory: &'a Inventory) -> Vec<&'a PathInfo> {
    if !config.enabled {
        return Vec::new();
    }
    router::monitored(config, inventory)
        .chain(inventory.wan.iter().filter(|_| config.direct_wan))
        .collect()
}

#[derive(Serialize, Deserialize)]
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
    let paths: Vec<&PathInfo> = inventory
        .paths
        .iter()
        .chain(inventory.wan.as_ref())
        .collect();
    probe_paths(&paths)
}

fn probe_paths(paths: &[&PathInfo]) -> Vec<Egress> {
    let jobs = paths.iter().map(|path| {
        let (interface, source, mark) = (path.interface.clone(), path.source, path.mark);
        move || trace(&interface, source, mark)
    });
    paths
        .iter()
        .zip(wayline_support::within(LIMIT, jobs))
        .map(|(path, outcome)| {
            let base = Egress {
                id: path.id.clone(),
                name: path.name.clone(),
                interface: path.interface.clone(),
                ip: None,
                location: None,
                colo: None,
                error: None,
            };
            match outcome
                .context("The egress probe timed out.")
                .and_then(convert::identity)
            {
                Ok(trace) => Egress {
                    ip: Some(trace.ip),
                    location: trace.location,
                    colo: trace.colo,
                    ..base
                },
                Err(error) => Egress {
                    error: Some(format!("{error:#}")),
                    ..base
                },
            }
        })
        .collect()
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
    use std::io::{Read, Write};
    use std::net::{SocketAddrV4, TcpStream};

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
