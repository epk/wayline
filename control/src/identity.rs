use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

const PATH: &str = "/etc/wayline/identity.json";

#[derive(Serialize, Deserialize)]
struct Identity {
    device: String,
}

fn valid(device: &str) -> bool {
    device.len() == 32 && device.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn create(path: &Path) -> Result<String> {
    let mut random = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let device = random.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    wayline_support::atomic(
        path,
        &serde_json::to_vec(&Identity {
            device: device.clone(),
        })?,
    )?;
    Ok(device)
}

pub fn load_or_create(path: &Path) -> Result<String> {
    if path.exists() {
        let identity: Identity = serde_json::from_slice(&wayline_support::read(path, 4096)?)
            .context("The device identity file is not valid.")?;
        ensure!(valid(&identity.device), "The device identity is not valid.");
        return Ok(identity.device);
    }
    create(path)
}

#[cfg(not(test))]
pub fn device() -> String {
    static DEVICE: OnceLock<String> = OnceLock::new();
    DEVICE
        .get_or_init(|| load_or_create(Path::new(PATH)).unwrap_or_else(|_| "unknown".into()))
        .clone()
}

#[cfg(test)]
pub fn device() -> String {
    static DEVICE: OnceLock<String> = OnceLock::new();
    let _ = PATH;
    DEVICE.get_or_init(|| "0".repeat(32)).clone()
}

#[cfg(test)]
mod tests {
    use super::{load_or_create, valid};
    use anyhow::{Result, ensure};

    #[test]
    fn identity_is_created_once_and_kept() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("wayline/identity.json");
        let first = load_or_create(&path)?;
        ensure!(valid(&first), "Invalid id {first}.");
        ensure!(load_or_create(&path)? == first, "The id changed.");
        std::fs::write(&path, br#"{"device":"nothex"}"#)?;
        ensure!(
            load_or_create(&path).is_err(),
            "An invalid id was accepted."
        );
        Ok(())
    }
}
