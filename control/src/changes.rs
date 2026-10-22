use crate::{
    history,
    router::{Inventory, PathInfo},
    runtime::{directory, load, locked, now, store},
    settings::Settings,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, iter, path::Path};

pub const RECENT: usize = 50;
const MAX_EVENTS: usize = 100;

#[derive(Serialize, Deserialize)]
pub struct Event {
    pub at: u64,
    pub subject: String,
    pub kind: String,
    pub text: String,
}

pub struct Fact<'a> {
    key: String,
    subject: &'a str,
    label: &'a str,
    fields: BTreeMap<&'static str, String>,
}

pub fn read(config: &Settings) -> Vec<Event> {
    load(history::database(config).with_file_name("events.json"))
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
        format!("tunnel:{}", path.id),
        &path.id,
        "Tunnel",
        [
            ("interface", path.interface.clone()),
            ("address", path.source.to_string()),
            ("profile", path.profile.clone()),
        ],
    );
    let server = path.endpoint.as_ref().map(|server| {
        fact(
            format!("server:{}", path.id),
            &path.id,
            "Server",
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
        "server" | "address" => format!("{label} address changed from {before} to {after}."),
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
                events.extend(fact.fields.iter().filter_map(|(field, after)| {
                    let old = before.get(*field).filter(|old| *old != after)?;
                    Some(Event {
                        at,
                        subject: fact.subject.into(),
                        kind: (*field).into(),
                        text: describe(fact.label, field, old, after),
                    })
                }));
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
    locked("history", || record_inner(config, inventory))
}

fn record_inner(config: &Settings, inventory: &Inventory) -> Result<()> {
    let database = history::database(config);
    let facts_path = database.with_file_name("facts.json");
    directory(facts_path.parent().context("Invalid history directory.")?)?;
    let mut stored: BTreeMap<String, String> = load(&facts_path);
    let (added, changed) = diff(now() * 1000, &mut stored, &facts(inventory));
    if changed {
        store(&facts_path, &stored)?;
    }
    if !added.is_empty() {
        append(&database, added)?;
    }
    Ok(())
}

pub fn append(database: &Path, added: impl IntoIterator<Item = Event>) -> Result<()> {
    let path = database.with_file_name("events.json");
    let mut events: Vec<Event> = load(&path);
    events.extend(added);
    let excess = events.len().saturating_sub(MAX_EVENTS);
    events.drain(..excess);
    store(&path, &events)
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
            "Server",
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
            events[0].text == "Server address changed from 184.147.43.41 to 184.147.43.99.",
            "Unexpected text: {}",
            events[0].text
        );
        let (events, changed) = diff(3, &mut stored, &server("184.147.43.99"));
        ensure!(
            events.is_empty() && !changed,
            "An unchanged fact is silent."
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
