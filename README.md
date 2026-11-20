# MIO — MITM Inspection & Orchestration

Pure Rust + GPUI (`gpui-kit = "=0.9.0"`) + Rama `0.4` (`rustls` + `ring`) cross-platform (macOS / Linux / Windows) HTTP/HTTPS/H2/SOCKS5/WS/SSE MITM inspection workbench and AI-orchestrated security testing environment (`mio` GUI + `mio-cli` local IPC). Zero Python runtime, zero JVM, zero embedded WebView.

---

## Scope & Explicit Non-Goals

- **Security isolation / sandboxing is out of scope**: MIO is a local single-user workbench. Rhai scripts and Native worker plugins are assumed user-trusted; MIO does not enforce OS sandboxes or untrusted-code containment.
- **No WASM plugin sandbox roadmap**: WASM plugin execution is not developed or maintained. **Native executable workers** over Protobuf stdio (`mio-plugin-api`) are the sole supported extension architecture (`plugins/wasm-sandbox-lab/` is retained only as a legacy test fixture).
- **Clean-room native boundaries**: No Java/Jython `.jar` plugin execution, no embedded browser/WebView, no public cloud Collaborator (local loopback HTTP/DNS OAST or explicit self-hosted endpoints), and external binary archive source files are strictly read-only.

---

## Implemented Capabilities

| Subsystem | Key Implemented Capabilities |
|---|---|
| **UI & Workbench** | **MIO Light** default theme (`#DB612F` accent, `#B5BCC7` borders, Light/Dark mode), custom cross-platform titlebar, 12 tools (`Dashboard`, `Target`, `Proxy`, `Intruder`, `Repeater`, `Collaborator`, `Sequencer`, `Decoder`, `Comparer`, `Logger`, `Organiser`, `Extensions`), Lucide icons, CJK support, `HexGridView`, soft-wrap visual-row virtualization (`gpui-base`), lazy `MessageEditor` / Repeater tab LRU |
| **Proxy & Network Stack** | Single-port multiplexing (`127.0.0.1:8080`) for HTTP/1.1, HTTPS `CONNECT` MITM (TLS 1.2/1.3), dual-leg `h2`, SOCKS5/SOCKS5h, WS/WSS, SSE, gzip/deflate/brotli unpack, upstream proxy routing, custom hostname resolution, SSL Pass-Through, PEM/PKCS#12 mTLS, persistent user-scoped `MIO local CA` + `http://mio/cert` portal |
| **Repeater & Intruder** | Repeater: horizontal & collapsible 15-color vertical tab groups, single-packet batch send, HTTP/1.1 ↔ HTTP/2, SSE, auto-redirect, WS Repeater, **Custom actions** sidebar (14 built-in mutating/inspect presets + persistent custom scripts); Intruder: 4 attack types, 18 payload generators + plugin generator, payload processing rules, literal & `re:` regex Grep Match/Extract/Payloads, resource pools, 429 throttling |
| **Storage & Archive Import** | SQLite WAL `.mio` project DB (`SCHEMA = 27`) with denormalized `flows` metadata, lazy payload loading, `http_query` UDFs, and `SlimSaveProjectAs` (`VACUUM` compaction); streaming read-only external archive (`schema 228/229`) importer with `ExtCoverageReport` |
| **Scripting & Plugins** | Embedded `rhai` `1.27` MioScript engine across HTTP/WS History filters (`Convert to script`), Match & Replace, Repeater Custom Actions (with live request mutation), and Custom Scan Checks; Protobuf `v1.4` Native plugin host (`mio-plugin-sdk`) with 11 extension contributions, 19 `HostCall` ops, & 14 declarative UI components |
| **AI Agent IPC (`mio-cli`)** | GUI & headless `mio-cli serve` share Unix `0600` UID-checked socket / Windows loopback TCP IPC with `--json` structured envelopes (`status`, `flows`, `ws`, `repeater`, `proxy`, `workspace`, `ca export`, `slim-save`, `shutdown`) |

---

## Build, Test & Packaging

```sh
# Workspace build + full unit/integration/doc test suite
cargo test --workspace --all-targets --locked --no-run
bash scripts/test.sh

# Crate-targeted fast checks
cargo test -p mio-core
TMPDIR="$PWD/.local/qa-macos" CARGO_TARGET_DIR="$PWD/target" RUST_MIN_STACK=33554432 cargo test -p mio-ui

# Self-contained Release packages (macOS / Linux / Windows x arm64 / x86_64)
bash scripts/package-macos.sh --release --target aarch64-apple-darwin
bash scripts/package-macos.sh --release --target x86_64-apple-darwin
bash scripts/package-linux.sh --release --target aarch64-unknown-linux-gnu.2.34
bash scripts/package-linux.sh --release --target x86_64-unknown-linux-gnu.2.34
bash scripts/package-windows.sh --release --target aarch64-pc-windows-msvc
bash scripts/package-windows.sh --release --target x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1 -Profile release
```

---

## Persistent Root CA (`MIO local CA`)

Generated once (ECDSA P-256, 20-year validity, `O=MIO local interception`, `CN=MIO local CA`) and persisted in OS user storage (override with `MIO_IDENTITY_DIR`):

| Platform | Default Path |
|---|---|
| macOS | `~/Library/Application Support/MIO/identity/authority.json` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/mio/identity/authority.json` |
| Windows | `%APPDATA%\MIO\identity\authority.json` |

```sh
# Download via running proxy or export via CLI
curl -x http://127.0.0.1:8080 http://mio/cert -o mio-root-ca.der
curl -x http://127.0.0.1:8080 http://mio/cert.pem -o mio-root-ca.crt
mio-cli ca export /tmp/mio-root-ca.crt --pem

# Trust installation examples
sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain /tmp/mio-root-ca.crt
sudo cp /tmp/mio-root-ca.crt /usr/local/share/ca-certificates/mio-root-ca.crt && sudo update-ca-certificates
certutil.exe -f -addstore "Root" C:\path\to\mio-root-ca.cer
```

---

## `mio-cli` Quick Reference

Specify instance descriptor via `--socket <path>` or `MIO_IPC_SOCKET` (`MIO_SOCKET` alias):

```sh
mio-cli --socket /tmp/mio.sock serve --listen 127.0.0.1:8080 --export-ca /tmp/mio-root-ca.crt
mio-cli status --json
mio-cli flows list --filter example.com --method POST --status 200 --limit 20 --json
mio-cli flows get <flow-id> [--original] --json
mio-cli ws list --limit 50 --json
mio-cli repeater send --flow <flow-id> --set-header "X-Role: admin" --body '{"debug":true}' --json
mio-cli repeater open --flow <flow-id> --title "Auth Bypass Check" --json
mio-cli proxy intercept --requests on --responses off --json
mio-cli proxy pending --json
mio-cli proxy decide <flow-id> --revision <rev> --forward --set-header "X-Trace: 1" --json
mio-cli workspace get --json > /tmp/ws.json && mio-cli workspace set /tmp/ws.json --json
mio-cli slim-save /tmp/project-slim.mio --json
```

Tools (`crates/mio-core/examples/`):
- External archive read-only precheck/conversion: `cargo run --release --locked -p mio-core --example ext_archive_project_check -- <src.archive> <out.mio> <save-as.mio>`
- Native plugin scaffolding/check/pack: `cargo run --release --locked -p mio-core --example mio_plugin_cli -- scaffold <dir> <id> --native`
- Benchmarks: `cargo run --release --locked -p mio-core --example r5_bench` / `workflow_bench`

---

## Documentation Index

- [LICENSE](LICENSE): MIT license.
- [docs/architecture.md](docs/architecture.md): Crate boundaries, SQLite schema `27` & external archive importer, network stack, `mio-cli` IPC schema.
- [docs/scripting-api.md](docs/scripting-api.md): Rhai / MioScript scripting engine spec, all 5 entry points, object schemas, built-in & MIO functions.
- [docs/plugin-api.md](docs/plugin-api.md): Native plugin Protobuf `v1.4` spec (`mio-plugin-api` / `mio-plugin-sdk`), manifest schema, 11 hooks, 14 UI components, 19 `HostCall` operations.
- [docs/adr/README.md](docs/adr/README.md): Architecture Decision Records (`0081`–`0084`).
- [plugins/protocol-lab/README.md](plugins/protocol-lab/README.md): Reference Native worker plugin implementing all 11 protocol contributions.
