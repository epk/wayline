use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fmt::Write,
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

const ROOT: &str = "artifacts/local/firmware-root";
use wayline_support::FIRMWARE_CONTRACT as CONTRACT;
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
    let (success, output) = wayline_support::output_status("chroot", &args, 524_288, 50)?;
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
    crate::run("ip", &["link", "set", "lo", "up"])?;
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
        "tmp/log/nginx",
        "tmp/lib/nginx",
    ] {
        fs::create_dir_all(Path::new(ROOT).join(directory))?;
    }
    fs::write(
        Path::new(ROOT).join("tmp/resolv.conf"),
        "nameserver 127.0.0.1\n",
    )?;
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
    let console =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)?;
    rustix::pty::grantpt(&console)?;
    rustix::pty::unlockpt(&console)?;
    let slave = rustix::pty::ptsname(&console, Vec::new())?;
    let destination = Path::new(ROOT).join("dev/console");
    if !destination.exists() {
        fs::File::create(&destination)?;
    }
    crate::run(
        "mount",
        &[
            "--bind",
            slave.to_str()?,
            destination.to_str().context("Invalid console path.")?,
        ],
    )?;
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
    let mut session = service("/usr/bin/eco", &["/usr/sbin/gl-ngx-session"])?;
    thread::sleep(Duration::from_secs(1));
    ensure!(
        session.try_wait()?.is_none(),
        "The native session service exited: {}",
        fs::read_to_string(Path::new(ROOT).join("tmp/eco.log"))?
    );
    fs::write(
        Path::new(ROOT).join("tmp/wayline-nginx.conf"),
        "user root;\nworker_processes 1;\ninclude /etc/nginx/module.d/*.module;\npid /var/run/nginx.pid;\nerror_log /var/log/nginx/error.log notice;\nevents { worker_connections 64; }\nhttp {\n lua_package_path \"/usr/lib/lua/?.lua;;\";\n access_log off;\n lua_shared_dict shmem 12k;\n lua_shared_dict nonces 16k;\n lua_shared_dict sessions 16k;\n init_by_lua_file /usr/share/gl-ngx/oui-init.lua;\n server { listen 127.0.0.1:80;\n location = /rpc { access_by_lua_file /usr/share/gl-ngx/oui-access.lua; content_by_lua_file /usr/share/gl-ngx/oui-rpc.lua; }\n }\n}\n",
    )?;
    let mut http = service(
        "/usr/sbin/nginx",
        &["-c", "/tmp/wayline-nginx.conf", "-g", "daemon off;"],
    )?;
    thread::sleep(Duration::from_secs(1));
    ensure!(
        http.try_wait()?.is_none(),
        "The native HTTP service exited: {}",
        fs::read_to_string(Path::new(ROOT).join("tmp/nginx.log"))?
    );
    println!("{}", call("/bin/ubus", &["list"])?);
    let result = package_checks();
    if result.is_err() {
        println!(
            "Native HTTP log: {}",
            fs::read_to_string(Path::new(ROOT).join("tmp/log/nginx/error.log")).unwrap_or_default()
        );
    }
    let _ = http.kill();
    let _ = http.wait();
    let _ = session.kill();
    let _ = session.wait();
    let _ = rpc.kill();
    let _ = rpc.wait();
    let _ = bus.kill();
    let _ = bus.wait();
    result
}

fn native(method: &str, params: &Value) -> Result<Value> {
    let response: Value = serde_json::from_str(&call(
        "/bin/ubus",
        &[
            "-t",
            "35",
            "call",
            "gl-session",
            "call",
            &json!({"module":"vpn-client","func":method,"params":params}).to_string(),
        ],
    )?)?;
    ensure!(
        response["error"].is_null() && response.get("result").is_some(),
        "The native request failed: {response}"
    );
    Ok(response["result"].clone())
}

fn fixtures() -> Result<()> {
    let current = native("get_tunnel", &json!({}))?;
    for tunnel in current["tunnels"]
        .as_array()
        .context("Unknown native tunnels.")?
    {
        if tunnel["tunnel_id"] != 2001 && tunnel["tunnel_id"] != 2002 {
            continue;
        }
        let id = tunnel["id"].as_str().context("Unknown native section.")?;
        let _ = call(
            "/sbin/uci",
            &["-q", "delete", &format!("route_policy.{id}")],
        );
    }
    call("/sbin/uci", &["commit", "route_policy"])?;
    let policy = Path::new(ROOT).join("etc/config/route_policy");
    let original = fs::read_to_string(&policy)?;
    let mut rules = original;
    for (id, number) in [("wayline_lab", 2001), ("wayline_other", 2002)] {
        let _ =
            fs::remove_file(Path::new(ROOT).join(format!("etc/vpn_profiles.d/profile{number}")));
        write!(
            rules,
            "\nconfig rule '{id}'\n option name '{id}'\n option tunnel_id '{number}'\n option enabled '0'\n option via_type 'wireguard'\n option via 'wgclient1'\n option from_type 'default'\n option to_type 'default'\n option group_id '1'\n option peer_id '1'\n"
        )?;
    }
    fs::write(policy, rules)?;
    let wireguard = Path::new(ROOT).join("etc/config/wireguard");
    let original = fs::read_to_string(&wireguard)?;
    if !original.contains("peer_991") {
        fs::write(
            wireguard,
            format!(
                "{original}\nconfig peers 'peer_991'\n option name 'Lab peer'\n option group_id '991'\n"
            ),
        )?;
    }
    Ok(())
}

fn package_checks() -> Result<()> {
    let version = crate::release_version(None)?;
    let package = format!("dist/wayline_{version}_aarch64_cortex-a53.ipk");
    fs::copy(package, Path::new(ROOT).join("tmp/wayline.ipk"))?;
    println!("Checking the package with native firmware binaries.");
    fixtures()?;
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
    config["targets"] = json!(if config["targets"][0] == "8.8.8.8" {
        ["1.1.1.1", "9.9.9.9"]
    } else {
        ["8.8.8.8", "9.9.9.9"]
    });
    config["tunnels"] = json!([]);
    let mut input = config.clone();
    input["tunnels"] = json!({});
    let args = json!({"expected_revision":state["revision"],"config":input});
    request("set_config", &args)?;
    let stored = request("get_state", &json!({}))?["config"].clone();
    ensure!(
        stored == config,
        "Native settings did not persist. Sent {config}, read {stored}."
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
    compatibility_checks()?;
    tunnel_controls()?;
    bridge_checks()?;
    request("install", &json!({}))?;
    let cron = fs::read_to_string(Path::new(ROOT).join("etc/crontabs/root"))?;
    ensure!(
        cron.matches("/usr/sbin/wayline-ctl tick").count() == 1,
        "Installation duplicated its scheduler entry."
    );
    let started = request("check_update", &json!({}))?;
    ensure!(
        started["started"] == true && started["update"]["current"].is_string(),
        "The update check did not start immediately."
    );
    let mut update = Value::Null;
    for _ in 0..30 {
        update = request("get_state", &json!({}))?["update"].clone();
        if update["phase"] == "failed" {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    ensure!(
        update["failed"] == true && update["phase"] == "failed",
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
            &json!({"firmware_contract":CONTRACT,"package_install_remove":true,"settings_read_write":true,"stale_write_rejected":true,"unchanged_write_avoided":true,"scheduler_idempotent":true,
                "native_start_stop":true,"monitor_choice_retained":true,"tunnel_priority_retained":true,"missing_profile_rejected":true,"invalid_tunnel_request_rejected":true,
                "authenticated_bridge":true,"single_http_worker":true,"missing_session_rejected":true,"revoked_session_rejected":true,"local_session_bypass_rejected":true,"physical_router":false}),
        )?,
    )?;
    Ok(())
}

fn compatibility_checks() -> Result<()> {
    let module = Path::new(ROOT).join("usr/lib/oui-httpd/rpc/vpn-client");
    let original = fs::read(&module)?;
    let mut changed = original.clone();
    changed.push(b'\n');
    fs::write(&module, changed)?;
    let policy = Path::new(ROOT).join("etc/config/route_policy");
    let saved = fs::read(&policy)?;
    let result = (|| -> Result<()> {
        let state = request("get_state", &json!({}))?;
        ensure!(
            state["compat"]["level"] == "full"
                && state["compat"]["verified"] == false
                && state["compat"]["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty())
                && state["capabilities"]["select"] == true
                && state["capabilities"]["control"] == true
                && state["capabilities"]["update"] == true,
            "A newer VPN module did not keep full controls with a note."
        );
        ensure!(
            fs::read(&policy)? == saved,
            "An unverified module changed VPN policy."
        );
        Ok(())
    })();
    fs::write(module, original)?;
    result?;
    println!("Newer-module compatibility checks passed in the native sandbox.");
    Ok(())
}

fn session(method: &str, params: &Value) -> Result<Value> {
    Ok(serde_json::from_str(&call(
        "/bin/ubus",
        &["call", "gl-session", method, &params.to_string()],
    )?)?)
}

fn http(sid: &str, method: &str, params: &Value) -> Result<Value> {
    wayline_support::atomic(
        &Path::new(ROOT).join("tmp/wayline-http.json"),
        &serde_json::to_vec(
            &json!({"jsonrpc":"2.0","id":1,"method":"call","params":[sid,"wayline",method,params]}),
        )?,
    )?;
    Ok(serde_json::from_str(&call(
        "/usr/bin/curl",
        &[
            "--noproxy",
            "*",
            "--silent",
            "--max-time",
            "15",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@/tmp/wayline-http.json",
            "http://127.0.0.1/rpc",
        ],
    )?)?)
}

fn bridge_checks() -> Result<()> {
    let shadow = Path::new(ROOT).join("etc/shadow");
    let original = fs::read_to_string(&shadow)?;
    let crypt = call(
        "/usr/bin/openssl",
        &[
            "passwd",
            "-6",
            "-salt",
            "waylinelab",
            "temporary-lab-password",
        ],
    )?;
    let mut temporary = format!("root:{}:0:0:99999:7:::\n", crypt.trim());
    for line in original.lines().filter(|line| !line.starts_with("root:")) {
        writeln!(temporary, "{line}")?;
    }
    fs::write(&shadow, temporary)?;
    let result = (|| -> Result<String> {
        let challenge = session("challenge", &json!({"username":"root"}))?;
        let nonce = challenge["data"]["nonce"]
            .as_str()
            .context("The lab login challenge failed.")?;
        let hash = wayline_support::digest(format!("root:{}:{nonce}", crypt.trim()).as_bytes());
        let login = session("login", &json!({"username":"root","hash":hash}))?;
        Ok(login["data"]["sid"]
            .as_str()
            .context("The lab login failed.")?
            .into())
    })();
    fs::write(shadow, original)?;
    let sid = result?;
    let response = http(&sid, "get_state", &json!({}))?;
    ensure!(
        response["error"].is_null() && response["result"]["capabilities"]["control"] == true,
        "An authenticated bridge read failed."
    );
    let state = &response["result"];
    let row = state["tunnels"]
        .as_array()
        .context("Missing tunnel rows.")?
        .iter()
        .find(|row| row["monitorId"] == "vpn:2001")
        .context("The tunnel row disappeared.")?;
    let args =
        json!({"section_id":row["id"],"enabled":false,"expected_revision":state["revision"]});
    ensure!(
        http(&sid, "set_tunnel_enabled", &args)?["result"]["unchanged"] == true,
        "An authenticated tunnel control failed."
    );
    let policy = fs::read(Path::new(ROOT).join("etc/config/route_policy"))?;
    ensure!(
        http("", "set_tunnel_enabled", &args)?["error"].is_object(),
        "The bridge accepted a missing session."
    );
    session("logout", &json!({"sid":sid}))?;
    ensure!(
        http(&sid, "set_tunnel_enabled", &args)?["error"].is_object(),
        "The bridge accepted a revoked session."
    );
    ensure!(
        fs::read(Path::new(ROOT).join("etc/config/route_policy"))? == policy,
        "A rejected session changed the policy."
    );
    ensure!(
        session(
            "call",
            &json!({"module":"wayline","func":"get_state","params":{}})
        )?["error"]
            .is_object(),
        "A local RPC request bypassed administrator access."
    );
    println!("Native authenticated bridge checks passed with one HTTP worker.");
    Ok(())
}

fn tunnel_controls() -> Result<()> {
    let state = request("get_state", &json!({}))?;
    let path = Path::new(ROOT).join("etc/config/route_policy");
    let saved = fs::read(&path)?;
    let args =
        json!({"section_id":"wayline_lab","enabled":false,"expected_revision":state["revision"]});
    ensure!(
        request("set_tunnel_enabled", &args)?["unchanged"] == true,
        "A repeated Stop request was not idempotent."
    );
    ensure!(
        fs::read(&path)? == saved,
        "A repeated Stop request changed the policy."
    );
    for rejected in [
        json!({"section_id":"wayline_lab","enabled":true,"expected_revision":"stale"}),
        json!({"section_id":"unknown","enabled":true,"expected_revision":state["revision"]}),
        json!({"section_id":"wayline_lab","enabled":"true","expected_revision":state["revision"]}),
        json!({"section_id":"wayline_lab","enabled":true,"expected_revision":state["revision"]}),
    ] {
        ensure!(
            call(
                "/usr/sbin/wayline-ctl",
                &["set_tunnel_enabled", &rejected.to_string()]
            )
            .is_err(),
            "An invalid tunnel request succeeded."
        );
        ensure!(
            fs::read(&path)? == saved,
            "A rejected tunnel request changed the policy."
        );
    }
    native(
        "set_tunnel",
        &json!({"tunnel_id":2001,"via":{"type":"wireguard","configs":[{"group_id":991,"id_list":[991]}]}}),
    )?;
    println!(
        "Configured native fixture: {}",
        native("get_tunnel", &json!({}))?
    );
    let before = native("get_tunnel", &json!({}))?;
    let mut config = request("get_state", &json!({}))?["config"].clone();
    config["tunnels"] = json!(["vpn:2001"]);
    let current = request("get_state", &json!({}))?;
    request(
        "set_config",
        &json!({"config":config,"expected_revision":current["revision"]}),
    )?;
    for enabled in [true, true, false, false] {
        let current = request("get_state", &json!({}))?;
        let id = current["tunnels"]
            .as_array()
            .context("Missing tunnel rows.")?
            .iter()
            .find(|row| row["monitorId"] == "vpn:2001")
            .context("The tunnel row disappeared.")?["id"]
            .clone();
        let result = request(
            "set_tunnel_enabled",
            &json!({"section_id":id,"enabled":enabled,"expected_revision":current["revision"]}),
        )?;
        ensure!(
            result["applied"] == true && result["enabled"] == enabled,
            "The native tunnel state did not match the request."
        );
        let observed = request("get_state", &json!({}))?;
        let row = observed["tunnels"]
            .as_array()
            .context("Missing tunnel rows.")?
            .iter()
            .find(|row| row["monitorId"] == "vpn:2001")
            .context("The tunnel row disappeared.")?;
        ensure!(
            row["enabled"] == enabled && row["connected"] == false && row["eligible"] == false,
            "An offline tunnel reported a connected path."
        );
        let after = native("get_tunnel", &json!({}))?;
        ensure!(
            before["default_tunnels"] == after["default_tunnels"]
                && before["tunnels"][0]["tunnel_id"] == after["tunnels"][0]["tunnel_id"]
                && before["tunnels"][1] == after["tunnels"][1],
            "Tunnel control changed another group or its priority."
        );
        ensure!(
            observed["config"]["tunnels"] == json!(["vpn:2001"]),
            "A tunnel state change lost its monitoring choice."
        );
    }
    println!("Native tunnel control checks passed.");
    Ok(())
}
