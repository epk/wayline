use crate::{
    history,
    router::{Inventory, PathInfo},
    runtime::{directory, now},
    settings::Settings,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, fs, path::Path};
use wayline_support::atomic;

pub const RECENT: usize = 50;
const MAX_EVENTS: usize = 100;

#[derive(Clone, Serialize, Deserialize)]
pub struct Event {
    pub at: u64,
    pub subject: String,
    pub kind: String,
    pub text: String,
}

pub struct Fact {
    key: String,
    subject: String,
    label: String,
    fields: BTreeMap<&'static str, String>,
}

fn load<T: Default + DeserializeOwned>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

pub fn read(config: &Settings) -> Vec<Event> {
    load(&history::database(config).with_file_name("events.json"))
}

fn fact<const N: usize>(
    key: String,
    subject: &str,
    label: &str,
    fields: [(&'static str, String); N],
) -> Fact {
    Fact {
        key,
        subject: subject.into(),
        label: label.into(),
        fields: fields.into_iter().collect(),
    }
}

fn tunnel_fact(path: &PathInfo) -> Vec<Fact> {
    let mut facts = vec![fact(
        format!("tunnel:{}", path.id),
        &path.id,
        "Tunnel",
        [
            ("interface", path.interface.clone()),
            ("address", path.source.to_string()),
            ("profile", path.profile.clone()),
        ],
    )];
    if let Some(server) = &path.endpoint {
        facts.push(fact(
            format!("server:{}", path.id),
            &path.id,
            "Server",
            [("server", server.to_string())],
        ));
    }
    facts
}

fn facts(inventory: &Inventory) -> Vec<Fact> {
    let mut facts: Vec<Fact> = inventory.paths.iter().flat_map(tunnel_fact).collect();
    if let Some(wan) = &inventory.wan {
        facts.push(fact(
            "wan".into(),
            "wan",
            "Direct WAN",
            [
                ("uplink", wan.name.clone()),
                ("interface", wan.interface.clone()),
                ("address", wan.source.to_string()),
            ],
        ));
    }
    facts.extend(inventory.uplinks.iter().map(|uplink| {
        fact(
            uplink.id.clone(),
            &uplink.id,
            &uplink.name,
            [
                ("interface", uplink.interface.clone()),
                ("address", uplink.source.to_string()),
            ],
        )
    }));
    facts
}

fn describe(label: &str, field: &str, before: &str, after: &str) -> String {
    match field {
        "uplink" => format!("{label} moved from {before} to {after}."),
        "server" | "address" => format!("{label} address changed from {before} to {after}."),
        _ => format!("{label} {field} changed from {before} to {after}."),
    }
}

pub fn diff(at: u64, stored: &mut BTreeMap<String, String>, facts: &[Fact]) -> Vec<Event> {
    let mut events = Vec::new();
    for fact in facts {
        let encoded = serde_json::to_string(&fact.fields).unwrap_or_default();
        if let Some(previous) = stored
            .get(&fact.key)
            .filter(|value| value.as_str() != encoded)
        {
            let before: BTreeMap<String, String> =
                serde_json::from_str(previous).unwrap_or_default();
            for (field, after) in &fact.fields {
                if let Some(old) = before.get(*field).filter(|old| *old != after) {
                    events.push(Event {
                        at,
                        subject: fact.subject.clone(),
                        kind: (*field).into(),
                        text: describe(&fact.label, field, old, after),
                    });
                }
            }
        }
        stored.insert(fact.key.clone(), encoded);
    }
    events
}

pub fn record(config: &Settings, inventory: &Inventory) -> Result<()> {
    let facts_path = history::database(config).with_file_name("facts.json");
    directory(facts_path.parent().context("Invalid history directory.")?)?;
    let mut stored: BTreeMap<String, String> = load(&facts_path);
    let before = stored.clone();
    let added = diff(now() * 1000, &mut stored, &facts(inventory));
    if stored != before {
        atomic(&facts_path, &serde_json::to_vec(&stored)?)?;
    }
    if !added.is_empty() {
        append(&history::database(config), added)?;
    }
    Ok(())
}

pub fn append(database: &Path, added: Vec<Event>) -> Result<()> {
    let path = database.with_file_name("events.json");
    let mut events: Vec<Event> = load(&path);
    events.extend(added);
    let excess = events.len().saturating_sub(MAX_EVENTS);
    events.drain(..excess);
    atomic(&path, &serde_json::to_vec(&events)?)
}

#[cfg(test)]
mod tests {
    use super::{Fact, diff, fact};
    use anyhow::{Result, ensure};
    use std::collections::BTreeMap;

    fn server(address: &str) -> Vec<Fact> {
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
        ensure!(diff(1, &mut stored, &server("184.147.43.41")).is_empty());
        let events = diff(2, &mut stored, &server("184.147.43.99"));
        ensure!(events.len() == 1, "Expected one server event.");
        ensure!(events[0].kind == "server" && events[0].subject == "a");
        ensure!(
            events[0].text == "Server address changed from 184.147.43.41 to 184.147.43.99.",
            "Unexpected text: {}",
            events[0].text
        );
        ensure!(diff(3, &mut stored, &server("184.147.43.99")).is_empty());
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
        let events = diff(2, &mut stored, &wan("wwan"));
        ensure!(events.len() == 1 && events[0].kind == "uplink");
        ensure!(events[0].text == "Direct WAN moved from wan to wwan.");
        Ok(())
    }
}
