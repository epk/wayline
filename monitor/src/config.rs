use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::net::Ipv4Addr;
use std::path::Path;
use wayline_protocol::{Config, MAX_SERIES};

const MAX_CONFIG: usize = 32 * 1024;

fn valid_name(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

fn valid_path_id(value: &str) -> bool {
    match value.split_once(':') {
        Some((kind, name)) => {
            valid_name(kind, 64)
                && 64_usize
                    .checked_sub(kind.len())
                    .and_then(|rest| rest.checked_sub(1))
                    .is_some_and(|max| valid_name(name, max))
        }
        None => valid_name(value, 64),
    }
}

const fn valid_ip(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && ip.octets()[0] != 0
}

pub fn read(path: &Path) -> Result<Config, Box<dyn Error>> {
    let mut bytes = Vec::with_capacity(MAX_CONFIG + 1);
    File::open(path)?
        .take((MAX_CONFIG + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG {
        return Err("configuration exceeds 32 KiB".into());
    }
    let config: Config = serde_json::from_slice(&bytes)?;
    if (config.tunnels.is_empty() && config.wifi.is_none() && config.telemetry.is_none())
        || config.tunnels.len() > MAX_SERIES
    {
        return Err(format!("configure between one and {MAX_SERIES} measurement series").into());
    }
    if config
        .wifi
        .as_ref()
        .is_some_and(|wifi| !(1_000..=60_000).contains(&wifi.interval_ms))
    {
        return Err("the Wi-Fi interval must be 1000..60000 ms".into());
    }
    if let Some(telemetry) = &config.telemetry {
        let mut seen = std::collections::HashSet::new();
        if !(1_000..=60_000).contains(&telemetry.interval_ms)
            || telemetry.interfaces.len() > 32
            || !telemetry.interfaces.iter().all(|counter| {
                valid_path_id(&counter.id)
                    && valid_name(&counter.device, 15)
                    && counter.device != "lo"
                    && !counter.name.is_empty()
                    && counter.name.len() <= 80
                    && !counter.name.chars().any(char::is_control)
                    && ["uplink", "tunnel", "wifi"].contains(&counter.kind.as_str())
                    && seen.insert(counter.id.as_str())
            })
        {
            return Err("invalid telemetry interval or interface list".into());
        }
    }
    if !(1_000..=300_000).contains(&config.interval_ms)
        || !(50..=5_000).contains(&config.timeout_ms)
    {
        return Err("interval must be 1000..300000 ms. Timeout must be 50..5000 ms.".into());
    }
    if !valid_ip(config.target) {
        return Err("target must be a unicast IPv4 address".into());
    }
    for (i, tunnel) in config.tunnels.iter().enumerate() {
        if tunnel.target.is_some_and(|ip| !valid_ip(ip))
            || (!tunnel.path_id.is_empty() && !valid_path_id(&tunnel.path_id))
            || !valid_name(&tunnel.id, 64)
            || !valid_name(&tunnel.interface, 15)
            || tunnel.interface == "lo"
            || tunnel.name.is_empty()
            || tunnel.name.len() > 80
            || tunnel.name.chars().any(char::is_control)
            || !valid_ip(tunnel.source)
        {
            return Err("invalid tunnel identity, interface, name, or source address".into());
        }
        if config.tunnels[..i].iter().any(|other| {
            other.id == tunnel.id
                || (other.interface == tunnel.interface
                    && other.target.unwrap_or(config.target)
                        == tunnel.target.unwrap_or(config.target))
        }) {
            return Err("duplicate tunnel ID or interface".into());
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::{read, valid_path_id};
    use std::error::Error;

    #[test]
    fn path_ids_limit_the_whole_id_to_64_bytes() {
        let kind = "k".repeat(64);
        assert!(!valid_path_id(&format!("{kind}:x")));
        assert!(!valid_path_id(&format!("{kind}:")));
        assert!(!valid_path_id(&format!("{}:x", "k".repeat(63))));
        assert!(valid_path_id(&format!("{}:x", "k".repeat(62))));
        assert!(!valid_path_id(&format!("{}:xy", "k".repeat(62))));
        assert!(valid_path_id("uplink:wan"));
        assert!(valid_path_id(&"k".repeat(64)));
    }

    #[test]
    fn a_64_byte_kind_is_rejected_at_load() -> Result<(), Box<dyn Error>> {
        let path =
            std::env::temp_dir().join(format!("waylined-config-{}.json", std::process::id()));
        let id = format!("{}:wan", "k".repeat(64));
        let text = format!(
            r#"{{"target":"1.1.1.1","tunnels":[],"telemetry":{{"interval_ms":5000,"interfaces":[{{"id":"{id}","name":"WAN","device":"eth1","kind":"uplink"}}]}}}}"#
        );
        std::fs::write(&path, text)?;
        let result = read(&path);
        std::fs::remove_file(&path)?;
        assert!(result.is_err());
        Ok(())
    }
}
