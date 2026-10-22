#![forbid(unsafe_code)]
#![expect(
    clippy::missing_errors_doc,
    reason = "The project prohibits code comments."
)]

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

pub const FIRMWARE_CONTRACT: &str =
    "9c8a9102c046ff12ba95e4c9a03b4e5392c461584c5d4d8ab9c5591ab6ec9237";

#[must_use]
pub fn digest(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[must_use]
pub fn within<T: Send + 'static, F: FnOnce() -> T + Send + 'static>(
    limit: Duration,
    jobs: impl IntoIterator<Item = F>,
) -> Vec<Option<T>> {
    let deadline = Instant::now() + limit;
    let pending: Vec<_> = jobs
        .into_iter()
        .map(|job| {
            let (sender, receiver) = mpsc::channel();
            thread::Builder::new()
                .spawn(move || {
                    let _ = sender.send(job());
                })
                .ok()
                .map(|_| receiver)
        })
        .collect();
    pending
        .iter()
        .map(|receiver| {
            receiver
                .as_ref()?
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok()
        })
        .collect()
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

const MAX_INPUT: usize = 2 * 1024 * 1024;

pub fn output(program: &str, args: &[&str], limit: usize, seconds: u64) -> Result<Vec<u8>> {
    let output = output_status(program, args, limit, seconds)?;
    ensure!(output.success, "{program} failed.");
    Ok(output.bytes)
}

pub struct Output {
    pub success: bool,
    pub bytes: Vec<u8>,
}

pub fn output_status(program: &str, args: &[&str], limit: usize, seconds: u64) -> Result<Output> {
    run(program, args, None, limit, seconds)
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
    ensure!(
        input.len() <= MAX_INPUT,
        "Helper input exceeds its size limit."
    );
    let output = run(program, args, Some(input), limit, seconds)?;
    ensure!(output.success, "{program} failed.");
    Ok(output.bytes)
}

fn run(
    program: &str,
    args: &[&str],
    input: Option<&[u8]>,
    limit: usize,
    seconds: u64,
) -> Result<Output> {
    let stdin = match input {
        Some(bytes) => {
            let mut source = tempfile::tempfile()?;
            source.write_all(bytes)?;
            source.rewind()?;
            Stdio::from(source)
        }
        None => Stdio::null(),
    };
    let mut data = tempfile::tempfile()?;
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(stdin)
        .stdout(Stdio::from(data.try_clone()?))
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Cannot start {program}."))?;
    let Some(status) = wait(&mut child, Duration::from_secs(seconds))? else {
        child.kill()?;
        child.wait()?;
        anyhow::bail!("{program} exceeded its time limit.");
    };
    ensure!(
        data.metadata()?.len() <= u64::try_from(limit)?,
        "Helper output exceeds its size limit."
    );
    data.rewind()?;
    let mut bytes = Vec::new();
    data.read_to_end(&mut bytes)?;
    Ok(Output {
        success: status.success(),
        bytes,
    })
}

fn wait(child: &mut Child, limit: Duration) -> Result<Option<ExitStatus>> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}
