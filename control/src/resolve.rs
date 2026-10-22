use crate::{
    runtime::{RUNTIME, load, now, store},
    settings::Target,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, ToSocketAddrs},
    time::Duration,
};
use wayline_support::within;

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
        load(Self::path())
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
        let jobs = hosts.iter().cloned().map(|host| {
            move || {
                let found = (host.as_str(), 0)
                    .to_socket_addrs()
                    .into_iter()
                    .flatten()
                    .filter_map(|address| match address.ip() {
                        IpAddr::V4(address) => Some(address),
                        IpAddr::V6(_) => None,
                    })
                    .collect::<Vec<_>>();
                (host, found)
            }
        });
        self.hosts.retain(|host, _| hosts.contains(host));
        for (host, found) in within(DEADLINE, jobs).into_iter().flatten() {
            if !found.is_empty() || !self.hosts.contains_key(&host) {
                self.hosts.insert(host, found);
            }
        }
        self.at = now();
        store(Self::path(), self)
    }
}
