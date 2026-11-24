# MIO Native Plugin API Specification (`v1.6` & `mio-plugin-sdk`)

Authoritative specification matching `crates/mio-plugin-api/src/`, `crates/mio-plugin-sdk/src/`, and `crates/mio-core/src/plugins/`.

## 1. Extension Model & Non-Goals

- **Sole extension path**: Out-of-process **Native Worker executables** (`engine: "native"`) over 4-byte big-endian length-prefixed Protobuf `Envelope` streams on `stdin` / `stdout`. Authors should use `crates/mio-plugin-sdk` (`MioPlugin` + `HostClient` + `ui::*` builders).
- **No OS sandboxing**: Workers execute as normal child processes; `manifest.permissions` enforces host API broker authorization and UI consent dialogs, not OS syscall containment.

---

## 2. Package Layout, Manifest Schema & CLI

Directory contains `manifest.json` + relative native executable binary.

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

Manifest validation (`PluginManifest`, `#[serde(deny_unknown_fields)]` in `crates/mio-core/src/plugins/package.rs`):
- `id`: `1..=128` ASCII `[A-Za-z0-9._-]`.
- `version`: `1..=64` chars; `api_version` == `1` (`PROTOCOL_MAJOR`).
- `engine`: `"native"`.
- `entry`: relative non-escaping path inside package directory (`0o111` executable with valid Mach-O/ELF/PE magic).
- `permissions` (unique subset from **9** exact strings):
  `"traffic.read"` (`TrafficRead`), `"draft.write"` (`DraftWrite`), `"repeater.open"` (`RepeaterOpen`), `"network.send"` (`NetworkSend`), `"traffic.modify"` (`TrafficModify`), `"session.hook"` (`SessionHook`), `"payload.process"` (`PayloadProcess`), `"storage.kv"` (`Storage`), `"audit.write"` (`AuditWrite`).
- `contributions` (non-empty subset from **11** exact strings):
  `"suite_panel"`, `"message_editor"`, `"message_response_editor"`, `"context_menu"`, `"http_hook"`, `"ws_hook"`, `"session_action"`, `"payload_generator"`, `"payload_processor"`, `"passive_check"`, `"active_check"`.

CLI helper (`crates/mio-core/examples/mio_plugin_cli.rs`):
```sh
CARGO_INCREMENTAL=0 cargo run --release --locked -p mio-core --example mio_plugin_cli -- scaffold /tmp/my-plugin org.example.demo --native
CARGO_INCREMENTAL=0 cargo run --release --locked -p mio-core --example mio_plugin_cli -- check /tmp/my-plugin
CARGO_INCREMENTAL=0 cargo run --release --locked -p mio-core --example mio_plugin_cli -- pack /tmp/my-plugin /tmp/my-plugin.tar.gz
```

---

## 3. Wire Framing, Limits, Backpressure & Lifecycle (`v1.6`)

- **Framing**: `[u32 big-endian length][Protobuf Envelope bytes]`.
- **Protocol Negotiation**: Host attempts `PROTOCOL_MAJOR = 1`, `PROTOCOL_MINOR = 6`; if startup fails and manifest does not require `audit.write`, `passive_check`, or `active_check`, host automatically falls back once to `LEGACY_COMPAT_MINOR = 5`.
- **Size Limits**:
  - `MAX_FRAME_BYTES = 1 MiB` (`1024 * 1024`) — total encoded `Envelope` limit verified during `WorkerSession::accept` (`limits.max_frame_bytes == MAX_FRAME_BYTES`).
  - `MAX_INLINE_BODY = 512 KiB` (`512 * 1024`) per `RequestDto`/`ResponseDto` body, WS payload, `ContextMenuCall.selected_bytes`, `ContextMenuResult.replace_request`, or `PayloadCall`/`PayloadResult` batch total.
  - `MAX_EDITOR_BYTES = 256 KiB`, `MAX_NODES = 512`, `MAX_DEPTH = 24`, `MAX_TABLE_ROWS = 128`, `MAX_DRAW_COMMANDS = 2048`.
  - `MAX_STORAGE_KEY_BYTES = 256`, `MAX_STORAGE_VALUE_BYTES = 64 KiB`.
  - HTTP DTOs: `<= 512` headers/trailers, header name `1..=256B`, header value `<= 64 KiB`, method `1..=32B`, URL `1..=16 KiB`.
- **Overflow Handling (Critical Distinction)**:
  - **Live/background hooks (`http_hook`, `ws_hook`, pre-send `session_action`/`http_hook`, `passive_check`, `active_check`)**: Host runs O(1) pre-checks (`http_request_fits_inline_fast` / `http_response_fits_inline_fast`) plus worst-case `Envelope` validation via `protocol::try_into_wire_body` before dispatch. Payloads `> 512 KiB`, header/URL overflows, or combined frames `> 1 MiB` **cleanly bypass (`Pass`)** that plugin hook without failing traffic.
  - **Explicit RPCs (`OpenRequest`, `OpenResponse`, `CommitEditor`, `CommitResponseEditor`, `ContextMenuCall`, `ReadFlow`, `NetworkSend` response)**: Rejected items return a correlated error without truncating payloads (exception: `CollaboratorInteractions` truncates individual `raw_request` captures at `MAX_INLINE_BODY`).
- **Concurrency & Rate Limits**:
  - Control queue `QUEUE_CAPACITY = 128`, with **`HOOK_CONCURRENCY_LIMIT = 96`** semaphore backpressure in `PluginHandle` (reserves 32 slots for interactive UI/Open/Commit/Close).
  - Unsolicited worker -> host frames capped at `512 frames/sec`; **correlated `Hook` replies** (`HttpHookResult`, `WebSocketHookResult`, etc. matching an active `PendingKind::Hook`) are excluded from this counter so high-throughput proxy/WS traffic does not terminate healthy workers.
  - `MAX_OPERATION = 5s` per operation; `64` live handles on `PANEL_ID = 1`, `128` across multiplexed child panels, `MAX_MULTIPLEXED_PANELS = 64` per worker, `MAX_CHILD_EDITORS = 64` project-wide (`panel_id >= 2`), `MAX_SERVICE_CALLS = 16`.
  - Outbound control calls (`dispatch_hook`, `start_open`, `dispatch_commit_*`) validate via `preview_envelope_for` before advancing `outgoing_sequence`.
- **Handshake (`WorkerSession::accept` / `mio_plugin_sdk::run_stdio`)**:
  1. Host -> Worker: `Hello(max_frame_bytes = 1 MiB, max_operation_ms = 5000, granted_permissions, host_minor)`
  2. Worker -> Host (`request_id = 1`, `in_reply_to = hello.request_id`): `Register(plugin_id, plugin_version, required_components, panel, supports_panel_multiplexing)`
  3. Host -> Worker: `Ready` (`in_reply_to = 1`) -> dispatch loop by `panel_id` until `Shutdown`.

---

## 4. Extension Hooks (`envelope::Body` Tags 20–53)

| Hook Contribution | Host Request (`Body` & tag) | Worker Reply (`Body` & tag) | Exact Semantics |
|---|---|---|---|
| Lifecycle / Recovery | `Hello`(20), `Ready`(22), `Cancel`(31), `Shutdown`(32), `Resync`(34) | `Register`(21), `Ack`(35), `Error`(33) | Strict `in_reply_to` correlation & `5s` deadlines |
| `suite_panel` | `UiEvent`(25) | `Snapshot`(23) / `Patch`(24) / `Ack`(35) | Top-level workbench tab with native declarative tree |
| `message_editor` | `OpenRequest`(26), `CloseRequest`(36), `CommitEditor`(27) | `Snapshot`(23) / `Patch`(24), `Committed`(28), `Ack`(35) | Custom request tab (`CloseRequest.destroy_panel`: `false` = revoke resource, `true` = teardown child panel via `on_panel_destroy`) |
| `message_response_editor` | `OpenResponse`(45), `CloseRequest`(36), `CommitResponseEditor`(46) | `Snapshot`(23) / `Patch`(24), `ResponseCommitted`(47), `Ack`(35) | Custom response tab (supports multiplexed `panel_id >= 2`) |
| `context_menu` | `ContextMenuCall`(48) | `ContextMenuResult`(49) | Passes `source`, `action_id`, selection range/bytes, `request`, `response`; supports `status_message`, `log_line`, `open_repeater`, `replace_request` |
| `http_hook` | `HttpHookCall`(37) | `HttpHookResult`(38) | Request & response (`response_stage`, `response_head_only`) returning `HookAction::{Pass=0, Modify=1, Intercept=2, Drop=3}`; auto-bypassed if `> 512 KiB` / `> 1 MiB` frame |
| `ws_hook` | `WebSocketHookCall`(39) | `WebSocketHookResult`(40) | Frame hook (`client_to_server`, `is_binary`, `payload`); auto-bypassed if `payload > 512 KiB` |
| `session_action` | `SessionActionCall`(41) | `SessionActionResult`(42) | Session rule macro/signer (`modified`, optional `request`); pre-send auto-bypassed if oversized |
| `payload_generator` | `PayloadCall(Generate)`(43) | `PayloadResult`(44) | `config`, `max_items` -> `outputs: Vec<Vec<u8>>` (`<= 10_000` items, `<= 512 KiB` total) |
| `payload_processor` | `PayloadCall(Process)`(43) | `PayloadResult`(44) | `config`, `inputs` -> `outputs` |
| `passive_check` | `PassiveCheckCall`(50) | `PassiveCheckResult`(51) | Returns `findings: Vec<PluginFindingDto>` (`<= 64`) + `log_line`; skipped if `> 512 KiB` / `> 1 MiB` frame |
| `active_check` | `ActiveCheckCall`(52) | `ActiveCheckResult`(53) | Receives `flow_id`, `base_request`, `base_response`, `parameter_name`, `parameter_value`; skipped if `> 512 KiB` / `> 1 MiB` frame |

Core DTOs (`crates/mio-plugin-api/src/protocol/dto.rs`, `hooks.rs`):
- `RequestDto` (`HttpRequestDto`): `{ method, url, version: HttpVersionDto (0=Http10,1=Http11,2=Http2), headers: [HeaderDto { name, value: bytes }], body: bytes, trailers: [HeaderDto] }`
- `ResponseDto` (`HttpResponseDto`): `{ status: 100..=599, version, headers, body, trailers }`
- `PluginFindingDto`: `{ title (<=512), detail (<=8192), remediation (<=4096), severity: FindingSeverity (0..=3), confidence: FindingConfidence (0..=2), parameter (<=512), evidence (<=4096) }`

---

## 5. Declarative Panel UI (`crates/mio-plugin-api/src/panel.rs`, `validators.rs`)

- **`Node`**: `{ id (u64 != 0, unique), accessibility_label (1..=1024), layout?: { min_width, min_height, flex, padding, gap }, kind }`
- **14 `node::Kind` Variants (`tags = 10..=23`)**:
  - Containers: `Row(10)`, `Column(11)`, `ScrollColumn(19)`, `Split(12)` (`Axis`, `fraction: 0.05..=0.95`, `first`, `second`), `Tabs(13)` (`Axis`, `active` child node id, non-empty `tabs: [{ title, content }]`)
  - Widgets: `Text(14)` (`text <=16KiB`, `Tone`), `Button(15)` (`label`, `enabled`, `commit_editors`), `Editor(16)` (`document_id != 0`, `revision != 0`, `contents <=256KiB`, `EditorFormat::{Utf8=0, Binary=1}`, `read_only`), `VirtualTable(17)` (`columns <=32`, `total_rows`, `offset`, `viewport_revision`, `rows <=128`), `Canvas(18)` (`width`, `height`, `commands <=2048` `[Rect, Path, Text, Image PNG<=256KiB & <=2048x2048, PushClip, PopClip, PushTransform, PopTransform]`, `hit_regions <=256`), `Checkbox(20)`, `Select(21)` (`options: 1..=128`, `selected_index`), `Badge(22)`, `Progress(23)` (`percent: 0..=100`)
- **`Tone`**: `Foreground=0`, `Muted=1`, `Accent=2`, `Error=3`, `Background=4`, `Border=5`, `Selection=6`
- **`PanelPatch`**: `{ base_revision, revision, operations: [PatchOperation { node_id, change: Replace(Node) | Editor(EditorUpdate) | Table(VirtualTable) | Canvas(Canvas) }] }`
- **12 `ui_event::Event` Variants**: `Click(2)`, `EditorChanged(3)`, `TableSelection(4)`, `Viewport(5)`, `Pointer(6)`, `Key(7)`, `Focus(8)`, `TabSelected(9)`, `SplitResized(10)`, `Filter(11)`, `Toggled(13)`, `SelectedOption(14)` (`UiEvent.editors` attaches IME-committed editor snapshots when `Button.commit_editors == true`).

---

## 6. All 19 `HostCall` Operations (`v1.6`, Worker -> Host)

`HostCall.parent_request_id` (tag 20, set automatically by `mio-plugin-sdk`) must reference an active, uncancelled request on the same `panel_id`, inherits the parent deadline, and cascades cancellation on timeout/revoke/`CancelPanelOp`. During `MioPlugin::on_cancel` on a **live** panel (closing panels reject `HostCall`s as `Stale`), an allowlisted cleanup window (`<= MAX_OPERATION`, closed on `Cancel` `Ack`/`Error` or legacy `Snapshot`/`Patch`) permits `QueryHistory`, `ReadFlow`, `ScopeCheck`, `QuerySiteMap`, `ReadAnnotations`, `StorageGet`, `StorageSet`, `PreferencesGet`, and `PreferencesSet`, while side-effect calls fail as `ErrorCode::Cancelled`:

| `host_call::Operation` (tag) | `HostClient` Method | Required `Permission` | `host_result::Outcome` (tag) | Notes / Limits |
|---|---|---|---|---|
| `QueryHistory(HistoryQuery)` (1) | `query_history(offset, limit, filter)` | `traffic.read` | `History(HistoryPage)` (1) | `1 <= limit <= 128`, `filter <= 1024B` |
| `ReadFlow(16B UUID)` (2) | `read_flow(flow_id)` | `traffic.read` | `Record(RecordDto)` (2) | Fails if record > inline/frame limits |
| `UpdateDraft(DraftUpdate)` (3) | `update_draft(id, expected_rev, req)` | `draft.write` | `Resource(ResourceRevision)` (4) | Optimistic revision check |
| `OpenRepeater(RequestDto)` (4) | `open_repeater(req)` | `repeater.open` | `Resource(ResourceRevision)` (4) | Opens Repeater tab |
| `OpenIntruder(RequestDto)` (8) | `open_intruder(req)` | `repeater.open` | `Done` (5) | Sends request to Intruder |
| `NetworkSend(RequestDto)` (5) | `network_send(req)` | `network.send` | `Response(ResponseDto)` (3) | Sends via host HTTP stack (concurrency `<= 16`) |
| `StorageGet` (6) / `StorageSet` (7) | `storage_get` / `storage_set` | `storage.kv` | `StorageValue` (7) / `Done` (5) | Per-project KV (`key<=256B`, `val<=64KiB`) |
| `PreferencesGet` (9) / `PreferencesSet` (10) | `preferences_get` / `preferences_set` | `storage.kv` | `StorageValue` (7) / `Done` (5) | Cross-project (`plugin-preferences.json`) |
| `ScopeCheck(url)` (11) | `is_in_scope(url)` | `traffic.read` | `ScopeStatus(ScopeStatusDto)` (8) | `url <= 16 KiB` |
| `ScopeUpdate(ScopeUpdateDto)` (12) | `add_scope_include` / `add_scope_exclude` | `traffic.modify` | `Done` (5) | Enabled prefix rule (`<= 2048B`) |
| `QuerySiteMap(SiteMapQueryDto)` (13) | `query_site_map(prefix, in_scope_only, offset, limit)` | `traffic.read` | `SiteMap(SiteMapPageDto)` (9) | `1 <= limit <= 128` |
| `ReadAnnotations(16B UUID)` (14) | `read_annotations(flow_id)` | `traffic.read` | `Annotations(AnnotationsDto)` (10) | Notes + 0..=9 color + revisions |
| `UpdateAnnotations(UpdateAnnotationsDto)` (15) | `update_annotations(flow_id, notes, color)` | `draft.write` | `Annotations(AnnotationsDto)` (10) | `notes <= 64 KiB`, `color <= 9` |
| `OpenOrganiser(OpenOrganiserDto)` (16) | `open_organiser(req, resp, notes, color)` | `repeater.open` | `Resource(ResourceRevision)` (4) | Appends Organiser item |
| `SubmitAuditIssue(SubmitAuditIssueDto)` (17) | `submit_audit_issue(flow_id, url, finding)` | `audit.write` | `Done` (5) | Merges finding (`0x00900102`) into Dashboard/Target |
| `CollaboratorGenerate(CollaboratorGenerateDto)` (18) | `collaborator_generate(label)` | `network.send` | `CollaboratorPayload(CollaboratorPayloadDto)` (11) | Starts local OAST if idle & issues token |
| `CollaboratorPoll(CollaboratorPollDto)` (19) | `collaborator_poll(token_filter, limit)` | `network.send` | `CollaboratorInteractions(CollaboratorInteractionsDto)` (12) | Drains observed OAST events (`<= 128`) |

`ErrorCode`: `Invalid=0`, `Denied=1`, `Stale=2`, `Cancelled=3`, `Deadline=4`, `Internal=5`.

---

## 7. Minimal `mio-plugin-sdk` Worker

`Cargo.toml` (`edition = "2024"`): depend on `mio-plugin-sdk = { path = "crates/mio-plugin-sdk" }`. `src/main.rs`:

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
            findings.push(
                ui::finding("Exposed Secret")
                    .severity(FindingSeverity::High)
                    .confidence(FindingConfidence::Certain)
                    .detail("Response leaked SECRET")
                    .remediation("Remove debug secret")
                    .parameter("body")
                    .evidence("SECRET")
                    .into(),
            );
        }
        Ok(PassiveCheckResult { findings, log_line: format!("checked {}", call.url) })
    }
}

fn main() -> io::Result<()> {
    run_stdio(MyPlugin)
}
```

---

## 8. Protocol Limitations & Known Gaps

1. **No `> 512 KiB` Inline Inspection in `v1.5`/`v1.6`**: Compiled workers validate `limits.max_frame_bytes == 1 MiB` during `WorkerSession::accept`. Existing hooks therefore cannot read or modify bodies exceeding `512 KiB` (or combined `Envelope`s exceeding `1 MiB`) without a future protocol revision (`v1.7+` head-only `body_omitted`, streaming chunks, or shared file handles).
2. **Interaction with `unpack_compressed_responses`**: Proxy `FlowTransform` decompresses `gzip`/`deflate`/`br` responses before calling `http_hook`; small compressed payloads that expand beyond `512 KiB` bypass `http_hook`.
3. **All-Traffic IPC Overhead**: Host does not yet support declarative host/path/MIME pre-filtering per plugin before serializing `HttpHookCall` / `WebSocketHookCall`.
4. **Silent Overflow Bypass & Fail-Closed Proxy Errors**: Oversized hook bypasses do not yet surface counters in the Extensions UI; runtime crashes, panics, or `5s` timeouts in `http_hook`/`ws_hook` fail the active proxy flow rather than falling back to pass-through.
