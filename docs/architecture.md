# MIO Architecture & CLI IPC Specification

AI-oriented technical specification of MIO workspace crates, persistence, MITM stack, and `MioAT` / `mio-cli` IPC.

## 1. Scope & Non-Goals

- **Security isolation / sandboxing is out of scope**: All Rhai scripts and Native worker plugins are executed as user-trusted local code. MIO provides capability declarations and process boundaries for lifecycle/stability, NOT adversarial security containment.
- **Native worker plugins only**: Native executable plugins (`engine: "native"`) communicating over Protobuf stdio (`v1.5`) are the sole supported extension architecture.
- **Other hard boundaries**: No JVM/.jar/Jython plugin loading, no embedded WebView/browser, external archive sources are strictly read-only, OAST uses local loopback HTTP/DNS or explicit self-hosted endpoints.

---

## 2. Workspace Crates & Dependency Rules

| Crate | Path | Responsibility | Hard Boundary |
|---|---|---|---|
| `mio-app` | `crates/mio-app` | `mio` desktop GUI binary | Thin entrypoint; no duplicated business logic |
| `mio-cli` | `crates/mio-cli` | Standalone `mio-cli` CLI client & headless `serve` daemon (`MioAT` IPC) | Zero GPUI dependency; depends only on `mio-core` |
| `mio-ui` | `crates/mio-ui` | GPUI (`gpui-kit = "=0.9.0"`) workbench, themes, message editors, panel renderer | Sole crate depending on `gpui-kit`; dispatches all I/O to `mio-core` via async messages |
| `mio-core` | `crates/mio-core` | Project SQLite DB, external archive streaming importer, Proxy/Repeater/Intruder/Scanner/OAST/Sessions/Sequencer/Organiser services, Rhai MioScript engine, Native plugin host (`v1.5`), `AiIpcServer` | Zero GPUI dependency |
| `mio-proxy` | `crates/mio-proxy` | Rama `=0.4.0` + `rustls` (`ring`) adapter, TLS MITM, H1/H2/SOCKS5/WS/SSE transport, single-packet barrier | Isolates Rama types behind `mio-core` facade |
| `mio-plugin-api` | `crates/mio-plugin-api` | Versioned Protobuf `Envelope` wire protocol (`v1.5`), manifest/DTO types, `PanelSnapshot` / `PanelPatch` / `UiEvent` | Zero GPUI, SQLite, or host internal types |
| `mio-plugin-sdk` | `crates/mio-plugin-sdk` | Ergonomic Native worker SDK (`MioPlugin` trait, `HostClient` typed `v1.5` RPC helpers, `ui::*` builders, `run_stdio`) | Zero GPUI or host internals; depends only on `mio-plugin-api` |
| `mio-protocol-lab` | `plugins/protocol-lab` | Reference Native worker implementing all 11 protocol contributions | Standalone binary (`protocol-lab`) |

Vendored workspace patches in `vendor/` (detailed in [vendor/README.md](../vendor/README.md)): `gpui-base` (including soft-wrap visual-row virtualization, multi-token inline editing, bounded regex search, and custom gutters), `gpui-component`, `gpui-pre-windows`, `gpui-pre-linux`. Toolchain: Rust `1.99`, edition `2024`.

---

## 3. Storage & External Archive Import

| Layer | Implementation Details |
|---|---|
| **`.mio` Project DB** | SQLite WAL (`crates/mio-core/src/store.rs`, `SCHEMA = 27`). Denormalized `flows` metadata table enables SQL-side sorting/filtering; payloads load lazily on demand; SQLite UDFs implement `http_query` / MioScript history filtering and search; `SlimSaveProjectAs` strips unpinned static/probe noise and executes `VACUUM`. |
| **User Preferences & Script Libraries** | Stored alongside `preferences.json` (overridable via `MIO_USER_PREFERENCES_PATH`): macOS `~/Library/Application Support/MIO/`, Linux `${XDG_CONFIG_HOME:-~/.config}/mio/`, Windows `%APPDATA%\MIO\`. Separate script stores: `http-filter-scripts.json` (Script Library) and `repeater-custom-actions.json` (Repeater Custom Actions). |
| **External Archive Importer** | Read-only streaming parser for legacy binary `schema 228 / 229` project archives (`crates/mio-core/src/ext_archive/`). Maps 4 HTTP message versions (original/edited request & response), notes, 9 highlight colors, timestamps, listener ports, archived WebSockets, Repeater tabs/15-color groups/history cursor, and safe `project_config` subsets; outputs `ExtCoverageReport` for unmapped proprietary slots without altering source SHA-256. |

---

## 4. Network & MITM Stack

- **Listener**: Default `127.0.0.1:8080`. Single port multiplexes HTTP/1.1 forward proxy, HTTPS `CONNECT` MITM (TLS 1.2/1.3), dual-leg HTTP/2 (`h2`), inbound/outbound SOCKS5 & SOCKS5h, WebSocket (`ws://` / `wss://`), SSE streaming.
- **Root CA**: `crates/mio-core/src/authority.rs`. Generates persistent 20-year ECDSA P-256 root CA (`O=MIO local interception`, `CN=MIO local CA`) saved to `authority.json` (override dir via `MIO_IDENTITY_DIR`). Dynamically issues 90-day SAN leaf certs (cached per host, supports FQDN trailing-dot normalization & raw IP SANs).
- **CA Portal**: Proxy intercepts `http://mio/`, `http://mio/cert`, `http://mio/cert.pem`.
- **Provenance**: `FlowProvenance` records ordered request/response mutations across auto-unpack (`gzip`/`deflate`/`br`), Match & Replace rules, Session actions, intercept edits, and Plugin HTTP hooks.

---

## 5. `MioAT` (`mio-cli`) & Local Agent IPC (`AiIpcServer`)

Defined in `crates/mio-core/src/service/ai_ipc.rs` and `crates/mio-cli/src/main.rs` (`render.rs`):

- **Transport & Default Path** (overridable by `--socket <path>` or `MIO_IPC_SOCKET`):
  - macOS: `~/Library/Application Support/MIO/run/mio.sock` (Unix domain socket `0600`, UID check; falls back to `/tmp/mio-<euid>/mio.sock` if `> 96` bytes).
  - Linux: `${XDG_RUNTIME_DIR}/mio/mio.sock` or `/tmp/mio-<euid>/mio.sock`.
  - Windows: Loopback TCP `127.0.0.1` descriptor file (`%LOCALAPPDATA%\MIO\run\mio.sock` or `%TEMP%\mio-run\mio.sock`) + 256-bit auth token.
- **Wire Limits**: JSON-Lines, `MAX_IPC_LINE_BYTES = 8 MiB`, `MAX_CONCURRENT_IPC_CLIENTS = 16`, standard timeout `30s`, `repeater send` timeout `305s`.
- **Response Envelope (`AiIpcEnvelope`)**:
  ```json
  { "ok": true, ...payload_fields }
  { "ok": false, "error": "reason" }
  ```

### CLI Subcommands & Flags

Global options: `--socket <path>`, `--json`, `help` / `-h` / `--help`.

| Subcommand | Exact Arguments & Flags | `AiIpcRequest` Variant |
|---|---|---|
| `status` | — | `Status` |
| `flows list` | `[--filter <str>] [--method <M>] [--status <code>] [--offset <n>] [--limit <n>]` | `FlowsList` (default `limit=50`) |
| `flows get` | `<flow-id> [--original]` | `FlowsGet` |
| `ws list` | `[--limit <n>]` | `WebSocketList` (default `limit=50`) |
| `repeater open` | `(--flow <flow-id> [--original] \| --target <origin> --raw <raw-request>) [--title <title>]` | `RepeaterOpen` |
| `repeater send` | `(--flow <flow-id> [--original] \| --target <origin> --raw <raw-request>) [--add-header <K:V>] [--remove-header <K>] [--body <str>] [--insecure]` | `RepeaterSend` |
| `proxy start` | `[--listen <addr>]` | `ProxyStart` |
| `proxy stop` | — | `ProxyStop` |
| `proxy intercept` | `[--requests on\|off] [--responses on\|off]` | `ProxyIntercept` |
| `proxy pending` | — | `ProxyPending` |
| `proxy decide` | `<flow-id> --revision <rev> (--forward [--add-header <K:V>] [--remove-header <K>] [--body <str>] [--status <code>] \| --drop)` | `ProxyDecide` |
| `workspace get` | — | `WorkspaceGet` |
| `workspace set` | `<json-file>` | `WorkspacePatch` |
| `ca export` | `<dest-path> [--pem\|--der]` | `ExportCa` |
| `slim-save` | `<dest-path>` | `SlimSave` |
| `shutdown` | — | `Shutdown` |
| `serve` | `[--listen <addr>] [--export-ca <path>]` | Starts headless runtime + IPC loop |
