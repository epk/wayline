use crate::run;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};
use wayline_support::{output, text};

const PROFILE: &str = "wayline-lab";

pub fn stop() -> Result<()> {
    run("colima", &["--profile", PROFILE, "stop"])
}

pub fn start() -> Result<()> {
    let root = fs::canonicalize(".")?;
    if cfg!(target_os = "linux") {
        let executable = std::env::current_exe()?;
        let path = std::env::var("WAYLINE_LAB_PATH")?;
        return run(
            "sudo",
            &[
                "env",
                &format!("PATH={path}"),
                &format!("WAYLINE_ROOT={}", root.display()),
                executable.to_str().context("Invalid executable path.")?,
                "lab-inside",
            ],
        );
    }
    run(
        "colima",
        &[
            "--profile",
            PROFILE,
            "start",
            "--runtime",
            "containerd",
            "--vm-type",
            "vz",
            "--cpu",
            "2",
            "--memory",
            "1",
            "--disk",
            "6",
            "--root-disk",
            "4",
            "--port-forwarder",
            "none",
            "--ssh-config=false",
            "--activate=false",
            "--binfmt=false",
            "--mount",
            "/nix/store",
            "--mount",
            &format!("{}:w", root.display()),
        ],
    )?;
    let executable = root.join("target/aarch64-unknown-linux-musl/release/wayline-dev");
    ensure!(executable.exists(), "Build the ARM64 lab executable first.");
    let path = std::env::var("WAYLINE_LAB_PATH")?;
    let result = run(
        "colima",
        &[
            "--profile",
            PROFILE,
            "ssh",
            "--",
            "sudo",
            "env",
            &format!("PATH={path}"),
            &format!("WAYLINE_ROOT={}", root.display()),
            executable.to_str().context("Invalid executable path.")?,
            "lab-inside",
        ],
    );
    let stopped = stop();
    result?;
    stopped
}

pub fn inside() -> Result<()> {
    ensure!(cfg!(target_os = "linux"), "The closed lab requires Linux.");
    ensure!(
        text("id", &["-u"], 64, 1)?.trim() == "0",
        "The isolated namespace lab requires root access."
    );
    let root = std::env::var("WAYLINE_ROOT").unwrap_or_else(|_| ".".into());
    std::env::set_current_dir(root)?;
    if std::env::var("WAYLINE_ISOLATED").is_ok() {
        return scenarios();
    }
    let executable = std::env::current_exe()?;
    ensure!(
        Command::new("unshare")
            .args([
                "--mount",
                "--net",
                "--pid",
                "--fork",
                "--mount-proc",
                "--kill-child"
            ])
            .arg(executable)
            .arg("lab-inside")
            .env("WAYLINE_ISOLATED", "1")
            .status()?
            .success(),
        "The closed lab failed."
    );
    Ok(())
}

fn ip(namespace: &str, args: &[&str]) -> Result<()> {
    let mut all = vec!["-n", namespace];
    all.extend_from_slice(args);
    run("ip", &all)
}

fn net(namespace: &str, program: &str, args: &[&str]) -> Result<()> {
    let mut all = vec!["netns", "exec", namespace, program];
    all.extend_from_slice(args);
    run("ip", &all)
}

fn topology() -> Result<()> {
    run("mount", &["--make-rprivate", "/"])?;
    run("mount", &["-t", "tmpfs", "-o", "mode=755", "tmpfs", "/run"])?;
    fs::create_dir_all("/run/netns")?;
    fs::create_dir_all("/run/wayline-lab/keys")?;
    fs::create_dir_all("/run/wayline")?;
    fs::set_permissions("/run/wayline", fs::Permissions::from_mode(0o700))?;
    fs::set_permissions("/run/wayline-lab/keys", fs::Permissions::from_mode(0o700))?;
    run("ip", &["netns", "add", "router"])?;
    ip("router", &["link", "set", "lo", "up"])?;
    for index in 1..=8 {
        links(index)?;
        wireguard(index)?;
        policy(index)?;
    }
    ip("router", &["route", "add", "default", "dev", "wg1"])?;
    Ok(())
}

fn links(index: u32) -> Result<()> {
    let exit = format!("exit{index}");
    let r = format!("r{index}");
    let e = format!("e{index}");
    run("ip", &["netns", "add", &exit])?;
    ip(&exit, &["link", "set", "lo", "up"])?;
    run(
        "ip",
        &["link", "add", &r, "type", "veth", "peer", "name", &e],
    )?;
    run("ip", &["link", "set", &r, "netns", "router"])?;
    run("ip", &["link", "set", &e, "netns", &exit])?;
    ip(
        "router",
        &["address", "add", &format!("172.20.{index}.1/30"), "dev", &r],
    )?;
    ip(
        &exit,
        &["address", "add", &format!("172.20.{index}.2/30"), "dev", &e],
    )?;
    ip("router", &["link", "set", &r, "up"])?;
    ip(&exit, &["link", "set", &e, "up"])?;
    Ok(())
}

fn wireguard(index: u32) -> Result<()> {
    let exit = format!("exit{index}");
    let wg = format!("wg{index}");
    let rk = key(&format!("r{index}"))?;
    let ek = key(&format!("e{index}"))?;
    ip("router", &["link", "add", &wg, "type", "wireguard"])?;
    ip(&exit, &["link", "add", "wg0", "type", "wireguard"])?;
    net(
        "router",
        "wg",
        &[
            "set",
            &wg,
            "private-key",
            &format!("/run/wayline-lab/keys/r{index}"),
            "peer",
            &ek,
            "allowed-ips",
            "0.0.0.0/0",
            "endpoint",
            &format!("172.20.{index}.2:51820"),
        ],
    )?;
    net(
        &exit,
        "wg",
        &[
            "set",
            "wg0",
            "listen-port",
            "51820",
            "private-key",
            &format!("/run/wayline-lab/keys/e{index}"),
            "peer",
            &rk,
            "allowed-ips",
            &format!("10.90.{index}.2/32"),
        ],
    )?;
    ip(
        "router",
        &["address", "add", &format!("10.90.{index}.2/24"), "dev", &wg],
    )?;
    ip(
        &exit,
        &[
            "address",
            "add",
            &format!("10.90.{index}.1/24"),
            "dev",
            "wg0",
        ],
    )?;
    for target in 1..=12 {
        ip(
            &exit,
            &[
                "address",
                "add",
                &format!("198.18.0.{target}/32"),
                "dev",
                "lo",
            ],
        )?;
    }
    ip("router", &["link", "set", &wg, "up"])?;
    ip(&exit, &["link", "set", "wg0", "up"])?;
    Ok(())
}

fn policy(index: u32) -> Result<()> {
    let exit = format!("exit{index}");
    let wg = format!("wg{index}");
    ip(
        "router",
        &[
            "route",
            "add",
            "default",
            "dev",
            &wg,
            "table",
            &(1000 + index).to_string(),
        ],
    )?;
    ip(
        "router",
        &[
            "route",
            "add",
            "blackhole",
            "default",
            "metric",
            "254",
            "table",
            &(1000 + index).to_string(),
        ],
    )?;
    ip(
        "router",
        &[
            "rule",
            "add",
            "pref",
            &(6000 + index).to_string(),
            "fwmark",
            &format!("{}/0xf000", index * 4096),
            "table",
            &(1000 + index).to_string(),
        ],
    )?;
    net(
        &exit,
        "tc",
        &[
            "qdisc",
            "add",
            "dev",
            "wg0",
            "root",
            "netem",
            "delay",
            &format!("{}ms", index * 10),
        ],
    )?;
    Ok(())
}

fn key(name: &str) -> Result<String> {
    let private = output("wg", &["genkey"], 128, 2)?;
    let public = wayline_support::input("wg", &["pubkey"], &private, 128, 2)?;
    wayline_support::atomic(
        &PathBuf::from(format!("/run/wayline-lab/keys/{name}")),
        &private,
    )?;
    Ok(String::from_utf8(public)?.trim().into())
}

fn configuration(interval: u64, timeout: u64, multiple: bool) -> Value {
    let mut series = Vec::new();
    for index in 1..=8 {
        for target in 1..=if multiple { 11 } else { 1 } {
            series.push(json!({"id":format!("exit{index}_{target}"),"name":format!("Exit {index}"),"interface":format!("wg{index}"),"source":format!("10.90.{index}.2"),"mark":index*4096,"target":format!("198.18.0.{target}")}));
        }
    }
    if multiple {
        for index in 1..=2 {
            series.push(json!({"id":format!("extra{index}"),"name":"Additional destination","interface":format!("wg{index}"),"source":format!("10.90.{index}.2"),"mark":index*4096,"target":"198.18.0.12"}));
        }
    }
    json!({"target":"198.18.0.1","interval_ms":interval,"timeout_ms":timeout,"tunnels":series})
}

fn start_monitor(config: &Value) -> Result<Child> {
    fs::write("/run/wayline-lab/config.json", serde_json::to_vec(config)?)?;
    let binary = fs::canonicalize("target/aarch64-unknown-linux-musl/release/waylined")?;
    Ok(Command::new("ip")
        .args(["netns", "exec", "router"])
        .arg(binary)
        .args([
            "monitor",
            "--config",
            "/run/wayline-lab/config.json",
            "--socket",
            "/run/wayline/status.sock",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(
            "/run/wayline-lab/monitor.log",
        )?))
        .spawn()?)
}

fn snapshot() -> Result<Value> {
    let socket = UnixStream::connect("/run/wayline/status.sock")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut data = Vec::new();
    socket.take(786_433).read_to_end(&mut data)?;
    ensure!(data.len() <= 786_432, "Oversized status snapshot.");
    Ok(serde_json::from_slice(&data)?)
}

fn after(seconds: u64) -> Result<Value> {
    thread::sleep(Duration::from_secs(seconds));
    snapshot()
}

fn delay(index: u32, args: &[&str]) -> Result<()> {
    let mut all = vec!["qdisc", "replace", "dev", "wg0", "root", "netem"];
    all.extend_from_slice(args);
    net(&format!("exit{index}"), "tc", &all)
}

fn scenarios() -> Result<()> {
    topology()?;
    crate::native::check()?;
    let mut child = start_monitor(&configuration(1000, 500, false))?;
    let result = failures(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn failures(child: &mut Child) -> Result<()> {
    let mut results = serde_json::Map::new();
    let baseline = after(3)?;
    ensure!(
        baseline["tunnels"].as_array().is_some_and(|items| items
            .iter()
            .all(|item| item["received"].as_u64().unwrap_or(0) > 0)),
        "An isolated WireGuard path did not respond."
    );
    results.insert("baseline".into(), baseline);
    rejection_checks()?;
    results.insert("input_and_endpoint_guards".into(), json!(true));
    println!("The WireGuard paths respond. Checking injected faults.");
    delay(2, &["delay", "100ms"])?;
    let delayed = after(5)?;
    ensure!(
        delayed["tunnels"][1]["median_us"].as_u64().unwrap_or(0) >= 90_000,
        "The monitor did not detect added latency."
    );
    results.insert("added_delay".into(), delayed);
    delay(2, &["delay", "20ms"])?;
    delay(3, &["loss", "100%"])?;
    let loss = after(6)?;
    ensure!(
        loss["tunnels"][2]["median_us"].is_null(),
        "An unavailable path retained a recent latency."
    );
    results.insert("packet_loss".into(), loss);
    delay(3, &["delay", "30ms"])?;
    route_failures(&mut results)?;
    child.kill()?;
    child.wait()?;
    let stale = Path::new("/run/wayline/status.sock").exists();
    ensure!(stale, "The crash check did not leave a stale endpoint.");
    let config = configuration(15_000, 2000, true);
    *child = start_monitor(&config)?;
    let before = after(18)?;
    let resources_start = resources(child.id())?;
    let started = std::time::Instant::now();
    for index in 1..=8 {
        delay(index, &["loss", "100%"])?;
    }
    let outage = after(80)?;
    let resources_end = resources(child.id())?;
    let cpu = cpu_percent(
        &resources_start,
        &resources_end,
        started.elapsed().as_secs_f64(),
    )?;
    results.insert("resources".into(), json!({"start":resources_start,"end":resources_end,"one_core_cpu_percent":cpu,"seconds":started.elapsed().as_secs_f64()}));
    ensure!(
        outage["tunnels"]
            .as_array()
            .is_some_and(|items| items.len() == 90
                && items
                    .iter()
                    .all(|item| item["sent"].as_u64().unwrap_or(0) >= 6)),
        "The timeout schedule did not serve every series."
    );
    results.insert("full_timeout_schedule".into(), outage);
    results.insert("before_outage".into(), before);
    ensure!(
        child.try_wait()?.is_none(),
        "The monitor exited during the failure checks."
    );
    child.kill()?;
    child.wait()?;
    one_second_checks(&mut results)?;
    let output = Path::new("artifacts/local/current");
    fs::create_dir_all(output)?;
    results.insert(
        "binary_sha256".into(),
        json!(wayline_support::digest(&fs::read(
            "target/aarch64-unknown-linux-musl/release/waylined"
        )?)),
    );
    results.insert(
        "kernel".into(),
        json!(text("uname", &["-r"], 128, 2)?.trim()),
    );
    fs::write(
        output.join("scenarios.json"),
        serde_json::to_vec_pretty(&results)?,
    )?;
    println!("The closed-box routing checks passed.");
    Ok(())
}

fn rejection_checks() -> Result<()> {
    let routes = text(
        "ip",
        &["-n", "router", "-j", "route", "show", "table", "all"],
        65_536,
        2,
    )?;
    let binary = fs::canonicalize("target/aarch64-unknown-linux-musl/release/waylined")?;
    let binary = binary.to_str().context("Invalid monitor path.")?;
    let valid = configuration(1000, 500, false);
    let mut invalid = vec![json!({}), valid.clone(), valid.clone(), valid];
    invalid[1]["tunnels"][0]["source"] = json!("127.0.0.1");
    invalid[2]["tunnels"][1] = invalid[2]["tunnels"][0].clone();
    invalid[3]["interval_ms"] = json!(0);
    for config in invalid {
        fs::write(
            "/run/wayline-lab/invalid.json",
            serde_json::to_vec(&config)?,
        )?;
        ensure!(
            text(
                binary,
                &[
                    "monitor",
                    "--config",
                    "/run/wayline-lab/invalid.json",
                    "--socket",
                    "/run/wayline/invalid.sock",
                    "--run-for",
                    "1"
                ],
                1024,
                3
            )
            .is_err(),
            "The monitor accepted invalid configuration."
        );
    }
    ensure!(
        text(
            binary,
            &[
                "monitor",
                "--config",
                "/run/wayline-lab/config.json",
                "--socket",
                "/run/wayline/status.sock",
                "--run-for",
                "1"
            ],
            1024,
            3
        )
        .is_err(),
        "A duplicate monitor replaced the status endpoint."
    );
    fs::write("/run/wayline/ordinary.sock", b"preserve")?;
    ensure!(
        text(
            binary,
            &[
                "monitor",
                "--config",
                "/run/wayline-lab/config.json",
                "--socket",
                "/run/wayline/ordinary.sock",
                "--run-for",
                "1"
            ],
            1024,
            3
        )
        .is_err(),
        "The monitor replaced an ordinary file."
    );
    ensure!(
        fs::read("/run/wayline/ordinary.sock")? == b"preserve",
        "The endpoint guard changed an ordinary file."
    );
    ensure!(
        routes
            == text(
                "ip",
                &["-n", "router", "-j", "route", "show", "table", "all"],
                65_536,
                2
            )?,
        "Monitor requests changed routing state."
    );
    Ok(())
}

fn route_failures(results: &mut serde_json::Map<String, Value>) -> Result<()> {
    ip(
        "router",
        &["route", "del", "default", "dev", "wg4", "table", "1004"],
    )?;
    let blackhole = after(2)?;
    ensure!(
        blackhole["tunnels"][3]["state"] == "route_unavailable",
        "The monitor did not reject a blackhole route."
    );
    ip(
        "router",
        &[
            "route",
            "del",
            "blackhole",
            "default",
            "metric",
            "254",
            "table",
            "1004",
        ],
    )?;
    let mismatch = after(2)?;
    ensure!(
        mismatch["tunnels"][3]["state"] == "route_mismatch",
        "The monitor did not reject another exit."
    );
    results.insert("wrong_exit".into(), mismatch);
    ip(
        "router",
        &["route", "add", "default", "dev", "wg4", "table", "1004"],
    )?;
    ip(
        "router",
        &[
            "route",
            "add",
            "blackhole",
            "default",
            "metric",
            "254",
            "table",
            "1004",
        ],
    )?;
    ip(
        "router",
        &[
            "rule",
            "add",
            "pref",
            "5000",
            "ipproto",
            "icmp",
            "fwmark",
            "0x4000/0xf000",
            "blackhole",
        ],
    )?;
    let blocked = after(2)?;
    ensure!(
        blocked["tunnels"][3]["state"] == "route_unavailable",
        "The route query used an incorrect protocol."
    );
    ip("router", &["rule", "del", "pref", "5000"])?;
    ip("router", &["link", "set", "wg5", "down"])?;
    ensure!(
        after(2)?["tunnels"][4]["state"] == "interface_down",
        "A down interface remained active."
    );
    ip("router", &["link", "set", "wg5", "up"])?;
    ip(
        "router",
        &["route", "replace", "default", "dev", "wg5", "table", "1005"],
    )?;
    Ok(())
}

fn cpu_percent(start: &Value, end: &Value, seconds: f64) -> Result<f64> {
    let ticks = text("getconf", &["CLK_TCK"], 32, 2)?
        .trim()
        .parse::<f64>()?;
    Ok(
        (end["cpu_ticks"].as_f64().unwrap_or(0.0) - start["cpu_ticks"].as_f64().unwrap_or(0.0))
            / ticks
            / seconds
            * 100.0,
    )
}

fn status(since: u64) -> Result<Value> {
    let binary = fs::canonicalize("target/aarch64-unknown-linux-musl/release/waylined")?;
    let binary = binary.to_str().context("Invalid monitor path.")?;
    let since = since.to_string();
    let data = text(
        binary,
        &[
            "status",
            "--socket",
            "/run/wayline/status.sock",
            "--since",
            &since,
        ],
        786_432,
        3,
    )?;
    Ok(serde_json::from_str(&data)?)
}

fn aligned(item: &Value) -> bool {
    item["buckets"].as_array().is_some_and(|buckets| {
        buckets
            .iter()
            .all(|bucket| bucket["at"].as_u64().is_some_and(|at| at % 15 == 0))
    })
}

fn clean_bucket(item: &Value) -> bool {
    item["buckets"].as_array().is_some_and(|buckets| {
        buckets.iter().any(|bucket| {
            bucket["sent"].as_u64().unwrap_or(0) >= 12
                && bucket["lost"].as_u64() == Some(0)
                && !bucket["median_us"].is_null()
        })
    })
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn one_second_checks(results: &mut serde_json::Map<String, Value>) -> Result<()> {
    for index in 1..=8 {
        let rate = format!("{}ms", index * 10);
        delay(index, &["delay", rate.as_str()])?;
    }
    let mut child = start_monitor(&configuration(1000, 500, false))?;
    let result = healthy_one_second(&mut child, results);
    stop_child(&mut child);
    result?;
    delay(3, &["loss", "100%"])?;
    let mut child = start_monitor(&configuration(1000, 500, false))?;
    let result = lossy_one_second(&mut child, results);
    stop_child(&mut child);
    delay(3, &["delay", "30ms"])?;
    result
}

fn healthy_one_second(
    child: &mut Child,
    results: &mut serde_json::Map<String, Value>,
) -> Result<()> {
    let start = resources(child.id())?;
    let started = std::time::Instant::now();
    thread::sleep(Duration::from_secs(35));
    let snapshot = status(0)?;
    let seconds = started.elapsed().as_secs_f64();
    let end = resources(child.id())?;
    ensure!(
        child.try_wait()?.is_none(),
        "The one-second monitor exited."
    );
    ensure!(
        snapshot["tunnels"]
            .as_array()
            .is_some_and(|items| items.len() == 8
                && items.iter().all(|item| clean_bucket(item) && aligned(item))),
        "A healthy one-second series lacks a clean closed bucket."
    );
    let cpu = cpu_percent(&start, &end, seconds)?;
    results.insert(
        "one_second_resources".into(),
        json!({"start":start,"end":end,"one_core_cpu_percent":cpu,"rss_kib":end["rss_kib"],"seconds":seconds}),
    );
    results.insert("one_second_healthy".into(), snapshot);
    Ok(())
}

fn lossy_one_second(child: &mut Child, results: &mut serde_json::Map<String, Value>) -> Result<()> {
    thread::sleep(Duration::from_secs(35));
    let snapshot = status(0)?;
    ensure!(
        child.try_wait()?.is_none(),
        "The one-second monitor exited."
    );
    let lossy = snapshot["tunnels"]
        .get(2)
        .context("Missing lossy series.")?;
    ensure!(
        lossy["id"] == "exit3_1",
        "The lossy series is not the expected path."
    );
    let buckets = lossy["buckets"].as_array().context("Missing buckets.")?;
    ensure!(
        buckets
            .iter()
            .any(|bucket| bucket["sent"].as_u64().unwrap_or(0) >= 12)
            && buckets.iter().all(|bucket| {
                bucket["sent"].as_u64().unwrap_or(0) > 0
                    && bucket["lost"] == bucket["sent"]
                    && bucket["median_us"].is_null()
            }),
        "The lossy path did not keep one probe per second with every probe lost."
    );
    results.insert("one_second_loss".into(), snapshot);
    Ok(())
}

fn resources(pid: u32) -> Result<Value> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let memory = fs::read_to_string(root.join("smaps_rollup"))?;
    let rss = memory
        .lines()
        .find_map(|line| line.strip_prefix("Rss:"))
        .and_then(|value| value.split_whitespace().next())
        .context("The kernel did not report memory use.")?
        .parse::<u64>()?;
    let stat = fs::read_to_string(root.join("stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .context("Invalid process status.")?
        .1
        .split_whitespace()
        .collect();
    let ticks = fields
        .get(11)
        .context("Missing user CPU time.")?
        .parse::<u64>()?
        + fields
            .get(12)
            .context("Missing system CPU time.")?
            .parse::<u64>()?;
    Ok(
        json!({"rss_kib":rss,"cpu_ticks":ticks,"file_descriptors":fs::read_dir(root.join("fd"))?.count(),"threads":fs::read_dir(root.join("task"))?.count()}),
    )
}
