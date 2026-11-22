use crate::native::{self, Level};
use crate::runtime;
use crate::runtime::{MONITOR_CONFIG, SETTINGS, STATUS_SOCKET};
use crate::settings::Settings;
use crate::{changes, history, router, update, wifi};
use anyhow::Result;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

const FILES: [&str; 6] = [
    "/www/views/gl-sdk4-ui-wayline.common.js.gz",
    "/usr/share/oui/menu.d/wayline.json",
    "/usr/lib/oui-httpd/rpc/wayline",
    "/usr/share/gl-validator.d/wayline.lua",
    "/etc/wayline/release.pub",
    "/usr/sbin/waylined",
];

fn show(ok: bool, label: &str, detail: &str) -> bool {
    println!("[{}] {label}: {detail}", if ok { " ok " } else { "FAIL" });
    ok
}

fn warn(warnings: &mut usize, label: &str, detail: &str) {
    *warnings += 1;
    println!("[WARN] {label}: {detail}");
}

fn chunks(path: &str) -> usize {
    fs::read_dir(path).map_or(0, |items| {
        items
            .filter_map(std::result::Result::ok)
            .filter(|item| item.file_name().to_string_lossy().ends_with(".zst"))
            .count()
    })
}

fn installed() -> bool {
    let mut ok = true;
    for path in FILES {
        ok &= show(Path::new(path).exists(), path, &size(path));
    }
    let chunks = chunks("/www/views/wayline");
    ok &= show(
        chunks > 0,
        "/www/views/wayline",
        &format!("{chunks} page chunks"),
    );
    ok &= show(
        Path::new("/etc/rc.d/S95wayline").exists(),
        "startup entry",
        "/etc/rc.d/S95wayline",
    );
    ok
}

fn size(path: &str) -> String {
    fs::metadata(path).map_or_else(|_| "missing".into(), |meta| format!("{} bytes", meta.len()))
}

fn topology(
    warnings: &mut usize,
    settings: &Result<Settings>,
    inventory: &Result<router::Inventory>,
) {
    let Ok(value) = inventory else {
        return;
    };
    match &value.wan {
        Some(wan) => {
            show(
                true,
                "WAN path",
                &format!("{} on {}", wan.name, wan.interface),
            );
        }
        None => warn(warnings, "WAN path", &value.wan_reason),
    }
    for uplink in &value.uplinks {
        let active = value
            .wan
            .as_ref()
            .is_some_and(|wan| wan.name == uplink.name);
        show(
            true,
            "uplink",
            &format!(
                "{}{} on {} {}",
                uplink.name,
                if active { " (active)" } else { "" },
                uplink.interface,
                uplink.source
            ),
        );
    }
    if let Ok(config) = settings
        && router::endpoints_enabled(config)
    {
        for path in router::monitored(config, value).filter(|path| path.endpoint.is_none()) {
            warn(
                warnings,
                &format!("VPN endpoint {}", path.name),
                &path.endpoint_reason,
            );
        }
    }
}

fn history(warnings: &mut usize, settings: &Result<Settings>) {
    let Ok(config) = settings else {
        return;
    };
    println!("probe interval: {} s", config.interval);
    let report = history::report(config);
    println!(
        "history storage: {} bytes of {} bytes cap (Parquet chunks, zstd)",
        report.bytes, report.cap
    );
    for line in &report.lines {
        println!("{line}");
    }
    if report.bytes * 5 > report.cap * 4 {
        warn(
            warnings,
            "history storage",
            "Above 80% of its cap. The oldest chunks are removed when it fills.",
        );
    }
}

fn wifi_status(
    warnings: &mut usize,
    settings: &Result<Settings>,
    status: Option<&wayline_protocol::WifiStatus>,
) {
    let Ok(config) = settings else {
        return;
    };
    if !config.upstream_wifi {
        println!("upstream Wi-Fi monitoring: off by setting");
        return;
    }
    let Some(status) = status else {
        warn(
            warnings,
            "upstream Wi-Fi",
            "The monitor reports no Wi-Fi status.",
        );
        return;
    };
    let state = serde_json::to_value(status.state).unwrap_or_default();
    println!(
        "upstream Wi-Fi: {} on {} (candidates: {}), sample age {} ms, {} live bucket(s)",
        state.as_str().unwrap_or("unknown"),
        status.interface.as_deref().unwrap_or("none"),
        status.candidates.join(", "),
        status
            .sample_age_ms
            .map_or_else(|| "unknown".into(), |age| age.to_string()),
        status.buckets.len()
    );
    if let Some(link) = &status.link {
        println!(
            "upstream Wi-Fi link: channel {}, signal {} dBm, noise {} dBm, tx {} kbit/s ({}), rx {} kbit/s",
            wifi::channel(link),
            link.signal
                .map_or_else(|| "?".into(), |value| value.to_string()),
            link.noise
                .map_or_else(|| "?".into(), |value| value.to_string()),
            link.tx
                .kbps
                .map_or_else(|| "?".into(), |value| value.to_string()),
            link.tx.mode.as_deref().unwrap_or("?"),
            link.rx
                .kbps
                .map_or_else(|| "?".into(), |value| value.to_string()),
        );
    }
}

fn monitor_details(warnings: &mut usize, settings: &Result<Settings>) {
    let parts = runtime::current();
    println!("measurement series: {}", parts.series.len());
    wifi_status(warnings, settings, parts.wifi.as_ref());
    telemetry_status(settings, parts.telemetry.as_ref());
}

fn telemetry_status(
    settings: &Result<Settings>,
    status: Option<&wayline_protocol::TelemetryStatus>,
) {
    if !settings.as_ref().is_ok_and(|config| config.telemetry) {
        println!("router telemetry: off by setting");
        return;
    }
    let Some(status) = status else {
        println!("router telemetry: no status from the monitor");
        return;
    };
    if let Some(system) = status.system {
        println!(
            "router telemetry: cpu {} permille, load {:?} centi, memory {} of {} bytes available, temperature {} millidegrees, conntrack {}/{}",
            system
                .cpu_permille
                .map_or_else(|| "?".into(), |value| value.to_string()),
            system.load_centi,
            system.mem_available,
            system.mem_total,
            system
                .temperature_milli
                .map_or_else(|| "?".into(), |value| value.to_string()),
            system
                .conntrack
                .map_or_else(|| "?".into(), |value| value.to_string()),
            system
                .conntrack_max
                .map_or_else(|| "?".into(), |value| value.to_string()),
        );
    }
    for link in &status.links {
        println!(
            "interface {} ({}, {}): present {}, rx {} bit/s, tx {} bit/s, errors {}, drops {}, handshake age {}",
            link.id,
            link.device,
            link.kind,
            link.present,
            link.rx_bps
                .map_or_else(|| "?".into(), |value| value.to_string()),
            link.tx_bps
                .map_or_else(|| "?".into(), |value| value.to_string()),
            link.errors,
            link.drops,
            link.handshake_age
                .map_or_else(|| "none".into(), |value| format!("{value} s")),
        );
    }
}

fn recent_changes(settings: &Result<Settings>) {
    if let Ok(config) = settings {
        for event in changes::recent(config, 5).iter().rev() {
            println!("recent change ({}): {}", event.subject, event.text);
        }
    }
}

pub fn run() -> bool {
    let mut healthy = true;
    let mut warnings = 0;
    println!("Wayline doctor. Send this output when you report a problem.");
    println!("package version: {}", runtime::version());
    let compat = native::compat();
    println!("firmware: {}", compat.firmware);
    let level = match compat.level {
        Level::Full if compat.verified => "full (tested module)",
        Level::Full => "full (module newer than tested)",
        Level::Monitoring => "monitoring only",
        Level::Unsupported => "unsupported",
    };
    healthy &= show(
        compat.level != Level::Unsupported,
        "firmware support",
        &format!("{level}. {}", compat.message),
    );
    healthy &= installed();
    let settings = Settings::read();
    healthy &= show(
        settings.is_ok(),
        SETTINGS,
        &settings.as_ref().map_or_else(ToString::to_string, |value| {
            format!(
                "monitoring {}, {} tunnel(s) selected, automatic exit {}",
                if value.enabled { "on" } else { "off" },
                value.tunnels.len(),
                if value.auto_exit { "on" } else { "off" }
            )
        }),
    );
    let inventory = router::inventory();
    healthy &= show(
        inventory.is_ok(),
        "tunnel inventory",
        &inventory
            .as_ref()
            .map_or_else(ToString::to_string, |value| {
                format!(
                    "{} tunnel(s), {} connected",
                    value.tunnels.len(),
                    value.paths.len()
                )
            }),
    );
    topology(&mut warnings, &settings, &inventory);
    history(&mut warnings, &settings);
    recent_changes(&settings);
    let list = native::List::read();
    let detail = list.as_ref().map_or_else(ToString::to_string, |value| {
        format!("{} tunnel(s) in priority order", value.tunnels.len())
    });
    show(list.is_ok(), "native VPN reply", &detail);
    let service = native::bus::<Value>("service", "list", &json!({"name":"wayline"}))
        .is_ok_and(|value| value["wayline"]["instances"]["monitor"]["running"] == true);
    let wanted = settings.as_ref().is_ok_and(Settings::monitoring);
    healthy &= show(
        service || !wanted,
        "monitor service",
        if service {
            "running"
        } else if wanted {
            "not running"
        } else {
            "off by setting"
        },
    );
    println!(
        "monitor config: {}; status socket: {}",
        size(MONITOR_CONFIG),
        size(STATUS_SOCKET)
    );
    monitor_details(&mut warnings, &settings);
    if let Some(status) = update::Status::read() {
        let value = serde_json::to_value(&status).unwrap_or_default();
        println!("last update check: {value}");
        if value["failed"] == true {
            warn(
                &mut warnings,
                "last update check",
                "The last update failed.",
            );
        }
    }
    let result = match (healthy, warnings) {
        (false, _) => "Result: problems found".to_owned(),
        (true, 0) => "Result: healthy".to_owned(),
        (true, count) => format!("Result: healthy with {count} warning(s)"),
    };
    println!("{result}");
    healthy
}
