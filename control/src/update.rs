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
    prerelease: bool,
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
        url.starts_with("https://api.github.com/repos/epk/wayline/")
            || url.starts_with("https://github.com/epk/wayline/releases/download/v"),
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
    if status.trim() == "404" && url.ends_with("/releases/latest") {
        anyhow::bail!("No stable release is available.");
    }
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

fn metadata(folder: &Path) -> Result<(Manifest, Artifact, String)> {
    let release_path = folder.join("release.json");
    download(
        "https://api.github.com/repos/epk/wayline/releases/latest",
        &release_path,
        65_536,
    )?;
    let release: Release = serde_json::from_slice(&read(&release_path, 65_536)?)?;
    let tag = release
        .tag_name
        .strip_prefix('v')
        .context("Invalid release tag.")?;
    let parsed = Version::parse(tag)?;
    ensure!(
        !release.draft && !release.prerelease && parsed.pre.is_empty() && parsed.build.is_empty(),
        "The release is not a stable version."
    );
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
    let (manifest, artifact, prefix) = metadata(folder.path())?;
    if Version::parse(&manifest.version)? <= Version::parse(&version())? {
        return Ok(Status::checked("Wayline is current.", false, version()));
    }
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

fn space(path: &Path, minimum: u64) -> Result<()> {
    let capacity = rustix::fs::statvfs(path)?;
    ensure!(
        capacity.f_bavail.saturating_mul(capacity.f_frsize) >= minimum,
        "Insufficient free space for a safe update."
    );
    Ok(())
}

pub fn scheduled(config: &Settings) -> Result<()> {
    locked("update", || {
        let request = PathBuf::from(format!("{RUNTIME}/update-request"));
        let requested = fs::read_to_string(&request).unwrap_or_default();
        let previous = Status::read();
        ensure!(
            previous
                .as_ref()
                .is_none_or(|status| !status.installing && !status.needs_attention),
            "A previous installation needs checking."
        );
        if requested.is_empty()
            && (!config.auto_updates
                || now().saturating_sub(previous.map_or(0, |status| status.checked_at)) < 86_400)
        {
            return Ok(());
        }
        let status =
            execute(requested == "install_update" || (requested.is_empty() && config.auto_updates))
                .unwrap_or_else(|error| Status {
                    message: format!("Update stopped. {error}"),
                    failed: true,
                    needs_attention: Status::read().is_some_and(|status| status.installing),
                    checked_at: now(),
                    ..Status::default()
                });
        status.write()?;
        if request.exists() {
            fs::remove_file(request)?;
        }
        Ok(())
    })
}
