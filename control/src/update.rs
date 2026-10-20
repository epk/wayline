use crate::{
    history::now,
    locked, router,
    settings::{RUNTIME, Settings},
    version,
};
use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};
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
    firmware_contracts: Vec<String>,
    artifacts: Vec<Artifact>,
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
    let release: Value = serde_json::from_slice(&read(&release_path, 65_536)?)?;
    let tag = release["tag_name"]
        .as_str()
        .and_then(|value| value.strip_prefix('v'))
        .context("Invalid release tag.")?;
    let parsed = Version::parse(tag)?;
    ensure!(
        release["draft"] == false
            && release["prerelease"] == false
            && parsed.pre.is_empty()
            && parsed.build.is_empty(),
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
    let contract = digest(&read(
        Path::new("/usr/lib/oui-httpd/rpc/vpn-client"),
        131_072,
    )?);
    ensure!(
        router::contract() && data.firmware_contracts.contains(&contract),
        "The release does not support this firmware."
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

fn package_identity(path: &Path, expected: &str) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(path)?));
    let mut control = None;
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.path()?.as_ref() == Path::new("control.tar.gz")
            || entry.path()?.as_ref() == Path::new("./control.tar.gz")
        {
            let mut bytes = Vec::new();
            entry.take(65_537).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 65_536,
                "Package control data exceeds its limit."
            );
            control = Some(bytes);
            break;
        }
    }
    let control = control.context("Package control data is absent.")?;
    let mut inner = tar::Archive::new(flate2::read::GzDecoder::new(control.as_slice()));
    let mut fields = std::collections::BTreeMap::new();
    for entry in inner.entries()? {
        let entry = entry?;
        if [Path::new("control"), Path::new("./control")].contains(&entry.path()?.as_ref()) {
            let mut data = String::new();
            entry.take(16_385).read_to_string(&mut data)?;
            ensure!(
                data.len() <= 16_384,
                "Package control data exceeds its limit."
            );
            for line in data.lines() {
                if let Some((key, value)) = line.split_once(": ") {
                    ensure!(
                        fields.insert(key.to_owned(), value.to_owned()).is_none(),
                        "Duplicate package control field."
                    );
                }
            }
            break;
        }
    }
    ensure!(
        fields
            .get("Package")
            .is_some_and(|value| value == "wayline")
            && fields.get("Version").is_some_and(|value| value == expected)
            && fields
                .get("Architecture")
                .is_some_and(|value| value == "aarch64_cortex-a53"),
        "The package identity does not match its signed manifest."
    );
    Ok(())
}

fn execute(install: bool) -> Result<Value> {
    let folder = tempfile::tempdir_in(RUNTIME)?;
    let (manifest, artifact, prefix) = metadata(folder.path())?;
    let current = Version::parse(&version())?;
    if Version::parse(&manifest.version)? <= current {
        return Ok(
            json!({"message":"Wayline is current.","available":false,"version":version(),"checked_at":now()}),
        );
    }
    if !install {
        return Ok(
            json!({"message":"A signed update is available.","available":true,"version":manifest.version,"checked_at":now()}),
        );
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
    atomic(
        Path::new(&format!("{RUNTIME}/update.json")),
        &serde_json::to_vec(
            &json!({"message":"Installing the signed update.","installing":true,"checked_at":now()}),
        )?,
    )?;
    text("opkg", &["install", path], 32_768, 120)?;
    ensure!(
        version() == manifest.version,
        "The installed version needs checking. Do not retry automatically."
    );
    Ok(
        json!({"message":"Wayline updated.","available":false,"version":manifest.version,"checked_at":now()}),
    )
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
        let status_path = PathBuf::from(format!("{RUNTIME}/update.json"));
        let previous: Value = fs::read(&status_path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or(Value::Null);
        ensure!(
            previous["installing"] != true && previous["needs_attention"] != true,
            "A previous installation needs checking."
        );
        if requested.is_empty()
            && (!config.auto_updates
                || now().saturating_sub(previous["checked_at"].as_u64().unwrap_or(0)) < 86_400)
        {
            return Ok(());
        }
        let result =
            execute(requested == "install_update" || (requested.is_empty() && config.auto_updates));
        let status = match result {
            Ok(value) => value,
            Err(error) => {
                let installing = fs::read(&status_path)
                    .ok()
                    .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
                    .is_some_and(|value| value["installing"] == true);
                json!({"message":format!("Update stopped. {error}"),"failed":true,"needs_attention":installing,"checked_at":now()})
            }
        };
        atomic(&status_path, &serde_json::to_vec(&status)?)?;
        if request.exists() {
            fs::remove_file(request)?;
        }
        Ok(())
    })
}
