use anyhow::{Context, Result, ensure};
use cargo_metadata::{Metadata, MetadataCommand};
use std::fs;
use std::io::Write;
use xshell::{Shell, cmd};

pub fn workspace() -> Result<Metadata> {
    Ok(MetadataCommand::new()
        .other_options(vec!["--locked".into(), "--offline".into()])
        .exec()?)
}

pub fn version() -> Result<semver::Version> {
    manifest_version(&fs::read_to_string("monitor/Cargo.toml")?)
}

pub fn manifest_version(source: &str) -> Result<semver::Version> {
    let manifest: toml::Table = source.parse()?;
    let version = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .context("The package version is missing.")?;
    Ok(semver::Version::parse(version)?)
}

fn revision(value: &str) -> Result<&str> {
    ensure!(
        value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid revision {value}."
    );
    Ok(value)
}

fn remote_revision(listing: &str) -> Option<&str> {
    listing
        .lines()
        .find_map(|line| line.split_whitespace().next())
}

struct Remote {
    tag_exists: bool,
    head: Option<String>,
}

fn remote(sh: &Shell, tag: &str) -> Result<Remote> {
    let tag_ref = format!("refs/tags/{tag}");
    let tags = cmd!(sh, "git ls-remote --tags origin {tag_ref}").read()?;
    let heads = cmd!(sh, "git ls-remote --heads origin refs/heads/main").read()?;
    Ok(Remote {
        tag_exists: remote_revision(&tags).is_some(),
        head: remote_revision(&heads).map(str::to_owned),
    })
}

fn due(
    version: &semver::Version,
    previous: Option<&semver::Version>,
    remote: &Remote,
    commit: &str,
) -> Result<bool> {
    if let Some(previous) = previous {
        ensure!(version >= previous, "The package version cannot decrease.");
    }
    if remote.tag_exists {
        println!("v{version} is already tagged, so this run publishes nothing.");
        return Ok(false);
    }
    if remote.head.as_deref() != Some(commit) {
        println!("main has moved past {commit}, so a newer run publishes v{version}.");
        return Ok(false);
    }
    println!("v{version} has no release yet, so this run publishes it.");
    Ok(true)
}

fn publish(version: &semver::Version) -> Result<bool> {
    if std::env::var("WAYLINE_PUBLISH").as_deref() != Ok("true") {
        return Ok(false);
    }
    let sh = Shell::new()?;
    let base = std::env::var("WAYLINE_BASE_SHA").unwrap_or_default();
    let previous = if base.is_empty() || base.bytes().all(|byte| byte == b'0') {
        None
    } else {
        let object = format!("{}:monitor/Cargo.toml", revision(&base)?);
        Some(manifest_version(&cmd!(sh, "git show {object}").read()?)?)
    };
    let commit = std::env::var("GITHUB_SHA").context("The commit revision is missing.")?;
    due(
        version,
        previous.as_ref(),
        &remote(&sh, &format!("v{version}"))?,
        revision(&commit)?,
    )
}

pub fn ci() -> Result<()> {
    let version = version()?;
    ensure!(
        version.build.is_empty(),
        "Release versions cannot contain build metadata."
    );
    let publish = publish(&version)?;
    let output = std::env::var("GITHUB_OUTPUT").context("The Actions output path is missing.")?;
    let mut output = fs::OpenOptions::new().append(true).open(output)?;
    writeln!(
        output,
        "version={version}\ntag=v{version}\nprerelease={}\npublish={publish}",
        !version.pre.is_empty()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Remote, due, remote_revision};

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn state(tag_exists: bool, head: Option<&str>) -> Remote {
        Remote {
            tag_exists,
            head: head.map(str::to_owned),
        }
    }

    #[test]
    fn an_unreleased_version_publishes_after_a_failed_bump_run() -> anyhow::Result<()> {
        let version = semver::Version::parse("0.1.0-alpha.26")?;
        assert!(due(
            &version,
            Some(&version),
            &state(false, Some(COMMIT)),
            COMMIT
        )?);
        assert!(due(&version, None, &state(false, Some(COMMIT)), COMMIT)?);
        Ok(())
    }

    #[test]
    fn a_tagged_version_does_not_publish_again() -> anyhow::Result<()> {
        let version = semver::Version::parse("0.1.0-alpha.26")?;
        let previous = semver::Version::parse("0.1.0-alpha.25")?;
        assert!(!due(
            &version,
            Some(&previous),
            &state(true, Some(COMMIT)),
            COMMIT
        )?);
        Ok(())
    }

    #[test]
    fn a_superseded_commit_does_not_publish() -> anyhow::Result<()> {
        let version = semver::Version::parse("0.1.0-alpha.26")?;
        assert!(!due(&version, None, &state(false, Some(OTHER)), COMMIT)?);
        assert!(!due(&version, None, &state(false, None), COMMIT)?);
        Ok(())
    }

    #[test]
    fn the_version_cannot_decrease() -> anyhow::Result<()> {
        let version = semver::Version::parse("0.1.0-alpha.25")?;
        let previous = semver::Version::parse("0.1.0-alpha.26")?;
        assert!(
            due(
                &version,
                Some(&previous),
                &state(false, Some(COMMIT)),
                COMMIT
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn remote_listings_name_the_first_revision() {
        assert_eq!(
            remote_revision(&format!("{COMMIT}\trefs/tags/v0.1.0\n")),
            Some(COMMIT)
        );
        assert_eq!(remote_revision(""), None);
    }
}
