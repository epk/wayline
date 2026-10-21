#![forbid(unsafe_code)]

mod history;
mod router;
mod settings;
mod update;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use settings::{RUNTIME, SETTINGS, Settings};
use std::{fs, os::unix::fs::symlink, path::Path};
use wayline_support::{atomic, output, text};

fn snapshot() -> Value {
    output(
        "/usr/sbin/waylined",
        &["status", "--socket", "/var/run/wayline/status.sock"],
        393_216,
        3,
    )
    .ok()
    .and_then(|data| serde_json::from_slice(&data).ok())
    .unwrap_or_else(|| json!({"tunnels":[]}))
}

fn version() -> String {
    fs::read_to_string("/usr/share/wayline/version")
        .unwrap_or_else(|_| "0.1.0-alpha.2".into())
        .trim()
        .into()
}

fn locked<T>(name: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let path = format!("{RUNTIME}/{name}.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    lock.try_lock().context("Another operation is active.")?;
    action()
}

fn state() -> Result<Value> {
    let revision = router::revision()?;
    let mut config = Settings::read()?;
    let inventory = router::inventory()?;
    router::normalize(&mut config, &inventory);
    let snapshot = snapshot();
    let native = router::native("get_tunnel", &json!({})).unwrap_or(Value::Null);
    let preferred = native["tunnels"][0]["id"].as_str();
    let measurements = snapshot["tunnels"].as_array().cloned().unwrap_or_default();
    let mut tunnels = Vec::new();
    let mut diagnostics = serde_json::Map::new();
    let compatible = router::contract();
    for tunnel in &inventory.tunnels {
        let path = inventory.paths.iter().find(|path| path.id == tunnel.id);
        let item = path
            .and_then(|path| {
                measurements
                    .iter()
                    .find(|value| value["id"] == path.series(&config.target.to_string()))
            })
            .cloned()
            .unwrap_or(Value::Null);
        let recent: Vec<_> = item["history_us"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| value.as_f64().map(|value| value / 1000.0))
            .collect();
        let eligible = ["reachable", "probing", "checking_route"]
            .contains(&item["state"].as_str().unwrap_or_default())
            && compatible
            && router::safe_order(&native, &inventory, &tunnel.id).is_ok()
            && item["success_age_ms"]
                .as_u64()
                .is_some_and(|age| age <= u64::from(config.interval) * 3000)
            && item["lost"] == 0;
        let status = if !tunnel.enabled {
            "Stopped"
        } else if path.is_none() {
            "Enabled, offline"
        } else {
            "Connected"
        };
        tunnels.push(json!({"id":tunnel.id,"monitorId":tunnel.key,"name":tunnel.name,"enabled":tunnel.enabled,"connected":path.is_some(),
            "controllable":compatible && router::native_tunnel(&native, &inventory, &tunnel.id).is_ok_and(|native| tunnel.enabled || router::startable(native)),
            "preferred":preferred==Some(tunnel.id.as_str()),"eligible":eligible,
            "reason":if eligible {""} else {"The traffic rules or recent measurements prevent selection."},
            "stateLabel":status,"measurementState":item["state"],"age":item["success_age_ms"].as_u64().map_or_else(||"unknown".into(),|age|format!("{}s",age/1000)),
            "median":item["median_us"].as_f64().map(|value|value/1000.0),"samples":item["samples"].as_u64().unwrap_or(0),"lost":item["lost"].as_u64().unwrap_or(0),"recent":recent}));
    }
    for target in &config.targets {
        let mut values = serde_json::Map::new();
        for path in inventory.paths.iter().chain(inventory.wan.iter()) {
            let latency = measurements
                .iter()
                .find(|item| item["id"] == path.series(&target.to_string()))
                .and_then(|item| item["median_us"].as_f64())
                .map(|value| value / 1000.0);
            values.insert(path.id.clone(), json!(latency));
        }
        diagnostics.insert(target.to_string(), Value::Object(values));
    }
    ensure!(
        router::revision()? == revision,
        "State changed during discovery. Refresh again."
    );
    Ok(
        json!({"version":version(),"config":config,"tunnels":tunnels,"diagnostics":diagnostics,"revision":revision,
        "scope":"Equivalent native traffic rules","capabilities":{"configure":compatible,"select":compatible,"control":compatible,"reason":if compatible {""} else {"This firmware is unsupported."}},
        "update":fs::read(format!("{RUNTIME}/update.json")).ok().and_then(|data|serde_json::from_slice::<Value>(&data).ok())}),
    )
}

fn set_config(args: &Value) -> Result<Value> {
    locked("settings", || {
        ensure!(router::contract(), "This firmware is unsupported.");
        ensure!(
            args["expected_revision"] == router::revision()?,
            "State changed. Refresh first."
        );
        let mut config: Settings = serde_json::from_value(args["config"].clone())?;
        let inventory = router::inventory()?;
        router::normalize(&mut config, &inventory);
        config.validate()?;
        ensure!(
            config
                .tunnels
                .iter()
                .all(|id| inventory.tunnels.iter().any(|tunnel| &tunnel.key == id)),
            "A selected tunnel is unavailable."
        );
        if Settings::read()? != config {
            atomic(Path::new(SETTINGS), &serde_json::to_vec(&config)?)?;
            reconcile()?;
        }
        Ok(json!({"applied":true}))
    })
}

fn set_tunnel_enabled(args: &Value) -> Result<Value> {
    locked("selection", || {
        locked("settings", || {
            ensure!(router::contract(), "This firmware is unsupported.");
            let id = args["section_id"]
                .as_str()
                .context("A tunnel section is required.")?;
            let enabled = args["enabled"]
                .as_bool()
                .context("A boolean tunnel state is required.")?;
            ensure!(settings::id(id), "Invalid tunnel section.");
            let revision = router::revision()?;
            ensure!(
                args["expected_revision"] == revision,
                "State changed. Refresh first."
            );
            let inventory = router::inventory()?;
            let before = router::native("get_tunnel", &json!({}))?;
            let tunnel = router::native_tunnel(&before, &inventory, id)?;
            if tunnel["enabled"] == enabled {
                return Ok(json!({"applied":true,"unchanged":true,"enabled":enabled}));
            }
            ensure!(
                !enabled || router::startable(tunnel),
                "Assign a VPN profile in the native dashboard before starting this tunnel."
            );
            let number = tunnel["tunnel_id"].clone();
            ensure!(
                router::revision()? == revision,
                "State changed before the request. Refresh first."
            );
            let mut config = Settings::read()?;
            router::normalize(&mut config, &inventory);
            if config != Settings::read()? {
                atomic(Path::new(SETTINGS), &serde_json::to_vec(&config)?)?;
            }
            router::native("set_tunnel", &json!({"tunnel_id":number,"enabled":enabled}))
                .context("Tunnel outcome is unknown. Refresh before another change.")?;
            let after = router::native("get_tunnel", &json!({}))
                .context("Tunnel outcome is unknown. Refresh before another change.")?;
            let observed = router::inventory()?;
            let actual_id = after["tunnels"]
                .as_array()
                .context("Unknown native tunnel schema.")?
                .iter()
                .find(|item| item["tunnel_id"] == number)
                .and_then(|item| item["id"].as_str())
                .context("Tunnel outcome is unknown. Refresh first.")?;
            let actual = router::native_tunnel(&after, &observed, actual_id)?;
            ensure!(
                actual["tunnel_id"] == number && actual["enabled"] == enabled,
                "Tunnel outcome differs from the request. Refresh first."
            );
            let before_tunnels = before["tunnels"]
                .as_array()
                .context("Unknown native tunnel schema.")?;
            let after_tunnels = after["tunnels"]
                .as_array()
                .context("Unknown native tunnel schema.")?;
            ensure!(
                before_tunnels.len() == after_tunnels.len()
                    && before_tunnels
                        .iter()
                        .zip(after_tunnels)
                        .all(
                            |(previous, current)| previous["tunnel_id"] == current["tunnel_id"]
                                && (previous["tunnel_id"] == number
                                    || (previous["id"] == current["id"]
                                        && previous["enabled"] == current["enabled"]))
                        )
                    && before["default_tunnels"] == after["default_tunnels"],
                "The VPN state changed concurrently. Refresh first."
            );
            let monitor_pending = reconcile().is_err();
            Ok(json!({"applied":true,"enabled":enabled,"monitor_pending":monitor_pending}))
        })
    })
}

fn select(args: &Value) -> Result<Value> {
    locked("selection", || {
        ensure!(router::contract(), "This firmware is unsupported.");
        let id = args["section_id"]
            .as_str()
            .context("A tunnel section is required.")?;
        let inventory = router::inventory()?;
        let native = router::native("get_tunnel", &json!({}))?;
        let (desired, changed) = router::safe_order(&native, &inventory, id)?;
        if !changed {
            return Ok(json!({"applied":true,"unchanged":true}));
        }
        ensure!(
            args["expected_revision"] == router::revision()?,
            "State changed. Refresh first."
        );
        let current = state()?;
        ensure!(
            current["tunnels"].as_array().is_some_and(|items| items
                .iter()
                .any(|item| item["id"] == id && item["eligible"] == true)),
            "The selected tunnel has no recent healthy measurements."
        );
        ensure!(
            args["expected_revision"] == router::revision()?,
            "State changed before selection. Refresh first."
        );
        router::native("order_tunnel", &json!({"id_list":desired}))?;
        let observed = router::native("get_tunnel", &json!({}))?;
        let order: Vec<_> = observed["tunnels"]
            .as_array()
            .context("Selection outcome is unknown. Refresh first.")?
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        ensure!(
            order == desired,
            "Selection changed concurrently. Refresh first."
        );
        Ok(json!({"applied":true}))
    })
}

fn reconcile() -> Result<()> {
    locked("reconcile", reconcile_inner)
}

fn reconcile_inner() -> Result<()> {
    let config = Settings::read()?;
    let path = Path::new("/var/run/wayline/monitor.json");
    if !config.enabled || !router::contract() {
        let _ = fs::remove_file(path);
        let _ = router::bus("service", "delete", &json!({"name":"wayline"}));
        return Ok(());
    }
    let desired = router::monitor(&config, &router::inventory()?);
    if desired["tunnels"].as_array().is_none_or(Vec::is_empty) {
        let _ = fs::remove_file(path);
        let _ = router::bus("service", "delete", &json!({"name":"wayline"}));
        return Ok(());
    }
    let encoded = serde_json::to_vec(&desired)?;
    let changed = fs::read(path).ok().as_deref() != Some(&encoded);
    let running = router::bus("service", "list", &json!({"name":"wayline"}))
        .is_ok_and(|value| value["wayline"]["instances"]["monitor"]["running"] == true);
    if changed || !running {
        if changed {
            atomic(path, &encoded)?;
        }
        router::bus(
            "service",
            "set",
            &json!({"name":"wayline","instances":{"monitor":{
            "command":["/usr/sbin/waylined","monitor","--config",path,"--socket","/var/run/wayline/status.sock"],
            "respawn":[3600,5,3],"limits":{"core":"0","nofile":"128 128"},"stderr":true,"file":[path]}}}),
        )?;
    }
    Ok(())
}

fn lifecycle(install: bool) -> Result<()> {
    let cron = Path::new("/etc/crontabs/root");
    let mut lines: Vec<_> = fs::read_to_string(cron)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.contains("/usr/sbin/wayline-ctl tick"))
        .map(str::to_owned)
        .collect();
    let startup = Path::new("/etc/rc.d/S95wayline");
    if install {
        lines.push("* * * * * /usr/sbin/wayline-ctl tick >/dev/null 2>&1".into());
        if !startup.exists() {
            symlink("/usr/sbin/wayline-ctl", startup)?;
        }
    } else {
        let _ = fs::remove_file(startup);
        let _ = router::bus("service", "delete", &json!({"name":"wayline"}));
    }
    fs::create_dir_all("/etc/crontabs")?;
    atomic(cron, format!("{}\n", lines.join("\n")).as_bytes())?;
    let _ = text("/etc/init.d/cron", &["reload"], 8192, 10);
    if install {
        let _ = router::bus("service", "delete", &json!({"name":"wayline"}));
        reconcile()?;
    }
    Ok(())
}

fn tick() -> Result<()> {
    locked("tick", || {
        reconcile()?;
        let config = Settings::read()?;
        let last = fs::read_to_string(format!("{RUNTIME}/last-collect"))
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let now = history::now();
        if config.enabled && (now.saturating_sub(last) >= 300 || now < last) {
            let result = history::collect(&config, &snapshot());
            atomic(
                Path::new(&format!("{RUNTIME}/last-collect")),
                now.to_string().as_bytes(),
            )?;
            result?;
        }
        update::scheduled(&config)
    })
}

fn run() -> Result<Value> {
    ensure!(
        rustix::process::geteuid().is_root(),
        "Root access is required."
    );
    settings::directory(Path::new(RUNTIME))?;
    let mut arguments = std::env::args().skip(1);
    let method = arguments.next().context("An operation is required.")?;
    let raw = arguments.next().unwrap_or_else(|| "{}".into());
    ensure!(
        raw.len() <= 32_768 && arguments.next().is_none(),
        "Invalid request size."
    );
    let args: Value = serde_json::from_str(&raw)?;
    match method.as_str() {
        "get_state" => state(),
        "set_config" => set_config(&args),
        "select_tunnel" => select(&args),
        "set_tunnel_enabled" => set_tunnel_enabled(&args),
        "get_history" => history::query(
            &Settings::read()?,
            &snapshot(),
            &router::inventory()?,
            &args,
        ),
        "check_update" | "install_update" => {
            atomic(
                Path::new(&format!("{RUNTIME}/update-request")),
                method.as_bytes(),
            )?;
            Ok(json!({"message":"The request is queued. Its status appears within one minute."}))
        }
        "tick" => {
            tick()?;
            Ok(json!({"applied":true}))
        }
        "restart" => {
            locked("reconcile", || {
                router::bus("service", "delete", &json!({"name":"wayline"}))?;
                reconcile_inner()
            })?;
            Ok(json!({"applied":true}))
        }
        "boot" | "start" | "reload" | "reconcile" => {
            reconcile()?;
            Ok(json!({"applied":true}))
        }
        "install" | "enable" => {
            lifecycle(true)?;
            Ok(json!({"applied":true}))
        }
        "remove" | "disable" => {
            lifecycle(false)?;
            Ok(json!({"applied":true}))
        }
        "stop" => {
            router::bus("service", "delete", &json!({"name":"wayline"}))?;
            Ok(json!({"applied":true}))
        }
        _ => anyhow::bail!("Unknown control operation."),
    }
}

fn main() {
    let result = match run() {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    println!("{result}");
    if result["ok"] != true {
        std::process::exit(1);
    }
}
