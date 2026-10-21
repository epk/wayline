#![forbid(unsafe_code)]

mod api;
mod auto;
mod changes;
mod doctor;
mod egress;
mod history;
mod native;
mod resolve;
mod router;
mod runtime;
mod service;
mod settings;
mod update;

use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::path::Path;

fn parse<T: DeserializeOwned>(raw: &str) -> Result<T> {
    serde_json::from_str(raw).context("Invalid request.")
}

fn reply(value: impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

fn applied(result: Result<()>) -> Result<Value> {
    result.map(|()| json!({"applied":true}))
}

fn privileged() -> Result<()> {
    ensure!(
        rustix::process::geteuid().is_root(),
        "Root access is required."
    );
    runtime::directory(Path::new(runtime::RUNTIME))
}

fn run() -> Result<Value> {
    privileged()?;
    let mut arguments = std::env::args().skip(1);
    let method = arguments.next().context("An operation is required.")?;
    let raw = arguments.next().unwrap_or_else(|| "{}".into());
    ensure!(
        raw.len() <= 32_768 && arguments.next().is_none(),
        "Invalid request size."
    );
    match method.as_str() {
        "get_state" => reply(api::state()?),
        "get_history" => reply(api::history(&parse(&raw)?)?),
        "set_config" => api::set_config(parse(&raw)?),
        "select_tunnel" => api::select(&parse(&raw)?),
        "set_tunnel_enabled" => api::set_tunnel_enabled(&parse(&raw)?),
        "check_update" | "install_update" => api::queue_update(&method),
        "tick" => applied(service::tick()),
        "restart" => applied(service::restart()),
        "boot" | "start" | "reload" | "reconcile" => applied(service::reconcile()),
        "install" | "enable" => applied(service::lifecycle(true)),
        "remove" | "disable" => applied(service::lifecycle(false)),
        "stop" => applied(service::stop()),
        "running" | "status" => applied(service::running()),
        "enabled" => applied(service::enabled()),
        _ => anyhow::bail!("Unknown operation {method}. Run wayline-ctl help."),
    }
}

const HELP: &str = "Usage: wayline-ctl <command>

Commands for people:
  help             Show this help.
  doctor           Check the installation and print a report to share.
  update           Install the newest signed release, including alpha and beta releases.
  update --check   Report whether a newer signed release exists.
  egress           Show the public egress IP through each tunnel and the WAN uplink.

The admin page, the scheduler and the service manager use the remaining
operations, which print JSON: get_state, get_history, set_config,
select_tunnel, set_tunnel_enabled, check_update, install_update,
tick, boot, start, stop, restart, reload, running, enabled, install, remove.";

fn human(command: &str, option: Option<&str>) -> Option<Result<bool>> {
    match (command, option) {
        ("help" | "--help" | "-h", None) => {
            println!("{HELP}");
            Some(Ok(true))
        }
        ("doctor", None) => Some(Ok(doctor::run())),
        ("update", None | Some("--check")) => Some(
            privileged()
                .and_then(|()| update::run_now(option.is_none()))
                .map(|status| {
                    println!("{}", status.message());
                    true
                }),
        ),
        ("egress", None) => Some(privileged().and_then(|()| router::inventory()).map(
            |inventory| {
                println!("{}", egress::render(&egress::probe(&inventory)));
                true
            },
        )),
        _ => None,
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next().unwrap_or_default();
    let option = arguments.next();
    if command.is_empty() {
        eprintln!("{HELP}");
        std::process::exit(2);
    }
    if let Some(outcome) = human(&command, option.as_deref()) {
        match outcome {
            Ok(healthy) => std::process::exit(i32::from(!healthy)),
            Err(error) => {
                eprintln!("wayline-ctl: {error:#}");
                std::process::exit(1);
            }
        }
    }
    let result = match run() {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    println!("{result}");
    if result["ok"] != true {
        std::process::exit(1);
    }
}
