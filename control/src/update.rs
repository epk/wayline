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
    process::Stdio,
};
use wayline_support::{atomic, digest, read, text};

const RELEASES: &str = "https://api.github.com/repos/epk/wayline/releases?per_page=30";
const STALE_AFTER: u64 = 240;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Verifying,
    Installing,
    Done,
    Failed,
}

impl Phase {
    const fn working(self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading | Self::Verifying | Self::Installing
        )
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    current: String,
    latest: Option<String>,
    available: bool,
    phase: Phase,
    message: String,
    failed: bool,
    needs_attention: bool,
    checked_at: u64,
    updated_at: u64,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Stored {
    current: Option<String>,
    #[serde(alias = "version")]
    latest: Option<String>,
    available: Option<bool>,
    phase: Option<Phase>,
    installing: bool,
    message: String,
    failed: bool,
    #[serde(alias = "needs_attention")]
    needs_attention: bool,
    #[serde(alias = "checked_at")]
    checked_at: u64,
    updated_at: u64,
}

impl From<Stored> for Status {
    fn from(stored: Stored) -> Self {
        let phase = stored.phase.unwrap_or(if stored.installing {
            Phase::Installing
        } else if stored.failed {
            Phase::Failed
        } else {
            Phase::Done
        });
        Self {
            current: stored.current.unwrap_or_else(version),
            latest: stored.latest,
            available: stored.available.unwrap_or(false),
            phase,
            message: stored.message,
            failed: stored.failed,
            needs_attention: stored.needs_attention,
            checked_at: stored.checked_at,
            updated_at: stored.updated_at,
        }
    }
}

impl Status {
    pub fn read() -> Option<Self> {
        Self::parse(&fs::read(Self::path()).ok()?)
    }

    fn parse(raw: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Stored>(raw).ok().map(Self::from)
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

    fn working(previous: Option<&Self>, phase: Phase, message: &str) -> Self {
        Self {
            current: version(),
            latest: previous.and_then(|status| status.latest.clone()),
            available: previous.is_some_and(|status| status.available),
            phase,
            message: message.into(),
            failed: false,
            needs_attention: previous.is_some_and(|status| status.needs_attention),
            checked_at: previous.map_or(0, |status| status.checked_at),
            updated_at: now(),
        }
    }

    fn finished(message: String, latest: String, available: bool) -> Self {
        let at = now();
        Self {
            current: version(),
            latest: Some(latest),
            available,
            phase: Phase::Done,
            message,
            failed: false,
            needs_attention: false,
            checked_at: at,
            updated_at: at,
        }
    }

    fn stopped(error: &anyhow::Error) -> Self {
        let current = Self::read();
        let at = now();
        Self {
            current: version(),
            latest: current.as_ref().and_then(|status| status.latest.clone()),
            available: current.as_ref().is_some_and(|status| status.available),
            phase: Phase::Failed,
            message: format!("Update stopped. {error}"),
            failed: true,
            needs_attention: current
                .as_ref()
                .is_some_and(|status| status.needs_attention || status.phase == Phase::Installing),
            checked_at: at,
            updated_at: at,
        }
    }
}

fn effective(mut status: Status, at: u64) -> Status {
    if status.phase.working() && at.saturating_sub(status.updated_at) > STALE_AFTER {
        status.needs_attention |= status.phase == Phase::Installing;
        status.phase = Phase::Failed;
        status.failed = true;
        status.message =
            "The update did not finish. Run wayline-ctl update on the router for details.".into();
    }
    status
}

pub fn view() -> Status {
    effective(
        Status::read().unwrap_or_else(|| Status {
            current: version(),
            ..Status::default()
        }),
        now(),
    )
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

fn execute(install: bool, report: &dyn Fn(Phase, &str) -> Result<()>) -> Result<Status> {
    report(Phase::Checking, "Checking for a newer release…")?;
    let folder = tempfile::tempdir_in(RUNTIME)?;
    let Some(latest) = newer(folder.path())? else {
        return Ok(Status::finished(
            "Wayline is up to date.".into(),
            version(),
            false,
        ));
    };
    if install {
        report(Phase::Downloading, "Downloading the signed update.")?;
    }
    let (manifest, artifact, prefix) = metadata(folder.path(), &latest)?;
    if !install {
        let message = format!("Version {} is available.", manifest.version);
        return Ok(Status::finished(message, manifest.version, true));
    }
    space(folder.path(), artifact.bytes.saturating_mul(2) + 1_048_576)?;
    space(Path::new("/etc/wayline"), 4_194_304)?;
    let package = folder.path().join("package.ipk");
    download(
        &format!("{prefix}{}", artifact.name),
        &package,
        artifact.bytes,
    )?;
    report(Phase::Verifying, "Verifying the signed package.")?;
    let bytes = read(&package, 4_194_304)?;
    ensure!(
        u64::try_from(bytes.len())? == artifact.bytes && digest(&bytes) == artifact.sha256,
        "The release size or digest does not match."
    );
    package_identity(&package, &manifest.version)?;
    let path = package.to_str().context("Invalid package path.")?;
    text("opkg", &["--noaction", "install", path], 16_384, 30)?;
    report(Phase::Installing, "Installing the signed update.")?;
    text("opkg", &["install", path], 32_768, 120)?;
    ensure!(
        version() == manifest.version,
        "The installed version needs checking. Do not retry automatically."
    );
    let message = format!("Updated to {}.", manifest.version);
    Ok(Status::finished(message, manifest.version, false))
}

fn attempt(install: bool) -> Result<Status> {
    let previous = Status::read();
    let report =
        |phase: Phase, message: &str| Status::working(previous.as_ref(), phase, message).write();
    let status = if settled(previous.as_ref()) {
        execute(install, &report).unwrap_or_else(|error| Status::stopped(&error))
    } else {
        Status::stopped(&anyhow::anyhow!("A previous installation needs checking."))
    };
    status.write()?;
    Ok(status)
}

fn settled(previous: Option<&Status>) -> bool {
    previous.is_none_or(|status| status.phase != Phase::Installing && !status.needs_attention)
}

fn space(path: &Path, minimum: u64) -> Result<()> {
    let capacity = rustix::fs::statvfs(path)?;
    ensure!(
        capacity.f_bavail.saturating_mul(capacity.f_frsize) >= minimum,
        "Insufficient free space for a safe update."
    );
    Ok(())
}

fn request_path() -> PathBuf {
    PathBuf::from(format!("{RUNTIME}/update-request"))
}

fn idle() -> bool {
    locked("update", || Ok(())).is_ok()
}

pub fn start(method: &str) -> Result<bool> {
    if !idle() {
        return Ok(false);
    }
    atomic(&request_path(), method.as_bytes())?;
    std::process::Command::new(std::env::current_exe()?)
        .arg("update-job")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(true)
}

pub fn job() -> Result<()> {
    locked("update", || {
        let request = request_path();
        let install = fs::read_to_string(&request).unwrap_or_default() == "install_update";
        let _ = fs::remove_file(&request);
        attempt(install).map(drop)
    })
}

pub fn run_now(install: bool) -> Result<Status> {
    locked("update", || {
        let previous = Status::read();
        ensure!(
            settled(previous.as_ref()),
            "A previous installation needs checking."
        );
        attempt(install)
    })
}

pub fn scheduled(config: &Settings) -> Result<()> {
    if !idle() {
        return Ok(());
    }
    locked("update", || {
        let request = request_path();
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
        attempt(requested == "install_update" || (requested.is_empty() && config.auto_updates))?;
        if request.exists() {
            fs::remove_file(request)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{Phase, Release, Status, effective, newest};
    use anyhow::{Context, Result, ensure};
    use semver::Version;

    fn release(tag_name: &str, draft: bool) -> Release {
        Release {
            tag_name: tag_name.into(),
            draft,
        }
    }

    fn parse(raw: &str) -> Result<Status> {
        Status::parse(raw.as_bytes()).context("The status is unreadable.")
    }

    fn at(phase: Phase, updated_at: u64) -> Status {
        Status {
            phase,
            updated_at,
            ..Status::default()
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

    #[test]
    fn migrates_available_status() -> Result<()> {
        let status = parse(
            r#"{"message":"A signed update is available.","available":true,"version":"0.2.0","checked_at":5}"#,
        )?;
        ensure!(status.latest.as_deref() == Some("0.2.0"));
        ensure!(status.available && status.phase == Phase::Done);
        ensure!(status.checked_at == 5 && !status.needs_attention && !status.failed);
        Ok(())
    }

    #[test]
    fn migrates_installing_status() -> Result<()> {
        let status = parse(
            r#"{"message":"Installing the signed update.","installing":true,"checked_at":7}"#,
        )?;
        ensure!(status.phase == Phase::Installing);
        ensure!(!status.needs_attention && !status.available);
        Ok(())
    }

    #[test]
    fn migrates_needs_attention_status() -> Result<()> {
        let status = parse(
            r#"{"message":"Update stopped. Refused.","failed":true,"needs_attention":true}"#,
        )?;
        ensure!(status.phase == Phase::Failed && status.failed && status.needs_attention);
        ensure!(!super::settled(Some(&status)));
        Ok(())
    }

    #[test]
    fn writes_camel_case_phases() -> Result<()> {
        let value = serde_json::to_value(at(Phase::Downloading, 9))?;
        ensure!(value["phase"] == "downloading");
        ensure!(value["updatedAt"] == 9 && value["checkedAt"] == 0);
        ensure!(value["latest"].is_null() && value["needsAttention"] == false);
        Ok(())
    }

    #[test]
    fn fresh_work_is_kept() -> Result<()> {
        let status = effective(at(Phase::Downloading, 100), 200);
        ensure!(status.phase == Phase::Downloading && !status.failed);
        Ok(())
    }

    #[test]
    fn stale_work_fails() -> Result<()> {
        ensure!(effective(at(Phase::Checking, 100), 340).phase == Phase::Checking);
        let status = effective(at(Phase::Checking, 100), 341);
        ensure!(status.phase == Phase::Failed && status.failed);
        ensure!(
            status.message
                == "The update did not finish. Run wayline-ctl update on the router for details."
        );
        ensure!(!status.needs_attention);
        Ok(())
    }

    #[test]
    fn stale_install_needs_attention() -> Result<()> {
        let status = effective(at(Phase::Installing, 0), 1000);
        ensure!(status.phase == Phase::Failed && status.needs_attention);
        Ok(())
    }

    #[test]
    fn finished_status_is_not_stale() -> Result<()> {
        ensure!(effective(at(Phase::Done, 0), 10_000).phase == Phase::Done);
        Ok(())
    }
}
