use crate::run;
use anyhow::{Result, ensure};
use std::{fs, path::Path};
use walkdir::WalkDir;

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
    run("cargo", &["fmt", "--all", "--check"])?;
    run(
        "cargo",
        &[
            "clippy",
            "--locked",
            "--offline",
            "--all-targets",
            "--target",
            "aarch64-unknown-linux-musl",
            "--workspace",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run(
        "cargo",
        &[
            "clippy",
            "--locked",
            "--offline",
            "--all-targets",
            "--package",
            "xtask",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run(
        "cargo",
        &[
            "test",
            "--locked",
            "--offline",
            "--package",
            "wayline-control",
        ],
    )?;
    run(
        "luacheck",
        &[
            "package/files/usr/lib/oui-httpd/rpc/wayline",
            "package/files/usr/share/gl-validator.d/wayline.lua",
            "--config",
            ".luacheckrc",
        ],
    )?;
    run(
        "stylua",
        &[
            "--check",
            "package/files/usr/lib/oui-httpd/rpc/wayline",
            "package/files/usr/share/gl-validator.d/wayline.lua",
        ],
    )?;
    run("shellcheck", &SHELL_SCRIPTS)?;
    run(
        "shfmt",
        &[&["-d", "-i", "4"][..], &SHELL_SCRIPTS[..]].concat(),
    )?;
    run(
        "nixfmt",
        &[
            "--check",
            "devenv.nix",
            "nix/bootstrap.nix",
            "nix/usign.nix",
        ],
    )?;
    run("actionlint", &[])?;
    run("pnpm", &["--dir", "ui", "check"])?;
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
    Ok(())
}

pub fn format() -> Result<()> {
    run("cargo", &["fmt", "--all"])?;
    run(
        "stylua",
        &[
            "package/files/usr/lib/oui-httpd/rpc/wayline",
            "package/files/usr/share/gl-validator.d/wayline.lua",
        ],
    )?;
    run(
        "shfmt",
        &[&["-w", "-i", "4"][..], &SHELL_SCRIPTS[..]].concat(),
    )?;
    run(
        "nixfmt",
        &["devenv.nix", "nix/bootstrap.nix", "nix/usign.nix"],
    )?;
    run("pnpm", &["--dir", "ui", "format"])
}
