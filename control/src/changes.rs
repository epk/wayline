use crate::egress::Cache;
use crate::history;
use crate::router::{Inventory, PathInfo};
use crate::runtime;
use crate::settings::Settings;
use crate::wifi;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::iter;
use std::path::Path;

pub const RECENT: usize = 50;
const MAX_EVENTS: usize = 100;

#[derive(Serialize, Deserialize)]
pub struct Event {
    pub at: u64,
    #[serde(default)]
    pub seq: u64,
    pub subject: String,
    pub kind: String,
    pub text: String,
}

pub struct Fact<'subject> {
    key: String,
    subject: &'subject str,
    label: &'subject str,
    fields: BTreeMap<&'static str, String>,
}

pub fn read(config: &Settings) -> Vec<Event> {
    let mut events: Vec<Event> = runtime::load(history::root(config).join("events.json"));
    for event in &mut events {
        if event.kind == "server" {
            event.text =
                event
                    .text
                    .replacen("Server address changed", "VPN server endpoint changed", 1);
        }
    }
    events
}

fn fact<'a, const N: usize>(
    key: String,
    subject: &'a str,
    label: &'a str,
    fields: [(&'static str, String); N],
) -> Fact<'a> {
    Fact {
        key,
        subject,
        label,
        fields: fields.into_iter().collect(),
    }
}

fn tunnel_facts(path: &PathInfo) -> impl Iterator<Item = Fact<'_>> {
    let tunnel = fact(
        format!("tunnel:{}", path.key),
        &path.key,
        "Tunnel",
        [
            ("interface", path.interface.clone()),
            ("address", path.source.to_string()),
            ("profile", path.profile.clone()),
        ],
    );
    let server = path.endpoint.as_ref().map(|server| {
        fact(
            format!("server:{}", path.key),
            &path.key,
            "VPN server",
            [("server", server.to_string())],
        )
    });
    iter::once(tunnel).chain(server)
}

fn facts(inventory: &Inventory) -> Vec<Fact<'_>> {
    let wan = inventory.wan.as_ref().map(|wan| {
        fact(
            "wan".into(),
            "wan",
            "Direct WAN",
            [
                ("uplink", wan.name.clone()),
                ("interface", wan.interface.clone()),
                ("address", wan.source.to_string()),
            ],
        )
    });
    let uplinks = inventory.uplinks.iter().map(|uplink| {
        fact(
            uplink.id.clone(),
            &uplink.id,
            &uplink.name,
            [
                ("interface", uplink.interface.clone()),
                ("address", uplink.source.to_string()),
            ],
        )
    });
    inventory
        .paths
        .iter()
        .flat_map(tunnel_facts)
        .chain(wan)
        .chain(uplinks)
        .collect()
}

fn describe(label: &str, field: &str, before: &str, after: &str) -> String {
    match field {
        "uplink" => format!("{label} moved from {before} to {after}."),
        "server" => format!("{label} endpoint changed from {before} to {after}."),
        "egress" => format!("{label} public exit IP changed from {before} to {after}."),
        "address" => format!("{label} address changed from {before} to {after}."),
        "network" => format!("{label} network changed from {before} to {after}."),
        "access_point" => format!("{label} access point changed from {before} to {after}."),
        _ => format!("{label} {field} changed from {before} to {after}."),
    }
}

pub fn diff(at: u64, stored: &mut BTreeMap<String, String>, facts: &[Fact]) -> (Vec<Event>, bool) {
    let mut events = Vec::new();
    let mut changed = false;
    for fact in facts {
        let encoded = serde_json::to_string(&fact.fields).unwrap_or_default();
        match stored.get_mut(&fact.key) {
            Some(previous) if *previous == encoded => {}
            Some(previous) => {
                let before: BTreeMap<String, String> =
                    serde_json::from_str(previous).unwrap_or_default();
                let mut found: Vec<Event> = fact
                    .fields
                    .iter()
                    .filter_map(|(field, after)| {
                        let old = before.get(*field).filter(|old| *old != after)?;
                        Some(Event {
                            at,
                            seq: 0,
                            subject: fact.subject.into(),
                            kind: (*field).into(),
                            text: describe(fact.label, field, old, after),
                        })
                    })
                    .collect();
                if found.iter().any(|event| event.kind == "network") {
                    found.retain(|event| event.kind == "network");
                }
                events.extend(found);
                *previous = encoded;
                changed = true;
            }
            None => {
                stored.insert(fact.key.clone(), encoded);
                changed = true;
            }
        }
    }
    (events, changed)
}

pub fn record(config: &Settings, inventory: &Inventory) -> Result<()> {
    runtime::writing("history", || record_inner(config, inventory))
}

fn record_inner(config: &Settings, inventory: &Inventory) -> Result<()> {
    let root = history::root(config);
    let facts_path = root.join("facts.json");
    runtime::directory(facts_path.parent().context("Invalid history directory.")?)?;
    let mut stored: BTreeMap<String, String> = runtime::load(&facts_path);
    let cache = Cache::view(config, inventory);
    let mut current = facts(inventory);
    current.extend(cache.paths.iter().filter_map(|path| {
        let ip = path.ip.as_ref()?;
        let key = &inventory
            .paths
            .iter()
            .chain(inventory.wan.iter())
            .find(|item| item.id == path.id)?
            .key;
        Some(fact(
            format!("egress:{key}"),
            key,
            &path.name,
            [("egress", ip.clone())],
        ))
    }));
    if config.upstream_wifi
        && let Some(status) = runtime::current().wifi
        && let Some(link) = status.link
    {
        let network = if link.ssid.is_empty() {
            "a hidden network".to_owned()
        } else {
            link.ssid.clone()
        };
        current.push(fact(
            "wifi:upstream".into(),
            wifi::SUBJECT,
            "Upstream Wi-Fi",
            [
                ("network", network),
                ("access_point", link.bssid.clone()),
                ("channel", wifi::channel(&link)),
            ],
        ));
    }
    let (added, changed) = diff(runtime::now() * 1000, &mut stored, &current);
    if changed {
        runtime::store(&facts_path, &stored)?;
    }
    if !added.is_empty() {
        append(&root, added)?;
    }
    Ok(())
}

#[cfg(not(test))]
fn sequence_path(_: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from("/etc/wayline/sequence.json")
}

#[cfg(test)]
fn sequence_path(root: &Path) -> std::path::PathBuf {
    root.join("sequence.json")
}

pub fn append(root: &Path, added: impl IntoIterator<Item = Event>) -> Result<()> {
    let path = root.join("events.json");
    let mut events: Vec<Event> = runtime::load(&path);
    let counter = sequence_path(root);
    let last: u64 =
        runtime::load::<u64>(&counter).max(events.iter().map(|event| event.seq).max().unwrap_or(0));
    let mut next = last;
    events.extend(added.into_iter().map(|mut event| {
        next += 1;
        event.seq = next;
        event
    }));
    if next == last {
        return Ok(());
    }
    if let Some(parent) = counter.parent() {
        std::fs::create_dir_all(parent)?;
    }
    runtime::store(&counter, &next)?;
    let excess = events.len().saturating_sub(MAX_EVENTS);
    events.drain(..excess);
    runtime::store(&path, &events)
}

#[cfg(test)]
mod tests {
    use super::{Fact, diff, fact};
    use anyhow::{Result, ensure};
    use std::collections::BTreeMap;

    fn server(address: &str) -> Vec<Fact<'static>> {
        vec![fact(
            "server:a".into(),
            "a",
            "VPN server",
            [("server", address.into())],
        )]
    }

    #[test]
    fn diff_reports_changes_but_not_first_sightings() -> Result<()> {
        let mut stored = BTreeMap::new();
        let (events, changed) = diff(1, &mut stored, &server("184.147.43.41"));
        ensure!(events.is_empty() && changed, "A first sighting is silent.");
        let (events, changed) = diff(2, &mut stored, &server("184.147.43.99"));
        ensure!(changed && events.len() == 1, "Expected one server event.");
        ensure!(events[0].kind == "server" && events[0].subject == "a");
        ensure!(
            events[0].text == "VPN server endpoint changed from 184.147.43.41 to 184.147.43.99.",
            "Unexpected text: {}",
            events[0].text
        );
        let (events, changed) = diff(3, &mut stored, &server("184.147.43.99"));
        ensure!(
            events.is_empty() && !changed,
            "An unchanged fact is silent."
        );
        let exit = |ip: &str| {
            vec![fact(
                "egress:a".into(),
                "a",
                "Tunnel A",
                [("egress", ip.into())],
            )]
        };
        ensure!(diff(4, &mut stored, &exit("203.0.113.1")).0.is_empty());
        ensure!(diff(5, &mut stored, &[]).0.is_empty());
        let (events, _) = diff(6, &mut stored, &exit("203.0.113.2"));
        ensure!(events.len() == 1 && events[0].kind == "egress");
        ensure!(
            events[0].text == "Tunnel A public exit IP changed from 203.0.113.1 to 203.0.113.2."
        );
        Ok(())
    }

    #[test]
    fn wifi_network_changes_report_one_event() -> Result<()> {
        let link = |network: &str, access_point: &str, channel: &str| {
            vec![fact(
                "wifi:upstream".into(),
                "wifi",
                "Upstream Wi-Fi",
                [
                    ("network", network.into()),
                    ("access_point", access_point.into()),
                    ("channel", channel.into()),
                ],
            )]
        };
        let mut stored = BTreeMap::new();
        diff(
            1,
            &mut stored,
            &link("Hotel", "aa:aa:aa:aa:aa:01", "36 · 5 GHz · 80 MHz"),
        );
        let (events, _) = diff(
            2,
            &mut stored,
            &link("Hotel", "aa:aa:aa:aa:aa:02", "36 · 5 GHz · 80 MHz"),
        );
        ensure!(events.len() == 1 && events[0].kind == "access_point");
        ensure!(
            events[0].text
                == "Upstream Wi-Fi access point changed from aa:aa:aa:aa:aa:01 to aa:aa:aa:aa:aa:02."
        );
        let (events, _) = diff(
            3,
            &mut stored,
            &link("Cafe", "bb:bb:bb:bb:bb:01", "6 · 2.4 GHz · 20 MHz"),
        );
        ensure!(
            events.len() == 1
                && events[0].text == "Upstream Wi-Fi network changed from Hotel to Cafe."
        );
        Ok(())
    }

    #[test]
    fn appended_events_get_increasing_sequence_numbers() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let event = |text: &str| super::Event {
            at: 1,
            seq: 0,
            subject: "a".into(),
            kind: "k".into(),
            text: text.into(),
        };
        super::append(directory.path(), [event("one"), event("two")])?;
        std::fs::remove_file(directory.path().join("events.json"))?;
        super::append(directory.path(), [event("three")])?;
        let events: Vec<super::Event> = crate::runtime::load(directory.path().join("events.json"));
        ensure!(
            events.len() == 1 && events[0].seq == 3,
            "Sequence restarted after a purge."
        );
        Ok(())
    }

    #[test]
    fn diff_describes_uplink_moves() -> Result<()> {
        let wan = |name: &str| {
            vec![fact(
                "wan".into(),
                "wan",
                "Direct WAN",
                [("uplink", name.into()), ("address", "192.168.0.99".into())],
            )]
        };
        let mut stored = BTreeMap::new();
        diff(1, &mut stored, &wan("wan"));
        let (events, _) = diff(2, &mut stored, &wan("wwan"));
        ensure!(events.len() == 1 && events[0].kind == "uplink");
        ensure!(events[0].text == "Direct WAN moved from wan to wwan.");
        Ok(())
    }
}
