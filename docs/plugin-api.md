# MIO Native Plugin API Specification (Protocol `v1.4` & `mio-plugin-sdk`)

Authoritative specification generated from `crates/mio-plugin-api/src/`, `crates/mio-plugin-sdk/src/`, and `crates/mio-core/src/plugins/`.

## 1. Architecture & Scope Notice

- **Sole supported extension path**: Out-of-process **Native Worker executables** (`engine: "native"`) communicating over 4-byte length-prefixed Protobuf `Envelope` frames on `stdin` / `stdout`. Authors should use `crates/mio-plugin-sdk` (`MioPlugin` + `HostClient` + `ui::*` builders).
- **Security isolation / sandboxing is NOT in scope**: Native workers execute directly as OS child processes without OS sandboxing. `manifest.permissions` mediates MIO `HostCall` broker dispatch (`PermissionBroker`) and UI trust prompts, not OS syscall containment.
- **WASM sandbox unmaintained**: `engine: "wasm"` (`crates/mio-core/src/plugins/wasm*`) and `plugins/wasm-sandbox-lab/` are frozen legacy test fixtures and will not receive new capabilities.

---

## 2. Package Layout, Manifest Schema & CLI

A plugin package directory contains `manifest.json` + the relative native executable binary.

```json
{
  "id": "org.example.my-plugin",
  "version": "0.1.0",
  "api_version": 1,
  "engine": "native",
  "entry": "bin/my-plugin",
  "permissions": ["traffic.read", "storage.kv"],
  "contributions": ["suite_panel", "passive_check"]
}
```

Validation rules (`PluginManifest`, `#[serde(deny_unknown_fields)]` in `crates/mio-core/src/plugins/package.rs`):
- `id`: `1..=128` ASCII `[A-Za-z0-9._-]`.
- `version`: `1..=64` chars; `api_version` must equal `1` (`PROTOCOL_MAJOR`).
- `engine`: `"native"` (`"wasm"` legacy only).
- `entry`: relative non-escaping path inside package directory (`0o111` executable with valid Mach-O/ELF/PE magic).
- `permissions` (unique list from 8 exact strings):
  - `"traffic.read"` (`TrafficRead`)
  - `"draft.write"` (`DraftWrite`)
  - `"repeater.open"` (`RepeaterOpen`)
  - `"network.send"` (`NetworkSend`)
  - `"traffic.modify"` (`TrafficModify`)
  - `"session.hook"` (`SessionHook`)
  - `"payload.process"` (`PayloadProcess`)
  - `"storage.kv"` (`Storage`)
- `contributions` (non-empty list from supported identifiers):
  `"suite_panel"`, `"message_editor"`, `"message_response_editor"`, `"context_menu"`, `"http_hook"`, `"ws_hook"`, `"session_action"`, `"payload_generator"`, `"payload_processor"`, `"passive_check"`, `"active_check"` (legacy alias `"proxy_hook"` also accepted by validator).

CLI tool (`crates/mio-core/examples/mio_plugin_cli.rs`):
```sh
cargo run --release --locked -p mio-core --example mio_plugin_cli -- scaffold /tmp/my-plugin org.example.demo --native
# Build your binary into /tmp/my-plugin/worker (matching manifest.json entry)
cargo run --release --locked -p mio-core --example mio_plugin_cli -- check /tmp/my-plugin
cargo run --release --locked -p mio-core --example mio_plugin_cli -- pack /tmp/my-plugin /tmp/my-plugin.tar.gz
```

---

## 3. Wire Framing, Limits & Lifecycle (`v1.4`)

- **Framing**: `[u32 big-endian length][Protobuf Envelope bytes]`.
- **Hard Limits**:
  - `PROTOCOL_MAJOR = 1`, `PROTOCOL_MINOR = 4`
  - `MAX_FRAME_BYTES = 1 MiB` (`1024 * 1024`)
  - `MAX_INLINE_BODY = 512 KiB` (`512 * 1024`) — requests/responses/WS payloads exceeding this error rather than truncate
  - `MAX_EDITOR_BYTES = 256 KiB`
  - `MAX_NODES = 512`, `MAX_DEPTH = 24`, `MAX_TABLE_ROWS = 128`, `MAX_DRAW_COMMANDS = 2048`
  - `MAX_STORAGE_KEY_BYTES = 256`, `MAX_STORAGE_VALUE_BYTES = 64 KiB`
  - Header validation: `<= 512` headers, name `1..=256` bytes, value `<= 64 KiB`; method `1..=32` bytes; URL `1..=16 KiB`
  - Max 64 live resource handles per plugin instance; host service concurrency `16`; `5s` operation timeout
- **Envelope Header**: `major`, `minor`, `request_id` (strictly increasing per sender, non-zero), `instance_id` (16B UUID), `project_id` (16B UUID), `project_generation` (`!= 0`), `panel_id` (`PANEL_ID = 1`), `revision`, `deadline_unix_ms`, `in_reply_to`, `body`.
- **Handshake (`WorkerSession::accept`, handled automatically by `mio_plugin_sdk::run_stdio`)**:
  1. Host -> Worker: `Hello(max_frame_bytes, max_operation_ms, granted_permissions)`
  2. Worker -> Host (`request_id = 1`, `in_reply_to = hello.request_id`): `Register(plugin_id, plugin_version, required_components, panel)`
  3. Host -> Worker: `Ready` (`in_reply_to = 1`)
  4. Runtime loop: dispatch incoming envelopes until `Shutdown`.

---

## 4. Extension Hooks (`envelope::Body` Tags 20–53)

| Hook Contribution | Host Request (`Body` & tag) | Worker Reply (`Body` & tag) | Exact Semantics |
|---|---|---|---|
| Handshake / Lifecycle | `Hello`(20), `Ready`(22), `Cancel`(31), `Shutdown`(32), `Resync`(34) | `Register`(21), `Ack`(35), `Error`(33) | Strict `in_reply_to` correlation & deadlines |
| `suite_panel` | `UiEvent`(25) | `Snapshot`(23) / `Patch`(24) / `Ack`(35) | Top-level `42pt` workbench tab with native declarative tree |
| `message_editor` | `OpenRequest`(26), `CloseRequest`(36), `CommitEditor`(27) | `Snapshot`(23) / `Patch`(24), `Committed`(28) | Custom request tab (Proxy / Repeater / Target / Organiser / external archive); `Send` waits for `Committed` |
| `message_response_editor` | `OpenResponse`(45), `CommitResponseEditor`(46) | `Snapshot`(23) / `Patch`(24), `ResponseCommitted`(47) | Custom response tab |
| `context_menu` | `ContextMenuCall`(48) | `ContextMenuResult`(49) | Passes `source`, `action_id`, selection range/bytes, `request`, `response`; supports `status_message`, `log_line`, `open_repeater`, `replace_request` (raw bytes) |
| `http_hook` | `HttpHookCall`(37) | `HttpHookResult`(38) | Request & response (`response_stage`, `response_head_only`) interception returning `HookAction::{Pass=0, Modify=1, Intercept=2, Drop=3}` |
| `ws_hook` | `WebSocketHookCall`(39) | `WebSocketHookResult`(40) | Frame hook (`client_to_server`, `is_binary`, `payload`) returning `HookAction` + modified `payload` |
| `session_action` | `SessionActionCall`(41) | `SessionActionResult`(42) | Session rule macro/signer (`modified`, optional `request`) |
| `payload_generator` | `PayloadCall(Op=Generate)`(43) | `PayloadResult`(44) | `config`, `max_items` -> `outputs: Vec<Vec<u8>>` (`<= 10_000` items, `<= 512 KiB` total) |
| `payload_processor` | `PayloadCall(Op=Process)`(43) | `PayloadResult`(44) | `config`, `inputs` -> `outputs` |
| `passive_check` | `PassiveCheckCall`(50) | `PassiveCheckResult`(51) | Returns `findings: Vec<PluginFindingDto>` (`<= 64`) + `log_line` |
| `active_check` | `ActiveCheckCall`(52) | `ActiveCheckResult`(53) | Correlated by `call_id`; receives `base_request`, `base_response`, `parameter_name`, `parameter_value` |

### Core DTOs (`crates/mio-plugin-api/src/protocol/dto.rs` & `hooks.rs`)

- **`RequestDto` (`HttpRequestDto`)**: `{ method, url, version: HttpVersionDto (0=Http10,1=Http11,2=Http2), headers: [HeaderDto { name, value: bytes }], body: bytes, trailers: [HeaderDto] }`
- **`ResponseDto` (`HttpResponseDto`)**: `{ status: 100..=599, version, headers, body, trailers }`
- **`PluginFindingDto`**: `{ title (<=512), detail (<=8192), remediation (<=4096), severity (<=64), confidence (<=64), parameter (<=512), evidence (<=4096) }` (`severity`: `Information`/`Low`/`Medium`/`High`; `confidence`: `Tentative`/`Firm`/`Certain`)

---

## 5. Declarative Panel UI (`crates/mio-plugin-api/src/panel.rs` & `validators.rs`)

- **`Node`**: `{ id (u64 != 0, unique), accessibility_label (1..=1024), layout?: { min_width, min_height, flex, padding, gap }, kind }`
- **14 `node::Kind` Variants (`tags = 10..=23`)**:
  - Containers: `Row(10)`, `Column(11)`, `ScrollColumn(19)`, `Split(12)` (`Axis`, `fraction: 0.05..=0.95`, `first`, `second`), `Tabs(13)` (`Axis`, `active` child node id, non-empty `tabs: [{ title, content }]`)
  - Widgets: `Text(14)` (`text <=16KiB`, `Tone`), `Button(15)` (`label`, `enabled`, `commit_editors: bool`), `Editor(16)` (`document_id != 0`, `revision != 0`, `contents <=256KiB`, `EditorFormat::{Utf8=0, Binary=1}`, `read_only`), `VirtualTable(17)` (`columns <=32`, `total_rows`, `offset`, `viewport_revision`, `rows <=128`), `Canvas(18)` (`width`, `height`, `commands <=2048` `[Rect, Path, Text, Image PNG<=256KiB & <=2048x2048, PushClip, PopClip, PushTransform, PopTransform]`, `hit_regions <=256`), `Checkbox(20)`, `Select(21)` (`options: 1..=128`, `selected_index`), `Badge(22)`, `Progress(23)` (`percent: 0..=100`)
- **`Tone`**: `Foreground=0`, `Muted=1`, `Accent=2`, `Error=3`, `Background=4`, `Border=5`, `Selection=6`
- **`PanelPatch`**: `{ base_revision, revision, operations: [PatchOperation { node_id, change: Replace(Node) | Editor(EditorUpdate) | Table(VirtualTable) | Canvas(Canvas) }] }`
- **12 `ui_event::Event` Variants**: `Click(2)`, `EditorChanged(3)`, `TableSelection(4)`, `Viewport(5)`, `Pointer(6)`, `Key(7)`, `Focus(8)`, `TabSelected(9)`, `SplitResized(10)`, `Filter(11)`, `Toggled(13)`, `SelectedOption(14)`. `UiEvent.editors` attaches fresh IME-committed editor snapshots when `Button.commit_editors == true`.

---

## 6. All 19 `HostCall` Operations (`v1.4`, Worker -> Host)

Worker sends `Envelope(Body::HostCall(call))`; host replies `Envelope(Body::HostResult(res))` with `in_reply_to`:

| `host_call::Operation` (tag) | `mio-plugin-sdk` Method (`HostClient`) | Required `Permission` | `host_result::Outcome` (tag) | Notes / Limits |
|---|---|---|---|---|
| `QueryHistory(HistoryQuery)` (1) | `query_history(offset, limit, filter)` | `traffic.read` | `History(HistoryPage)` (1) | `1 <= limit <= 128`, `filter <= 1024B` |
| `ReadFlow(16B UUID)` (2) | `read_flow(flow_id)` | `traffic.read` | `Record(RecordDto)` (2) | Loads full `FlowRecord` |
| `UpdateDraft(DraftUpdate)` (3) | `update_draft(id, expected_rev, req)` | `draft.write` | `Resource(ResourceRevision)` (4) | Optimistic revision check |
| `OpenRepeater(RequestDto)` (4) | `open_repeater(req)` | `repeater.open` | `Resource(ResourceRevision)` (4) | Opens visible Repeater tab |
| `OpenIntruder(RequestDto)` (8) | `open_intruder(req)` | `repeater.open` | `Done` (5) | Sends request to Intruder |
| `NetworkSend(RequestDto)` (5) | `network_send(req)` | `network.send` | `Response(ResponseDto)` (3) | Sends HTTP request via host stack |
| `StorageGet(key)` (6) / `StorageSet(StorageEntry)` (7) | `storage_get` / `storage_set` | `storage.kv` | `StorageValue` (7) / `Done` (5) | Per-project KV (`key<=256B`, `val<=64KiB`) |
| `PreferencesGet(key)` (9) / `PreferencesSet(StorageEntry)` (10) | `preferences_get` / `preferences_set` | `storage.kv` | `StorageValue` (7) / `Done` (5) | Cross-project prefs (`plugin-preferences.json`) |
| `ScopeCheck(url)` (11) | `is_in_scope(url)` | `traffic.read` | `ScopeStatus(ScopeStatusDto)` (8) | Tests Target Scope (`url <= 16KiB`) |
| `ScopeUpdate(ScopeUpdateDto)` (12) | `add_scope_include` / `add_scope_exclude` | `traffic.modify` | `Done` (5) | Adds enabled Target Scope prefix rule (`<=2048B`) |
| `QuerySiteMap(SiteMapQueryDto)` (13) | `query_site_map(prefix, in_scope_only, offset, limit)` | `traffic.read` | `SiteMap(SiteMapPageDto)` (9) | Paginated Site Map items (`1<=limit<=128`) |
| `ReadAnnotations(16B UUID)` (14) | `read_annotations(flow_id)` | `traffic.read` | `Annotations(AnnotationsDto)` (10) | Notes + 0..=9 highlight color + revisions |
| `UpdateAnnotations(UpdateAnnotationsDto)` (15) | `update_annotations(flow_id, notes, color)` | `draft.write` | `Annotations(AnnotationsDto)` (10) | Persists notes (`<=64KiB`) / color (`0..=9`) |
| `OpenOrganiser(OpenOrganiserDto)` (16) | `open_organiser(req, resp, notes, color)` | `repeater.open` | `Resource(ResourceRevision)` (4) | Appends item to Organiser & returns ID + number |
| `SubmitAuditIssue(SubmitAuditIssueDto)` (17) | `submit_audit_issue(flow_id, url, finding)` | `traffic.read` | `Done` (5) | Merges standalone finding (`0x00900102`) into Dashboard/Target |
| `CollaboratorGenerate(CollaboratorGenerateDto)` (18) | `collaborator_generate(label)` | `network.send` | `CollaboratorPayload(CollaboratorPayloadDto)` (11) | Starts local OAST if idle & issues HTTP/DNS payload |
| `CollaboratorPoll(CollaboratorPollDto)` (19) | `collaborator_poll(token_filter, limit)` | `network.send` | `CollaboratorInteractions(CollaboratorInteractionsDto)` (12) | Drains & returns observed OAST interactions (`<=128`) |

`ErrorCode`: `Invalid=0`, `Denied=1`, `Stale=2`, `Cancelled=3`, `Deadline=4`, `Internal=5`.

---

## 7. `mio-plugin-sdk` Usage & Minimal Worker Skeleton

`crates/mio-plugin-sdk` handles handshake, request correlation, `deferred` interleaved-frame buffering during synchronous `HostCall`s, and clean shutdown.

`Cargo.toml`:
```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[dependencies]
mio-plugin-sdk = { path = "/path/to/MIO/crates/mio-plugin-sdk" }
```

`src/main.rs`:
```rust
use std::io;
use mio_plugin_sdk::{HostClient, MioPlugin, protocol::*, run_stdio, ui};

struct MyPlugin;

impl MioPlugin for MyPlugin {
    fn id(&self) -> &str { "org.example.my-plugin" }

    fn on_passive_check(
        &mut self,
        ctx: &mut dyn HostClient,
        call: PassiveCheckCall,
    ) -> Result<PassiveCheckResult, String> {
        let mut findings = Vec::new();
        if ctx.is_in_scope(&call.url).unwrap_or(false)
            && call.response.as_ref().is_some_and(|r| r.body.windows(6).any(|w| w == b"SECRET"))
        {
            findings.push(ui::finding(
                "Exposed Secret", "High", "Certain", "Response leaked SECRET",
                "Remove debug secret", "body", "SECRET",
            ));
        }
        Ok(PassiveCheckResult { findings, log_line: format!("checked {}", call.url) })
    }
}

fn main() -> io::Result<()> {
    run_stdio(MyPlugin)
}
```
