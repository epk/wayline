# Third-party notices

Wayline uses the Rust crates in `Cargo.lock`.
The native UI bundles React and React DOM (MIT), the `scheduler` package (MIT), UI SDK (`@ui-sdk/core` and `@ui-sdk/react`, Apache-2.0), uPlot (MIT), Flechette (`@uwdata/flechette`, BSD-3-Clause; its GeoArrow decoder is stubbed out) and fzstd (MIT).
`wayline-ctl` statically links Apache Arrow and Apache Parquet for Rust (Apache-2.0) and the Zstandard library (BSD-3-Clause) through the `zstd` crates.
The router supplies Vue, Lua, and the OpenWrt services.
Wayline does not redistribute GL.iNet firmware or factory UI bundles.

Release archives must include the dependency license files.
The release workflow collects these files from the locked dependency sources.
Research references retain their original licenses.
Their presence in the research list does not grant permission to copy their code.
