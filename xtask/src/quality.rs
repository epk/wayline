use anyhow::{Context, Result, ensure};
use std::fs;
use std::path::Path;
use walkdir::WalkDir;
use xshell::{Shell, cmd};

const LUA: [&str; 2] = [
    "package/files/usr/lib/oui-httpd/rpc/wayline",
    "package/files/usr/share/gl-validator.d/wayline.lua",
];
const NIX: [&str; 3] = ["devenv.nix", "nix/bootstrap.nix", "nix/usign.nix"];
const SHELL_SCRIPTS: [&str; 4] = [
    "package/postinst",
    "package/prerm",
    "package/files/etc/hotplug.d/iface/95-wayline",
    "install.sh",
];

fn sources(root: &str, extension: &str) -> Vec<String> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|item| {
            item.file_type().is_file()
                && item
                    .path()
                    .extension()
                    .is_some_and(|value| value == extension)
        })
        .map(|item| item.path().to_string_lossy().into_owned())
        .collect()
}

pub fn check() -> Result<()> {
    lint()?;
    test()
}

pub fn test() -> Result<()> {
    let sh = Shell::new()?;
    cmd!(
        sh,
        "cargo test --locked --offline --package wayline-control --package waylined --package wayline-protocol --package xtask"
    )
    .run()?;
    Ok(())
}

pub fn lint() -> Result<()> {
    let sh = Shell::new()?;
    cmd!(sh, "cargo fmt --all --check").run()?;
    cmd!(sh, "cargo clippy --locked --offline --all-targets --target aarch64-unknown-linux-musl --workspace -- -D warnings").run()?;
    cmd!(
        sh,
        "cargo clippy --locked --offline --all-targets --package xtask -- -D warnings"
    )
    .run()?;
    cmd!(sh, "luacheck {LUA...} --config .luacheckrc").run()?;
    cmd!(sh, "stylua --check {LUA...}").run()?;
    cmd!(sh, "shellcheck {SHELL_SCRIPTS...}").run()?;
    cmd!(sh, "shfmt -d -i 4 {SHELL_SCRIPTS...}").run()?;
    cmd!(sh, "nixfmt --check {NIX...}").run()?;
    cmd!(sh, "actionlint").run()?;
    cmd!(sh, "pnpm --dir ui check").run()?;
    for root in [
        "monitor/src",
        "control/src",
        "protocol/src",
        "support/src",
        "xtask/src",
    ] {
        for path in sources(root, "rs") {
            let data = fs::read_to_string(&path)?;
            ensure!(
                !data.lines().any(|line| line.trim_start().starts_with("//")
                    || line.trim_start().starts_with("/*")),
                "Code comments are prohibited: {path}"
            );
        }
    }
    for entry in WalkDir::new(".")
        .max_depth(3)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "py")
            && !entry
                .path()
                .components()
                .any(|part| part.as_os_str() == ".devenv")
        {
            anyhow::bail!("Python project tools are prohibited.");
        }
    }
    ensure!(
        Path::new("devenv.lock").is_file(),
        "The Nix inputs need a lock file."
    );
    versions()
}

fn readme_versions(readme: &str) -> Result<Vec<&str>> {
    let marker = "This README describes **";
    let start = readme
        .find(marker)
        .context("README.md does not name the version it describes.")?
        + marker.len();
    let described = readme[start..]
        .split("**")
        .next()
        .context("README.md does not name the version it describes.")?;
    let mut versions = vec![described];
    versions.extend(readme.split("WAYLINE_VERSION=").skip(1).map(|rest| {
        rest.split(|character: char| character.is_whitespace() || character == '`')
            .next()
            .unwrap_or_default()
    }));
    Ok(versions)
}

fn versions() -> Result<()> {
    let version = crate::metadata::version()?.to_string();
    for found in readme_versions(&fs::read_to_string("README.md")?)? {
        ensure!(
            found == version,
            "README.md names version {found}, but monitor/Cargo.toml is {version}."
        );
    }
    let control = crate::metadata::manifest_version(&fs::read_to_string("control/Cargo.toml")?)?;
    ensure!(
        control.to_string() == version,
        "control/Cargo.toml is {control}, but monitor/Cargo.toml is {version}."
    );
    Ok(())
}

pub fn format() -> Result<()> {
    let sh = Shell::new()?;
    cmd!(sh, "cargo fmt --all").run()?;
    cmd!(sh, "stylua {LUA...}").run()?;
    cmd!(sh, "shfmt -w -i 4 {SHELL_SCRIPTS...}").run()?;
    cmd!(sh, "nixfmt {NIX...}").run()?;
    cmd!(sh, "pnpm --dir ui format").run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::readme_versions;

    #[test]
    fn readme_versions_include_the_description_and_examples() -> anyhow::Result<()> {
        let readme = "This README describes **1.2.3**, which adds things.\n\n```sh\ncurl x | WAYLINE_VERSION=1.2.2 sh\n```\nRun `WAYLINE_VERSION=1.2.3 sh install.sh`.\n";
        assert_eq!(readme_versions(readme)?, ["1.2.3", "1.2.2", "1.2.3"]);
        assert!(readme_versions("No version here.").is_err());
        Ok(())
    }
}
