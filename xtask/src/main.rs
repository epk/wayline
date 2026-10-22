#![forbid(unsafe_code)]

mod lab;
mod metadata;
mod native;
mod package;
mod quality;

use anyhow::{Context, Result};
use std::{fs, path::Path, process::Child};
use xshell::{Shell, cmd};

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn build() -> Result<()> {
    let sh = Shell::new()?;
    cmd!(sh, "cargo build --release --locked --offline --target aarch64-unknown-linux-musl --package waylined --package wayline-control --package xtask").run()?;
    cmd!(sh, "pnpm --dir ui build").run()?;
    Ok(())
}

fn deps() -> Result<()> {
    let sh = Shell::new()?;
    cmd!(sh, "cargo fetch --locked").run()?;
    cmd!(
        sh,
        "pnpm --dir ui install --frozen-lockfile --ignore-scripts"
    )
    .run()?;
    Ok(())
}

fn preview() -> Result<()> {
    let server = tiny_http::Server::http("127.0.0.1:8787")
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    println!("Open http://127.0.0.1:8787/preview/index.html");
    for request in server.incoming_requests() {
        let url = request.url().split('?').next().unwrap_or("/");
        if url.contains("..") || url.contains('%') {
            request.respond(tiny_http::Response::empty(400))?;
            continue;
        }
        let path = Path::new("ui").join(url.trim_start_matches('/'));
        if !path.is_file() {
            request.respond(tiny_http::Response::empty(404))?;
            continue;
        }
        let data = fs::read(&path)?;
        let mime = match path.extension().and_then(|value| value.to_str()) {
            Some("js") => "text/javascript",
            Some("css") => "text/css",
            _ => "text/html",
        };
        let header = tiny_http::Header::from_bytes("Content-Type", mime)
            .map_err(|()| anyhow::anyhow!("Invalid response header."))?;
        request.respond(tiny_http::Response::from_data(data).with_header(header))?;
    }
    Ok(())
}

fn main() -> Result<()> {
    if std::env::args().next().is_some_and(|name| {
        Path::new(&name)
            .file_name()
            .is_some_and(|file| file == "vpn-client")
    }) {
        return native::reload_probe();
    }
    if std::env::args().next().is_some_and(|name| {
        Path::new(&name)
            .file_name()
            .is_some_and(|file| file == "ubus")
    }) {
        return native::bus_probe();
    }
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("deps") => deps(),
        Some("check") => quality::check(),
        Some("lint") => quality::lint(),
        Some("test") => quality::test(),
        Some("ci-metadata") => metadata::ci(),
        Some("format") => quality::format(),
        Some("build") => build(),
        Some("package") => package::build(&release_version(args.next())?),
        Some("release") => package::release(&release_version(args.next())?),
        Some("preview") => preview(),
        Some("lab") => lab::start(),
        Some("lab-native") => {
            native::prepare(Path::new(
                &args
                    .next()
                    .context("An extracted firmware root is required.")?,
            ))?;
            lab::start()
        }
        Some("lab-inside") => lab::inside(),
        Some("lab-stop") => lab::stop(),
        _ => anyhow::bail!(
            "Use deps, check, lint, test, ci-metadata, format, build, package, release, preview, lab, or lab-stop."
        ),
    }
}

fn release_version(argument: Option<String>) -> Result<String> {
    let version = argument.unwrap_or(metadata::version()?.to_string());
    let version = version.strip_prefix('v').unwrap_or(&version).to_owned();
    semver::Version::parse(&version)?;
    Ok(version)
}
