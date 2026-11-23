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
    if (config.tunnels.is_empty()
        && config.wifi.is_none()
        && config.telemetry.is_none()
        && config.sqm.is_none())
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
    if let Some(sqm) = &config.sqm {
        let mut seen = std::collections::HashSet::new();
        let valid_rates = |min_kbps: u32, base_kbps: u32, max_kbps: u32| {
            min_kbps >= 1_000
                && min_kbps <= base_kbps
                && base_kbps <= max_kbps
                && max_kbps <= 2_000_000
        };
        if sqm.interfaces.len() > 16
            || !(5..=500).contains(&sqm.delay_threshold_ms)
            || !valid_rates(sqm.min_dl_kbps, sqm.base_dl_kbps, sqm.max_dl_kbps)
            || !valid_rates(sqm.min_ul_kbps, sqm.base_ul_kbps, sqm.max_ul_kbps)
            || !sqm.interfaces.iter().all(|iface| {
                valid_path_id(&iface.id)
                    && !iface.name.is_empty()
                    && iface.name.len() <= 80
                    && !iface.name.chars().any(char::is_control)
                    && valid_name(&iface.egress_device, 15)
                    && iface.egress_device != "lo"
                    && valid_name(&iface.ingress_device, 15)
                    && iface.ingress_device != "lo"
                    && seen.insert(iface.id.as_str())
            })
        {
            return Err("invalid sqm configuration".into());
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

    fn load_text(text: &str) -> Result<wayline_protocol::Config, Box<dyn Error>> {
        let path = std::env::temp_dir().join(format!(
            "waylined-sqm-config-{}-{}.json",
            std::process::id(),
            text.len()
        ));
        std::fs::write(&path, text)?;
        let result = read(&path);
        std::fs::remove_file(&path)?;
        result
    }

    #[test]
    fn valid_sqm_config_uses_defaults() -> Result<(), Box<dyn Error>> {
        let text = r#"{"target":"1.1.1.1","tunnels":[],"sqm":{"interfaces":[{"id":"wan","name":"Primary WAN","egress_device":"eth1","ingress_device":"ifb4eth1"}]}}"#;
        let config = load_text(text)?;
        let sqm = config.sqm.ok_or("missing sqm config")?;
        assert_eq!(sqm.delay_threshold_ms, 20);
        assert_eq!(sqm.min_dl_kbps, 5_000);
        assert_eq!(sqm.base_dl_kbps, 40_000);
        assert_eq!(sqm.max_dl_kbps, 500_000);
        assert_eq!(sqm.min_ul_kbps, 5_000);
        assert_eq!(sqm.base_ul_kbps, 40_000);
        assert_eq!(sqm.max_ul_kbps, 500_000);
        Ok(())
    }

    #[test]
    fn invalid_sqm_configs_are_rejected() {
        let bad_threshold = r#"{"target":"1.1.1.1","tunnels":[],"sqm":{"delay_threshold_ms":4,"interfaces":[{"id":"wan","name":"WAN","egress_device":"eth1","ingress_device":"ifb4eth1"}]}}"#;
        assert!(load_text(bad_threshold).is_err());

        let bad_rates = r#"{"target":"1.1.1.1","tunnels":[],"sqm":{"min_dl_kbps":50000,"base_dl_kbps":40000,"interfaces":[{"id":"wan","name":"WAN","egress_device":"eth1","ingress_device":"ifb4eth1"}]}}"#;
        assert!(load_text(bad_rates).is_err());

        let bad_ul_rates = r#"{"target":"1.1.1.1","tunnels":[],"sqm":{"min_ul_kbps":999,"interfaces":[{"id":"wan","name":"WAN","egress_device":"eth1","ingress_device":"ifb4eth1"}]}}"#;
        assert!(load_text(bad_ul_rates).is_err());

        let loopback = r#"{"target":"1.1.1.1","tunnels":[],"sqm":{"interfaces":[{"id":"wan","name":"WAN","egress_device":"lo","ingress_device":"ifb4eth1"}]}}"#;
        assert!(load_text(loopback).is_err());

        let dup_id = r#"{"target":"1.1.1.1","tunnels":[],"sqm":{"interfaces":[{"id":"wan","name":"WAN 1","egress_device":"eth1","ingress_device":"ifb4eth1"},{"id":"wan","name":"WAN 2","egress_device":"eth2","ingress_device":"ifb4eth2"}]}}"#;
        assert!(load_text(dup_id).is_err());

        let control_name = "{\"target\":\"1.1.1.1\",\"tunnels\":[],\"sqm\":{\"interfaces\":[{\"id\":\"wan\",\"name\":\"WAN\\n1\",\"egress_device\":\"eth1\",\"ingress_device\":\"ifb4eth1\"}]}}";
        assert!(load_text(control_name).is_err());

        let too_many_items: Vec<String> = (0..17)
            .map(|i| {
                format!(
                    r#"{{"id":"wan{i}","name":"WAN {i}","egress_device":"eth{i}","ingress_device":"ifb4eth{i}"}}"#
                )
            })
            .collect();
        let too_many = format!(
            r#"{{"target":"1.1.1.1","tunnels":[],"sqm":{{"interfaces":[{}]}}}}"#,
            too_many_items.join(",")
        );
        assert!(load_text(&too_many).is_err());
    }
}
