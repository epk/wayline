#![forbid(unsafe_code)]
#![expect(
    clippy::missing_errors_doc,
    reason = "The project prohibits code comments."
)]

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tempfile::NamedTempFile;
use wait_timeout::ChildExt;

#[must_use]
pub fn digest(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

pub fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut data = Vec::new();
    file.take(u64::try_from(limit)?.saturating_add(1))
        .read_to_end(&mut data)?;
    ensure!(data.len() <= limit, "File exceeds its size limit.");
    Ok(data)
}

pub fn atomic(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("The file needs a parent directory.")?;
    let mut file = NamedTempFile::new_in(parent)?;
    file.write_all(data)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn output(program: &str, args: &[&str], limit: usize, seconds: u64) -> Result<Vec<u8>> {
    let (success, bytes) = output_status(program, args, limit, seconds)?;
    ensure!(success, "{program} failed.");
    Ok(bytes)
}

pub fn output_status(
    program: &str,
    args: &[&str],
    limit: usize,
    seconds: u64,
) -> Result<(bool, Vec<u8>)> {
    let mut data = NamedTempFile::new()?;
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::from(data.reopen()?))
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Cannot start {program}."))?;
    let status = child.wait_timeout(Duration::from_secs(seconds))?;
    if status.is_none() {
        child.kill()?;
        child.wait()?;
    }
    ensure!(status.is_some(), "{program} exceeded its time limit.");
    ensure!(
        data.as_file().metadata()?.len() <= u64::try_from(limit)?,
        "Helper output exceeds its size limit."
    );
    let mut bytes = Vec::new();
    data.read_to_end(&mut bytes)?;
    Ok((status.is_some_and(|value| value.success()), bytes))
}

pub fn text(program: &str, args: &[&str], limit: usize, seconds: u64) -> Result<String> {
    String::from_utf8(output(program, args, limit, seconds)?)
        .context("Helper output is not valid UTF-8.")
}

pub fn input(
    program: &str,
    args: &[&str],
    input: &[u8],
    limit: usize,
    seconds: u64,
) -> Result<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    ensure!(
        input.len() <= 2 * 1024 * 1024,
        "Helper input exceeds its size limit."
    );
    let mut source = NamedTempFile::new()?;
    source.write_all(input)?;
    source.seek(SeekFrom::Start(0))?;
    let mut destination = NamedTempFile::new()?;
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::from(source.reopen()?))
        .stdout(Stdio::from(destination.reopen()?))
        .stderr(Stdio::null())
        .spawn()?;
    let status = child.wait_timeout(Duration::from_secs(seconds))?;
    if status.is_none() {
        child.kill()?;
        child.wait()?;
    }
    ensure!(
        status.is_some_and(|value| value.success()),
        "{program} failed or exceeded its time limit."
    );
    ensure!(
        destination.as_file().metadata()?.len() <= u64::try_from(limit)?,
        "Helper output exceeds its size limit."
    );
    let mut bytes = Vec::new();
    destination.read_to_end(&mut bytes)?;
    Ok(bytes)
}
