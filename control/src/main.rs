#![forbid(unsafe_code)]

mod api;
mod auto;
mod doctor;
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

fn run() -> Result<Value> {
    ensure!(
        rustix::process::geteuid().is_root(),
        "Root access is required."
    );
    runtime::directory(Path::new(runtime::RUNTIME))?;
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
        _ => anyhow::bail!("Unknown control operation."),
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("doctor") {
        std::process::exit(i32::from(!doctor::run()));
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
