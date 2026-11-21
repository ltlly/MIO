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

        let api_rows: [(&str, &str); 15] = [
            ("Engine: wasm", "受限 WASM 沙箱引擎（零 WASI，无直接 OS 文件或网络访问）"),
            ("Engine: native", "本地 Native worker 进程（不受 OS 沙箱限制，需用户显式信任）"),
            ("Contribution: suite_panel", "在顶部工具栏注册独立扩展标签页视图"),
            ("Contribution: tool_panel", "在 Extensions 详情页中展示扩展管理与交互面板"),
            ("Contribution: message_editor", "在 Repeater / Proxy Intercept / History 注册自定义请求编辑器"),
            ("Contribution: message_response_editor", "注册自定义 HTTP 响应查看与编辑视图"),
            ("Contribution: context_menu", "对当前活动请求/响应执行自定义上下文动作"),
            ("Contribution: payload_generator / processor", "为 Intruder 提供自定义载荷生成与处理规则"),
            ("Protocol v1.5: panel_multiplexing", "在单个 worker 进程内按 Envelope.panel_id (>= 2) 复用多个子编辑器标签（需在 Register 设置 supports_panel_multiplexing）"),
            ("Permission: traffic.read", permission_label(Permission::TrafficRead)),
            ("Permission: draft.write", permission_label(Permission::DraftWrite)),
            ("Permission: repeater.open", permission_label(Permission::RepeaterOpen)),
            ("Permission: network.send", permission_label(Permission::NetworkSend)),
            ("Permission: traffic.modify", permission_label(Permission::TrafficModify)),
            ("Permission: session.hook / payload / kv", "支持会话钩子、载荷处理与项目专属键值存储授权"),
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
                    .child("MIO 扩展协议支持受限 WASM 沙箱与 Native worker，每项宿主 API 权限均需由用户逐项显式授予。"),
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
        "security_scope": "security isolation / sandboxing is explicitly out of scope for native workers; WASM plugins execute with zero WASI and no direct OS/network access",
        "protocol": {
            "major": PROTOCOL_MAJOR,
            "minor": PROTOCOL_MINOR,
            "default_panel_id": PANEL_ID,
            "framing": "4-byte big-endian length prefix + Protobuf Envelope",
            "max_frame_bytes": MAX_FRAME_BYTES,
            "max_inline_body_bytes": MAX_INLINE_BODY,
            "max_storage_key_bytes": MAX_STORAGE_KEY_BYTES,
            "max_storage_value_bytes": MAX_STORAGE_VALUE_BYTES
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
                "engine": { "type": "string", "enum": ["native", "wasm"] },
                "entry": { "type": "string", "description": "Relative path to worker binary or .wasm module" },
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
                            "storage.kv"
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
            { "id": "storage.kv", "description": permission_label(Permission::Storage) }
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
         > Native executable workers (`mio-plugin-sdk`) are trusted local processes; `wasm` modules run under a constrained sandbox with zero WASI and no direct OS file/network access.\n\n\
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
         - `storage.kv`: {}\n\n\
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
         Return `true`, a non-empty evidence string, or `#{{ matched: true, evidence: \"...\" }}` to report a finding.\n",
        permission_label(Permission::TrafficRead),
        permission_label(Permission::DraftWrite),
        permission_label(Permission::RepeaterOpen),
        permission_label(Permission::NetworkSend),
        permission_label(Permission::TrafficModify),
        permission_label(Permission::SessionHook),
        permission_label(Permission::PayloadProcess),
        permission_label(Permission::Storage),
    )
}
