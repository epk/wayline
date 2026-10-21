use std::net::Ipv4Addr;
use std::{error::Error, fs::File, io::Read, path::Path};
use wayline_protocol::{Config, MAX_SERIES};

const MAX_CONFIG: usize = 32 * 1024;

fn valid_name(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
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
    if config.tunnels.is_empty() || config.tunnels.len() > MAX_SERIES {
        return Err("configure between one and 45 measurement series".into());
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
            || (!tunnel.path_id.is_empty() && !valid_name(&tunnel.path_id, 64))
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
