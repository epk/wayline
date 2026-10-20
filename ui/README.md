# Native UI

The Vue 2 component reuses the factory Vue runtime.
It loads as `gl-sdk4-ui-wayline.common.js.gz`.
The native bundle contains no sample adapter and no chart library.

Run these commands from the repository's devenv shell:

```sh
cargo xtask deps
cargo xtask build
cargo xtask preview
```

Open `http://127.0.0.1:8787/preview/index.html`.
The preview labels all values as sample data.
It does not control a router or download updates.

The component shows tunnel state, recent latency, loss, selection controls, history, and optional Internet diagnostics.
Settings provide every product option.
SVG graphs show median lines, minimum-to-maximum bands, and gaps.

The fixed RPC methods are `get_state`, `get_history`, `set_config`, `select_tunnel`, `check_update`, and `install_update`.
The backend requires administrator access.
It checks all inputs and expected revisions.
A failed native request never activates sample data.

Visible pages refresh state every five seconds.
Hidden pages stop polling.
Requests have a twelve-second UI timeout.
The backend separately bounds native commands.
