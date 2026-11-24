use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::command::{Command as CommandView, CommandItem};
use mio_core::{GlobalSearchHit, GlobalSearchQuery, GlobalSearchResult, GlobalSearchTarget};

use super::*;

pub(in crate::workbench) struct GlobalSearchUi {
    pub(in crate::workbench) open: bool,
    pub(in crate::workbench) input: Entity<InputState>,
    pub(in crate::workbench) regex: bool,
    pub(in crate::workbench) case_sensitive: bool,
    pub(in crate::workbench) in_scope_only: bool,
    pub(in crate::workbench) loading: bool,
    pub(in crate::workbench) cancel: Option<CancellationToken>,
    pub(in crate::workbench) revision: u64,
    pub(in crate::workbench) result: Result<GlobalSearchResult, String>,
    pub(in crate::workbench) results_scroll: gpui_kit::UniformListScrollHandle,
    pub(in crate::workbench) focus: FocusHandle,
    pub(in crate::workbench) command_palette_focus: FocusHandle,
    pub(in crate::workbench) _subscription: Subscription,
}

impl GlobalSearchUi {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索 HTTP 原始与修改后报文、WebSocket、备注、站点地图、Repeater、Intruder、Organiser、Logger 与审计问题…"));
        let _subscription = cx.subscribe_in(&input, window, |view, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) { view.query_global_search(window, cx); }
        });
        Self { open: false, input, regex: false, case_sensitive: false, in_scope_only: false,
            loading: false, cancel: None, revision: 0, result: Ok(GlobalSearchResult::default()),
            results_scroll: gpui_kit::UniformListScrollHandle::new(),
            focus: cx.focus_handle(), command_palette_focus: cx.focus_handle(), _subscription }
    }
}

impl Workbench {
    pub(in crate::workbench) fn close_global_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancel) = self.global_search.cancel.take() {
            cancel.cancel();
        }
        self.global_search.loading = false;
        self.command_palette_open = false;
        self.global_search.open = false;
        self.mio_ai_panel_open = false;
        self.csrf_poc_modal = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancel) = self.global_search.cancel.take() {
            cancel.cancel();
        }
        self.global_search.loading = false;
        self.global_search.open = false;
        self.mio_ai_panel_open = false;
        self.csrf_poc_modal = None;
        self.command_palette_open = true;
        self.command_palette.update(cx, |state, cx| {
            state.set_query("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn open_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_palette_open = false;
        self.mio_ai_panel_open = false;
        self.csrf_poc_modal = None;
        self.global_search.open = true;
        self.global_search.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        if !self.global_search.loading && !self.global_search.input.read(cx).value().is_empty() {
            self.query_global_search(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn reset_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancel) = self.global_search.cancel.take() {
            cancel.cancel();
        }
        self.command_palette_open = false;
        self.global_search.open = false;
        self.global_search.loading = false;
        self.global_search.revision = self.global_search.revision.saturating_add(1);
        self.global_search.result = Ok(GlobalSearchResult::default());
        self.global_search.input.update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn global_search_disconnected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        if let Some(cancel) = self.global_search.cancel.take() {
            cancel.cancel();
        }
        self.command_palette_open = false;
        self.global_search.loading = false;
        if self.global_search.open {
            self.global_search.result = Err("后台服务已断开，全局搜索结果未确认；已保留输入。".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn query_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let term = match editing::input_snapshot(&self.global_search.input, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                if let Some(cancel) = self.global_search.cancel.take() {
                    cancel.cancel();
                }
                self.global_search.loading = false;
                self.global_search.result = Err(error.into());
                cx.notify();
                return;
            }
        };
        if term.is_empty() {
            if let Some(cancel) = self.global_search.cancel.take() {
                cancel.cancel();
            }
            self.global_search.loading = false;
            self.global_search.result = Err("请输入搜索关键词。".into());
            cx.notify();
            return;
        }
        let Some(project) = self.project else {
            if let Some(cancel) = self.global_search.cancel.take() {
                cancel.cancel();
            }
            self.global_search.loading = false;
            self.global_search.result = Err("项目尚未打开，无法全局搜索。".into()); cx.notify(); return;
        };
        if !self.backend_available() || self.transition.is_some() {
            if let Some(cancel) = self.global_search.cancel.take() {
                cancel.cancel();
            }
            self.global_search.loading = false;
            self.global_search.result = Err("项目当前不可查询，请在项目状态恢复后搜索。".into()); cx.notify(); return;
        }
        if let Some(cancel) = self.global_search.cancel.take() {
            cancel.cancel();
        }
        let mut workspace = (*self.workspace).clone();
        if self.capture_drafts(window, cx).is_ok() { workspace = (*self.workspace).clone(); }
        self.global_search.revision = self.global_search.revision.saturating_add(1);
        let query = GlobalSearchQuery { project, query_revision: self.global_search.revision, term,
            regex: self.global_search.regex, case_sensitive: self.global_search.case_sensitive,
            in_scope_only: self.global_search.in_scope_only, limit: 1000, workspace };
        let cancel = self.shutdown.child_token();
        if self.enqueue(Command::QueryGlobalSearch { query, cancel: cancel.clone() }, cx) {
            self.global_search.cancel = Some(cancel);
            self.global_search.loading = true;
            self.global_search.result = Ok(GlobalSearchResult::default());
        } else { self.global_search.loading = false; }
        cx.notify();
    }

    pub(in crate::workbench) fn on_global_search_page(&mut self, query: GlobalSearchQuery, result: Result<GlobalSearchResult, String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.project != Some(query.project) || self.global_search.revision != query.query_revision { return; }
        self.global_search.cancel = None;
        self.global_search.loading = false;
        let has_hits = result.as_ref().is_ok_and(|res| !res.hits.is_empty());
        self.global_search.result = result;
        if has_hits {
            self.global_search
                .results_scroll
                .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        }
        if self.global_search.open { self.global_search.input.update(cx, |input, cx| input.focus(window, cx)); }
        cx.notify();
    }

    pub(in crate::workbench) fn open_global_search_hit(&mut self, hit: &GlobalSearchHit, window: &mut Window, cx: &mut Context<Self>) {
        if !self.backend_available() || self.transition.is_some() {
            self.fail("项目当前不可读取搜索结果，请在项目状态恢复后重试。", cx); return;
        }
        match &hit.target {
            GlobalSearchTarget::ProxyHttp(id) => {
                self.select_tool(Tool::Proxy, window, cx);
                self.proxy_page = ProxyPage::History;
                self.select_history_entry(*id, window, cx);
            }
            GlobalSearchTarget::ProxyWebSocket(id) => {
                self.select_tool(Tool::Proxy, window, cx);
                self.proxy_page = ProxyPage::WebSocketHistory;
                if let Some(summary) = &hit.websocket_summary {
                    self.websocket.table.update(cx, |table, cx| {
                        let delegate = table.delegate_mut();
                        if !delegate.rows.iter().any(|row| row.id == *id) { delegate.rows.push(summary.clone()); }
                        delegate.rows.sort_by_key(|row| row.ordinal);
                        table.refresh(cx);
                    });
                }
                self.select_websocket(*id, window, cx);
            }
            GlobalSearchTarget::TargetSiteMap { node_key, .. } => {
                let Some(row) = &hit.history_row else { self.fail("站点地图搜索结果缺少记录。", cx); return; };
                let Some((entry_id, revision)) = hit.site_map_entry else { self.fail("站点地图搜索结果缺少读取身份。", cx); return; };
                self.open_site_map_explicit_entry(row.clone(), entry_id, revision, Some(node_key.clone()), window, cx);
            }
            GlobalSearchTarget::RepeaterHttp(id) => {
                self.select_tool(Tool::Repeater, window, cx);
                self.workspace.active = Some(*id);
                self.workspace.active_websocket_tab = None;
                self.sync_tab_editors(window, cx);
            }
            GlobalSearchTarget::RepeaterWebSocket(id) => {
                self.select_tool(Tool::Repeater, window, cx);
                self.workspace.active = None;
                self.workspace.active_websocket_tab = Some(*id);
                self.sync_websocket_repeater_editors(window, cx);
            }
            GlobalSearchTarget::IntruderTab(id) => {
                self.select_tool(Tool::Intruder, window, cx);
                self.workspace.intruder.active = Some(*id);
                self.sync_intruder_editors(window, cx);
            }
            GlobalSearchTarget::OrganiserItem(id) => {
                self.select_tool(Tool::Organiser, window, cx);
                self.organiser.search_term.clear();
                self.organiser.status_filter = None;
                self.sync_organiser_table(window, cx);
                self.select_organiser_item(*id, window, cx);
            }
            GlobalSearchTarget::LoggerFlow(id) => {
                self.select_tool(Tool::Logger, window, cx);
                self.select_logger_entry(*id, window, cx);
            }
            GlobalSearchTarget::AuditIssue(id) => {
                self.open_dashboard(cx);
                self.select_audit_finding(*id, window, cx);
            }
        }
        self.global_search.open = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn command_palette_entries() -> Vec<(&'static str, Box<dyn gpui_kit::Action>)> {
        vec![
            ("切换到 Dashboard", Box::new(SelectToolDashboard)),
            ("切换到 Target", Box::new(SelectToolTarget)),
            ("切换到 Proxy", Box::new(SelectToolProxy)),
            ("切换到 Intruder", Box::new(SelectToolIntruder)),
            ("切换到 Repeater", Box::new(SelectToolRepeater)),
            ("切换到 Collaborator", Box::new(SelectToolCollaborator)),
            ("切换到 Sequencer", Box::new(SelectToolSequencer)),
            ("切换到 Decoder", Box::new(SelectToolDecoder)),
            ("切换到 Comparer", Box::new(SelectToolComparer)),
            ("切换到 Logger", Box::new(SelectToolLogger)),
            ("切换到 Organiser", Box::new(SelectToolOrganiser)),
            ("切换到 Extensions", Box::new(SelectToolExtensions)),
            ("切换到 Discover", Box::new(SelectToolDiscover)),
            ("打开配置浏览器", Box::new(crate::OpenConfiguredBrowser)),
            ("打开全局搜索", Box::new(crate::OpenGlobalSearch)),
            ("打开命令面板", Box::new(crate::OpenCommandPalette)),
            ("打开设置", Box::new(crate::OpenSettings)),
            ("用户设置", Box::new(crate::OpenUserSettings)),
            ("项目设置", Box::new(crate::OpenProjectSettings)),
            ("搜索设置", Box::new(crate::SearchSettings)),
            ("Proxy 设置", Box::new(crate::OpenProxySettings)),
            ("Intruder 设置", Box::new(crate::OpenIntruderSettings)),
            ("Repeater 设置", Box::new(crate::OpenRepeaterSettings)),
            ("目标范围设置", Box::new(crate::OpenScopeSettings)),
            ("新建窗口", Box::new(crate::NewWindow)),
            ("新建项目", Box::new(NewProject)),
            ("打开项目", Box::new(OpenProject)),
            ("在新窗口中打开项目", Box::new(crate::OpenProjectInNewWindow)),
            ("导入 外部归档项目", Box::new(ImportExtProject)),
            ("查看 外部导入记录", Box::new(ShowExtReport)),
            ("保存项目", Box::new(SaveProject)),
            ("另存为", Box::new(SaveProjectAs)),
            ("瘦身另存为", Box::new(crate::SlimSaveProjectAs)),
            ("导出 Proxy CA", Box::new(ExportCa)),
            ("退出 MIO", Box::new(crate::Quit)),
            ("切换 Intercept", Box::new(crate::ToggleProxyIntercept)),
            ("开始 Intruder 攻击", Box::new(crate::StartIntruderAttack)),
            ("打开已保存的攻击", Box::new(crate::OpenSavedIntruderAttacks)),
            ("自动标记 Payload 位置", Box::new(crate::AutoMarkIntruderPositions)),
            ("清除 Payload 位置", Box::new(crate::ClearIntruderPositions)),
            ("复制标签页", Box::new(crate::DuplicateActiveTab)),
            ("恢复默认标签布局", Box::new(crate::RestoreDefaultTabLayout)),
            ("Repeater：更新 Content-Length", Box::new(crate::ToggleRepeaterUpdateContentLength)),
            ("Repeater：解包压缩响应", Box::new(crate::ToggleRepeaterUnpackCompressed)),
            ("Repeater：规范化 HTTP/1 换行符", Box::new(crate::ToggleRepeaterNormalizeLineEndings)),
            ("Repeater：强制 Connection: close", Box::new(crate::ToggleRepeaterHttp1Close)),
            ("Repeater：剥离 HTTP/2 Connection 标头", Box::new(crate::ToggleRepeaterStripH2Connection)),
            ("重定向：从不", Box::new(crate::SetRepeaterRedirectNever)),
            ("重定向：仅同站", Box::new(crate::SetRepeaterRedirectOnSite)),
            ("重定向：仅范围内", Box::new(crate::SetRepeaterRedirectInScope)),
            ("重定向：始终", Box::new(crate::SetRepeaterRedirectAlways)),
            ("发送到 Intruder", Box::new(OpenInIntruder)),
            ("发送到 Repeater", Box::new(OpenInRepeater)),
            ("发送到 Decoder", Box::new(crate::SendToDecoder)),
            ("发送到 Sequencer", Box::new(crate::SendToSequencer)),
            ("发起主动扫描", Box::new(crate::SendToScanner)),
            ("打开扫描配置", Box::new(crate::OpenScanLauncher)),
            ("发起被动扫描", Box::new(crate::SendToPassiveScanner)),
            ("发送到 Organiser", Box::new(crate::SendToOrganiser)),
            ("添加到站点地图", Box::new(crate::AddToSiteMap)),
            ("发送到 Comparer：请求", Box::new(crate::SendToComparerRequest)),
            ("发送到 Comparer：响应", Box::new(crate::SendToComparerResponse)),
            ("在浏览器中打开响应", Box::new(crate::OpenResponseInBrowser)),
            ("复制 URL", Box::new(CopyUrl)),
            ("复制 curl 命令", Box::new(CopyCurlCommand)),
            ("复制响应", Box::new(crate::CopyResponse)),
            ("复制响应中的链接", Box::new(crate::CopyLinksInResponse)),
            ("复制美化报文", Box::new(CopyPrettified)),
            ("切换请求方法", Box::new(ChangeRequestMethod)),
            ("请求方法：GET", Box::new(crate::ChangeRequestMethodGet)),
            ("请求方法：POST", Box::new(crate::ChangeRequestMethodPost)),
            ("请求方法：PUT", Box::new(crate::ChangeRequestMethodPut)),
            ("请求方法：DELETE", Box::new(crate::ChangeRequestMethodDelete)),
            ("请求方法：PATCH", Box::new(crate::ChangeRequestMethodPatch)),
            ("请求方法：OPTIONS", Box::new(crate::ChangeRequestMethodOptions)),
            ("请求体：循环切换编码", Box::new(crate::ChangeBodyEncodingToggle)),
            ("请求体：Form URL-encoded", Box::new(crate::ChangeBodyEncodingForm)),
            ("请求体：Multipart/form-data", Box::new(crate::ChangeBodyEncodingMultipart)),
            ("请求体：Application/JSON", Box::new(crate::ChangeBodyEncodingJson)),
            ("粘贴 URL 为请求", Box::new(PasteUrlAsRequest)),
            ("复制到文件", Box::new(crate::CopyToFile)),
            ("保存选区到文件", Box::new(crate::SaveSelectedTextToFile)),
            ("保存响应正文到文件", Box::new(crate::CopyResponseBodyToFile)),
            ("导出选中 HTTP 历史 XML", Box::new(crate::ExportSelectedHistoryXml)),
            ("从文件粘贴", Box::new(crate::PasteFromFile)),
            ("从文件 Base64 编码插入", Box::new(crate::PasteBase64FromFile)),
            ("插入 Collaborator Payload", Box::new(crate::InsertCollaboratorPayload)),
            ("删除选中 HTTP 历史", Box::new(crate::DeleteSelectedHttpHistory)),
            ("清空实时历史", Box::new(crate::ClearLiveProxyHistory)),
            ("添加到范围", Box::new(crate::AddToScope)),
            ("从范围排除", Box::new(crate::ExcludeFromScope)),
            ("添加 HTTP 备注", Box::new(AddHttpNotes)),
            ("发送请求", Box::new(SendRequest)),
            ("取消请求", Box::new(CancelRequest)),
            ("新建标签页", Box::new(NewTab)),
            ("关闭标签页", Box::new(CloseTab)),
            ("重新打开已关闭标签", Box::new(RestoreTab)),
            ("下一个标签页", Box::new(NextTab)),
            ("上一个标签页", Box::new(PreviousTab)),
            ("标签页上移", Box::new(MoveTabUp)),
            ("标签页下移", Box::new(MoveTabDown)),
            ("切换工作标签方向", Box::new(ToggleTabOrientation)),
        ]
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_global_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut overlays = Vec::new();
        if self.command_palette_open {
            let state = self.command_palette.clone();
            let owner = cx.weak_entity();
            let confirm_owner = owner.clone();
            let command = CommandView::new(&state)
                .items(Self::command_palette_entries().into_iter().map(|(label, action)| CommandItem::new().label(label).action(action)))
                .placeholder("搜索工作台全部命令…")
                .on_confirm(move |_, window, cx| {
                    let _ = confirm_owner.update(cx, |view, cx| {
                        view.command_palette_open = false;
                        if view.command_palette.read(cx).focus_handle(cx).is_focused(window) {
                            view.focus.focus(window, cx);
                        }
                        cx.notify();
                    });
                })
                .on_cancel(move |window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.command_palette_open = false;
                        view.focus.focus(window, cx);
                        cx.notify();
                    });
                });
            overlays.push(
                div()
                    .id("global-command-palette-backdrop")
                    .test_support()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                        this.command_palette_open = false;
                        this.focus.focus(window, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .id("global-command-palette-overlay")
                            .test_support()
                            .occlude()
                            .absolute()
                            .top(px(12.))
                            .left(px(180.))
                            .w(px(600.))
                            .max_h(px(480.))
                            .bg(theme::surface())
                            .border_1()
                            .border_color(theme::border())
                            .rounded(px(6.))
                            .overflow_hidden()
                            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .focus_trap(
                                "global-command-palette-overlay",
                                &self.global_search.command_palette_focus,
                            )
                            .child(command),
                    )
                    .into_any_element(),
            );
        }
        if self.global_search.open {
            let input = self.global_search.input.clone();
            let regex = self.global_search.regex;
            let case_sensitive = self.global_search.case_sensitive;
            let in_scope_only = self.global_search.in_scope_only;
            let loading = self.global_search.loading;
            let list_body: AnyElement = match &self.global_search.result {
                Ok(result) => {
                    let hit_count = result.hits.len();
                    let total = result.total;
                    let weak = cx.weak_entity();
                    let results_scroll = self.global_search.results_scroll.clone();
                    let row_h = theme::scaled_px(48.);
                    let row_hover_bg: gpui_kit::Hsla = rgb(if theme::is_dark() { 0x26334a } else { 0xeeeeee }).into();
                    let virtual_list = gpui_kit::uniform_list(
                        "global-search-results",
                        hit_count,
                        move |range, _, cx| {
                            let visible_hits: Vec<(usize, GlobalSearchHit)> = {
                                let Some(owner) = weak.upgrade() else {
                                    return Vec::new();
                                };
                                let view = owner.read(cx);
                                let Ok(res) = &view.global_search.result else {
                                    return Vec::new();
                                };
                                range
                                    .filter_map(|index| {
                                        res.hits.get(index).cloned().map(|hit| (index, hit))
                                    })
                                    .collect()
                            };
                            visible_hits
                                .into_iter()
                                .map(|(index, hit)| {
                                    let label = format!(
                                        "{} · {} · {} {}",
                                        hit.tool_label,
                                        hit.method_or_direction,
                                        hit.url,
                                        hit.matched_in
                                    );
                                    let snippet = hit.snippet.clone();
                                    let click_owner = weak.clone();
                                    div()
                                        .id(SharedString::from(format!(
                                            "global-search-result-{index}"
                                        )))
                                        .test_support()
                                        .role(gpui_kit::accesskit::Role::Button)
                                        .aria_label(label.clone())
                                        .w_full()
                                        .h(row_h)
                                        .px(px(8.))
                                        .py(px(6.))
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(row_hover_bg))
                                        .border_b_1()
                                        .border_color(theme::border())
                                        .flex()
                                        .flex_col()
                                        .justify_center()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .w_full()
                                                .min_w_0()
                                                .truncate()
                                                .child(label),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .min_w_0()
                                                .text_color(theme::muted_foreground())
                                                .truncate()
                                                .child(snippet),
                                        )
                                        .on_click(move |_: &gpui_kit::ClickEvent, window, cx| {
                                            let _ = click_owner.update(cx, |this, cx| {
                                                this.open_global_search_hit(&hit, window, cx);
                                            });
                                        })
                                        .into_any_element()
                                })
                                .collect()
                        },
                    )
                    .track_scroll(&results_scroll)
                    .size_full();
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .flex_col()
                        .child(div().flex_1().min_h_0().w_full().child(virtual_list))
                        .child(
                            div()
                                .flex_shrink_0()
                                .p(px(8.))
                                .border_t_1()
                                .border_color(theme::border())
                                .child(format!("共 {total} 条匹配")),
                        )
                        .into_any_element()
                }
                Err(error) => div()
                    .id("global-search-error")
                    .test_support()
                    .p(px(10.))
                    .child(error.clone())
                    .into_any_element(),
            };
            overlays.push(
                div()
                    .id("global-search-backdrop")
                    .test_support()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                        this.close_global_overlays(window, cx);
                        cx.stop_propagation();
                    }))
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .child(
                        div().id("global-search-overlay").test_support()
                            .occlude()
                            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .absolute().top(px(12.)).left(px(24.)).right(px(24.)).bottom(px(24.))
                            .bg(theme::background()).border_1().border_color(theme::border()).rounded(px(6.))
                            .flex().flex_col().focus_trap("global-search-overlay", &self.global_search.focus)
                            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                                if event.keystroke.key == "escape" {
                                    this.close_global_overlays(window, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .child(toolbar()
                                .child(div().w(px(320.)).child(Input::new(&input).id("global-search-input").aria_label("全局搜索").small()))
                                .child(button("global-search-regex", "正则").selected(regex).on_click(cx.listener(|this, _, window, cx| { this.global_search.regex = !this.global_search.regex; this.query_global_search(window, cx); })))
                                .child(button("global-search-case", "区分大小写").selected(case_sensitive).on_click(cx.listener(|this, _, window, cx| { this.global_search.case_sensitive = !this.global_search.case_sensitive; this.query_global_search(window, cx); })))
                                .child(button("global-search-scope", "仅范围内").selected(in_scope_only).on_click(cx.listener(|this, _, window, cx| { this.global_search.in_scope_only = !this.global_search.in_scope_only; this.query_global_search(window, cx); })))
                                .child(button("global-search-run", "搜索").disabled(loading).on_click(cx.listener(|this, _, window, cx| this.query_global_search(window, cx))))
                                .child(button("global-search-close", "关闭").icon(gpui_kit::assets::IconName::Close).on_click(cx.listener(|this, _, window, cx| this.close_global_overlays(window, cx)))))
                            .child(if loading { div().id("global-search-loading").test_support().p(px(10.)).child("正在读取全部工具的原始与修改后内容…").into_any_element() } else { list_body }),
                    )
                    .into_any_element(),
            );
        }
        if self.mio_ai_panel_open {
            overlays.push(self.render_mio_ai_control_center(window, cx));
        }
        if let Some((title, html)) = self.csrf_poc_modal.clone() {
            overlays.push(self.render_csrf_poc_modal(title, html, window, cx));
        }
        overlays
    }

    fn build_mio_ai_prompt(template_idx: usize, context: Option<&str>) -> (&'static str, String) {
        const TEMPLATES: [(&str, &str); 3] = [
            (
                "分析越权与认证缺陷",
                "请作为资深应用安全工程师，针对下方 HTTP 流量 / 安全上下文重点分析：\n\
                 1. 水平越权 (IDOR / BOLA) 与垂直越权 (BFLA) 风险点（识别用户标识、租户 ID、对象 ID、角色字段）；\n\
                 2. 认证与会话机制弱点（JWT 算法/声明/过期校验、Cookie HttpOnly/Secure/SameSite、Token 重放）；\n\
                 3. 给出 3-5 个可直接用于 MIO Repeater 的验证性请求篡改方案与预期响应判断依据。",
            ),
            (
                "检查注入点与参数污染",
                "请作为资深渗透测试工程师，针对下方 HTTP 流量 / 安全上下文系统梳理潜在注入面：\n\
                 1. 逐一列出 Query、Path、Body (JSON/Form/Multipart)、Cookie 与自定义请求头中的所有可控输入点；\n\
                 2. 评估 SQLi、NoSQLi、命令注入、SSTI、SSRF、XSS、XXE、反序列化与 HTTP 参数污染 (HPP) 风险；\n\
                 3. 针对高优先级输入点给出安全无害的探测 Payload 与差异观察指标。",
            ),
            (
                "生成复现步骤与修复建议",
                "请基于下方 HTTP 流量 / 安全上下文生成一份结构化漏洞报告与修复建议：\n\
                 1. 漏洞概要、严重度评级 (CVSS 考量) 与业务影响分析；\n\
                 2. 清晰的可复现步骤（含最小化 HTTP 请求示例与关键响应证据）；\n\
                 3. 可落地的服务端代码级修复方案、安全响应头加固配置与回归验证检查清单。",
            ),
        ];
        let (title, instruction) = TEMPLATES.get(template_idx).copied().unwrap_or(TEMPLATES[0]);
        let prompt = match context.map(str::trim).filter(|c| !c.is_empty()) {
            Some(ctx) => format!("{instruction}\n\n--- 附带流量 / 漏洞上下文 ---\n{ctx}"),
            None => format!("{instruction}\n\n（尚未附带具体流量上下文，请在 MIO 中通过右键菜单或 Repeater「MioAI」按钮载入目标报文。）"),
        };
        (title, prompt)
    }

    #[inline(never)]
    fn render_mio_ai_control_center(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let template_state =
            window.use_keyed_state("mio-ai-prompt-template-idx", cx, |_, _| 0usize);
        let panel_focus = window
            .use_keyed_state("mio-ai-panel-focus", cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let selected_tpl = *template_state.read(cx);
        let ipc_path = self.effective_ipc_socket_path();
        let ipc_path_str = ipc_path.display().to_string();
        let cli_cmd = format!("mio-cli --socket \"{ipc_path_str}\" status");
        let backend_ok = self.backend_available();
        let proxy_summary = self
            .proxy_address
            .map(|addr| format!("Proxy 运行中 ({addr})"))
            .unwrap_or_else(|| "Proxy 未启动".into());
        let context_opt = self.mio_ai_context.clone();
        let has_context = context_opt
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty());
        let (tpl_name, combined_prompt) =
            Self::build_mio_ai_prompt(selected_tpl, context_opt.as_deref());
        let json_payload = serde_json::to_string_pretty(&serde_json::json!({
            "ipc_socket": ipc_path_str.as_str(),
            "ipc_ready": backend_ok,
            "proxy_address": self.proxy_address.map(|a| a.to_string()),
            "project_name": self.workspace.project_name.as_deref(),
            "template": tpl_name,
            "context": context_opt.as_deref(),
            "prompt": combined_prompt.as_str(),
        }))
        .unwrap_or_else(|_| "{}".into());

        let dark = theme::is_dark();
        let card_bg: gpui_kit::Hsla = rgb(if dark { 0x222428 } else { 0xf7f8fa }).into();
        let status_bg = rgb(if backend_ok {
            if dark { 0x1e3a2b } else { 0xe6f4ea }
        } else if dark {
            0x452b18
        } else {
            0xfef3e6
        });
        let status_fg = rgb(if backend_ok {
            if dark { 0x7ee787 } else { 0x137333 }
        } else if dark {
            0xffb454
        } else {
            0xb54708
        });

        let copy_sock = ipc_path_str.clone();
        let copy_cli = cli_cmd.clone();
        let copy_prompt = combined_prompt.clone();
        let copy_json = json_payload;

        let mut tpl_bar = div().flex().flex_wrap().items_center().gap(px(5.));
        for (idx, (btn_id, label)) in [
            ("mio-ai-tpl-authz", "分析越权与认证缺陷"),
            ("mio-ai-tpl-injection", "检查注入点与参数污染"),
            ("mio-ai-tpl-remediation", "生成复现步骤与修复建议"),
        ]
        .into_iter()
        .enumerate()
        {
            let tpl_entity = template_state.clone();
            tpl_bar = tpl_bar.child(
                button(btn_id, label)
                    .selected(selected_tpl == idx)
                    .tooltip(format!("切换为结构化安全分析模板：{label}"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        tpl_entity.update(cx, |cur, _| *cur = idx);
                        cx.notify();
                    })),
            );
        }

        div()
            .id("mio-ai-control-center-backdrop")
            .test_support()
            .absolute()
            .inset_0()
            .occlude()
            .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.mio_ai_panel_open = false;
                this.focus.focus(window, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("mio-ai-control-center")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Dialog)
                    .aria_label("MioAT / Agent IPC 控制中心")
                    .occlude()
                    .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .absolute()
                    .top(px(12.))
            .right(px(12.))
            .bottom(px(12.))
            .w(px(520.))
            .max_w(px(640.))
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .rounded(px(6.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .focus_trap("mio-ai-control-center", &panel_focus)
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.mio_ai_panel_open = false;
                    this.focus.focus(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                toolbar()
                    .flex_nowrap()
                    .gap(px(6.))
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Sparkles)
                            .size(px(15.))
                            .text_color(rgb(0x4946ae)),
                    )
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("MioAT / Agent IPC 控制中心"),
                    )
                    .child(
                        div()
                            .id("mio-ai-ipc-status-badge")
                            .test_support()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(3.))
                            .bg(status_bg)
                            .text_color(status_fg)
                            .text_size(theme::scaled_px(11.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(if backend_ok { "IPC 已就绪" } else { "IPC 已断开" }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("mio-ai-panel-close")
                            .ghost()
                            .small()
                            .compact()
                            .w(theme::scaled_px(24.))
                            .h(theme::scaled_px(24.))
                            .icon(gpui_kit::assets::IconName::Close)
                            .accessibility_label("关闭 MioAT 控制中心")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.mio_ai_panel_open = false;
                                this.focus.focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("mio-ai-control-center-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .p(px(8.))
                            .rounded(px(4.))
                            .bg(card_bg)
                            .border_1()
                            .border_color(theme::border())
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .child("本地 Agent IPC 通道"),
                                    )
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(11.))
                                            .text_color(theme::muted_foreground())
                                            .child(proxy_summary),
                                    ),
                            )
                            .child(
                                div()
                                    .id("mio-ai-socket-path")
                                    .test_support()
                                    .px(px(6.))
                                    .py(px(4.))
                                    .rounded(px(3.))
                                    .bg(theme::background())
                                    .border_1()
                                    .border_color(theme::border())
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::scaled_px(11.))
                                    .text_color(theme::foreground())
                                    .child(format!("{}={}", mio_core::ENV_MIO_IPC_SOCKET, ipc_path_str)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        button("mio-ai-copy-socket", "复制 Socket 路径")
                                            .icon(gpui_kit::assets::IconName::Copy)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        copy_sock.clone(),
                                                    ),
                                                );
                                                this.status =
                                                    "已复制 Agent IPC Socket 路径到剪贴板。".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        button("mio-ai-copy-cli", "复制 CLI 状态命令")
                                            .icon(gpui_kit::assets::IconName::Terminal)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        copy_cli.clone(),
                                                    ),
                                                );
                                                this.status =
                                                    "已复制 mio-cli Agent IPC 状态命令到剪贴板。"
                                                        .into();
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .p(px(8.))
                            .rounded(px(4.))
                            .bg(card_bg)
                            .border_1()
                            .border_color(theme::border())
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .child(format!(
                                                "预载流量 / 漏洞上下文{}",
                                                context_opt
                                                    .as_ref()
                                                    .map(|c| format!("（{} 字符）", c.chars().count()))
                                                    .unwrap_or_default()
                                            )),
                                    )
                                    .child(
                                        button("mio-ai-clear-context", "清空")
                                            .icon(gpui_kit::assets::IconName::Trash)
                                            .disabled(!has_context)
                                            .tooltip("清空当前预载的流量与问题上下文")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.mio_ai_context = None;
                                                this.status = "已清空 MioAT 上下文。".into();
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .id("mio-ai-context-preview")
                                    .test_support()
                                    .max_h(px(140.))
                                    .overflow_y_scroll()
                                    .p(px(6.))
                                    .rounded(px(3.))
                                    .bg(theme::background())
                                    .border_1()
                                    .border_color(theme::border())
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::scaled_px(11.))
                                    .text_color(if has_context {
                                        theme::foreground()
                                    } else {
                                        theme::muted_foreground()
                                    })
                                    .child(
                                        context_opt.unwrap_or_else(|| {
                                            "尚未载入报文或漏洞上下文；可在 Repeater 点击「MioAI」或在各工具右键菜单中选择发送到 MioAT。".into()
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .p(px(8.))
                            .rounded(px(4.))
                            .bg(card_bg)
                            .border_1()
                            .border_color(theme::border())
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child("一键结构化安全分析提示词模板"),
                            )
                            .child(tpl_bar)
                            .child(
                                div()
                                    .id("mio-ai-prompt-preview")
                                    .test_support()
                                    .max_h(px(200.))
                                    .overflow_y_scroll()
                                    .p(px(8.))
                                    .rounded(px(3.))
                                    .bg(theme::background())
                                    .border_1()
                                    .border_color(theme::border())
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::scaled_px(11.5))
                                    .text_color(theme::foreground())
                                    .child(combined_prompt),
                            ),
                    ),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        button("mio-ai-copy-prompt", "复制完整提示词")
                            .primary()
                            .icon(gpui_kit::assets::IconName::Copy)
                            .tooltip("将所选安全分析模板与预载上下文拼接后复制到剪贴板")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    copy_prompt.clone(),
                                ));
                                this.status = "已复制 MioAT 完整安全分析提示词到剪贴板。".into();
                                cx.notify();
                            })),
                    )
                    .child(
                        button("mio-ai-copy-json", "复制 JSON 上下文")
                            .icon(gpui_kit::assets::IconName::Code)
                            .tooltip("将 Agent IPC 路径、提示词与上下文以结构化 JSON 复制到剪贴板")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    copy_json.clone(),
                                ));
                                this.status = "已复制 MioAT JSON 上下文到剪贴板。".into();
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("mio-ai-close", "关闭").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.mio_ai_panel_open = false;
                                this.focus.focus(window, cx);
                                cx.notify();
                            },
                        )),
                    ),
            )
            )
            .into_any_element()
    }

    fn save_csrf_poc_to_file(
        &mut self,
        html: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_picker_pending() || !self.backend_available() || self.transition.is_some() {
            return;
        }
        let project = self.project;
        let picker = mio_core::Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some("csrf-poc.html"),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let pending_dest = owner
                .update(cx, |_, _, cx| {
                    this.update(cx, |view, cx| {
                        if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                            return None;
                        }
                        if view.project != project {
                            view.fail("项目已切换，本次 CSRF PoC 保存已取消。", cx);
                            return None;
                        }
                        match result {
                            Ok(Ok(Some(mut dest))) => {
                                if dest.extension().is_none() {
                                    dest.set_extension("html");
                                }
                                Some(dest)
                            }
                            Ok(Ok(None)) => None,
                            Ok(Err(err)) => {
                                view.fail(format!("无法选择保存路径：{err}"), cx);
                                None
                            }
                            Err(_) => {
                                view.fail("文件选择器已意外关闭。", cx);
                                None
                            }
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(dest) = pending_dest else {
                return;
            };
            let write_dest = dest.clone();
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_export_file(&write_dest, html.as_bytes())
                })
                .await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if view.project != project {
                        return;
                    }
                    match write_result {
                        Ok(()) => {
                            view.error = None;
                            view.status = format!("已保存 CSRF PoC HTML 至：{}", dest.display());
                            cx.notify();
                        }
                        Err(err) => {
                            view.fail(format!("保存 CSRF PoC 失败：{err}"), cx);
                        }
                    }
                })
            });
        })
        .detach();
    }

    #[inline(never)]
    fn render_csrf_poc_modal(
        &self,
        title: String,
        html: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let modal_focus = window
            .use_keyed_state("csrf-poc-modal-focus", cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let html_for_copy = html.clone();
        let html_for_save = html.clone();
        let byte_len = html.len();
        div()
            .id("csrf-poc-modal-backdrop")
            .test_support()
            .occlude()
            .absolute()
            .inset_0()
            .bg(gpui_kit::black().opacity(0.35))
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.))
            .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.csrf_poc_modal = None;
                this.focus.focus(window, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("csrf-poc-modal")
                    .test_support()
                    .occlude()
                    .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .role(gpui_kit::accesskit::Role::Dialog)
                    .aria_label(title.clone())
                    .w(px(680.))
                    .max_w(px(860.))
                    .h(px(480.))
                    .max_h(px(620.))
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::border())
                    .rounded(px(6.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .focus_trap("csrf-poc-modal", &modal_focus)
                    .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.csrf_poc_modal = None;
                            this.focus.focus(window, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child(
                        toolbar()
                            .flex_nowrap()
                            .gap(px(6.))
                            .child(
                                gpui_kit::component::Icon::new(
                                    gpui_kit::assets::IconName::ShieldCheck,
                                )
                                .size(px(15.))
                                .text_color(theme::accent()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(11.))
                                    .text_color(theme::muted_foreground())
                                    .child(format!("{byte_len} bytes")),
                            )
                            .child(
                                Button::new("csrf-poc-modal-close-x")
                                    .ghost()
                                    .small()
                                    .compact()
                                    .w(theme::scaled_px(24.))
                                    .h(theme::scaled_px(24.))
                                    .icon(gpui_kit::assets::IconName::Close)
                                    .accessibility_label("关闭 CSRF PoC 弹窗")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.csrf_poc_modal = None;
                                        this.focus.focus(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("csrf-poc-html-code")
                            .test_support()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p(px(12.))
                            .bg(theme::background())
                            .font_family(theme::mono_font_family())
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::foreground())
                            .child(html),
                    )
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .border_t_1()
                            .border_color(theme::border())
                            .bg(theme::toolbar_surface())
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                button("csrf-poc-copy-html", "复制 HTML")
                                    .primary()
                                    .icon(gpui_kit::assets::IconName::Copy)
                                    .tooltip("复制完整 CSRF PoC HTML 代码到剪贴板")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(
                                            gpui_kit::ClipboardItem::new_string(
                                                html_for_copy.clone(),
                                            ),
                                        );
                                        this.status = "已复制 CSRF PoC HTML 到剪贴板。".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button("csrf-poc-save-html", "保存为 .html")
                                    .icon(gpui_kit::assets::IconName::Save)
                                    .disabled(
                                        self.file_picker_pending()
                                            || !self.backend_available()
                                            || self.transition.is_some(),
                                    )
                                    .tooltip("通过系统保存对话框将 CSRF PoC 导出为 .html 文件")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.save_csrf_poc_to_file(
                                            html_for_save.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(div().flex_1())
                            .child(button("csrf-poc-close", "关闭").on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.csrf_poc_modal = None;
                                    this.focus.focus(window, cx);
                                    cx.notify();
                                },
                            ))),
                    ),
            )
            .into_any_element()
    }
}
