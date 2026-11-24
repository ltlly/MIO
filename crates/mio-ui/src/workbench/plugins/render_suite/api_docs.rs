use mio_core::protocol::{
    MAX_FRAME_BYTES, MAX_INLINE_BODY, MAX_STORAGE_KEY_BYTES, MAX_STORAGE_VALUE_BYTES, PANEL_ID,
    PROTOCOL_MAJOR, PROTOCOL_MINOR, Permission,
};

use super::*;

impl Workbench {
    #[inline(never)]
    pub(super) fn render_extensions_apis(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let zebra_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();

        let api_rows: [(&str, &str); 19] = [
            ("Engine: native", "本地 Native worker 进程（不受 OS 沙箱限制，需用户显式信任）"),
            ("Contribution: suite_panel", "在顶部工具栏注册独立扩展标签页视图"),
            ("Contribution: message_editor", "在 Repeater / Proxy Intercept / History 注册自定义请求编辑器"),
            ("Contribution: message_response_editor", "注册自定义 HTTP 响应查看与编辑视图"),
            ("Contribution: context_menu", "对当前活动请求/响应执行自定义上下文动作"),
            ("Contribution: http_hook", "在 Proxy / Repeater / Intruder 收发阶段读写或改写 HTTP 请求/响应"),
            ("Contribution: ws_hook", "在 Proxy WebSocket 收发阶段读写、篡改或拦截帧载荷"),
            ("Contribution: session_action", "在会话规则链执行请求预处理/签名脚本"),
            ("Contribution: payload_generator / processor", "为 Intruder 提供自定义载荷生成与处理规则"),
            ("Contribution: passive_check / active_check", "参与实时被动审计与主动扫描并上报漏洞发现"),
            ("Protocol v1.5+: panel_multiplexing", "在单个 worker 进程内按 Envelope.panel_id (>= 2) 复用多个子编辑器标签（需在 Register 设置 supports_panel_multiplexing）"),
            ("Protocol v1.7+: large_body", "在 Register 设置 supports_large_body=true 后，>512 KiB 请求/响应/WS 载荷自动通过 body_omitted + 只读共享溢出文件传递"),
            ("Host Hook Pre-Filter & Fail-Open", "支持按插件配置 Scope/Host/Path 白黑名单、阶段与静态资源过滤，以及指标统计和 Fail-Open/Fail-Closed 策略"),
            ("Permission: traffic.read", permission_label(Permission::TrafficRead)),
            ("Permission: draft.write", permission_label(Permission::DraftWrite)),
            ("Permission: repeater.open", permission_label(Permission::RepeaterOpen)),
            ("Permission: network.send", permission_label(Permission::NetworkSend)),
            ("Permission: traffic.modify / audit.write", "修改实时流量、Target Scope 与向审计仪表盘提交漏洞发现"),
            ("Permission: session.hook / payload / kv", "支持会话钩子、载荷处理与项目/全局持久化键值存储"),
        ];

        let mut table = div()
            .w_full()
            .border_1()
            .border_color(table_border)
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(24.))
                    .w_full()
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(table_border)
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .text_color(header_fg)
                    .child(
                        div()
                            .w(px(260.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("Capability"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child("Description"),
                    ),
            );

        for (index, (cap, desc)) in api_rows.into_iter().enumerate() {
            table = table.child(
                div()
                    .min_h(px(24.))
                    .w_full()
                    .bg(if index % 2 == 1 {
                        zebra_bg
                    } else {
                        theme::surface()
                    })
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child(
                        div()
                            .w(px(260.))
                            .flex_shrink_0()
                            .px(px(8.))
                            .py(px(3.))
                            .font_family(theme::mono_font_family())
                            .text_size(px(12.))
                            .truncate()
                            .child(cap),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(8.))
                            .py(px(3.))
                            .child(desc),
                    ),
            );
        }

        div()
            .id("extensions-apis-view")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(format!(
                        "MIO Extensions API (v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR})"
                    )),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("MIO 扩展协议支持本地 Native worker，每项宿主 API 权限均需由用户逐项显式授予。"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(px(240.)).child(
                            mio_neutral_button(
                                "extensions-apis-save-files",
                                "导出接口定义文件… (Save interface files)",
                                "导出 mio-plugin-api 清单 Schema 与 Protobuf/Host 协议 JSON 参考文件",
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export_plugin_api_interface_files(false, window, cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(184.)).child(
                            mio_neutral_button(
                                "extensions-apis-save-md",
                                "导出 Markdown 接口文档…",
                                "导出可读的 Markdown 格式 mio-plugin-api 扩展开发参考文档",
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export_plugin_api_interface_files(true, window, cx);
                            })),
                        ),
                    ),
            )
            .child(table)
            .into_any_element()
    }
}

pub(in crate::workbench::plugins) fn build_mio_plugin_api_json() -> Result<Vec<u8>, serde_json::Error> {
    let doc = serde_json::json!({
        "name": "mio-plugin-api",
        "sdk_crate": "mio-plugin-sdk",
        "security_scope": "security isolation / sandboxing is explicitly out of scope for native workers; manifest permissions mediate MIO host API access only",
        "protocol": {
            "major": PROTOCOL_MAJOR,
            "minor": PROTOCOL_MINOR,
            "compat_minors": [6, 5],
            "default_panel_id": PANEL_ID,
            "framing": "4-byte big-endian length prefix + Protobuf Envelope",
            "max_frame_bytes": MAX_FRAME_BYTES,
            "max_inline_body_bytes": MAX_INLINE_BODY,
            "max_spill_body_bytes": mio_core::protocol::MAX_SPILL_BODY_BYTES,
            "max_storage_key_bytes": MAX_STORAGE_KEY_BYTES,
            "max_storage_value_bytes": MAX_STORAGE_VALUE_BYTES,
            "hook_concurrency_limit": 96,
            "large_body_support": "Workers setting Register.supports_large_body = true (v1.7+) receive body_omitted / payload_omitted + read-only temp spill handles for >512 KiB bodies or combined frames >1 MiB; modified spill outputs are validated inside the per-call spill directory",
            "host_hook_prefilter_and_policy": "Host-side per-plugin scope/host/path/stage/static-asset pre-filtering, atomic hook metrics (invoked, filtered_skipped, oversized_bypassed, errors_or_timeouts, fail_open_passes), and Fail-Open / Fail-Closed failure policy",
            "oversized_hook_policy": "Legacy v1.5/v1.6 workers and plugins with supports_large_body=false cleanly bypass payloads >512 KiB or combined frames >1 MiB without failing traffic",
            "known_limitations": [
                "Workers on v1.5/v1.6 or without supports_large_body=true cannot inspect/modify >512 KiB inline bodies",
                "Host/worker synchronous message editors (OpenRequest/CommitEditor) still enforce the 512 KiB inline payload cap"
            ]
        },
        "panel_multiplexing": {
            "supported_since_minor": 5,
            "register_field": "supports_panel_multiplexing",
            "envelope_field": "panel_id",
            "close_scope_field": "CloseRequest.destroy_panel",
            "host_call_parent_field": "HostCall.parent_request_id",
            "on_cancel_host_call_policy": "On live panels, allowlisted cleanup calls (QueryHistory, ReadFlow, ScopeCheck, QuerySiteMap, ReadAnnotations, StorageGet, StorageSet, PreferencesGet, PreferencesSet) bypass transient revision drift during the Cancel window (scheduling Resync only for active non-Resync Cancel cleanups when the callback envelope revision exceeds the host revision), while mutating/side-effect calls fail as Cancelled and closing panels reject HostCalls as Stale; window ends on Cancel Ack/Error (or legacy Snapshot/Patch)",
            "primary_panel_id": PANEL_ID,
            "child_panel_id_min": 2,
            "sdk_methods": [
                "MioPlugin::supports_panel_multiplexing",
                "MioPlugin::on_panel_open_request",
                "MioPlugin::on_panel_open_response",
                "MioPlugin::on_panel_ui_event",
                "MioPlugin::on_panel_commit_editor",
                "MioPlugin::on_panel_commit_response_editor",
                "MioPlugin::on_panel_close_request",
                "MioPlugin::on_panel_destroy",
                "MioPlugin::on_panel_resync",
                "HostClient::panel_id"
            ]
        },
        "manifest_schema": {
            "type": "object",
            "required": ["id", "version", "api_version", "engine", "entry", "permissions", "contributions"],
            "properties": {
                "id": { "type": "string", "description": "Unique extension identifier (e.g. com.example.ext)" },
                "version": { "type": "string", "description": "SemVer extension version" },
                "api_version": { "type": "integer", "const": PROTOCOL_MAJOR },
                "engine": { "type": "string", "enum": ["native"] },
                "entry": { "type": "string", "description": "Relative path to native worker executable" },
                "permissions": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": [
                            "traffic.read",
                            "draft.write",
                            "repeater.open",
                            "network.send",
                            "traffic.modify",
                            "session.hook",
                            "payload.process",
                            "storage.kv",
                            "audit.write"
                        ]
                    }
                },
                "contributions": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": [
                            "suite_panel",
                            "message_editor",
                            "message_response_editor",
                            "context_menu",
                            "http_hook",
                            "ws_hook",
                            "session_action",
                            "payload_generator",
                            "payload_processor",
                            "passive_check",
                            "active_check"
                        ]
                    }
                }
            }
        },
        "permissions": [
            { "id": "traffic.read", "description": permission_label(Permission::TrafficRead) },
            { "id": "draft.write", "description": permission_label(Permission::DraftWrite) },
            { "id": "repeater.open", "description": permission_label(Permission::RepeaterOpen) },
            { "id": "network.send", "description": permission_label(Permission::NetworkSend) },
            { "id": "traffic.modify", "description": permission_label(Permission::TrafficModify) },
            { "id": "session.hook", "description": permission_label(Permission::SessionHook) },
            { "id": "payload.process", "description": permission_label(Permission::PayloadProcess) },
            { "id": "storage.kv", "description": permission_label(Permission::Storage) },
            { "id": "audit.write", "description": permission_label(Permission::AuditWrite) }
        ],
        "ui_components": [
            "Row", "Column", "Split", "Tabs", "Text", "Button", "Editor",
            "VirtualTable", "Canvas", "ScrollColumn", "Checkbox", "Select", "Badge", "Progress"
        ],
        "envelope_messages": [
            "Hello", "Register", "Ready", "Snapshot", "Patch", "UiEvent",
            "OpenRequest", "CommitEditor", "Committed", "HostCall", "HostResult",
            "Cancel", "Shutdown", "Error", "Resync", "Ack", "CloseRequest",
            "HttpHookCall", "HttpHookResult", "WebSocketHookCall", "WebSocketHookResult",
            "SessionActionCall", "SessionActionResult", "PayloadCall", "PayloadResult",
            "OpenResponse", "CommitResponseEditor", "ResponseCommitted",
            "ContextMenuCall", "ContextMenuResult",
            "PassiveCheckCall", "PassiveCheckResult", "ActiveCheckCall", "ActiveCheckResult"
        ],
        "host_calls": [
            "QueryHistory", "ReadFlow", "UpdateDraft", "OpenRepeater", "NetworkSend",
            "StorageGet", "StorageSet", "OpenIntruder", "PreferencesGet", "PreferencesSet",
            "ScopeCheck", "ScopeUpdate", "QuerySiteMap", "ReadAnnotations", "UpdateAnnotations",
            "OpenOrganiser", "SubmitAuditIssue", "CollaboratorGenerate", "CollaboratorPoll"
        ],
        "custom_scan_check_rhai_api": {
            "variables": ["request", "response", "requestResponse", "utilities"],
            "return_values": [
                "bool (true => finding matched)",
                "string (non-empty => matched with evidence text)",
                "map (#{ matched: bool, evidence: string })"
            ]
        }
    });
    serde_json::to_vec_pretty(&doc)
}

pub(in crate::workbench::plugins) fn build_mio_plugin_api_markdown() -> String {
    format!(
        "# MIO Extensions API (v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR})\n\n\
         > Native executable workers (`mio-plugin-sdk`) are trusted local processes without OS sandboxing; `manifest.permissions` mediates MIO host API access only.\n\n\
         ## Transport & Framing\n\
         - Framing: 4-byte big-endian length prefix + Protobuf `Envelope`\n\
         - Default primary `panel_id`: `{PANEL_ID}` (`0` is invalid)\n\
         - Max frame size: `{MAX_FRAME_BYTES}` bytes\n\
         - Max inline body: `{MAX_INLINE_BODY}` bytes\n\
         - KV Storage limits: key `{MAX_STORAGE_KEY_BYTES}` bytes, value `{MAX_STORAGE_VALUE_BYTES}` bytes\n\n\
         ## Panel Multiplexing (v1.5)\n\
         - Set `Register.supports_panel_multiplexing = true` (in `mio-plugin-sdk`, override `MioPlugin::supports_panel_multiplexing() -> bool`) so child message editors and response viewers reuse the existing worker instead of spawning one OS process per tab.\n\
         - Primary suite/tool panel traffic uses `Envelope.panel_id = {PANEL_ID}`; multiplexed child panels receive distinct `panel_id >= 2` values and independent snapshot revision sequences.\n\
         - Implement `on_panel_open_request`, `on_panel_open_response`, `on_panel_ui_event`, `on_panel_commit_editor`, `on_panel_commit_response_editor`, `on_panel_close_request`, `on_panel_destroy`, and `on_panel_resync`, and read `ctx.panel_id()` (`HostClient::panel_id()`) inside handlers.\n\
         - `CloseRequest.destroy_panel = false` revokes a single `resource_id` (`on_panel_close_request`); `CloseRequest.destroy_panel = true` permanently tears down the child panel (`on_panel_destroy`).\n\
         - Derived `HostCall`s carry `HostCall.parent_request_id` (populated automatically by `mio-plugin-sdk`): non-zero values must reference an active, uncancelled request on the same `panel_id`, inherit parent deadline clamping, and cascade-cancel on timeout, revoke, or panel cancellation.\n\
         - During `on_cancel`, derived `HostCall`s referencing the `Cancel` notification run inside a bounded cleanup window (`MAX_OPERATION`) on a live panel (closing panels reject `HostCall`s as `Stale`): allowlisted read/KV calls (`QueryHistory`, `ReadFlow`, `ScopeCheck`, `QuerySiteMap`, `ReadAnnotations`, `StorageGet`, `StorageSet`, `PreferencesGet`, `PreferencesSet`) are allowed even if panel revision has transiently drifted (scheduling `Resync` only for active non-`Resync` `Cancel` cleanups when the callback envelope revision exceeds the host revision), while all other mutating/side-effect calls (`UpdateDraft`, `OpenRepeater`, `OpenIntruder`, `NetworkSend`, `ScopeUpdate`, `OpenOrganiser`, `UpdateAnnotations`, `SubmitAuditIssue`, `Collaborator*`) fail with `ErrorCode::Cancelled`. The window closes on `Cancel` `Ack`/`Error` (or legacy `Snapshot`/`Patch`).\n\n\
         ## Manifest (`manifest.json`)\n\
         ```json\n\
         {{\n  \"id\": \"com.example.extension\",\n  \"version\": \"1.0.0\",\n  \"api_version\": {PROTOCOL_MAJOR},\n  \"engine\": \"native\",\n  \"entry\": \"bin/extension\",\n  \"permissions\": [\"traffic.read\"],\n  \"contributions\": [\"suite_panel\", \"context_menu\", \"passive_check\"]\n}}\n\
         ```\n\n\
         ## Host Permissions\n\
         - `traffic.read`: {}\n\
         - `draft.write`: {}\n\
         - `repeater.open`: {}\n\
         - `network.send`: {}\n\
         - `traffic.modify`: {}\n\
         - `session.hook`: {}\n\
         - `payload.process`: {}\n\
         - `storage.kv`: {}\n\
         - `audit.write`: {}\n\n\
         ## Contribution Points\n\
         `suite_panel`, `message_editor`, `message_response_editor`, `context_menu`, `http_hook`, `ws_hook`, \
         `session_action`, `payload_generator`, `payload_processor`, `passive_check`, `active_check`\n\n\
         ## HostCall Operations (v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR})\n\
         `QueryHistory`, `ReadFlow`, `UpdateDraft`, `OpenRepeater`, `OpenIntruder`, `NetworkSend`, \
         `StorageGet`, `StorageSet`, `PreferencesGet`, `PreferencesSet`, \
         `ScopeCheck`, `ScopeUpdate`, `QuerySiteMap`, `ReadAnnotations`, `UpdateAnnotations`, \
         `OpenOrganiser`, `SubmitAuditIssue`, `CollaboratorGenerate`, `CollaboratorPoll`\n\n\
         ## Custom Scan Checks (Rhai)\n\
         Variables: `request`, `response`, `requestResponse`, `utilities`.\n\
         Return `true`, a non-empty evidence string, or `#{{ matched: true, evidence: \"...\" }}` to report a finding.\n\n\
         ## Large Body Support (v1.7+)\n\
         - Set `Register.supports_large_body = true` (in `mio-plugin-sdk`, override `MioPlugin::supports_large_body() -> bool`) to inspect/modify `> {MAX_INLINE_BODY}` bodies and combined frames `> {MAX_FRAME_BYTES}` across `http_hook`, `ws_hook`, `session_action`, `passive_check`, and `active_check`.\n\
         - When a body exceeds inline limits, the host writes a private read-only spill file, clears `body`, sets `body_omitted = true`, `original_body_len`, and `body_file_path` (or `payload_omitted` / `payload_file_path` for WS). Returning `body_omitted = true` preserves the original large body automatically; plugins rewriting `> 512 KiB` bodies can use `RequestDto::set_body_or_spill` / `ResponseDto::set_body_or_spill` / `WebSocketHookResult::with_modified_payload_from`.\n\
         - Host-side per-plugin hook pre-filtering (Scope, Host/Path allow/denylists, request/response stages, static resource skipping), hook metrics, and Fail-Open / Fail-Closed policies run in host memory prior to stdio serialization.\n\n\
         ## Limitations & Hook Overflow Policy\n\
         - Workers on `v1.5`/`v1.6` or with `supports_large_body = false` cleanly bypass (`Pass`) hook payloads exceeding `{MAX_INLINE_BODY}` or `{MAX_FRAME_BYTES}` without dropping traffic (recorded in `oversized_bypassed`).\n\
         - Synchronous message editor panels (`OpenRequest`/`CommitEditor`) still enforce `{MAX_INLINE_BODY}`.\n\
         - Concurrent hook dispatch is backpressure-bounded (`96` hook slots of `128` control queue capacity); correlated hook replies do not consume the `512 frames/s` unsolicited worker rate limit.\n\
         - Proxy hook timeouts (`5s`) or worker crashes fail the in-flight flow; host-side per-plugin hook pre-filtering is not yet supported.\n",
        permission_label(Permission::TrafficRead),
        permission_label(Permission::DraftWrite),
        permission_label(Permission::RepeaterOpen),
        permission_label(Permission::NetworkSend),
        permission_label(Permission::TrafficModify),
        permission_label(Permission::SessionHook),
        permission_label(Permission::PayloadProcess),
        permission_label(Permission::Storage),
        permission_label(Permission::AuditWrite),
    )
}
