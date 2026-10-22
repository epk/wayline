use anyhow::{Context, Result, ensure};
use cargo_metadata::{Metadata, MetadataCommand};
use std::{fs, io::Write};
use xshell::{Shell, cmd};

pub fn workspace() -> Result<Metadata> {
    Ok(MetadataCommand::new()
        .other_options(vec!["--locked".into(), "--offline".into()])
        .exec()?)
}

pub fn version() -> Result<semver::Version> {
    manifest_version(&fs::read_to_string("monitor/Cargo.toml")?)
}

fn manifest_version(source: &str) -> Result<semver::Version> {
    let manifest: toml::Table = source.parse()?;
    let version = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .context("The monitor version is missing.")?;
    Ok(semver::Version::parse(version)?)
}

pub fn ci() -> Result<()> {
    let version = version()?;
    ensure!(
        version.build.is_empty(),
        "Release versions cannot contain build metadata."
    );
    let publish = if std::env::var("WAYLINE_PUBLISH").as_deref() == Ok("true") {
        let base = std::env::var("WAYLINE_BASE_SHA")?;
        ensure!(
            base.len() == 40 && base.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid base revision."
        );
        if base.bytes().all(|byte| byte == b'0') {
            true
        } else {
            let sh = Shell::new()?;
            let object = format!("{base}:monitor/Cargo.toml");
            let previous = cmd!(sh, "git show {object}").read()?;
            let previous = manifest_version(&previous)?;
            ensure!(version >= previous, "The package version cannot decrease.");
            version != previous
        }
    } else {
        false
    };
    let output = std::env::var("GITHUB_OUTPUT").context("The Actions output path is missing.")?;
    let mut output = fs::OpenOptions::new().append(true).open(output)?;
    writeln!(
        output,
        "version={version}\ntag=v{version}\nprerelease={}\npublish={publish}",
        !version.pre.is_empty()
    )?;
    Ok(())
}
