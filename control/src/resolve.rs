use crate::{
    runtime::{RUNTIME, now},
    settings::Target,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    net::{IpAddr, Ipv4Addr, ToSocketAddrs},
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use wayline_support::atomic;

const DEADLINE: Duration = Duration::from_secs(4);
const MAX_AGE: u64 = 300;

#[derive(Default, Serialize, Deserialize)]
pub struct Resolved {
    at: u64,
    hosts: BTreeMap<String, Vec<Ipv4Addr>>,
}

impl Resolved {
    fn path() -> String {
        format!("{RUNTIME}/resolved.json")
    }

    pub fn read() -> Self {
        fs::read(Self::path())
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    pub fn host(&self, host: &str) -> Option<Ipv4Addr> {
        self.hosts.get(host)?.first().copied()
    }

    pub fn address(&self, target: &Target) -> Option<Ipv4Addr> {
        match target {
            Target::Address(address) => Some(*address),
            Target::Host(host) => self.host(host),
        }
    }

    pub fn stale(&self, hosts: &[String]) -> bool {
        !hosts.is_empty()
            && (now().saturating_sub(self.at) >= MAX_AGE
                || hosts.iter().any(|host| !self.hosts.contains_key(host)))
    }

    pub fn refresh(&mut self, hosts: &[String]) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        for host in hosts {
            let (sender, host) = (sender.clone(), host.clone());
            thread::spawn(move || {
                let found = (host.as_str(), 0)
                    .to_socket_addrs()
                    .map(|found| {
                        found
                            .filter_map(|address| match address.ip() {
                                IpAddr::V4(address) => Some(address),
                                IpAddr::V6(_) => None,
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let _ = sender.send((host, found));
            });
        }
        drop(sender);
        self.hosts.retain(|host, _| hosts.contains(host));
        let started = Instant::now();
        while let Some(left) = DEADLINE.checked_sub(started.elapsed()) {
            let Ok((host, found)) = receiver.recv_timeout(left) else {
                break;
            };
            if !found.is_empty() || !self.hosts.contains_key(&host) {
                self.hosts.insert(host, found);
            }
        }
        self.at = now();
        atomic(Path::new(&Self::path()), &serde_json::to_vec(self)?)
    }
}
