use crate::{
    runtime::{RUNTIME, locked, now, version},
    settings::Settings,
};
use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use wayline_support::{atomic, digest, read, text};

const RELEASES: &str = "https://api.github.com/repos/epk/wayline/releases?per_page=30";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    name: String,
    architecture: String,
    bytes: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u8,
    repository: String,
    package: String,
    version: String,
    #[expect(
        dead_code,
        reason = "The signed manifest lists tested modules for readers."
    )]
    firmware_contracts: Vec<String>,
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Status {
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    installing: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    failed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    needs_attention: bool,
    #[serde(default)]
    checked_at: u64,
}

impl Status {
    pub fn read() -> Option<Self> {
        serde_json::from_slice(&fs::read(Self::path()).ok()?).ok()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    fn path() -> PathBuf {
        PathBuf::from(format!("{RUNTIME}/update.json"))
    }

    fn write(&self) -> Result<()> {
        atomic(&Self::path(), &serde_json::to_vec(self)?)
    }

    fn checked(message: &str, available: bool, version: String) -> Self {
        Self {
            message: message.into(),
            available: Some(available),
            version: Some(version),
            checked_at: now(),
            ..Self::default()
        }
    }
}

fn download(url: &str, path: &Path, limit: u64) -> Result<()> {
    ensure!(
        url == RELEASES || url.starts_with("https://github.com/epk/wayline/releases/download/v"),
        "Invalid release URL."
    );
    let status = text(
        "curl",
        &[
            "--silent",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "8",
            "--max-time",
            "30",
            "--max-filesize",
            &limit.to_string(),
            "--output",
            path.to_str().context("Invalid download path.")?,
            "--write-out",
            "%{http_code}",
            url,
        ],
        1024,
        35,
    )?;
    ensure!(
        status.trim() == "200",
        "The release server rejected the request."
    );
    ensure!(
        fs::metadata(path)?.len() > 0 && fs::metadata(path)?.len() <= limit,
        "The release download exceeds its limit."
    );
    Ok(())
}

fn newest(releases: &[Release]) -> Option<Version> {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter_map(|release| release.tag_name.strip_prefix('v')?.parse::<Version>().ok())
        .filter(|version| version.build.is_empty())
        .max()
}

fn newer(folder: &Path) -> Result<Option<Version>> {
    let path = folder.join("releases.json");
    download(RELEASES, &path, 262_144)?;
    let releases: Vec<Release> = serde_json::from_slice(&read(&path, 262_144)?)?;
    let installed = Version::parse(&version())?;
    Ok(newest(&releases).filter(|latest| *latest > installed))
}

fn metadata(folder: &Path, latest: &Version) -> Result<(Manifest, Artifact, String)> {
    let tag = latest.to_string();
    let prefix = format!("https://github.com/epk/wayline/releases/download/v{tag}/");
    let manifest = folder.join("manifest.json");
    let signature = folder.join("manifest.json.sig");
    download(&format!("{prefix}manifest.json"), &manifest, 16_384)?;
    download(&format!("{prefix}manifest.json.sig"), &signature, 1024)?;
    text(
        "usign",
        &[
            "-V",
            "-p",
            "/etc/wayline/release.pub",
            "-m",
            manifest.to_str().context("Invalid manifest path.")?,
            "-x",
            signature.to_str().context("Invalid signature path.")?,
        ],
        1024,
        5,
    )?;
    let mut data: Manifest = serde_json::from_slice(&read(&manifest, 16_384)?)?;
    ensure!(
        data.schema == 1
            && data.repository == "epk/wayline"
            && data.package == "wayline"
            && data.version == tag,
        "The signed release identity does not match."
    );
    ensure!(
        data.artifacts.len() == 1,
        "The release has an unexpected artifact set."
    );
    let artifact = data
        .artifacts
        .pop()
        .context("The release has no artifact.")?;
    ensure!(
        artifact.architecture == "aarch64_cortex-a53"
            && artifact.name == format!("wayline_{}_aarch64_cortex-a53.ipk", data.version)
            && artifact.bytes > 0
            && artifact.bytes <= 4_194_304
            && artifact.sha256.len() == 64
            && artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "The release artifact is invalid."
    );
    let architectures = text("opkg", &["print-architecture"], 8192, 5)?;
    ensure!(
        architectures
            .lines()
            .any(|line| line.split_whitespace().nth(1) == Some(artifact.architecture.as_str())),
        "The release architecture is unsupported."
    );
    Ok((data, artifact, prefix))
}

fn member(reader: impl Read, name: &str, limit: u64) -> Result<Vec<u8>> {
    for entry in tar::Archive::new(flate2::read::GzDecoder::new(reader)).entries()? {
        let entry = entry?;
        if entry.path()?.strip_prefix(".").unwrap_or(&entry.path()?) == Path::new(name) {
            let mut bytes = Vec::new();
            entry.take(limit + 1).read_to_end(&mut bytes)?;
            ensure!(
                u64::try_from(bytes.len())? <= limit,
                "Package control data exceeds its limit."
            );
            return Ok(bytes);
        }
    }
    anyhow::bail!("Package control data is absent.")
}

fn package_identity(path: &Path, expected: &str) -> Result<()> {
    let control = member(fs::File::open(path)?, "control.tar.gz", 65_536)?;
    let data = String::from_utf8(member(control.as_slice(), "control", 16_384)?)?;
    let mut fields = std::collections::BTreeMap::new();
    for line in data.lines() {
        if let Some((key, value)) = line.split_once(": ") {
            ensure!(
                fields.insert(key, value).is_none(),
                "Duplicate package control field."
            );
        }
    }
    ensure!(
        fields
            .get("Package")
            .is_some_and(|value| *value == "wayline")
            && fields
                .get("Version")
                .is_some_and(|value| *value == expected)
            && fields
                .get("Architecture")
                .is_some_and(|value| *value == "aarch64_cortex-a53"),
        "The package identity does not match its signed manifest."
    );
    Ok(())
}

fn execute(install: bool) -> Result<Status> {
    let folder = tempfile::tempdir_in(RUNTIME)?;
    let Some(latest) = newer(folder.path())? else {
        return Ok(Status::checked("Wayline is current.", false, version()));
    };
    let (manifest, artifact, prefix) = metadata(folder.path(), &latest)?;
    if !install {
        return Ok(Status::checked(
            "A signed update is available.",
            true,
            manifest.version,
        ));
    }
    space(folder.path(), artifact.bytes.saturating_mul(2) + 1_048_576)?;
    space(Path::new("/etc/wayline"), 4_194_304)?;
    let package = folder.path().join("package.ipk");
    download(
        &format!("{prefix}{}", artifact.name),
        &package,
        artifact.bytes,
    )?;
    let bytes = read(&package, 4_194_304)?;
    ensure!(
        u64::try_from(bytes.len())? == artifact.bytes && digest(&bytes) == artifact.sha256,
        "The release size or digest does not match."
    );
    package_identity(&package, &manifest.version)?;
    let path = package.to_str().context("Invalid package path.")?;
    text("opkg", &["--noaction", "install", path], 16_384, 30)?;
    Status {
        message: "Installing the signed update.".into(),
        installing: true,
        checked_at: now(),
        ..Status::default()
    }
    .write()?;
    text("opkg", &["install", path], 32_768, 120)?;
    ensure!(
        version() == manifest.version,
        "The installed version needs checking. Do not retry automatically."
    );
    Ok(Status::checked("Wayline updated.", false, manifest.version))
}

fn attempt(install: bool) -> Status {
    execute(install).unwrap_or_else(|error| Status {
        message: format!("Update stopped. {error}"),
        failed: true,
        needs_attention: Status::read().is_some_and(|status| status.installing),
        checked_at: now(),
        ..Status::default()
    })
}

fn settled(previous: Option<&Status>) -> bool {
    previous.is_none_or(|status| !status.installing && !status.needs_attention)
}

fn space(path: &Path, minimum: u64) -> Result<()> {
    let capacity = rustix::fs::statvfs(path)?;
    ensure!(
        capacity.f_bavail.saturating_mul(capacity.f_frsize) >= minimum,
        "Insufficient free space for a safe update."
    );
    Ok(())
}

pub fn run_now(install: bool) -> Result<Status> {
    locked("update", || {
        let previous = Status::read();
        ensure!(
            settled(previous.as_ref()),
            "A previous installation needs checking."
        );
        let status = attempt(install);
        status.write()?;
        Ok(status)
    })
}

pub fn scheduled(config: &Settings) -> Result<()> {
    locked("update", || {
        let request = PathBuf::from(format!("{RUNTIME}/update-request"));
        let requested = fs::read_to_string(&request).unwrap_or_default();
        let previous = Status::read();
        ensure!(
            settled(previous.as_ref()),
            "A previous installation needs checking."
        );
        if requested.is_empty()
            && (!config.auto_updates
                || now().saturating_sub(previous.map_or(0, |status| status.checked_at)) < 86_400)
        {
            return Ok(());
        }
        let status =
            attempt(requested == "install_update" || (requested.is_empty() && config.auto_updates));
        status.write()?;
        if request.exists() {
            fs::remove_file(request)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{Release, newest};
    use anyhow::{Result, ensure};
    use semver::Version;

    fn release(tag_name: &str, draft: bool) -> Release {
        Release {
            tag_name: tag_name.into(),
            draft,
        }
    }

    #[test]
    fn skips_drafts() -> Result<()> {
        let releases = [release("v0.2.0", true), release("v0.1.0-alpha.4", false)];
        ensure!(newest(&releases) == Some(Version::parse("0.1.0-alpha.4")?));
        Ok(())
    }

    #[test]
    fn chooses_prereleases() -> Result<()> {
        let releases = [
            release("v0.1.0-alpha.3", false),
            release("v0.1.0-beta.1", false),
        ];
        ensure!(newest(&releases) == Some(Version::parse("0.1.0-beta.1")?));
        Ok(())
    }

    #[test]
    fn stable_beats_prereleases() -> Result<()> {
        let releases = [
            release("v0.1.0-rc.1", false),
            release("v0.1.0", false),
            release("v0.1.0-beta.1", false),
        ];
        ensure!(newest(&releases) == Some(Version::parse("0.1.0")?));
        Ok(())
    }

    #[test]
    fn skips_unparsable_tags() -> Result<()> {
        let releases = [
            release("vnext", false),
            release("0.3.0", false),
            release("v0.3.0+build.1", false),
            release("v0.1.0-alpha.4", false),
        ];
        ensure!(newest(&releases) == Some(Version::parse("0.1.0-alpha.4")?));
        Ok(())
    }

    #[test]
    fn empty_list_has_no_release() -> Result<()> {
        ensure!(newest(&[]).is_none());
        Ok(())
    }
}
