use crate::runtime;
use crate::runtime::RUNTIME;
use crate::settings::Settings;
use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RELEASES_LIMIT: usize = 2_097_152;
const RELEASES: &str = "https://api.github.com/repos/epk/wayline/releases?per_page=10";
const STALE_AFTER: u64 = 240;
const DONE_VISIBLE: u64 = 600;
const LATCH: &str = "/etc/wayline/update-attention";
const VERSION_FILE: &str = "/usr/share/wayline/version";
const BINARIES: [&str; 2] = ["/usr/sbin/wayline-ctl", "/usr/sbin/waylined"];
const BLOCKED: &str = "A previous installation needs checking. Confirm the router works, then run wayline-ctl update --clear or choose Clear on this page.";

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Opkg,
    Apk,
}

impl Format {
    fn detect() -> Self {
        if Path::new("/etc/apk/arch").is_file() {
            Self::Apk
        } else {
            Self::Opkg
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::Opkg => "ipk",
            Self::Apk => "apk",
        }
    }

    const fn database(self) -> &'static str {
        match self {
            Self::Opkg => "/usr/lib/opkg/status",
            Self::Apk => "/lib/apk/db/installed",
        }
    }

    const fn manifest(self) -> &'static str {
        match self {
            Self::Opkg => "manifest.json",
            Self::Apk => "manifest-apk.json",
        }
    }

    fn supports(self, architecture: &str) -> Result<bool> {
        Ok(match self {
            Self::Opkg => wayline_support::text("opkg", &["print-architecture"], 8192, 5)?
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(architecture)),
            Self::Apk => {
                wayline_support::read(Path::new("/etc/apk/arch"), 8192).is_ok_and(|data| {
                    String::from_utf8_lossy(&data)
                        .split_whitespace()
                        .any(|line| line == architecture)
                })
            }
        })
    }

    fn install(self, path: &str, simulate: bool) -> Result<()> {
        let (program, args): (&str, Vec<&str>) = match (self, simulate) {
            (Self::Opkg, true) => ("opkg", vec!["--noaction", "install", path]),
            (Self::Opkg, false) => ("opkg", vec!["install", path]),
            (Self::Apk, true) => ("apk", vec!["add", "--allow-untrusted", "--simulate", path]),
            (Self::Apk, false) => ("apk", vec!["add", "--allow-untrusted", path]),
        };
        let (limit, seconds) = if simulate {
            (16_384, 30)
        } else {
            (32_768, 120)
        };
        wayline_support::text(program, &args, limit, seconds)?;
        Ok(())
    }
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

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
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

impl Status {
    pub fn read() -> Option<Self> {
        runtime::load(Self::path())
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    fn path() -> PathBuf {
        Path::new(RUNTIME).join("update.json")
    }

    fn write(&self) -> Result<()> {
        runtime::store(Self::path(), self)
    }

    fn working(previous: Option<&Self>, phase: Phase, message: &str) -> Self {
        Self {
            current: runtime::version(),
            latest: previous.and_then(|status| status.latest.clone()),
            available: previous.is_some_and(|status| status.available),
            phase,
            message: message.into(),
            failed: false,
            needs_attention: previous.is_some_and(|status| status.needs_attention),
            checked_at: previous.map_or(0, |status| status.checked_at),
            updated_at: runtime::now(),
        }
    }

    fn finished(message: String, latest: String, available: bool, phase: Phase) -> Self {
        let at = runtime::now();
        Self {
            current: runtime::version(),
            latest: Some(latest),
            available,
            phase,
            message,
            failed: false,
            needs_attention: latched(),
            checked_at: at,
            updated_at: at,
        }
    }

    fn stopped(error: &anyhow::Error) -> Self {
        let previous = Self::read().unwrap_or_default();
        let at = runtime::now();
        Self {
            current: runtime::version(),
            latest: previous.latest,
            available: previous.available,
            phase: Phase::Failed,
            message: format!("Update stopped. {error}"),
            failed: true,
            needs_attention: latched(),
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
    if status.phase == Phase::Done && at.saturating_sub(status.updated_at) > DONE_VISIBLE {
        status.phase = Phase::Idle;
        status.message.clear();
    }
    status
}

pub fn view() -> Status {
    let mut status = effective(
        Status::read().unwrap_or_else(|| Status {
            current: runtime::version(),
            ..Status::default()
        }),
        runtime::now(),
    );
    status.needs_attention |= latched();
    status
}

fn latched_at(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

fn latched() -> bool {
    latched_at(Path::new(LATCH))
}

fn arm(path: &Path, version: &str) -> Result<()> {
    let note = format!("{version} {}\n", runtime::now());
    wayline_support::atomic(path, note.as_bytes()).context("Could not record the installation.")
}

fn disarm(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

fn apk_form(version: &str) -> Option<String> {
    let parsed = Version::parse(version).ok()?;
    let core = format!("{}.{}.{}", parsed.major, parsed.minor, parsed.patch);
    if parsed.pre.is_empty() {
        return Some(core);
    }
    let (label, number) = parsed
        .pre
        .as_str()
        .split_once('.')
        .unwrap_or((parsed.pre.as_str(), "0"));
    Some(format!("{core}_{label}{number}"))
}

fn recorded(database: &str, format: Format) -> Option<String> {
    database.split("\n\n").find_map(|stanza| {
        let field = |key: &str| {
            stanza
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(str::trim)
        };
        let found = match format {
            Format::Opkg => (field("Package:") == Some("wayline")
                && field("Status:").is_some_and(|status| status.ends_with(" installed")))
            .then(|| field("Version:"))
            .flatten(),
            Format::Apk => (field("P:") == Some("wayline"))
                .then(|| field("V:"))
                .flatten(),
        };
        found.map(str::to_owned)
    })
}

fn matches_record(installed: &str, record: &str, format: Format) -> bool {
    match format {
        Format::Opkg => record == installed,
        Format::Apk => apk_form(installed).is_some_and(|expected| {
            record.strip_prefix(&expected).is_some_and(|rest| {
                rest.is_empty()
                    || rest.strip_prefix("-r").is_some_and(|number| {
                        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                    })
            })
        }),
    }
}

fn verify_installed(format: Format) -> Result<String> {
    let installed = fs::read_to_string(VERSION_FILE)
        .context("The installed version file is missing. Reinstall Wayline.")?
        .trim()
        .to_owned();
    for binary in BINARIES {
        ensure!(
            Path::new(binary).is_file(),
            "{binary} is missing. Reinstall Wayline."
        );
    }
    let database = fs::read_to_string(format.database())
        .context("Could not read the package manager's records.")?;
    let record = recorded(&database, format)
        .context("The package manager does not list Wayline as installed. Reinstall Wayline.")?;
    ensure!(
        matches_record(&installed, &record, format),
        "Wayline {installed} does not match the package manager's record {record}. Reinstall Wayline."
    );
    Ok(installed)
}

pub fn clear() -> Result<Status> {
    runtime::locked("update", || {
        let installed = verify_installed(Format::detect())?;
        disarm(Path::new(LATCH))?;
        let previous = Status::read().unwrap_or_default();
        let status = Status {
            current: installed.clone(),
            latest: previous.latest,
            available: previous.available,
            phase: Phase::Idle,
            message: format!(
                "Wayline {installed} matches the package manager. Updates can run again."
            ),
            failed: false,
            needs_attention: false,
            checked_at: previous.checked_at,
            updated_at: runtime::now(),
        };
        status.write()?;
        Ok(status)
    })
}

fn download(url: &str, path: &Path, limit: u64) -> Result<()> {
    ensure!(
        url == RELEASES || url.starts_with("https://github.com/epk/wayline/releases/download/v"),
        "Invalid release URL."
    );
    let status = wayline_support::text(
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
    let size = fs::metadata(path)?.len();
    ensure!(
        size > 0 && size <= limit,
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
    download(RELEASES, &path, u64::try_from(RELEASES_LIMIT)?)?;
    let releases: Vec<Release> =
        serde_json::from_slice(&wayline_support::read(&path, RELEASES_LIMIT)?)?;
    let installed = Version::parse(&runtime::version())?;
    Ok(newest(&releases).filter(|latest| *latest > installed))
}

fn metadata(
    folder: &Path,
    latest: &Version,
    format: Format,
) -> Result<(Manifest, Artifact, String)> {
    let tag = latest.to_string();
    let prefix = format!("https://github.com/epk/wayline/releases/download/v{tag}/");
    let name = format.manifest();
    let manifest = folder.join(name);
    let signature = folder.join(format!("{name}.sig"));
    download(&format!("{prefix}{name}"), &manifest, 16_384)?;
    download(&format!("{prefix}{name}.sig"), &signature, 1024)?;
    wayline_support::text(
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
    let mut data: Manifest = serde_json::from_slice(&wayline_support::read(&manifest, 16_384)?)?;
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
            && artifact.name
                == format!(
                    "wayline_{}_aarch64_cortex-a53.{}",
                    data.version,
                    format.extension()
                )
            && artifact.bytes > 0
            && artifact.bytes <= 4_194_304
            && artifact.sha256.len() == 64
            && artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "The release artifact is invalid."
    );
    ensure!(
        format.supports(&artifact.architecture)?,
        "The release architecture is unsupported."
    );
    Ok((data, artifact, prefix))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Check,
    Install,
}

fn execute(operation: Operation, report: &dyn Fn(Phase, &str) -> Result<()>) -> Result<Status> {
    report(Phase::Checking, "Checking for a newer release…")?;
    let folder = tempfile::tempdir_in(RUNTIME)?;
    let Some(latest) = newer(folder.path())? else {
        return Ok(Status::finished(
            "Wayline is up to date.".into(),
            runtime::version(),
            false,
            Phase::Idle,
        ));
    };
    if operation == Operation::Install {
        report(Phase::Downloading, "Downloading the signed update.")?;
    }
    let format = Format::detect();
    let (manifest, artifact, prefix) = metadata(folder.path(), &latest, format)?;
    if operation == Operation::Check {
        let message = format!("Version {} is available.", manifest.version);
        return Ok(Status::finished(
            message,
            manifest.version,
            true,
            Phase::Idle,
        ));
    }
    space(folder.path(), artifact.bytes.saturating_mul(2) + 1_048_576)?;
    space(Path::new("/etc/wayline"), 4_194_304)?;
    let package = folder
        .path()
        .join(format!("package.{}", format.extension()));
    download(
        &format!("{prefix}{}", artifact.name),
        &package,
        artifact.bytes,
    )?;
    report(Phase::Verifying, "Verifying the signed package.")?;
    let bytes = wayline_support::read(&package, 4_194_304)?;
    ensure!(
        u64::try_from(bytes.len())? == artifact.bytes
            && wayline_support::digest(&bytes) == artifact.sha256,
        "The release size or digest does not match."
    );
    let path = package.to_str().context("Invalid package path.")?;
    format.install(path, true)?;
    arm(Path::new(LATCH), &manifest.version)?;
    report(Phase::Installing, "Installing the signed update.")?;
    format.install(path, false)?;
    ensure!(
        runtime::version() == manifest.version,
        "The installed version needs checking. Do not retry automatically."
    );
    disarm(Path::new(LATCH))?;
    let message = format!("Updated to {}.", manifest.version);
    Ok(Status::finished(
        message,
        manifest.version,
        false,
        Phase::Done,
    ))
}

fn attempt(operation: Operation, previous: Option<&Status>) -> Result<Status> {
    let status = if allowed(operation, previous, latched()) {
        let report =
            |phase: Phase, message: &str| Status::working(previous, phase, message).write();
        execute(operation, &report).unwrap_or_else(|error| Status::stopped(&error))
    } else {
        Status::stopped(&anyhow::anyhow!(BLOCKED))
    };
    status.write()?;
    Ok(status)
}

fn settled(previous: Option<&Status>, latched: bool) -> bool {
    !latched && previous.is_none_or(|status| status.phase != Phase::Installing)
}

fn allowed(operation: Operation, previous: Option<&Status>, latched: bool) -> bool {
    operation == Operation::Check || settled(previous, latched)
}

fn plan(requested: &str, auto_updates: bool, settled: bool) -> Operation {
    if requested == "install_update" || (requested.is_empty() && auto_updates && settled) {
        Operation::Install
    } else {
        Operation::Check
    }
}

fn automatic_wait(previous: Option<&Status>) -> u64 {
    if previous.is_some_and(|status| status.failed && !status.needs_attention) {
        3_600
    } else {
        86_400
    }
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
    Path::new(RUNTIME).join("update-request")
}

fn idle() -> bool {
    runtime::locked("update", || Ok(())).is_ok()
}

pub fn start(method: &str) -> Result<bool> {
    if !idle() {
        return Ok(false);
    }
    wayline_support::atomic(&request_path(), method.as_bytes())?;
    Command::new(std::env::current_exe()?)
        .arg("update-job")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(true)
}

pub fn job() -> Result<()> {
    runtime::locked("update", || {
        let request = request_path();
        let operation = if fs::read_to_string(&request).unwrap_or_default() == "install_update" {
            Operation::Install
        } else {
            Operation::Check
        };
        let _ = fs::remove_file(&request);
        attempt(operation, Status::read().as_ref()).map(drop)
    })
}

pub fn run_now(operation: Operation) -> Result<Status> {
    runtime::locked("update", || {
        let previous = Status::read();
        ensure!(allowed(operation, previous.as_ref(), latched()), BLOCKED);
        attempt(operation, previous.as_ref())
    })
}

pub fn scheduled(config: &Settings) -> Result<()> {
    if !idle() {
        return Ok(());
    }
    runtime::locked("update", || {
        let request = request_path();
        let requested = fs::read_to_string(&request).unwrap_or_default();
        let previous = Status::read();
        let checked_at = previous.as_ref().map_or(0, |status| status.checked_at);
        if requested.is_empty()
            && (!config.auto_updates
                || runtime::now().saturating_sub(checked_at) < automatic_wait(previous.as_ref()))
        {
            return Ok(());
        }
        let operation = plan(
            &requested,
            config.auto_updates,
            settled(previous.as_ref(), latched()),
        );
        attempt(operation, previous.as_ref())?;
        if request.exists() {
            fs::remove_file(request)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Format, Operation, Phase, Release, Status, allowed, apk_form, arm, automatic_wait, disarm,
        effective, latched_at, matches_record, newest, plan, recorded, settled,
    };
    use anyhow::{Result, ensure};
    use semver::Version;

    #[test]
    fn the_latch_survives_on_disk_until_disarmed() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let latch = folder.path().join("update-attention");
        ensure!(!latched_at(&latch));
        arm(&latch, "0.1.0-alpha.26")?;
        ensure!(latched_at(&latch));
        ensure!(std::fs::read_to_string(&latch)?.starts_with("0.1.0-alpha.26 "));
        disarm(&latch)?;
        ensure!(!latched_at(&latch));
        disarm(&latch)?;
        Ok(())
    }

    #[test]
    fn checks_run_while_latched_but_installs_do_not() {
        let installing = at(Phase::Installing, 0);
        assert!(allowed(Operation::Check, None, true));
        assert!(allowed(Operation::Check, Some(&installing), true));
        assert!(!allowed(Operation::Install, None, true));
        assert!(!allowed(Operation::Install, Some(&installing), false));
        assert!(allowed(Operation::Install, None, false));
        assert!(settled(Some(&at(Phase::Failed, 0)), false));
        assert!(!settled(Some(&at(Phase::Failed, 0)), true));
    }

    #[test]
    fn automatic_updates_only_check_while_latched() {
        assert!(plan("", true, true) == Operation::Install);
        assert!(plan("", true, false) == Operation::Check);
        assert!(plan("", false, true) == Operation::Check);
        assert!(plan("check_update", true, true) == Operation::Check);
        assert!(plan("install_update", false, false) == Operation::Install);
    }

    #[test]
    fn reads_the_package_manager_record() {
        let opkg = "Package: luci\nVersion: 1\nStatus: install user installed\n\nPackage: wayline\nVersion: 0.1.0-alpha.25\nDepends: curl\nStatus: install user installed\nArchitecture: aarch64_cortex-a53\n";
        assert_eq!(
            recorded(opkg, Format::Opkg).as_deref(),
            Some("0.1.0-alpha.25")
        );
        let half = "Package: wayline\nVersion: 0.1.0-alpha.26\nStatus: install user unpacked\n";
        assert_eq!(recorded(half, Format::Opkg), None);
        let apk = "C:Q1abc=\nP:busybox\nV:1.36\n\nC:Q1def=\nP:wayline\nV:0.1.0_alpha25\nA:aarch64_cortex-a53\n";
        assert_eq!(recorded(apk, Format::Apk).as_deref(), Some("0.1.0_alpha25"));
        assert_eq!(recorded("P:waylinex\nV:1\n", Format::Apk), None);
    }

    #[test]
    fn compares_versions_in_the_package_manager_form() {
        assert_eq!(apk_form("0.1.0-alpha.25").as_deref(), Some("0.1.0_alpha25"));
        assert_eq!(apk_form("1.2.3").as_deref(), Some("1.2.3"));
        assert!(matches_record(
            "0.1.0-alpha.25",
            "0.1.0-alpha.25",
            Format::Opkg
        ));
        assert!(!matches_record(
            "0.1.0-alpha.26",
            "0.1.0-alpha.25",
            Format::Opkg
        ));
        assert!(matches_record(
            "0.1.0-alpha.25",
            "0.1.0_alpha25",
            Format::Apk
        ));
        assert!(matches_record(
            "0.1.0-alpha.25",
            "0.1.0_alpha25-r1",
            Format::Apk
        ));
        assert!(!matches_record(
            "0.1.0-alpha.25",
            "0.1.0_alpha250",
            Format::Apk
        ));
        assert!(!matches_record(
            "0.1.0-alpha.25",
            "0.1.0_alpha25-r",
            Format::Apk
        ));
        assert!(!matches_record("garbage", "garbage", Format::Apk));
    }

    fn release(tag_name: &str, draft: bool) -> Release {
        Release {
            tag_name: tag_name.into(),
            draft,
        }
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
        ensure!(effective(at(Phase::Done, 0), 100).phase == Phase::Done);
        ensure!(effective(at(Phase::Done, 0), 10_000).phase == Phase::Idle);
        ensure!(effective(at(Phase::Idle, 0), 10_000).phase == Phase::Idle);
        Ok(())
    }

    #[test]
    fn a_failed_automatic_check_retries_after_an_hour() -> Result<()> {
        let failed = Status {
            failed: true,
            ..Status::default()
        };
        ensure!(automatic_wait(Some(&failed)) == 3_600);
        let stuck = Status {
            failed: true,
            needs_attention: true,
            ..Status::default()
        };
        ensure!(automatic_wait(Some(&stuck)) == 86_400);
        ensure!(automatic_wait(Some(&Status::default())) == 86_400);
        ensure!(automatic_wait(None) == 86_400);
        Ok(())
    }
}
