# MIO Architecture, IPC & Limitations

Authoritative technical specification of workspace boundaries, persistence, MITM stack, `MioAT` / `mio-cli` IPC, and known system limitations.

## 1. Scope & Non-Goals

- **No OS sandboxing**: Rhai scripts and Native worker executables run as user-trusted local code. `manifest.permissions` mediates MIO `HostCall` broker authorization and UI consent prompts, not OS syscall containment.
- **Native extensions only**: Out-of-process executable workers (`engine: "native"`) over 4-byte length-prefixed Protobuf stdio (`mio-plugin-api` `v1.6`, automatic `v1.5` handshake fallback) are the sole supported plugin format.
- **Clean-room boundaries**: No JVM/.jar/Jython execution, no embedded browser/WebView, external binary project archives are strictly read-only, and Collaborator uses local loopback HTTP/DNS listeners or explicit self-hosted endpoints.

---

## 2. Workspace Crates & Dependency Rules

| Crate | Path | Responsibility | Hard Boundary |
|---|---|---|---|
| `mio-app` | `crates/mio-app` | `mio` desktop GUI binary | Thin entrypoint; no duplicated business logic |
| `mio-cli` | `crates/mio-cli` | Standalone `mio-cli` client & headless `serve` daemon | Zero GPUI dependency; depends only on `mio-core` |
| `mio-ui` | `crates/mio-ui` | GPUI (`gpui-kit = "=0.9.0"`) workbench (`MIO Light`/`Dark`), custom titlebar, editors, plugin renderer | Sole crate depending on `gpui-kit`; dispatches all I/O to `mio-core` |
| `mio-core` | `crates/mio-core` | `.mio` SQLite WAL store, external archive importer, Proxy/Repeater/Intruder/Scanner/OAST/Sessions/Sequencer/Organiser, MioScript (`rhai`), plugin host (`v1.6`), `AiIpcServer` | Zero GPUI dependency |
| `mio-proxy` | `crates/mio-proxy` | Rama `=0.4.0` + `rustls` (`ring`) adapter, TLS MITM, H1/H2/SOCKS5/WS/SSE transport, single-packet batch barrier | Isolates Rama types behind `mio-core` facade |
| `mio-plugin-api` | `crates/mio-plugin-api` | Protobuf `Envelope` wire protocol (`v1.6`), DTO validators, `try_into_wire_body`, declarative panel tree | Zero GPUI, SQLite, or host internal types |
| `mio-plugin-sdk` | `crates/mio-plugin-sdk` | Native worker SDK (`MioPlugin`, `HostClient`, `ui::*`, `run_stdio`) | Zero GPUI or host internals; depends only on `mio-plugin-api` |
| `mio-protocol-lab` | `plugins/protocol-lab` | Reference Native worker implementing all 11 protocol contributions | Standalone binary (`protocol-lab`) |

Vendored patches in `vendor/` are documented in [vendor/README.md](../vendor/README.md). Toolchain: Rust `1.99`, edition `2024`.

---

## 3. Storage, Multi-Window Concurrency & Archive Import

| Subsystem | Implementation Details |
|---|---|
| **`.mio` Project DB** | SQLite WAL (`crates/mio-core/src/store.rs`, `SCHEMA = 1`, `APPLICATION_ID = 0x4d494f31`). Denormalized `flows` metadata table enables SQL-side sort/filter; payloads load lazily; SQLite UDFs run `http_query` / MioScript filtering; `SlimSaveProjectAs` strips unpinned noise and runs `VACUUM`. |
| **Concurrency & File Locks** | Multi-window / multi-process safety uses per-project advisory lock (`project_lock.rs`), proxy listener port lock (`listener_lock.rs`), Unix IPC sidecar lock (`mio.sock.lock`), and metadata-stamp hot-sync for `preferences.json` and `authority.json`. Secondary instances bind `mio-<pid>.sock`. |
| **Persistent Plugins** | Installed into user-scoped `MIO_PLUGINS_DIR` (`plugins/`); unpacked safely via `VerifiedPackage::read` (rejects path traversal, symlinks, hardlinks). `Disable extensions` blocks auto-load while still allowing uninstall. |
| **External Archive Importer** | Read-only streaming parser for `schema 228 / 229` archives (`crates/mio-core/src/ext_archive/`). Imports 4 HTTP versions, notes, 9 highlight colors, timestamps, listener ports, archived WS frames, Repeater tabs/15-color groups/history cursor, and safe `project_config` subsets; reports unmapped slots in `ExtCoverageReport`. |

---

## 4. Network & MITM Stack

- **Listener**: Default `127.0.0.1:8080`. Single-port multiplexing handles HTTP/1.1, HTTPS `CONNECT` (TLS 1.2/1.3), dual-leg HTTP/2 (`h2`), SOCKS5/SOCKS5h, WebSocket (`ws://`/`wss://`), and SSE.
- **SNI Pass-Through Sniffing**: Invisible TLS, IP `CONNECT`, and SOCKS5 connections peek up to `16 KiB + 5 B` via `PrefixedIo` rewind so fragmented or post-quantum `ClientHello`s still match `ssl_pass_through` rules by SNI before TLS termination.
- **Certificate Authority & Hot Reload**: `crates/mio-core/src/authority.rs` persists the ECDSA P-256 CA (`O=MIO local interception`, `CN=MIO local CA`, 20-year lifetime) and optional imported intermediate CA chain (`extra_certificates`, `256 KiB` file cap, chain SHA-256 stamp) in `authority.json`. Changes sync across running proxy handles (`StaticProxyTlsConfig` avoids pinning stale CA keys) and dynamically issue 90-day SAN leaves (with trailing-dot normalization and IP SANs). Proxy intercepts `http://mio/` (`http://mio/cert`, `http://mio/cert.pem`).
- **Built-in CA Presets**: `crates/mio-core/src/ca_preset.rs` embeds PEM bundles under `crates/mio-core/assets/ca-presets/` (issuer-first chain + private key). Selecting a preset imports it through `Command::ImportCa`; the proxy serves leaves signed by that CA. Presets target clients whose trust store already contains the anchor and that do not enforce revocation/CT (e.g. old Android devices). The shipped presets derive from the October 2026 eMudhra emSign intermediate-key compromise; both intermediates are revoked and removed from current major trust stores, so acceptance is limited to stale trust stores.
- **UI/Proxy Defaults**: UI HTTP History filter defaults to `hide_connect: true`. Proxy message settings default to `unpack_compressed_responses: true`, running decompression (`gzip`/`deflate`/`br`) inside `FlowTransform` *before* plugin `http_hook`. `FlowProvenance` tracks mutations across unpack, Match & Replace, Session rules, Intercept edits, and Plugin hooks.

---

## 5. `MioAT` (`mio-cli`) Local IPC (`AiIpcServer`)

Defined in `crates/mio-core/src/service/ai_ipc.rs` and `crates/mio-cli/src/main.rs`:

- **Transport**: Unix domain socket (`0600`, UID check; falls back to `/tmp/mio-<euid>/mio.sock` if path `> 96` bytes) on macOS/Linux; loopback TCP descriptor + 256-bit token on Windows. Override via `--socket <path>` or `MIO_IPC_SOCKET`.
- **Limits**: JSON-Lines, `MAX_IPC_LINE_BYTES = 8 MiB`, `MAX_CONCURRENT_IPC_CLIENTS = 16`, standard I/O timeout `30s`, `repeater send` timeout `305s`.
- **Envelope (`AiIpcEnvelope`)**: `{ "ok": true, ...fields }` or `{ "ok": false, "error": "reason" }`.

### CLI Subcommands & Flags

Global flags: `--socket <path>`, `--json`, `help` / `-h` / `--help`.

| Subcommand | Arguments & Flags | `AiIpcRequest` Variant |
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
| `ca presets` | — | `CaPresets` |
| `ca use` | `<preset-id>` | `CaPresetUse` |
| `slim-save` | `<dest-path>` | `SlimSave` |
| `shutdown` | — | `Shutdown` |
| `serve` | `[--listen <addr>] [--export-ca <path>] [--ca-preset <preset-id>]` | Headless runtime + IPC loop |

---

## 6. Known Limitations & Defects

1. **Plugin Hook `512 KiB` / `1 MiB` Payload Ceiling & Unpack Amplification**:
   - Wire protocol `v1.5`/`v1.6` enforces `MAX_INLINE_BODY = 512 KiB` per message and `MAX_FRAME_BYTES = 1 MiB` per Protobuf `Envelope` (`WorkerSession::accept` rejects mismatched `max_frame_bytes`).
   - Prior to `v0.3.7`, oversized responses or `> 1 MiB` combined request+response frames errored inside `http_hook` and severed live client connections. Since `v0.3.7`, live/background hooks (`http_hook`, `ws_hook`, Repeater/Intruder pre-send `session_action`/`http_hook`, `passive_check`, `active_check`) cleanly bypass (`Pass`) oversized calls without interrupting traffic.
   - **Remaining gaps**: Plugins still cannot inspect or mutate `> 512 KiB` bodies; because `unpack_compressed_responses` defaults to `true` *before* `http_hook`, compressed responses `< 512 KiB` on the wire often decompress past `512 KiB` and bypass plugin inspection unless unpacking is disabled; oversized bypasses are silent (no UI counter yet).
2. **No Host-Side Hook Interest Pre-Filter**:
   - When an `http_hook` / `ws_hook` plugin is loaded, every flow `<= 512 KiB` (including irrelevant static assets unless added to `SSL Pass-Through`) pays Protobuf stdio serialization round-trips.
3. **Fail-Closed Proxy Hook Errors**:
   - In Proxy `http_hook` / `ws_hook`, a worker crash, `5s` `MAX_OPERATION` timeout, or hook error still fails the in-flight client connection; there is no per-plugin Fail-Open switch yet.
4. **TLS & Compression Stack Boundaries**:
   - `rustls` + `ring` supports TLS 1.2 and TLS 1.3 only; servers requiring legacy TLS 1.0/1.1 or non-TLS protocols on port 443 must be routed through `SSL Pass-Through`.
   - Auto-unpack decodes `gzip`, `deflate`, and `br` (Brotli) only; responses advertising unsupported encodings (e.g. `zstd`) fail decompression while `unpack_compressed_responses` is enabled.
5. **MioScript Text-Only Bodies**:
   - Embedded Rhai exposes bodies as lossy UTF-8 `String` (`body` / `raw`) and uses Rust `regex` (linear-time DFA; no lookahead/lookbehind or backreferences). Binary bodies cannot be safely round-tripped through Rhai mutation scripts.
6. **Unsigned Distribution Artifacts**:
   - macOS bundles use ad-hoc codesigning (`codesign --sign -`, no Apple notarization) and Windows executables carry no Authenticode signature.
