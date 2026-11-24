use gpui_kit::assets::IconName;
use gpui_kit::component::{
    input::{Copy, Cut, EditorState, Paste, TextareaState},
    native_menu::NativeMenu,
};

use super::*;
use crate::{
    AutoMarkIntruderPositions, ChangeBodyEncodingForm, ChangeBodyEncodingJson,
    ChangeBodyEncodingMultipart, ChangeBodyEncodingToggle, ChangeRequestMethod,
    ClearIntruderPositions, CopyCurlCommand, CopyLinksInResponse, CopyUrl,
    ExportSelectedHistoryXml, InsertCollaboratorPayload, OpenResponseInBrowser, OpenScanLauncher,
    PasteFromFile, SaveSelectedTextToFile, SendToMioAt, SendToComparerRequest, SendToDecoder,
    SendToOrganiser, SendToPassiveScanner, SendToScanner, SendToSequencer,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum EditorContextActionKind {
    Noop,
    SendToComparerSmart,
    RecordNewIssue,
    AttachToIssue(mio_core::Uuid),
    RequestInBrowserOriginal,
    RequestInBrowserCurrent,
    EngagementFindReferences,
    EngagementDiscoverContent,
    EngagementScheduleTask,
    EngagementGenerateCsrfPoc,
    ToggleUrlEncodeAsYouType,
}

#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(in crate::workbench) struct EditorContextAction(
    pub(in crate::workbench) EditorContextActionKind,
);

struct EditorContextActionRegistry {
    owner: gpui_kit::WeakEntity<Workbench>,
    window: gpui_kit::AnyWindowHandle,
}

impl gpui_kit::Global for EditorContextActionRegistry {}

fn ensure_editor_context_actions_registered(
    owner: gpui_kit::WeakEntity<Workbench>,
    window: gpui_kit::AnyWindowHandle,
    cx: &mut App,
) {
    let already = cx.has_global::<EditorContextActionRegistry>();
    cx.set_global(EditorContextActionRegistry { owner, window });
    if already {
        return;
    }
    cx.on_action(|action: &EditorContextAction, cx: &mut App| {
        let Some(reg) = cx.try_global::<EditorContextActionRegistry>() else {
            return;
        };
        let owner = reg.owner.clone();
        let win = reg.window;
        let action = action.clone();
        cx.defer(move |cx| {
            let _ = win.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.handle_editor_context_action(&action, window, cx);
                });
            });
        });
    });
}

impl Workbench {
    pub(in crate::workbench) fn handle_editor_context_action(
        &mut self,
        action: &EditorContextAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &action.0 {
            EditorContextActionKind::Noop => {}
            EditorContextActionKind::SendToComparerSmart => self.send_to_comparer_smart(window, cx),
            EditorContextActionKind::RecordNewIssue => {
                self.record_manual_issue_for_active_context(cx);
            }
            EditorContextActionKind::AttachToIssue(finding_id) => {
                self.attach_active_context_to_issue(*finding_id, cx);
            }
            EditorContextActionKind::RequestInBrowserOriginal => {
                self.open_current_request_in_browser_with_session(true, window, cx);
            }
            EditorContextActionKind::RequestInBrowserCurrent => {
                self.open_current_request_in_browser_with_session(false, window, cx);
            }
            EditorContextActionKind::EngagementFindReferences => {
                self.engagement_find_references(window, cx);
            }
            EditorContextActionKind::EngagementDiscoverContent => {
                self.engagement_discover_content(window, cx);
            }
            EditorContextActionKind::EngagementScheduleTask => {
                self.engagement_schedule_task(window, cx);
            }
            EditorContextActionKind::EngagementGenerateCsrfPoc => {
                self.engagement_generate_csrf_poc(window, cx);
            }
            EditorContextActionKind::ToggleUrlEncodeAsYouType => {
                self.toggle_url_encode_as_you_type(cx);
            }
        }
    }

    pub(in crate::workbench) fn websocket_payload_context_menu(
        &self,
        input: Entity<EditorState>,
        readonly: bool,
    ) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
        let unavailable =
            self.project.is_none() || self.transition.is_some() || self.backend_disconnected;
        let owner = self.websocket.owner.clone();
        let owner_window = self.owner_window;
        let sel_input = input.clone();
        move |menu, window, cx| {
            ensure_editor_context_actions_registered(owner.clone(), owner_window, cx);
            input.update(cx, |state, cx| state.focus(window, cx));
            let selected = !sel_input.read(cx).selected_range().is_empty();
            menu.menu_with_disabled(
                "发送到 WebSocket Repeater",
                unavailable,
                Box::new(OpenInRepeater),
            )
            .menu_with_disabled("发送到 Decoder", unavailable, Box::new(SendToDecoder))
            .menu_with_disabled(
                "发送到 Comparer",
                unavailable,
                Box::new(SendToComparerRequest),
            )
            .separator()
            .submenu(
                "转换选中内容 (Convert selection)",
                if readonly {
                    crate::editing::copy_converted_selection_submenu(!selected)
                } else {
                    crate::editing::convert_selection_submenu(!selected)
                },
            )
            .separator()
            .menu_with_disabled("剪切", readonly || !selected, Box::new(Cut))
            .menu_with_disabled("复制", !selected, Box::new(Copy))
            .menu_with_disabled("粘贴", readonly, Box::new(Paste))
        }
    }

    pub(in crate::workbench) fn request_context_menu(
        &self,
        input: Entity<EditorState>,
        readonly: bool,
    ) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
        self.request_context_menu_with_options(input, readonly, None)
    }

    pub(in crate::workbench) fn request_context_menu_with_options(
        &self,
        input: Entity<EditorState>,
        readonly: bool,
        save_entire_history: Option<fn() -> Box<dyn gpui_kit::Action>>,
    ) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
        let sel_input = input.clone();
        self.transfer_context_menu(
            move |cx| !sel_input.read(cx).selected_range().is_empty(),
            Some(input),
            readonly,
            save_entire_history,
        )
    }

    pub(in crate::workbench) fn positions_context_menu(
        &self,
        input: Entity<TextareaState>,
        readonly: bool,
    ) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
        let unavailable =
            self.project.is_none() || self.transition.is_some() || self.backend_disconnected;
        let has_unplaced = self
            .workspace
            .intruder
            .active
            .and_then(|id| self.intruder.editors.get(&id))
            .is_some_and(|ed| ed.request.has_unplaced());
        let sel_input = input.clone();
        let base = self.transfer_context_menu(
            move |cx| !input.read(cx).selected_range().is_empty(),
            None,
            readonly,
            None,
        );
        move |menu, window, cx| {
            let menu = if menu.is_empty() {
                let markers_present = has_unplaced
                    || sel_input
                        .read(cx)
                        .tokens()
                        .iter()
                        .any(|span| span.token().id().starts_with("intruder-position:"));
                menu.menu_with_disabled(
                    "添加 § 标记",
                    unavailable || readonly,
                    Box::new(super::super::core_actions::AddIntruderPosition),
                )
                .menu_with_disabled(
                    "清除 § 标记",
                    unavailable || readonly || !markers_present,
                    Box::new(ClearIntruderPositions),
                )
                .menu_with_disabled(
                    "自动识别 § 标记",
                    unavailable || readonly,
                    Box::new(AutoMarkIntruderPositions),
                )
                .separator()
            } else {
                menu
            };
            base(menu, window, cx)
        }
    }

    fn transfer_context_menu(
        &self,
        has_selection: impl Fn(&App) -> bool + 'static,
        focus_editor: Option<Entity<EditorState>>,
        readonly: bool,
        save_entire_history: Option<fn() -> Box<dyn gpui_kit::Action>>,
    ) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
        let unavailable =
            self.project.is_none() || self.transition.is_some() || self.backend_disconnected;
        let plugin_items = self.active_context_menu_plugins();
        let owner = self.websocket.owner.clone();
        let owner_window = self.owner_window;
        move |menu, window, cx| {
            ensure_editor_context_actions_registered(owner.clone(), owner_window, cx);
            if let Some(editor) = &focus_editor {
                editor.update(cx, |state, cx| state.focus(window, cx));
            }
            let (existing_findings, url_encode_on) = owner
                .upgrade()
                .map(|entity| {
                    let view = entity.read(cx);
                    (
                        view.existing_audit_finding_ids(),
                        view.url_encode_as_you_type_enabled(),
                    )
                })
                .unwrap_or_default();
            crate::editing::set_url_encode_as_you_type(url_encode_on, cx);
            let selected = has_selection(cx);
            let req_edit_disabled = unavailable || readonly;

            let attach_submenu = if existing_findings.is_empty() {
                NativeMenu::new().menu_with_disabled(
                    "暂无现有问题",
                    true,
                    Box::new(EditorContextAction(EditorContextActionKind::Noop)),
                )
            } else {
                existing_findings
                    .into_iter()
                    .fold(NativeMenu::new(), |sub, (fid, title)| {
                        sub.menu_with_disabled(
                            title,
                            unavailable,
                            Box::new(EditorContextAction(EditorContextActionKind::AttachToIssue(
                                fid,
                            ))),
                        )
                    })
            };
            let record_issue_submenu = NativeMenu::new()
                .menu_with_disabled(
                    "创建新问题 (Create a new issue)",
                    unavailable,
                    Box::new(EditorContextAction(EditorContextActionKind::RecordNewIssue)),
                )
                .submenu("添加到现有问题 (Add to existing issue)", attach_submenu);

            let req_browser_submenu = NativeMenu::new()
                .menu_with_disabled(
                    "使用原始会话 (In original session)",
                    unavailable,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::RequestInBrowserOriginal,
                    )),
                )
                .menu_with_disabled(
                    "使用当前会话 (In current session)",
                    unavailable,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::RequestInBrowserCurrent,
                    )),
                );

            let engagement_submenu = NativeMenu::new()
                .menu_with_disabled(
                    "查找引用 (Find references)",
                    unavailable,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::EngagementFindReferences,
                    )),
                )
                .menu_with_disabled(
                    "发现内容 (Discover content)",
                    unavailable,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::EngagementDiscoverContent,
                    )),
                )
                .menu_with_disabled(
                    "计划任务 (Schedule task)",
                    unavailable,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::EngagementScheduleTask,
                    )),
                )
                .menu_with_disabled(
                    "生成 CSRF PoC (Generate CSRF PoC)",
                    unavailable,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::EngagementGenerateCsrfPoc,
                    )),
                );

            let mut menu = menu
                .menu_with_disabled("扫描 (Scan)", unavailable, Box::new(OpenScanLauncher))
                .menu_with_disabled(
                    "被动扫描所选消息",
                    unavailable,
                    Box::new(SendToPassiveScanner),
                )
                .menu_with_disabled("主动扫描所选消息", unavailable, Box::new(SendToScanner))
                .separator()
                .menu_with_disabled("发送到 Intruder", unavailable, Box::new(OpenInIntruder))
                .menu_with_disabled("发送到 Repeater", unavailable, Box::new(OpenInRepeater))
                .menu_with_disabled("发送到 Sequencer", unavailable, Box::new(SendToSequencer))
                .menu_with_disabled(
                    "发送到 Comparer",
                    unavailable,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::SendToComparerSmart,
                    )),
                )
                .menu_with_disabled("发送到 Decoder", unavailable, Box::new(SendToDecoder))
                .menu_with_disabled("发送到 Organizer", unavailable, Box::new(SendToOrganiser));

            if !readonly {
                menu = menu.menu_with_icon_disabled(
                    "插入 Collaborator Payload",
                    IconName::CirclePlus,
                    req_edit_disabled,
                    Box::new(InsertCollaboratorPayload),
                );
            }

            menu = menu
                .menu_with_icon_disabled(
                    "发送到 MioAT",
                    IconName::Zap,
                    unavailable,
                    Box::new(SendToMioAt),
                )
                .separator()
                .submenu("记录问题 (Record an issue)", record_issue_submenu)
                .separator()
                .menu_with_disabled(
                    "在浏览器中打开响应",
                    unavailable,
                    Box::new(OpenResponseInBrowser),
                )
                .submenu("在浏览器中请求", req_browser_submenu)
                .separator()
                .submenu("参与工具 (Engagement tools)", engagement_submenu);

            if !plugin_items.is_empty() {
                menu = menu.separator();
                for (instance_id, plugin_id) in &plugin_items {
                    menu = menu.menu_with_disabled(
                        format!("插件动作：{plugin_id}"),
                        unavailable,
                        Box::new(crate::editing::InvokePluginContextMenu(*instance_id)),
                    );
                }
            }

            menu = menu.separator();

            if !readonly {
                menu = menu.menu_with_icon_disabled(
                    "更改请求方法",
                    IconName::RefreshCw,
                    req_edit_disabled,
                    Box::new(ChangeRequestMethod),
                );
            }

            menu = menu.menu_with_icon_disabled(
                "剪切",
                IconName::Scissors,
                readonly || !selected,
                Box::new(Cut),
            );

            if !readonly {
                menu = menu.menu_with_icon_disabled(
                    "粘贴主机/URL 为请求",
                    IconName::ClipboardPaste,
                    req_edit_disabled,
                    Box::new(crate::PasteUrlAsRequest),
                );
            }

            menu = menu
                .menu_with_icon_disabled("复制", IconName::Copy, !selected, Box::new(Copy))
                .menu_with_icon_disabled(
                    "复制美化文本",
                    IconName::Copy,
                    unavailable,
                    Box::new(crate::CopyPrettified),
                )
                .menu_with_icon_disabled(
                    "将选中文本保存到文件…",
                    IconName::Save,
                    unavailable || !selected,
                    Box::new(SaveSelectedTextToFile),
                )
                .menu_with_icon_disabled(
                    "粘贴",
                    IconName::ClipboardPaste,
                    readonly,
                    Box::new(Paste),
                );

            if !readonly {
                menu = menu.menu_with_icon_disabled(
                    "从文件粘贴文本…",
                    IconName::ClipboardPaste,
                    req_edit_disabled,
                    Box::new(PasteFromFile),
                );
            }

            menu = menu
                .menu_with_check(
                    "输入时 URL 编码",
                    url_encode_on,
                    Box::new(EditorContextAction(
                        EditorContextActionKind::ToggleUrlEncodeAsYouType,
                    )),
                )
                .submenu(
                    "转换选中内容",
                    if readonly {
                        crate::editing::copy_converted_selection_submenu(!selected)
                    } else {
                        crate::editing::convert_selection_submenu(!selected)
                    },
                );

            if !readonly {
                menu = menu.submenu(
                    "更改正文编码",
                    NativeMenu::new()
                        .menu_with_disabled(
                            "Form URL-encoded",
                            req_edit_disabled,
                            Box::new(ChangeBodyEncodingForm),
                        )
                        .menu_with_disabled(
                            "Multipart",
                            req_edit_disabled,
                            Box::new(ChangeBodyEncodingMultipart),
                        )
                        .menu_with_disabled(
                            "JSON",
                            req_edit_disabled,
                            Box::new(ChangeBodyEncodingJson),
                        )
                        .separator()
                        .menu_with_disabled(
                            "循环切换正文编码 (Toggle body encoding)",
                            req_edit_disabled,
                            Box::new(ChangeBodyEncodingToggle),
                        ),
                );
            }

            menu = menu.separator();

            if let Some(make_save_history) = save_entire_history {
                menu = menu.menu_with_icon_disabled(
                    "保存完整历史…",
                    IconName::Save,
                    unavailable,
                    make_save_history(),
                );
            }

            menu.menu_with_icon_disabled("复制 URL", IconName::Copy, unavailable, Box::new(CopyUrl))
                .menu_with_icon_disabled(
                    "复制为 curl 命令 (bash)",
                    IconName::Copy,
                    unavailable,
                    Box::new(CopyCurlCommand),
                )
                .menu_with_icon_disabled(
                    "复制响应中的链接",
                    IconName::Copy,
                    unavailable,
                    Box::new(CopyLinksInResponse),
                )
                .menu_with_icon_disabled(
                    "保存条目",
                    IconName::Save,
                    unavailable,
                    Box::new(ExportSelectedHistoryXml),
                )
        }
    }

    fn origin_from_url(url: &str) -> String {
        let Some((scheme, tail)) = url.split_once("://") else {
            return url.to_owned();
        };
        let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
        format!("{scheme}://{}", &tail[..authority_end])
    }

    fn history_copy_source(
        &self,
        for_curl: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(Vec<u8>, String, HttpVersion), String> {
        let selected = self
            .selected_history
            .ok_or_else(|| "没有选中的历史记录请求。".to_owned())?;
        let editor_bytes = self.history_request.bytes(window, cx)?;
        match selected {
            HistoryEntryId::Live(flow_id) => {
                if let Some(error) = &self.detail_error {
                    return Err(format!("无法读取所选历史记录：{error}"));
                }
                let row = self
                    .history
                    .read(cx)
                    .delegate()
                    .row_by_id(HistoryEntryId::Live(flow_id))
                    .and_then(HistoryRow::live)
                    .cloned();
                if let Some(detail) = self
                    .detail
                    .as_ref()
                    .filter(|record| record.summary.id == flow_id)
                {
                    let request = if self.history_original_view {
                        self.detail_original
                            .request
                            .as_ref()
                            .unwrap_or(&detail.request)
                    } else {
                        &detail.request
                    };
                    let raw = if editor_bytes.is_empty() {
                        request_to_raw(request)
                    } else {
                        editor_bytes
                    };
                    return Ok((raw, Self::origin_from_url(&request.url), request.version));
                }
                let flow = row.ok_or_else(|| "所选历史记录摘要不可用。".to_owned())?;
                if !editor_bytes.is_empty() {
                    return Ok((
                        editor_bytes,
                        Self::origin_from_url(&flow.url),
                        HttpVersion::Http11,
                    ));
                }
                if !for_curl {
                    return Ok((Vec::new(), flow.url, HttpVersion::Http11));
                }
                Err("正在读取历史请求正文，请稍后重试复制为 curl 命令。".into())
            }
            HistoryEntryId::External { source_id, item_id } => {
                let row = self
                    .history
                    .read(cx)
                    .delegate()
                    .row_by_id(HistoryEntryId::External { source_id, item_id })
                    .and_then(|row| match row {
                        HistoryRow::ExtProxy(row) => Some(row.as_ref()),
                        HistoryRow::Live(_) => None,
                    })
                    .or_else(|| {
                        self.imported_history
                            .row
                            .as_ref()
                            .filter(|row| row.id() == HistoryEntryId::External { source_id, item_id })
                            .and_then(|row| match row {
                                HistoryRow::ExtProxy(row) => Some(row.as_ref()),
                                HistoryRow::Live(_) => None,
                            })
                    })
                    .ok_or_else(|| "所选归档历史记录摘要不可用。".to_owned())?;
                let origin = row
                    .origin
                    .clone()
                    .ok_or_else(|| "归档请求缺少连接目标 origin。".to_owned())?;
                let summary = self
                    .imported_history_request()
                    .or(row.request.as_ref())
                    .or(row.edited_request.as_ref())
                    .ok_or_else(|| "归档条目没有可复制的请求。".to_owned())?;
                let version = summary.declared_version.unwrap_or(HttpVersion::Http11);
                if !editor_bytes.is_empty() && self.imported_history_request_ready() {
                    return Ok((editor_bytes, origin, version));
                }
                if !for_curl {
                    let request_target = summary
                        .request_target
                        .as_deref()
                        .ok_or_else(|| "归档请求缺少请求路径。".to_owned())?;
                    let method = summary.method.as_deref().unwrap_or("GET");
                    let synthetic = format!("{method} {request_target} HTTP/1.1\r\n\r\n");
                    return Ok((synthetic.into_bytes(), origin, version));
                }
                Err("正在读取归档请求正文，请稍后重试复制为 curl 命令。".into())
            }
        }
    }

    pub(in crate::workbench) fn copy_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.transition.is_some() || self.project.is_none() || self.backend_disconnected {
            return;
        }
        let result = if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            match self.history_copy_source(false, window, cx) {
                Ok((raw, target, _)) if raw.is_empty() => Ok(target),
                Ok((raw, target, version)) => {
                    mio_core::copy_url_for_request(&raw, &target, version)
                        .map_err(|error| error.to_string())
                }
                Err(error) => Err(error),
            }
        } else {
            if !self.flush_workspace(window, cx) {
                return;
            }
            self.current_transfer_draft("复制 URL", window, cx)
                .and_then(|draft| {
                    mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
                        .map_err(|error| error.to_string())
                })
        };
        match result {
            Ok(value) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(value));
                self.error = None;
                self.status = "已复制 URL 到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn copy_curl_command(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || self.backend_disconnected {
            return;
        }
        let redact = self
            .user_preferences
            .misc
            .redact_sensitive_headers_in_copy_curl
            || self
                .user_preferences
                .confirmed
                .as_ref()
                .is_some_and(|p| p.misc.redact_sensitive_headers_in_copy_curl);
        let result = if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            self.history_copy_source(true, window, cx)
                .and_then(|(raw, target, version)| {
                    curl_command_maybe_redacted(&raw, &target, version, redact)
                })
        } else {
            if !self.flush_workspace(window, cx) {
                return;
            }
            self.current_transfer_draft("复制为 curl 命令", window, cx)
                .and_then(|draft| {
                    curl_command_maybe_redacted(&draft.raw, &draft.target, draft.version, redact)
                })
        };
        match result {
            Ok(value) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(value));
                self.error = None;
                self.status = "已复制 curl 命令到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn copy_response(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        match self.tool_message_bytes(true, window, cx) {
            Ok(bytes) => {
                let Ok(text) = String::from_utf8(bytes) else {
                    self.fail(
                        "响应包含非 UTF-8 二进制字节；请切换到 Hex 视图复制，未替换或丢失原始字节。",
                        cx,
                    );
                    return;
                };
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                self.error = None;
                self.status = "已复制响应到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn add_to_scope(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modify_scope_from_current_request(true, window, cx);
    }

    pub(in crate::workbench) fn exclude_from_scope(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modify_scope_from_current_request(false, window, cx);
    }

    fn modify_scope_from_current_request(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || self.backend_disconnected {
            return;
        }
        let result = if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            match self.history_copy_source(false, window, cx) {
                Ok((raw, target, _)) if raw.is_empty() => Ok(target),
                Ok((raw, target, version)) => {
                    mio_core::copy_url_for_request(&raw, &target, version)
                        .map_err(|error| error.to_string())
                }
                Err(error) => Err(error),
            }
        } else {
            if !self.flush_workspace(window, cx) {
                return;
            }
            self.current_transfer_draft("修改目标范围", window, cx)
                .and_then(|draft| {
                    mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
                        .map_err(|error| error.to_string())
                })
        };
        match result {
            Ok(url) => {
                let prefix = Self::origin_from_url(&url);
                self.add_scope_rule(self.project, include, prefix, cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn read_intruder_clean_request_bytes(
        &self,
        id: TabId,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Vec<u8>, String> {
        if self.intruder.positions_request.is_some() || self.intruder.starts.contains_key(&id) {
            return Err("Intruder 当前正在处理位置或启动攻击，请稍后重试。".into());
        }
        let editors = self
            .intruder
            .editors
            .get(&id)
            .ok_or_else(|| "Intruder 请求编辑器尚未载入。".to_owned())?;
        if editors.has_composition(window, cx) {
            return Err(editing::COMPOSITION_PENDING.into());
        }
        if editors.request.has_unplaced() {
            return Err("原 payload 位置范围无效且已保留；请先使用清除 § 或自动 § 修复。".into());
        }
        match editors.request.position_count(cx) {
            None => return Err("Intruder 请求包含未配对的 § 标记；请先修复或清除标记。".into()),
            Some(count) if count > 0 => {
                return Err("请先清除 Intruder § payload 标记后再切换请求方法或粘贴 URL。".into());
            }
            Some(_) => {}
        }
        editors.request.bytes(window, cx)
    }

    pub(in crate::workbench) fn apply_intruder_request_rewrite(
        &mut self,
        id: TabId,
        next_target: Option<String>,
        next_raw: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let _ = self.read_intruder_clean_request_bytes(id, window, cx)?;
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            if let Some(target) = next_target {
                editors
                    .target
                    .update(cx, |state, cx| state.set_value(target, window, cx));
            }
            editors.request.load(&next_raw, &[], window, cx);
            editors.hex_length = Some(next_raw.len());
            editors.dirty = true;
        }
        let _ = self.flush_workspace(window, cx);
        Ok(())
    }
}

fn curl_command_maybe_redacted(
    raw: &[u8],
    target: &str,
    version: HttpVersion,
    redact: bool,
) -> Result<String, String> {
    mio_core::curl_command_for_request_with_options(raw, target, version, redact)
        .map_err(|e| e.to_string())
}
