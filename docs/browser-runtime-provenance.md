# Browser runtime provenance

This record identifies the Macroquad code that Zubspace builds and ships. It is
copied unchanged into the release client bundle and the container images.

## Rust crates

Both the native and the `wasm32-unknown-unknown` targets in `Cargo.toml` pin
Macroquad to the public fork
[`pkieltyka/macroquad`](https://github.com/pkieltyka/macroquad), release tag
[`zubspace-v0.5.1`](https://github.com/pkieltyka/macroquad/releases/tag/zubspace-v0.5.1),
commit `25b88535a93d0422c7b9ee9a86dba4c026541612`. `Cargo.lock` resolves each
of these exactly once:

| Package | Version | Source |
| --- | --- | --- |
| `macroquad` | 0.5.1 | fork, commit above |
| `macroquad_macro` | 0.1.8 | fork, commit above |
| `miniquad` | 0.4.11 | vendored in the fork, commit above |
| `quad-snd` | 0.2.8 | crates.io |

The fork requires Rust 1.88 or later; the project toolchain
(`rust-toolchain.toml`) is 1.97.1.

## JavaScript runtime

`js/mq_js_bundle.js` is the fork's browser runtime for that tag, copied
unchanged: 43,504 bytes, SHA-256
`3a262ff4f74cc051b49d4b487ea136154fbd5c8e71a8053ff33785bc7de74d3e`.

`js/mq_js_bundle.provenance.json` is copied unchanged from the same tag. It
records the upstream Macroquad 0.4.16 runtime the bundle is derived from, the
patches the fork applies, and the ABI the Rust side expects: Miniquad ABI 2,
the required `macroquad_audio` plugin ABI 1, WebGL selection extension 2
(`init_webgl(version, antialias)`) and the `zubspace_audio_source_state`
extension (`env.audio_source_state`). Its Rust `source` fields are null
because the file is the fork's source template; the concrete revision is the
commit above, as resolved in `Cargo.lock`.

Rust and JavaScript must change together. The page loader
(`js/zubspace_runtime_contract.js`, called from `index.html`) rejects a WASM
module whose imports the runtime does not provide, before instantiation,
instead of stubbing them.

## Application choices that depend on the fork

- The client requests `sample_count: 4` (`src/main.rs`). The fork honours this
  on WebGL; `1` disables antialiasing.
- Client logging uses `tracing-subscriber` with a writer that forwards each
  event, at its level, to Miniquad's console imports
  (`src/browser_logging.rs`). `tracing-wasm` is not used because it needs
  wasm-bindgen glue that the Macroquad loader does not install.
- Runtime and WASM URLs carry an `mq-0.5.1-<build>` cache version
  (`index.html`).

## Verification

`node tools/macroquad_pairing.mjs` checks both `Cargo.toml` pins, the single
lockfile entries above, and the runtime's size, hash, tag and ABI fields. It
runs in `make build-client`, `make test-quick`, the release WASM build
(`tools/build_wasm_release.mjs`), `make check-release-bundle` and
`make check-container-bundles`.

From a checkout of the fork at the pinned commit,
`node tools/regenerate_browser_runtime.mjs --check` reproduces the runtime
from the upstream base and the vendored Miniquad sources.

## Licences

Macroquad and Miniquad are licensed MIT OR Apache-2.0; their source trees retain
the upstream license files. Original SubSpace asset ownership is described in
[`ASSETS.md`](../ASSETS.md). The renderer and WebRTC dependency workflows are
covered in [Vendoring](vendoring.md).
