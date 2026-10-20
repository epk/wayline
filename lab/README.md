# Closed-box ARM64 lab

Run `cargo xtask build` before `cargo xtask lab` inside the devenv shell.

On macOS, the Rust tool starts the dedicated `wayline-lab` Colima profile.
It uses two virtual CPUs, 1 GiB of RAM, and a small disk.
The tool stops the profile after the run.
It creates no containers.
Development and compilation remain on macOS.

The VM mounts the locked Linux tools from the Nix store.
It needs no guest package installation.
A private mount, PID, and network namespace contains the lab.
The topology has no Internet interface.

The lab creates one router namespace and eight exit namespaces.
Each exit uses a real WireGuard tunnel.
Netem injects delay and loss.
The monitor runs as an external process.
The harness reads its public status socket.

Scenarios check normal paths, increased delay, total loss, blackhole routes, another exit, ICMP-specific policy, interface failure, and crash recovery.
The maximum workload contains 45 measurement series.
A sustained full outage checks eight concurrent attempts with a 15-second interval and two-second probe timeout.

Reports appear under `artifacts/local/current`.
They include the exact binary digest, kernel, snapshots, and resource measurements.
Memory comes from `smaps_rollup`.
CPU use comes from process ticks over elapsed time.
Results describe the lab hardware and workload only.

Linux ARM64 CI uses the same tool and network namespaces directly.
No VM is required on the Linux runner.
Physical router integration remains a separate validation step.
