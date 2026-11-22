use crate::runtime;
use crate::runtime::RUNTIME;
use crate::settings::Target;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::time::Duration;

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
        runtime::load(Self::path())
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
            && (runtime::now().saturating_sub(self.at) >= MAX_AGE
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
        let resolved = wayline_support::within(DEADLINE, jobs)
            .into_iter()
            .flatten();
        self.merge(hosts, resolved);
        self.at = runtime::now();
        runtime::store(Self::path(), self)
    }

    fn merge(
        &mut self,
        hosts: &[String],
        resolved: impl IntoIterator<Item = (String, Vec<Ipv4Addr>)>,
    ) {
        self.hosts.retain(|host, _| hosts.contains(host));
        for (host, found) in resolved {
            let current = self.host(&host);
            if !found.is_empty() || !self.hosts.contains_key(&host) {
                self.hosts.insert(host, sticky(current, found));
            }
        }
    }
}

fn sticky(current: Option<Ipv4Addr>, mut found: Vec<Ipv4Addr>) -> Vec<Ipv4Addr> {
    found.sort_unstable();
    found.dedup();
    if let Some(index) =
        current.and_then(|current| found.iter().position(|&address| address == current))
    {
        found[..=index].rotate_right(1);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{Resolved, sticky};
    use std::net::Ipv4Addr;

    const A: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);
    const B: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 2);
    const C: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 3);

    #[test]
    fn answers_are_sorted_and_the_current_address_stays_first() {
        assert_eq!(sticky(None, vec![C, A, B, A]), [A, B, C]);
        assert_eq!(sticky(Some(C), vec![B, C, A]), [C, A, B]);
        assert_eq!(sticky(Some(Ipv4Addr::LOCALHOST), vec![B, A]), [A, B]);
        assert_eq!(sticky(Some(A), Vec::new()), Vec::<Ipv4Addr>::new());
    }

    #[test]
    fn rotating_answers_keep_the_monitored_address() {
        let hosts = ["probe.example".to_owned(), "gone.example".to_owned()];
        let mut resolved = Resolved::default();
        resolved.merge(
            &hosts,
            [(hosts[0].clone(), vec![B, A]), (hosts[1].clone(), vec![C])],
        );
        assert_eq!(resolved.host(&hosts[0]), Some(A));
        for answer in [vec![B, A], vec![A, B], vec![C, B, A]] {
            resolved.merge(&hosts, [(hosts[0].clone(), answer)]);
            assert_eq!(resolved.host(&hosts[0]), Some(A));
        }
        resolved.merge(&hosts, [(hosts[0].clone(), Vec::new())]);
        assert_eq!(resolved.host(&hosts[0]), Some(A));
        resolved.merge(&hosts, [(hosts[0].clone(), vec![C, B])]);
        assert_eq!(resolved.host(&hosts[0]), Some(B));
        resolved.merge(&hosts[..1], []);
        assert_eq!(resolved.host(&hosts[1]), None);
    }
}
