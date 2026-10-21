use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{fs, path::Path, sync::OnceLock};
use wayline_support::{FIRMWARE_CONTRACT, digest, read, text};

pub const MAX_TUNNELS: usize = 8;

#[derive(Deserialize)]
pub struct List {
    pub tunnels: Vec<Tunnel>,
    #[serde(default)]
    pub default_tunnels: Value,
}

#[derive(Deserialize)]
pub struct Tunnel {
    pub id: String,
    #[serde(rename = "tunnel_id")]
    pub number: u32,
    pub enabled: bool,
    #[serde(default)]
    pub via: Via,
}

#[derive(Default, Deserialize)]
pub struct Via {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    configs: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    #[serde(default)]
    id_list: Vec<Value>,
}

#[derive(Deserialize)]
struct Reply<T> {
    result: T,
}

impl Tunnel {
    pub fn startable(&self) -> bool {
        self.via
            .configs
            .iter()
            .any(|group| !group.id_list.is_empty())
    }
}

impl List {
    pub fn read() -> Result<Self> {
        call("get_tunnel", &json!({}))
    }

    pub fn only_changed(&self, after: &Self, number: u32) -> bool {
        self.tunnels.len() == after.tunnels.len()
            && self
                .tunnels
                .iter()
                .zip(&after.tunnels)
                .all(|(before, after)| {
                    before.number == after.number
                        && (before.number == number
                            || (before.id == after.id && before.enabled == after.enabled))
                })
            && self.default_tunnels == after.default_tunnels
    }
}

const MINIMUM: [u32; 3] = [4, 9, 0];
const VPN_MODULE: &str = "/usr/lib/oui-httpd/rpc/vpn-client";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Unsupported,
    Monitoring,
    Full,
}

#[derive(Clone, Serialize)]
pub struct Compat {
    pub level: Level,
    pub verified: bool,
    pub firmware: String,
    pub message: String,
    pub features: Vec<Feature>,
}

impl Compat {
    pub fn monitoring_only(&mut self) {
        self.level = Level::Monitoring;
        self.verified = false;
        self.features = features(self.level);
        self.message = format!(
            "Firmware {} replies in a format that Wayline does not know. Monitoring and history work. Exit selection and Start and Stop are off.",
            self.firmware
        );
    }
}

#[derive(Clone, Serialize)]
pub struct Feature {
    pub name: &'static str,
    pub available: bool,
}

pub fn features(level: Level) -> Vec<Feature> {
    [
        ("Latency monitoring and history", Level::Monitoring),
        ("Settings", Level::Monitoring),
        ("Exit selection", Level::Full),
        ("Start and Stop", Level::Full),
        ("Automatic exit switching", Level::Full),
        ("Signed updates", Level::Unsupported),
    ]
    .into_iter()
    .map(|(name, needs)| Feature {
        name,
        available: level >= needs,
    })
    .collect()
}

fn version(text: &str) -> Option<[u32; 3]> {
    let mut parts = text
        .trim()
        .split(|byte: char| !byte.is_ascii_digit() && byte != '.')
        .next()?
        .split('.')
        .map(str::parse::<u32>);
    let major = parts.next()?.ok()?;
    let minor = parts.next()?.ok()?;
    let patch = parts.next().and_then(Result::ok).unwrap_or(0);
    Some([major, minor, patch])
}

fn decide(firmware: &str, digest: Option<&str>) -> (Level, bool, String) {
    let Some(number) = version(firmware) else {
        return (
            Level::Unsupported,
            false,
            "Cannot read the firmware version.".into(),
        );
    };
    if number < MINIMUM {
        return (
            Level::Unsupported,
            false,
            format!("Firmware {firmware} is older than 4.9.0. Wayline needs 4.9.0 or newer."),
        );
    }
    match digest {
        None => (
            Level::Unsupported,
            false,
            "The native VPN module is missing.".into(),
        ),
        Some(FIRMWARE_CONTRACT) => (Level::Full, true, String::new()),
        Some(_) => (
            Level::Full,
            false,
            format!(
                "Firmware {firmware} is newer than the firmware that Wayline was tested on. Wayline checks every native reply and stops a change if a reply is not what it expects."
            ),
        ),
    }
}

fn assess() -> Compat {
    let firmware = fs::read_to_string("/etc/glversion").unwrap_or_default();
    let firmware = firmware.trim();
    let digest = read(Path::new(VPN_MODULE), 131_072)
        .ok()
        .map(|data| digest(&data));
    let (level, verified, message) = decide(firmware, digest.as_deref());
    Compat {
        features: features(level),
        level,
        verified,
        firmware: if firmware.is_empty() {
            "unknown".into()
        } else {
            firmware.into()
        },
        message,
    }
}

fn assessed() -> &'static Compat {
    static ASSESSED: OnceLock<Compat> = OnceLock::new();
    ASSESSED.get_or_init(assess)
}

pub fn compat() -> Compat {
    assessed().clone()
}

pub fn require(level: Level) -> Result<()> {
    let compat = assessed();
    ensure!(compat.level >= level, "{}", compat.message);
    Ok(())
}

pub fn contract() -> bool {
    assessed().level == Level::Full
}

pub fn supported() -> bool {
    assessed().level >= Level::Monitoring
}

pub fn bus<T: DeserializeOwned>(object: &str, method: &str, params: &Value) -> Result<T> {
    let encoded = serde_json::to_string(params)?;
    let reply = text(
        "ubus",
        &["-t", "3", "call", object, method, &encoded],
        262_144,
        4,
    )?;
    let reply = if reply.trim().is_empty() {
        "null"
    } else {
        &reply
    };
    serde_json::from_str(reply)
        .with_context(|| format!("Unknown reply from ubus {object} {method}."))
}

pub fn call<T: DeserializeOwned>(method: &str, params: &Value) -> Result<T> {
    let params = json!({"module":"vpn-client","func":method,"params":params}).to_string();
    let (timeout, deadline) = if method == "set_tunnel" {
        ("30", 35)
    } else {
        ("3", 4)
    };
    let reply: Reply<T> = serde_json::from_str(&text(
        "ubus",
        &["-t", timeout, "call", "gl-session", "call", &params],
        262_144,
        deadline,
    )?)
    .context("Unknown native RPC response.")?;
    Ok(reply.result)
}

#[cfg(test)]
mod tests {
    use super::{Level, decide, version};
    use wayline_support::FIRMWARE_CONTRACT;

    #[test]
    fn versions() {
        assert_eq!(version("4.9.1"), Some([4, 9, 1]));
        assert_eq!(version("4.10.0\n"), Some([4, 10, 0]));
        assert_eq!(version("5.0-beta1"), Some([5, 0, 0]));
        assert_eq!(version("4.9.1_release2"), Some([4, 9, 1]));
        assert_eq!(version("garbage"), None);
        assert_eq!(version(""), None);
    }

    #[test]
    fn policy() {
        let known = Some(FIRMWARE_CONTRACT);
        for firmware in ["4.9.0", "4.9.1", "4.10.3", "5.0.0"] {
            let (level, verified, _) = decide(firmware, known);
            assert!(level == Level::Full && verified, "{firmware}");
        }
        for firmware in ["4.9.2", "4.10.0", "5.1.0"] {
            let (level, verified, message) = decide(firmware, Some("changed"));
            assert!(level == Level::Full && !verified && !message.is_empty());
        }
        assert!(decide("4.8.9", known).0 == Level::Unsupported);
        assert!(decide("3.0.0", known).0 == Level::Unsupported);
        assert!(decide("", known).0 == Level::Unsupported);
        assert!(decide("4.9.1", None).0 == Level::Unsupported);
    }
}
