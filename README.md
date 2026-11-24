# MIO — MITM Inspection & Orchestration

Pure Rust + GPUI (`gpui-kit = "=0.9.0"`) + Rama `0.4` (`rustls` + `ring`) cross-platform (macOS / Linux / Windows) desktop MITM workbench (`mio` in `crates/mio-app`) and standalone local-IPC/headless CLI (`mio-cli` in `crates/mio-cli`). Zero Python runtime, zero JVM, zero embedded browser.

---

## Build, Test & Packaging

All `cargo` commands require `CARGO_INCREMENTAL=0`; unoptimized debug `mio-ui` tests require `RUST_MIN_STACK=33554432`:

```sh
# Workspace check & full test suite (runs under scripts/test.sh)
CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings
CARGO_INCREMENTAL=0 cargo test -p mio-plugin-api -p mio-plugin-sdk -p mio-proxy -p mio-core -p mio-cli -p mio-protocol-lab --locked
RUST_MIN_STACK=33554432 CARGO_INCREMENTAL=0 cargo test -p mio-ui --lib --locked
bash scripts/test.sh

# Release packaging (macOS / Linux / Windows x arm64 / x86_64)
bash scripts/package-macos.sh --release --target aarch64-apple-darwin
bash scripts/package-macos.sh --release --target x86_64-apple-darwin
bash scripts/package-linux.sh --release --target aarch64-unknown-linux-gnu.2.34
bash scripts/package-linux.sh --release --target x86_64-unknown-linux-gnu.2.34
bash scripts/package-windows.sh --release --target aarch64-pc-windows-msvc
bash scripts/package-windows.sh --release --target x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1 -Profile release
```

---

## Runtime State & Environment Overrides

| State | Override Env | Default Location (macOS / Linux / Windows) |
|---|---|---|
| Persistent CA (`authority.json`) | `MIO_IDENTITY_DIR` | Data dir `identity/` (`~/Library/Application Support/MIO/identity/`, `~/.local/share/mio/identity/`, `%APPDATA%\MIO\identity\`) |
| Preferences & Scripts | `MIO_USER_PREFERENCES_PATH` | Config dir `preferences.json` (`http-filter-scripts.json`, `repeater-custom-actions.json`) |
| Persistent Plugins | `MIO_PLUGINS_DIR` | Config dir `plugins/` |
| Local IPC Socket | `MIO_IPC_SOCKET` | macOS `~/Library/Application Support/MIO/run/mio.sock`, Linux `${XDG_RUNTIME_DIR}/mio/mio.sock` (or `/tmp/mio-<euid>/`), Windows `%LOCALAPPDATA%\MIO\run\mio.sock` |
| Updater API URL | `MIO_UPDATE_API_URL` | `https://api.github.com/repos/ltlly/MIO/releases/latest` |

CA export while proxy is running (`127.0.0.1:8080`) or via CLI:

```sh
curl -x http://127.0.0.1:8080 http://mio/cert -o mio-root-ca.der
curl -x http://127.0.0.1:8080 http://mio/cert.pem -o mio-root-ca.crt
mio-cli ca export /tmp/mio-root-ca.crt --pem
```

### Built-in CA Presets

`crates/mio-core/src/ca_preset.rs` ships embedded issuing CA bundles for clients whose system trust store already contains the anchor and does not enforce revocation (old Android devices without updated CA stores). Selection imports the bundle through the same `Command::ImportCa` path, so the listener hot-swaps to the preset CA and serves per-host leaves signed by it.

```sh
mio-cli ca presets
mio-cli ca use emudhra-emsign-dv-g3b   # emSign ECC Root CA - G3 chain
mio-cli ca use emudhra-aaa-dv-g3b      # AAA Certificate Services cross-sign chain
mio-cli serve --listen 127.0.0.1:8080 --ca-preset emudhra-emsign-dv-g3b
```

The shipped presets are the two emSign DV TLS CA - G3B cross-signs from the October 2026 eMudhra intermediate-key compromise. Both intermediates were revoked and removed from current major trust stores; they only work on clients with a stale trust store that skips revocation and CT checks. If an origin server still presents a certificate chaining to the same root, add that root to the TLS custom CA list (or use SSL Pass-Through) so the upstream connection verifies.

---

## `mio-cli` Quick Example

```sh
mio-cli --socket /tmp/mio.sock serve --listen 127.0.0.1:8080 --export-ca /tmp/mio-root-ca.crt
mio-cli flows list --filter example.invalid --method POST --status 200 --limit 20 --json
mio-cli repeater send --flow <flow-id> --add-header "X-Role: admin" --body '{"debug":true}' --json
mio-cli proxy decide <flow-id> --revision <rev> --forward --add-header "X-Trace: 1" --json
```

Developer tools in `crates/mio-core/examples/`:
- External archive precheck/conversion: `CARGO_INCREMENTAL=0 cargo run --release --locked -p mio-core --example ext_archive_project_check -- <src.archive> <out.mio> <save-as.mio>`
- Native plugin scaffolding/check/pack: `CARGO_INCREMENTAL=0 cargo run --release --locked -p mio-core --example mio_plugin_cli -- scaffold <dir> <id> --native`
- Benchmarks: `CARGO_INCREMENTAL=0 cargo run --release --locked -p mio-core --example r5_bench` / `workflow_bench`

---

## Documentation Index

- [docs/architecture.md](docs/architecture.md): Crate boundaries, SQLite schema (`SCHEMA = 1`), multi-window concurrency, MITM/CA stack, `MioAT` / `mio-cli` IPC spec, and **Known Limitations & Defects**.
- [docs/plugin-api.md](docs/plugin-api.md): Native plugin Protobuf `v1.6` (`v1.5` fallback) wire spec, hook overflow bypass semantics, 19 `HostCall`s, 14 UI components, and protocol limitations.
- [docs/scripting-api.md](docs/scripting-api.md): Embedded Rhai `1.27` MioScript spec across all 5 entry points, schemas, built-in methods, and constraints.
- [docs/ROADMAP.md](docs/ROADMAP.md): Remaining engineering roadmap and UI modularization targets.
- [docs/adr/README.md](docs/adr/README.md): Historical Architecture Decision Records (`0081`–`0084`).
- [vendor/README.md](vendor/README.md): Vendored patch inventory across `vendor/gpui-{base,component,pre-windows,pre-linux}`.
- [plugins/protocol-lab/README.md](plugins/protocol-lab/README.md): Reference Native worker plugin (`lab.protocol`).
- [LICENSE](LICENSE): MIT license.
