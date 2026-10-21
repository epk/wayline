use crate::run;
use anyhow::{Context, Result, ensure};
use flate2::{Compression, write::GzEncoder};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tar::{Builder, Header};
use walkdir::WalkDir;
use wayline_support::digest;

fn append(
    builder: &mut Builder<GzEncoder<Vec<u8>>>,
    path: &Path,
    data: &[u8],
    mode: u32,
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_size(u64::try_from(data.len())?);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_mode(mode);
    header.set_cksum();
    builder.append_data(&mut header, path, data)?;
    Ok(())
}

fn archive(root: &Path) -> Result<Vec<u8>> {
    let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::best()));
    let mut entries: Vec<_> = WalkDir::new(root)
        .into_iter()
        .collect::<std::result::Result<_, _>>()?;
    entries.sort_by(|left, right| left.path().cmp(right.path()));
    for item in entries {
        let relative = item.path().strip_prefix(root)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if item.file_type().is_file() {
            let executable = matches!(
                item.file_name().to_str(),
                Some("waylined" | "wayline-ctl" | "postinst" | "prerm" | "95-wayline")
            );
            append(
                &mut builder,
                relative,
                &fs::read(item.path())?,
                if executable { 0o755 } else { 0o644 },
            )?;
        }
        if item.file_type().is_symlink() {
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            builder.append_link(&mut header, relative, fs::read_link(item.path())?)?;
        }
        if item.file_type().is_dir() {
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(if relative == Path::new("etc/wayline") {
                0o700
            } else {
                0o755
            });
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            builder.append_data(&mut header, relative, std::io::empty())?;
        }
    }
    Ok(builder.into_inner()?.finish()?)
}

pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in WalkDir::new(source) {
        let item = entry?;
        let target = destination.join(item.path().strip_prefix(source)?);
        if item.file_type().is_dir() {
            fs::create_dir_all(target)?;
        } else if item.file_type().is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(item.path())?, target)?;
        } else {
            fs::copy(item.path(), target)?;
        }
    }
    Ok(())
}

const LOADER_PREFIX: &str = "(function(){var module={exports:{}};var exports=module.exports;";
const LOADER_SUFFIX: &str =
    "\nreturn (module.exports && module.exports.default) || module.exports;})()";
const LOADER_CHECK: &str = r"
const fs = require('fs');
const element = () => ({
    setAttribute() {},
    removeAttribute() {},
    appendChild(child) { return child; },
    insertBefore(child) { return child; },
    removeChild(child) { return child; },
    addEventListener() {},
    removeEventListener() {},
    style: {},
    childNodes: [],
    sheet: { cssRules: [], insertRule() { return 0; }, deleteRule() {} },
});
globalThis.window = globalThis;
globalThis.document = {
    head: element(),
    body: element(),
    createElement: element,
    createElementNS: element,
    createTextNode: () => ({}),
    querySelector: () => null,
    querySelectorAll: () => [],
    getElementsByTagName: () => [],
    addEventListener() {},
    removeEventListener() {},
};
globalThis.require = () => ({});
let ok = false;
try {
    const value = (0, eval)(fs.readFileSync(process.argv[1], 'utf8'));
    ok = value !== null && typeof value === 'object' && (typeof value.render === 'function' || value.components !== undefined || value.name !== undefined);
} catch {
    ok = false;
}
process.stdout.write(ok ? 'ok\n' : 'failed\n');
";

fn wrap_bundle(bundle: &[u8]) -> Result<Vec<u8>> {
    let mut wrapped = Vec::with_capacity(bundle.len() + LOADER_PREFIX.len() + LOADER_SUFFIX.len());
    wrapped.extend_from_slice(LOADER_PREFIX.as_bytes());
    wrapped.extend_from_slice(bundle);
    wrapped.extend_from_slice(LOADER_SUFFIX.as_bytes());
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(&wrapped)?;
    let path = file.path().to_str().context("Invalid bundle path.")?;
    let (success, output) =
        wayline_support::output_status("node", &["-e", LOADER_CHECK, path], 65536, 20)?;
    ensure!(
        success && String::from_utf8_lossy(&output).trim() == "ok",
        "The UI bundle does not evaluate to a Wayline component, so the page would stay empty. Rebuild the UI bundle."
    );
    Ok(wrapped)
}

pub fn build(version: &str) -> Result<()> {
    let parsed = semver::Version::parse(version)?;
    ensure!(
        parsed.build.is_empty(),
        "Release versions cannot contain build metadata."
    );
    licenses(version)?;
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("root");
    fs::create_dir(&root)?;
    copy_tree(Path::new("package/files"), &root)?;
    let binaries = root.join("usr/sbin");
    fs::create_dir_all(&binaries)?;
    for name in ["waylined", "wayline-ctl"] {
        fs::copy(
            format!("target/aarch64-unknown-linux-musl/release/{name}"),
            binaries.join(name),
        )?;
    }
    let views = root.join("www/views");
    fs::create_dir_all(&views)?;
    let bundle = wrap_bundle(&fs::read("ui/dist/gl-sdk4-ui-wayline.common.js")?)?;
    let mut gzip = GzEncoder::new(Vec::new(), Compression::best());
    gzip.write_all(&bundle)?;
    fs::write(
        views.join("gl-sdk4-ui-wayline.common.js.gz"),
        gzip.finish()?,
    )?;
    let metadata = root.join("usr/share/wayline");
    fs::create_dir_all(&metadata)?;
    fs::write(metadata.join("version"), format!("{version}\n"))?;
    fs::copy("LICENSE", metadata.join("LICENSE"))?;
    fs::copy(
        "THIRD_PARTY_NOTICES.md",
        metadata.join("THIRD_PARTY_NOTICES.md"),
    )?;
    fs::write("dist/licenses.tar.gz", archive(Path::new("dist/licenses"))?)?;
    fs::copy("dist/licenses.tar.gz", metadata.join("licenses.tar.gz"))?;
    let control = temporary.path().join("control");
    fs::create_dir(&control)?;
    fs::write(
        control.join("control"),
        format!(
            "Package: wayline\nVersion: {version}\nArchitecture: aarch64_cortex-a53\nMaintainer: epk\nSection: net\nPriority: optional\nLicense: MIT\nDepends: libc, rpcd, lua, lua-cjson, curl, ca-bundle, usign, sqlite3-cli\nDescription: VPN latency measurements and a native GL.iNet UI.\n"
        ),
    )?;
    fs::write(control.join("conffiles"), "/etc/wayline/settings.json\n")?;
    for name in ["postinst", "prerm"] {
        fs::copy(format!("package/{name}"), control.join(name))?;
    }
    let mut package = Builder::new(GzEncoder::new(Vec::new(), Compression::best()));
    append(&mut package, Path::new("./debian-binary"), b"2.0\n", 0o644)?;
    append(
        &mut package,
        Path::new("./control.tar.gz"),
        &archive(&control)?,
        0o644,
    )?;
    append(
        &mut package,
        Path::new("./data.tar.gz"),
        &archive(&root)?,
        0o644,
    )?;
    fs::create_dir_all("dist")?;
    let name = format!("dist/wayline_{version}_aarch64_cortex-a53.ipk");
    fs::write(&name, package.into_inner()?.finish()?)?;
    println!("{name}");
    Ok(())
}

fn licenses(version: &str) -> Result<()> {
    let data = wayline_support::output(
        "cargo",
        &["metadata", "--format-version", "1", "--locked", "--offline"],
        2 * 1024 * 1024,
        30,
    )?;
    let metadata: Value = serde_json::from_slice(&data)?;
    ensure!(
        metadata["packages"]
            .as_array()
            .is_some_and(
                |packages| packages.iter().any(|package| package["name"] == "waylined"
                    && package["source"].is_null()
                    && package["version"] == version)
            ),
        "The tag and package version differ."
    );
    let output = Path::new("dist/licenses");
    fs::create_dir_all(output)?;
    for package in metadata["packages"]
        .as_array()
        .context("Invalid dependency metadata.")?
    {
        if package["source"].is_null() {
            continue;
        }
        let source = Path::new(
            package["manifest_path"]
                .as_str()
                .context("Missing dependency source.")?,
        )
        .parent()
        .context("Invalid dependency source.")?;
        let name = format!(
            "{}-{}",
            package["name"].as_str().unwrap_or_default(),
            package["version"].as_str().unwrap_or_default()
        );
        let destination = output.join(name);
        fs::create_dir_all(&destination)?;
        for item in fs::read_dir(source)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            if item.file_type()?.is_file()
                && ["LICENSE", "COPYING", "NOTICE"]
                    .iter()
                    .any(|prefix| name.to_uppercase().starts_with(prefix))
            {
                fs::copy(item.path(), destination.join(name))?;
            }
        }
    }
    for name in ["vue-style-loader", "css-loader"] {
        let source = PathBuf::from(format!("ui/node_modules/{name}"));
        let destination = output.join(name);
        fs::create_dir_all(&destination)?;
        for item in fs::read_dir(source)? {
            let item = item?;
            if item
                .file_name()
                .to_string_lossy()
                .to_uppercase()
                .starts_with("LICENSE")
            {
                fs::copy(item.path(), destination.join(item.file_name()))?;
            }
        }
    }
    Ok(())
}

pub fn release(version: &str) -> Result<()> {
    let name = format!("wayline_{version}_aarch64_cortex-a53.ipk");
    let bytes = fs::read(Path::new("dist").join(&name))?;
    let manifest = json!({"schema":1,"repository":"epk/wayline","package":"wayline","version":version,
        "firmware_contracts":[wayline_support::FIRMWARE_CONTRACT],
        "artifacts":[{"name":name,"architecture":"aarch64_cortex-a53","bytes":bytes.len(),"sha256":digest(&bytes)}]});
    fs::write("dist/manifest.json", serde_json::to_vec_pretty(&manifest)?)?;
    let mut key = tempfile::NamedTempFile::new()?;
    let secret = std::env::var("RELEASE_SIGNING_KEY")
        .or_else(|_| {
            std::env::var("WAYLINE_SIGNING_KEY_FILE")
                .map_err(std::io::Error::other)
                .and_then(fs::read_to_string)
        })
        .context("The release signing key is unavailable.")?;
    key.write_all(secret.as_bytes())?;
    run(
        "usign",
        &[
            "-S",
            "-s",
            key.path().to_str().context("Invalid key path.")?,
            "-m",
            "dist/manifest.json",
        ],
    )?;
    run(
        "usign",
        &[
            "-V",
            "-p",
            "package/files/etc/wayline/release.pub",
            "-m",
            "dist/manifest.json",
        ],
    )?;
    fs::copy("package/files/etc/wayline/release.pub", "dist/release.pub")?;
    Ok(())
}

pub fn publish(version: &str) -> Result<()> {
    let tag = format!("v{version}");
    let title = format!("Wayline {version}");
    let artifact = format!("dist/wayline_{version}_aarch64_cortex-a53.ipk");
    let mut args = vec![
        "release",
        "create",
        &tag,
        "--repo",
        "epk/wayline",
        "--verify-tag",
        "--title",
        &title,
        "--notes",
        "",
        &artifact,
        "dist/manifest.json",
        "dist/manifest.json.sig",
        "dist/release.pub",
        "dist/licenses.tar.gz",
    ];
    if !semver::Version::parse(version)?.pre.is_empty() {
        args.push("--prerelease");
    }
    run("gh", &args)
}
