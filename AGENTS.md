# Project rules

Use the installed ASD-STE100 skill for all English text in this project.
The `ste` input in `devenv.yaml` supplies the skill.
Read `SKILL.md` at its locked revision before you change project text.
Use its strict rules for procedures, error messages, and safety text.
Use its prose rules for explanations and documents.
Preserve facts, limits, and uncertainty when you revise text.

Do not add code comments.
Keep interpreter directives such as `#!/bin/sh`.
Keep required third-party notices in separate notice files.
Do not change dependency sources to remove their comments.

Use existing libraries before you create protocol or storage code.
Use Rust for the resident service.
Keep CPU use and memory use bounded.
Use one shared probe core for tunnel and WAN measurements.
The product UI must provide all user settings.

Use closed-box end-to-end checks in an isolated environment.
Do not create a broad unit test suite.
Add only critical unit checks when end-to-end checks cannot cover a specific risk.

Keep GL in control of routes, DNS, tunnel lifecycle, and kill switches.
Do not enable exit changes on an unsupported firmware contract.
Record implemented features separately from proposed features.

Use Monoio with its legacy epoll backend for the Linux 5.4 target.
Disable Monoio default features and worker pools.
Use the locked devenv.sh environment for all project commands.

Target ARM64 only. Tune the Beryl 7 binary for Cortex-A53.
Do not add x86 targets, images, or release artifacts.

Write all project tooling in Rust.
Do not add Python tools or host shell scripts.
Vue and JavaScript can serve the frontend.
Lua and required OpenWrt service hooks can serve the router bridge.
Use declarative Nix inputs and devenv tasks for development, CI, and releases.
