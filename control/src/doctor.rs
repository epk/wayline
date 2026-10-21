use crate::{
    native::{self, Level},
    router,
    runtime::{MONITOR_CONFIG, SETTINGS, STATUS_SOCKET, snapshot, version},
    settings::Settings,
    update,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

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

fn size(path: &str) -> String {
    fs::metadata(path).map_or_else(|_| "missing".into(), |meta| format!("{} bytes", meta.len()))
}

pub fn run() -> bool {
    let mut healthy = true;
    println!("Wayline doctor. Send this output when you report a problem.");
    println!("package version: {}", version());
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
    for path in FILES {
        healthy &= show(Path::new(path).exists(), path, &size(path));
    }
    healthy &= show(
        Path::new("/etc/rc.d/S95wayline").exists(),
        "startup entry",
        "/etc/rc.d/S95wayline",
    );
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
                    "{} tunnel(s), {} connected, WAN path {}",
                    value.tunnels.len(),
                    value.paths.len(),
                    if value.wan.is_some() {
                        "found"
                    } else {
                        "missing"
                    }
                )
            }),
    );
    let list = native::List::read();
    let detail = list.as_ref().map_or_else(ToString::to_string, |value| {
        format!("{} tunnel(s) in priority order", value.tunnels.len())
    });
    show(list.is_ok(), "native VPN reply", &detail);
    let service = native::bus::<Value>("service", "list", &json!({"name":"wayline"}))
        .is_ok_and(|value| value["wayline"]["instances"]["monitor"]["running"] == true);
    let wanted = settings.as_ref().is_ok_and(|value| value.enabled);
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
    println!("measurement series: {}", snapshot().len());
    if let Some(status) = update::Status::read() {
        println!(
            "last update check: {}",
            serde_json::to_string(&status).unwrap_or_default()
        );
    }
    println!(
        "{}",
        if healthy {
            "Result: healthy"
        } else {
            "Result: problems found"
        }
    );
    healthy
}
