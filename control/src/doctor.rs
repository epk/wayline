use crate::native::{self, Level};
use crate::runtime;
use crate::runtime::{MONITOR_CONFIG, SETTINGS, STATUS_SOCKET};
use crate::settings::Settings;
use crate::{changes, history, router, update, wifi};
use anyhow::Result;
use serde::Serialize;
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

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Info,
}

#[derive(Serialize)]
pub struct Check {
    pub status: Status,
    pub label: String,
    pub detail: String,
}

#[derive(Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    fn push(&mut self, status: Status, label: &str, detail: &str) {
        self.checks.push(Check {
            status,
            label: label.to_owned(),
            detail: detail.to_owned(),
        });
    }

    fn show(&mut self, ok: bool, label: &str, detail: &str) -> bool {
        self.push(if ok { Status::Ok } else { Status::Fail }, label, detail);
        ok
    }

    fn warn(&mut self, label: &str, detail: &str) {
        self.push(Status::Warn, label, detail);
    }

    fn info(&mut self, label: &str, detail: impl AsRef<str>) {
        self.push(Status::Info, label, detail.as_ref());
    }

    pub fn healthy(&self) -> bool {
        self.checks.iter().all(|check| check.status != Status::Fail)
    }

    pub fn warnings(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == Status::Warn)
            .count()
    }
}

fn chunks(path: &str) -> usize {
    fs::read_dir(path).map_or(0, |items| {
        items
            .filter_map(std::result::Result::ok)
            .filter(|item| item.file_name().to_string_lossy().ends_with(".zst"))
            .count()
    })
}

fn installed(report: &mut Report) {
    for path in FILES {
        report.show(Path::new(path).exists(), path, &size(path));
    }
    let chunks = chunks("/www/views/wayline");
    report.show(
        chunks > 0,
        "/www/views/wayline",
        &format!("{chunks} page chunks"),
    );
    report.show(
        Path::new("/etc/rc.d/S95wayline").exists(),
        "startup entry",
        "/etc/rc.d/S95wayline",
    );
}

fn size(path: &str) -> String {
    fs::metadata(path).map_or_else(|_| "missing".into(), |meta| format!("{} bytes", meta.len()))
}

fn topology(
    report: &mut Report,
    settings: &Result<Settings>,
    inventory: &Result<router::Inventory>,
) {
    let Ok(value) = inventory else {
        return;
    };
    match &value.wan {
        Some(wan) => {
            report.show(
                true,
                "WAN path",
                &format!("{} on {}", wan.name, wan.interface),
            );
        }
        None => report.warn("WAN path", &value.wan_reason),
    }
    for uplink in &value.uplinks {
        let active = value
            .wan
            .as_ref()
            .is_some_and(|wan| wan.name == uplink.name);
        report.show(
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
            report.warn(
                &format!("VPN endpoint {}", path.name),
                &path.endpoint_reason,
            );
        }
    }
}

fn history(report: &mut Report, settings: &Result<Settings>) {
    let Ok(config) = settings else {
        return;
    };
    report.info("probe interval", format!("{} s", config.interval));
    let stored = history::report(config);
    report.info(
        "history storage",
        format!(
            "{} bytes of {} bytes cap (Parquet chunks, zstd)",
            stored.bytes, stored.cap
        ),
    );
    for line in &stored.lines {
        let (label, detail) = line.split_once(": ").unwrap_or((line, ""));
        report.info(label, detail);
    }
    if stored.bytes * 5 > stored.cap * 4 {
        report.warn(
            "history storage",
            "Above 80% of its cap. The oldest chunks are removed when it fills.",
        );
    }
}

fn wifi_status(
    report: &mut Report,
    settings: &Result<Settings>,
    status: Option<&wayline_protocol::WifiStatus>,
) {
    let Ok(config) = settings else {
        return;
    };
    if !config.upstream_wifi {
        report.info("upstream Wi-Fi monitoring", "off by setting");
        return;
    }
    let Some(status) = status else {
        report.warn("upstream Wi-Fi", "The monitor reports no Wi-Fi status.");
        return;
    };
    let state = serde_json::to_value(status.state).unwrap_or_default();
    report.info(
        "upstream Wi-Fi",
        format!(
            "{} on {} (candidates: {}), sample age {} ms, {} live bucket(s)",
            state.as_str().unwrap_or("unknown"),
            status.interface.as_deref().unwrap_or("none"),
            status.candidates.join(", "),
            status
                .sample_age_ms
                .map_or_else(|| "unknown".into(), |age| age.to_string()),
            status.buckets.len()
        ),
    );
    if let Some(link) = &status.link {
        report.info(
            "upstream Wi-Fi link",
            format!(
                "channel {}, signal {} dBm, noise {} dBm, tx {} kbit/s ({}), rx {} kbit/s",
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
            ),
        );
    }
}

fn monitor_details(report: &mut Report, settings: &Result<Settings>) {
    let parts = runtime::current();
    report.info("measurement series", parts.series.len().to_string());
    wifi_status(report, settings, parts.wifi.as_ref());
    telemetry_status(report, settings, parts.telemetry.as_ref());
}

fn telemetry_status(
    report: &mut Report,
    settings: &Result<Settings>,
    status: Option<&wayline_protocol::TelemetryStatus>,
) {
    if !settings.as_ref().is_ok_and(|config| config.telemetry) {
        report.info("router telemetry", "off by setting");
        return;
    }
    let Some(status) = status else {
        report.info("router telemetry", "no status from the monitor");
        return;
    };
    if let Some(system) = status.system {
        report.info(
            "router telemetry",
            format!(
            "cpu {} permille, load {:?} centi, memory {} of {} bytes available, temperature {} millidegrees, conntrack {}/{}",
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
            ),
        );
    }
    for link in &status.links {
        report.info(
            &format!("interface {} ({}, {})", link.id, link.device, link.kind),
            format!(
                "present {}, rx {} bit/s, tx {} bit/s, errors {}, drops {}, handshake age {}",
                link.present,
                link.rx_bps
                    .map_or_else(|| "?".into(), |value| value.to_string()),
                link.tx_bps
                    .map_or_else(|| "?".into(), |value| value.to_string()),
                link.errors,
                link.drops,
                link.handshake_age
                    .map_or_else(|| "none".into(), |value| format!("{value} s")),
            ),
        );
    }
}

fn recent_changes(report: &mut Report, settings: &Result<Settings>) {
    if let Ok(config) = settings {
        for event in changes::recent(config, 5).iter().rev() {
            report.info(&format!("recent change ({})", event.subject), &event.text);
        }
    }
}

pub fn collect() -> Report {
    let mut report = Report::default();
    report.info("package version", runtime::version());
    let compat = native::compat();
    report.info("firmware", &compat.firmware);
    let level = match compat.level {
        Level::Full if compat.verified => "full (tested module)",
        Level::Full => "full (module newer than tested)",
        Level::Monitoring => "monitoring only",
        Level::Unsupported => "unsupported",
    };
    report.show(
        compat.level != Level::Unsupported,
        "firmware support",
        &if compat.message.is_empty() {
            level.to_owned()
        } else {
            format!("{level}. {}", compat.message)
        },
    );
    installed(&mut report);
    let settings = Settings::read();
    report.show(
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
    report.show(
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
    topology(&mut report, &settings, &inventory);
    history(&mut report, &settings);
    recent_changes(&mut report, &settings);
    let list = native::List::read();
    let detail = list.as_ref().map_or_else(ToString::to_string, |value| {
        format!("{} tunnel(s) in priority order", value.tunnels.len())
    });
    report.push(
        if list.is_ok() {
            Status::Ok
        } else {
            Status::Warn
        },
        "native VPN reply",
        &detail,
    );
    let service = native::bus::<Value>("service", "list", &json!({"name":"wayline"}))
        .is_ok_and(|value| value["wayline"]["instances"]["monitor"]["running"] == true);
    let wanted = settings.as_ref().is_ok_and(Settings::monitoring);
    report.show(
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
    report.info(
        "monitor config",
        format!(
            "{}; status socket: {}",
            size(MONITOR_CONFIG),
            size(STATUS_SOCKET)
        ),
    );
    monitor_details(&mut report, &settings);
    if let Some(status) = update::Status::read() {
        let value = serde_json::to_value(&status).unwrap_or_default();
        report.info("last update check", value.to_string());
        if value["failed"] == true {
            report.warn("last update check", "The last update failed.");
        }
    }
    report
}

pub fn run() -> bool {
    println!("Wayline doctor. Send this output when you report a problem.");
    let report = collect();
    for check in &report.checks {
        match check.status {
            Status::Ok => println!("[ ok ] {}: {}", check.label, check.detail),
            Status::Fail => println!("[FAIL] {}: {}", check.label, check.detail),
            Status::Warn => println!("[WARN] {}: {}", check.label, check.detail),
            Status::Info => println!("{}: {}", check.label, check.detail),
        }
    }
    let healthy = report.healthy();
    let result = match (healthy, report.warnings()) {
        (false, _) => "Result: problems found".to_owned(),
        (true, 0) => "Result: healthy".to_owned(),
        (true, count) => format!("Result: healthy with {count} warning(s)"),
    };
    println!("{result}");
    healthy
}
