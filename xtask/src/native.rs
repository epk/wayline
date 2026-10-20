use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

const ROOT: &str = "artifacts/local/firmware-root";
const CONTRACT: &str = "9c8a9102c046ff12ba95e4c9a03b4e5392c461584c5d4d8ab9c5591ab6ec9237";
const NATIVE_PATH: &str = "PATH=/usr/sbin:/usr/bin:/sbin:/bin";

pub fn prepare(source: &Path) -> Result<()> {
    ensure!(
        wayline_support::digest(&fs::read(source.join("usr/lib/oui-httpd/rpc/vpn-client"))?)
            == CONTRACT,
        "The extracted firmware contract differs."
    );
    ensure!(
        !Path::new(ROOT).exists(),
        "The lab firmware directory already exists."
    );
    crate::package::copy_tree(source, Path::new(ROOT))
}

fn call(program: &str, arguments: &[&str]) -> Result<String> {
    let mut args = vec![ROOT, "/usr/bin/env", NATIVE_PATH, program];
    args.extend_from_slice(arguments);
    let (success, output) = wayline_support::output_status("chroot", &args, 524_288, 15)?;
    let output = String::from_utf8(output)?;
    ensure!(success, "Native {program} failed: {output}");
    Ok(output)
}

fn service(program: &str, arguments: &[&str]) -> Result<Child> {
    let name = Path::new(program)
        .file_name()
        .context("Invalid service path.")?;
    let log = fs::File::create(Path::new(ROOT).join("tmp").join(name).with_extension("log"))?;
    Ok(Command::new("chroot")
        .args([ROOT, "/usr/bin/env", NATIVE_PATH, program])
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()?)
}

fn request(method: &str, args: &Value) -> Result<Value> {
    Ok(serde_json::from_str::<Value>(&call(
        "/usr/sbin/wayline-ctl",
        &[method, &args.to_string()],
    )?)?["result"]
        .clone())
}

pub fn check() -> Result<()> {
    if !Path::new(ROOT).exists() {
        return Ok(());
    }
    ensure!(
        wayline_support::digest(&fs::read(
            Path::new(ROOT).join("usr/lib/oui-httpd/rpc/vpn-client")
        )?) == CONTRACT,
        "The firmware contract differs."
    );
    fs::create_dir_all(format!("{ROOT}/tmp"))?;
    crate::run(
        "mount",
        &[
            "-t",
            "tmpfs",
            "-o",
            "mode=1777",
            "tmpfs",
            &format!("{ROOT}/tmp"),
        ],
    )?;
    for directory in [
        "tmp",
        "tmp/run",
        "tmp/run/ubus",
        "tmp/lock",
        "proc",
        "dev",
        "etc/crontabs",
    ] {
        fs::create_dir_all(Path::new(ROOT).join(directory))?;
    }
    if !Path::new(ROOT).join("var").exists() {
        std::os::unix::fs::symlink("tmp", Path::new(ROOT).join("var"))?;
    }
    for device in ["null", "urandom"] {
        let destination = Path::new(ROOT).join("dev").join(device);
        if !destination.exists() {
            fs::File::create(&destination)?;
        }
        crate::run(
            "mount",
            &[
                "--bind",
                &format!("/dev/{device}"),
                destination.to_str().context("Invalid device path.")?,
            ],
        )?;
    }
    crate::run("mount", &["-t", "proc", "proc", &format!("{ROOT}/proc")])?;
    let network = Path::new(ROOT).join("etc/config/network");
    if !network.exists() {
        fs::write(
            network,
            "config interface 'loopback'\n option proto 'static'\n option ipaddr '127.0.0.1'\n option netmask '255.0.0.0'\n",
        )?;
    }
    let mut bus = service("/sbin/ubusd", &[])?;
    thread::sleep(Duration::from_secs(1));
    let mut rpc = service("/sbin/rpcd", &[])?;
    thread::sleep(Duration::from_secs(1));
    ensure!(
        bus.try_wait()?.is_none() && rpc.try_wait()?.is_none(),
        "A native RPC service exited."
    );
    println!("{}", call("/bin/ubus", &["list"])?);
    let result = package_checks();
    let _ = rpc.kill();
    let _ = rpc.wait();
    let _ = bus.kill();
    let _ = bus.wait();
    result
}

fn package_checks() -> Result<()> {
    let package = Path::new("dist/wayline_0.1.0-alpha.1_aarch64_cortex-a53.ipk");
    fs::copy(package, Path::new(ROOT).join("tmp/wayline.ipk"))?;
    println!("Checking the package with native firmware binaries.");
    println!(
        "{}",
        call(
            "/bin/opkg",
            &["--force-reinstall", "install", "/tmp/wayline.ipk"]
        )?
    );
    let state = request("get_state", &json!({}))?;
    ensure!(
        state["capabilities"]["configure"] == true
            && state["config"]["enabled"] == false
            && state["tunnels"].is_array(),
        "The native state contract failed."
    );
    let mut config = state["config"].clone();
    config["target"] = json!(if config["target"] == "8.8.8.8" {
        "1.1.1.1"
    } else {
        "8.8.8.8"
    });
    let mut input = config.clone();
    input["tunnels"] = json!({});
    let args = json!({"expected_revision":state["revision"],"config":input});
    request("set_config", &args)?;
    ensure!(
        request("get_state", &json!({}))?["config"] == config,
        "Native settings did not persist."
    );
    let path = Path::new(ROOT).join("etc/wayline/settings.json");
    let saved = fs::read(&path)?;
    ensure!(
        call("/usr/sbin/wayline-ctl", &["set_config", &args.to_string()]).is_err(),
        "A stale settings request succeeded."
    );
    ensure!(
        fs::read(&path)? == saved,
        "A rejected settings request changed configuration."
    );
    let current = request("get_state", &json!({}))?;
    let modified = fs::metadata(&path)?.modified()?;
    request(
        "set_config",
        &json!({"expected_revision":current["revision"],"config":config}),
    )?;
    ensure!(
        fs::metadata(&path)?.modified()? == modified,
        "Unchanged settings caused a write."
    );
    request("install", &json!({}))?;
    let cron = fs::read_to_string(Path::new(ROOT).join("etc/crontabs/root"))?;
    ensure!(
        cron.matches("/usr/sbin/wayline-ctl tick").count() == 1,
        "Installation duplicated its scheduler entry."
    );
    request("check_update", &json!({}))?;
    request("tick", &json!({}))?;
    ensure!(
        request("get_state", &json!({}))?["update"]["failed"] == true,
        "An offline update failure did not reach the UI state."
    );
    println!("{}", call("/bin/opkg", &["remove", "wayline"])?);
    ensure!(
        !fs::read_to_string(Path::new(ROOT).join("etc/crontabs/root"))?
            .contains("/usr/sbin/wayline-ctl tick"),
        "Removal retained its scheduler entry."
    );
    fs::create_dir_all("artifacts/local/current")?;
    fs::write(
        "artifacts/local/current/native-package.json",
        serde_json::to_vec_pretty(
            &json!({"firmware_contract":CONTRACT,"package_install_remove":true,"settings_read_write":true,"stale_write_rejected":true,"unchanged_write_avoided":true,"scheduler_idempotent":true,"physical_router":false}),
        )?,
    )?;
    Ok(())
}
