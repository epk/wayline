use anyhow::{Context, Result, ensure};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use tar::{Builder, EntryType, Header};
use walkdir::WalkDir;
use xshell::{Shell, cmd};

const ARCHITECTURE: &str = "aarch64_cortex-a53";
const DEPENDS: &str = "libc rpcd lua lua-cjson curl ca-bundle usign";
const DESCRIPTION: &str = "VPN latency measurements and a native GL.iNet UI.";

pub fn artifact(version: &str, format: &str) -> String {
    format!("wayline_{version}_{ARCHITECTURE}.{format}")
}

pub fn apk_version(version: &str) -> Result<String> {
    let parsed = semver::Version::parse(version)?;
    let core = format!("{}.{}.{}", parsed.major, parsed.minor, parsed.patch);
    if parsed.pre.is_empty() {
        return Ok(core);
    }
    let mut parts = parsed.pre.as_str().split('.');
    let label = parts.next().unwrap_or_default();
    let number = parts.next().unwrap_or("0");
    ensure!(
        matches!(label, "alpha" | "beta" | "pre" | "rc")
            && !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && parts.next().is_none(),
        "The prerelease {} has no APK equivalent.",
        parsed.pre
    );
    Ok(format!("{core}_{label}{number}"))
}

pub fn rpc_object(version: &str) -> String {
    format!("wayline_{}", version.replace('.', "_"))
}

fn header(mode: u32, size: u64) -> Header {
    let mut header = Header::new_gnu();
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header
}

fn append(
    builder: &mut Builder<GzEncoder<Vec<u8>>>,
    path: &Path,
    data: &[u8],
    mode: u32,
) -> Result<()> {
    let mut header = header(mode, u64::try_from(data.len())?);
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
            let mode = if executable { 0o755 } else { 0o644 };
            let mut data = header(mode, item.metadata()?.len());
            builder.append_data(&mut data, relative, fs::File::open(item.path())?)?;
        }
        if item.file_type().is_symlink() {
            let mut link = header(0o777, 0);
            link.set_entry_type(EntryType::Symlink);
            builder.append_link(&mut link, relative, fs::read_link(item.path())?)?;
        }
        if item.file_type().is_dir() {
            let mode = if relative == Path::new("etc/wayline") {
                0o700
            } else {
                0o755
            };
            let mut directory = header(mode, 0);
            directory.set_entry_type(EntryType::Directory);
            builder.append_data(&mut directory, relative, std::io::empty())?;
        }
    }
    Ok(builder.into_inner()?.finish()?)
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
    let output = wayline_support::output_status("node", &["-e", LOADER_CHECK, path], 65536, 20)?;
    ensure!(
        output.success && String::from_utf8_lossy(&output.bytes).trim() == "ok",
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
    let sh = Shell::new()?;
    cmd!(sh, "cp -a package/files/. {root}").run()?;
    let object = rpc_object(version);
    for (directory, source, alias) in [
        ("usr/lib/oui-httpd/rpc", "wayline", object.clone()),
        (
            "usr/share/gl-validator.d",
            "wayline.lua",
            format!("{object}.lua"),
        ),
    ] {
        std::os::unix::fs::symlink(source, root.join(directory).join(alias))?;
    }
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
    install_chunks(Path::new("ui/views/wayline"), &views.join("wayline"))?;
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
            "Package: wayline\nVersion: {version}\nArchitecture: {ARCHITECTURE}\nMaintainer: epk\nSection: net\nPriority: optional\nLicense: MIT\nDepends: {}\nDescription: {DESCRIPTION}\n",
            DEPENDS.replace(' ', ", ")
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
    let name = Path::new("dist").join(artifact(version, "ipk"));
    fs::write(&name, package.into_inner()?.finish()?)?;
    println!("{}", name.display());
    if cfg!(target_os = "linux") {
        build_apk(version)
    } else {
        println!(
            "Skipping the APK package. APK packages record owner names from the build host, so build them on Linux."
        );
        Ok(())
    }
}

const APK_SCRIPT: &str = "umask 022 && mkdir root && tar -xzf data.tar.gz -C root && chown -R 0:0 root && apk mkpkg --info name:wayline --info \"version:$1\" --info \"arch:$2\" --info \"description:$3\" --info license:MIT --info url:https://github.com/epk/wayline --info \"depends:$4\" --script post-install:scripts/postinst --script post-upgrade:scripts/postinst --script pre-deinstall:scripts/prerm --files root --output wayline.apk";

fn unpack_ipk(source: &Path, destination: &Path) -> Result<()> {
    let mut outer = tar::Archive::new(GzDecoder::new(fs::File::open(source)?));
    for entry in outer.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        if matches!(name.as_str(), "data.tar.gz" | "control.tar.gz") {
            entry.unpack(destination.join(name))?;
        }
    }
    let scripts = destination.join("scripts");
    fs::create_dir(&scripts)?;
    tar::Archive::new(GzDecoder::new(fs::File::open(
        destination.join("control.tar.gz"),
    )?))
    .unpack(&scripts)?;
    for name in ["postinst", "prerm"] {
        ensure!(
            scripts.join(name).is_file(),
            "The IPK has no {name} script."
        );
    }
    ensure!(
        destination.join("data.tar.gz").is_file(),
        "The IPK has no data archive."
    );
    Ok(())
}

pub fn build_apk(version: &str) -> Result<()> {
    let apk_version = apk_version(version)?;
    let temporary = tempfile::tempdir()?;
    unpack_ipk(
        &Path::new("dist").join(artifact(version, "ipk")),
        temporary.path(),
    )?;
    let sh = Shell::new()?;
    sh.change_dir(temporary.path());
    let script = APK_SCRIPT;
    if wayline_support::text("id", &["-u"], 64, 1)?.trim() == "0" {
        cmd!(
            sh,
            "sh -c {script} sh {apk_version} {ARCHITECTURE} {DESCRIPTION} {DEPENDS}"
        )
        .run()?;
    } else {
        cmd!(
            sh,
            "fakeroot -- sh -c {script} sh {apk_version} {ARCHITECTURE} {DESCRIPTION} {DEPENDS}"
        )
        .run()?;
    }
    let output = temporary.path().join("wayline.apk");
    let dump = cmd!(sh, "apk adbdump {output}").read()?;
    let owners: Vec<_> = dump
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("user:") || line.starts_with("group:"))
        .collect();
    ensure!(
        !owners.is_empty() && owners.iter().all(|line| line.ends_with(": root")),
        "The APK package records owners other than root."
    );
    ensure!(
        dump.lines()
            .any(|line| line.trim() == format!("version: {apk_version}"))
            && dump
                .lines()
                .any(|line| line.trim() == format!("arch: {ARCHITECTURE}")),
        "The APK package metadata is unexpected."
    );
    let name = Path::new("dist").join(artifact(version, "apk"));
    fs::copy(output, &name)?;
    println!("{}", name.display());
    Ok(())
}

fn licenses(version: &str) -> Result<()> {
    let metadata = crate::metadata::workspace()?;
    ensure!(
        metadata
            .packages
            .iter()
            .any(|package| package.name == "waylined"
                && package.source.is_none()
                && package.version.to_string() == version),
        "The tag and package version differ."
    );
    let output = Path::new("dist/licenses");
    if output.exists() {
        fs::remove_dir_all(output)?;
    }
    fs::create_dir_all(output)?;
    for package in &metadata.packages {
        if package.source.is_none() {
            continue;
        }
        let source = package
            .manifest_path
            .as_std_path()
            .parent()
            .context("Invalid dependency source.")?;
        let name = format!("{}-{}", package.name, package.version);
        copy_licenses(
            source,
            &output.join(name),
            &["LICENSE", "COPYING", "NOTICE"],
        )?;
    }
    for name in UI_PACKAGES {
        let direct = Path::new("ui/node_modules").join(name);
        let source = if direct.exists() {
            direct
        } else {
            Path::new("ui/node_modules/.pnpm/node_modules").join(name)
        };
        copy_licenses(&source, &output.join(name), &["LICENSE"])?;
        ensure!(
            fs::read_dir(output.join(name))?.next().is_some(),
            "No license file found for {name}."
        );
    }
    Ok(())
}

const UI_PACKAGES: [&str; 7] = [
    "react",
    "react-dom",
    "scheduler",
    "@ui-sdk/core",
    "@ui-sdk/react",
    "fzstd",
    "uplot",
];

fn install_chunks(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let mut core = false;
    for item in
        fs::read_dir(source).context("Build the page first: ui/views/wayline is missing.")?
    {
        let path = item?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("Invalid page chunk name.")?
            .to_owned();
        if Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zst"))
        {
            core |= name.starts_with("core-");
            fs::copy(&path, destination.join(&name))?;
        } else if Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
        {
            let mut gzip = GzEncoder::new(Vec::new(), Compression::best());
            gzip.write_all(&fs::read(&path)?)?;
            fs::write(destination.join(format!("{name}.gz")), gzip.finish()?)?;
        }
    }
    ensure!(core, "The page build has no core chunk.");
    Ok(())
}

fn copy_licenses(source: &Path, destination: &Path, prefixes: &[&str]) -> Result<()> {
    fs::create_dir_all(destination)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().to_uppercase();
        if item.file_type()?.is_file() && prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            fs::copy(item.path(), destination.join(item.file_name()))?;
        }
    }
    Ok(())
}

fn manifest(version: &str, format: &str, output: &str) -> Result<()> {
    let name = artifact(version, format);
    let bytes = fs::read(Path::new("dist").join(&name))
        .with_context(|| format!("The release needs {name}."))?;
    let manifest = json!({"schema":1,"repository":"epk/wayline","package":"wayline","version":version,
        "firmware_contracts":wayline_support::FIRMWARE_CONTRACTS,
        "artifacts":[{"name":name,"architecture":ARCHITECTURE,"bytes":bytes.len(),"sha256":wayline_support::digest(&bytes)}]});
    fs::write(output, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}

pub fn release(version: &str) -> Result<()> {
    manifest(version, "ipk", "dist/manifest.json")?;
    manifest(version, "apk", "dist/manifest-apk.json")?;
    let mut key = tempfile::NamedTempFile::new()?;
    let secret = std::env::var("RELEASE_SIGNING_KEY")
        .or_else(|_| {
            std::env::var("WAYLINE_SIGNING_KEY_FILE")
                .map_err(std::io::Error::other)
                .and_then(fs::read_to_string)
        })
        .context("The release signing key is unavailable.")?;
    key.write_all(secret.as_bytes())?;
    let sh = Shell::new()?;
    let key = key.path();
    for manifest in ["dist/manifest.json", "dist/manifest-apk.json"] {
        cmd!(sh, "usign -S -s {key} -m {manifest}").run()?;
        cmd!(
            sh,
            "usign -V -p package/files/etc/wayline/release.pub -m {manifest}"
        )
        .run()?;
    }
    fs::copy("package/files/etc/wayline/release.pub", "dist/release.pub")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apk_version, artifact, install_chunks};
    use std::fs;

    #[test]
    fn page_chunks_install_as_zstd_and_gzip_fallback() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("build");
        fs::create_dir(&source)?;
        fs::write(source.join("core-abc.js"), b"export const a = 1;")?;
        fs::write(source.join("core-abc.js.zst"), b"zstd")?;
        fs::write(source.join("size-report.json"), b"{}")?;
        let destination = directory.path().join("www/views/wayline");
        install_chunks(&source, &destination)?;
        let mut names: Vec<String> = fs::read_dir(&destination)?
            .map(|item| Ok(item?.file_name().to_string_lossy().into_owned()))
            .collect::<anyhow::Result<_>>()?;
        names.sort();
        assert_eq!(names, ["core-abc.js.gz", "core-abc.js.zst"]);
        fs::remove_file(source.join("core-abc.js.zst"))?;
        assert!(install_chunks(&source, &directory.path().join("other")).is_err());
        Ok(())
    }

    #[test]
    fn apk_versions_keep_release_order() -> anyhow::Result<()> {
        assert_eq!(apk_version("0.1.0-alpha.22")?, "0.1.0_alpha22");
        assert_eq!(apk_version("0.1.0-beta.3")?, "0.1.0_beta3");
        assert_eq!(apk_version("1.2.3-rc.1")?, "1.2.3_rc1");
        assert_eq!(apk_version("1.2.3")?, "1.2.3");
        assert!(apk_version("0.1.0-alpha").is_ok());
        assert!(apk_version("0.1.0-nightly.1").is_err());
        assert!(apk_version("0.1.0-alpha.1.2").is_err());
        assert_eq!(
            artifact("0.1.0-alpha.22", "apk"),
            "wayline_0.1.0-alpha.22_aarch64_cortex-a53.apk"
        );
        Ok(())
    }
}
