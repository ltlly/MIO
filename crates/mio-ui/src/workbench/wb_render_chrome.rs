use super::*;
use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::component::command::{Command as CommandView, CommandItem};
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use mio_core::{GlobalSearchHit, GlobalSearchQuery, GlobalSearchResult, GlobalSearchTarget};

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
        self.global_search.open = false;
        self.command_palette_open = true;
        self.command_palette.update(cx, |state, cx| {
            state.set_query("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn open_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_palette_open = false;
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
            ("新建项目", Box::new(NewProject)),
            ("打开项目", Box::new(OpenProject)),
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
            overlays.push(div().id("global-command-palette-overlay").test_support()
                .absolute().top(px(12.)).left(px(180.)).w(px(600.)).max_h(px(480.))
                .bg(theme::surface()).border_1().border_color(theme::border()).rounded(px(6.))
                .overflow_hidden().focus_trap("global-command-palette-overlay", &self.global_search.command_palette_focus)
                .child(command).into_any_element());
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
            overlays.push(div().id("global-search-overlay").test_support()
                .absolute().top(px(12.)).left(px(24.)).right(px(24.)).bottom(px(24.))
                .bg(theme::background()).border_1().border_color(theme::border()).rounded(px(6.))
                .flex().flex_col().focus_trap("global-search-overlay", &self.global_search.focus)
                .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_global_overlays(window, cx);
                    }
                }))
                .child(toolbar()
                    .child(div().w(px(320.)).child(Input::new(&input).id("global-search-input").aria_label("全局搜索").small()))
                    .child(button("global-search-regex", "正则").selected(regex).on_click(cx.listener(|this, _, window, cx| { this.global_search.regex = !this.global_search.regex; this.query_global_search(window, cx); })))
                    .child(button("global-search-case", "区分大小写").selected(case_sensitive).on_click(cx.listener(|this, _, window, cx| { this.global_search.case_sensitive = !this.global_search.case_sensitive; this.query_global_search(window, cx); })))
                    .child(button("global-search-scope", "仅范围内").selected(in_scope_only).on_click(cx.listener(|this, _, window, cx| { this.global_search.in_scope_only = !this.global_search.in_scope_only; this.query_global_search(window, cx); })))
                    .child(button("global-search-run", "搜索").disabled(loading).on_click(cx.listener(|this, _, window, cx| this.query_global_search(window, cx))))
                    .child(button("global-search-close", "关闭").icon(gpui_kit::assets::IconName::Close).on_click(cx.listener(|this, _, window, cx| this.close_global_overlays(window, cx)))))
                .child(if loading { div().id("global-search-loading").test_support().p(px(10.)).child("正在读取全部工具的原始与修改后内容…").into_any_element() } else { list_body })
                .into_any_element());
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
        let ipc_path = mio_core::env_ipc_socket_path()
            .unwrap_or_else(mio_core::default_ipc_socket_path);
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
            .id("mio-ai-control-center")
            .test_support()
            .role(gpui_kit::accesskit::Role::Dialog)
            .aria_label("MioAT / Agent IPC 控制中心")
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
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，本次 CSRF PoC 保存已取消。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut dest))) => {
                            if dest.extension().is_none() {
                                dest.set_extension("html");
                            }
                            match mio_core::ui_files::write_export_file(&dest, html.as_bytes()) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status =
                                        format!("已保存 CSRF PoC HTML 至：{}", dest.display());
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("保存 CSRF PoC 失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => view.fail(format!("无法选择保存路径：{err}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
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
            .absolute()
            .inset_0()
            .bg(gpui_kit::black().opacity(0.35))
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.))
            .child(
                div()
                    .id("csrf-poc-modal")
                    .test_support()
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

pub(in crate::workbench) fn button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .label(label)
        .small()
        .h(theme::control_height())
        .rounded(px(3.))
        .px(theme::scaled_px(10.))
}

pub(in crate::workbench) fn flat_button(
    id: impl Into<gpui_kit::ElementId>,
    inset_focus: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let id = id.into();
    let focus = window
        .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    gpui_kit::base::Button::new(id)
        .track_focus(&focus)
        .relative()
        .when(
            focus.is_focused(window) && window.last_input_was_keyboard(),
            |button| {
                if inset_focus {
                    // Kit's 3pt outer ring must stay inside edge-to-edge root tools.
                    button.child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .bottom(px(3.))
                            .left(px(3.))
                            .right(px(3.))
                            .focus_ring_style(window, cx),
                    )
                } else {
                    button.focus_ring_style(window, cx)
                }
            },
        )
}

#[allow(dead_code)]
pub(in crate::workbench) fn tool_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    tool_tab_with_icon(id, label, None, selected, window, cx)
}

pub(in crate::workbench) fn tool_tab_with_icon(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    icon: Option<gpui_kit::assets::IconName>,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label = label.into();
    let dark = theme::is_dark();
    let normal_bg: gpui_kit::Hsla = if dark {
        if selected {
            rgb(0x35383d).into()
        } else {
            theme::toolbar_surface()
        }
    } else {
        if selected {
            theme::background()
        } else {
            theme::surface()
        }
    };
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(if selected { 0x35383d } else { 0x2f3237 }).into()
    } else {
        rgb(if selected { 0xfbfbfb } else { 0xeeeeee }).into()
    };
    flat_button(id, true, window, cx)
        .role(gpui_kit::accesskit::Role::Tab)
        .accessibility_label(label.clone())
        .aria_selected(selected)
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .h(theme::tool_tab_height())
        .flex_shrink_0()
        .px(theme::scaled_px(10.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .line_height(theme::scaled_px(17.))
        .text_color(if selected && !dark { rgb(0x000000).into() } else { theme::foreground() })
        .flex()
        .items_center()
        .gap(px(3.5))
        .bg(normal_bg)
        .hover(move |s| s.bg(hover_bg))
        .when_some(icon, |tab, ic| {
            tab.child(
                gpui_kit::component::Icon::new(ic)
                    .size(px(11.5 * theme::ui_scale()))
                    .text_color(if selected {
                        theme::accent()
                    } else {
                        theme::muted_foreground()
                    }),
            )
        })
        .child(label)
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(1.))
                    .h(px(3.))
                    .bg(theme::accent()),
            )
        })
}

#[allow(dead_code)]
pub(in crate::workbench) fn secondary_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label = label.into();
    let dark = theme::is_dark();
    let base_bg = theme::toolbar_surface();
    let hover_bg: gpui_kit::Hsla = if selected {
        base_bg
    } else if dark {
        rgb(0x2f3237).into()
    } else {
        rgb(0xeeeeee).into()
    };
    flat_button(id, true, window, cx)
        .role(gpui_kit::accesskit::Role::Tab)
        .accessibility_label(label.clone())
        .aria_selected(selected)
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .h(theme::secondary_tab_height() - px(1.))
        .self_start()
        .flex_shrink_0()
        .px(theme::scaled_px(15.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .line_height(theme::scaled_px(17.))
        .text_color(if selected && !dark { rgb(0x000000).into() } else { theme::foreground() })
        .flex()
        .items_center()
        .gap(px(3.5))
        .bg(base_bg)
        .hover(move |s| s.bg(hover_bg))
        .child(label)
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(3.))
                    .bg(theme::accent()),
            )
        })
}

#[allow(dead_code)]
pub(in crate::workbench) fn secondary_tab_bar() -> gpui_kit::Div {
    div()
        .relative()
        .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(theme::border()))
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(theme::secondary_tab_height())
        .flex_shrink_0()
        .bg(theme::toolbar_surface())
}

pub(in crate::workbench) fn toolbar() -> gpui_kit::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .w_full()
        .min_w_0()
        .gap(theme::scaled_px(5.))
        .px(theme::scaled_px(10.))
        .py(theme::scaled_px(4.))
        .min_h(theme::toolbar_height())
        .flex_shrink_0()
        .bg(theme::toolbar_surface())
        .border_b_1()
        .border_color(theme::border())
}

pub(in crate::workbench) fn sidebar_rail() -> gpui_kit::Div {
    sidebar_rail_with_dock(false)
}

pub(in crate::workbench) fn sidebar_rail_with_dock(dock_left: bool) -> gpui_kit::Div {
    let dark = theme::is_dark();
    let divider_color: gpui_kit::Hsla = if dark {
        theme::border()
    } else {
        rgb(0xdfdfdf).into()
    };
    div()
        .w(theme::scaled_px(35.))
        .flex_shrink_0()
        .h_full()
        .relative()
        .pb(px(39.))
        .when(dock_left, |d| d.border_r_1())
        .when(!dock_left, |d| d.border_l_1())
        .border_color(theme::border())
        .bg(theme::toolbar_surface())
        .flex()
        .flex_col()
        .items_center()
        .child(
            Button::new("sidebar-rail-settings")
                .ghost()
                .small()
                .compact()
                .rounded(ButtonRounded::None)
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .w_full()
                .h(px(39.))
                .p_0()
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                        .size(px(18.))
                        .text_color(theme::foreground()),
                )
                .accessibility_label("用户设置")
                .tooltip("打开用户界面与消息编辑器设置")
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::OpenUserSettings), cx);
                }),
        )
        .child(
            div()
                .absolute()
                .bottom(px(38.))
                .left_0()
                .right_0()
                .h(px(1.))
                .bg(divider_color),
        )
}

fn xml_escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn sidebar_rail_rotated_label_svg_cached(
    label: &str,
    text_box_height: f32,
    dock_left: bool,
) -> std::sync::Arc<[u8]> {
    type RailSvgCache = HashMap<String, Vec<(u32, bool, std::sync::Arc<[u8]>)>>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<RailSvgCache>> =
        std::sync::OnceLock::new();
    let h = text_box_height.round().max(32.0);
    let h_bits = h.to_bits();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some(entries) = guard.get(label)
        && let Some((_, _, bytes)) = entries
            .iter()
            .find(|(bits, left, _)| *bits == h_bits && *left == dock_left)
    {
        return std::sync::Arc::clone(bytes);
    }
    let escaped = xml_escape_attr(label);
    let transform = if dock_left {
        format!("translate(15, {h}) rotate(-90)")
    } else {
        "translate(4, 0) rotate(90)".to_owned()
    };
    let generated: std::sync::Arc<[u8]> = std::sync::Arc::from(
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"19\" height=\"{h}\" viewBox=\"0 0 19 {h}\"><g transform=\"{transform}\"><text x=\"0\" y=\"0\" font-family=\"{font}, sans-serif\" font-size=\"15\" fill=\"#000000\">{escaped}</text></g></svg>",
            font = theme::svg_ui_font_family(),
        )
        .into_bytes(),
    );
    if let Ok(mut guard) = cache.lock() {
        if guard.len() >= 128 && !guard.contains_key(label) {
            guard.clear();
        }
        let entries = guard.entry(label.to_owned()).or_default();
        if !entries
            .iter()
            .any(|(bits, left, _)| *bits == h_bits && *left == dock_left)
        {
            entries.push((h_bits, dock_left, std::sync::Arc::clone(&generated)));
        }
    }
    generated
}

#[cfg(test)]
pub(in crate::workbench) fn sidebar_rail_rotated_label_svg(
    label: &str,
    text_box_height: f32,
    dock_left: bool,
) -> Vec<u8> {
    sidebar_rail_rotated_label_svg_cached(label, text_box_height, dock_left).to_vec()
}

pub(in crate::workbench) fn sidebar_rail_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    a11y_label: &'static str,
    icon: gpui_kit::assets::IconName,
    selected: bool,
    disabled: bool,
    tip: impl Into<SharedString>,
) -> Button {
    sidebar_rail_tab_with_dock(id, label, a11y_label, icon, selected, disabled, false, tip)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::workbench) fn sidebar_rail_tab_with_dock(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    a11y_label: &'static str,
    icon: gpui_kit::assets::IconName,
    selected: bool,
    disabled: bool,
    dock_left: bool,
    tip: impl Into<SharedString>,
) -> Button {
    let is_inspector = a11y_label.eq_ignore_ascii_case("Inspector")
        || label.eq_ignore_ascii_case("Inspector");
    let is_notes = a11y_label.eq_ignore_ascii_case("Notes")
        || a11y_label == "备注"
        || a11y_label == "备注与状态"
        || label.starts_with("Notes");
    let is_explanations = a11y_label.eq_ignore_ascii_case("Explanations")
        || label.eq_ignore_ascii_case("Explanations");
    let is_custom_actions = a11y_label.eq_ignore_ascii_case("Custom actions")
        || label.eq_ignore_ascii_case("Custom actions");
    let has_note_dot = is_notes && label.contains('•');
    let (slot_height, rotated_text, text_box_height): (f32, std::borrow::Cow<'static, str>, f32) =
        if is_inspector {
            (138.0_f32, std::borrow::Cow::Borrowed("Inspector"), 64.0_f32)
        } else if is_notes {
            (115.0_f32, std::borrow::Cow::Borrowed("Notes"), 41.0_f32)
        } else if is_explanations {
            (
                154.0_f32,
                std::borrow::Cow::Borrowed("Explanations"),
                88.0_f32,
            )
        } else if is_custom_actions {
            (
                162.0_f32,
                std::borrow::Cow::Borrowed("Custom actions"),
                102.0_f32,
            )
        } else {
            let clean: String = label
                .chars()
                .map(|ch| if ch == '\n' { ' ' } else { ch })
                .collect();
            (115.0_f32, std::borrow::Cow::Owned(clean), 56.0_f32)
        };
    let dark = theme::is_dark();
    let selected_fill: gpui_kit::Hsla = rgb(if dark { 0x2e436e } else { 0xcadaf0 }).into();
    let selected_fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x000000).into()
    };
    let border_bottom_color: gpui_kit::Hsla = if dark {
        theme::border()
    } else {
        rgb(0xdfdfdf).into()
    };
    let fg_color = if selected {
        selected_fg
    } else {
        theme::foreground()
    };
    let svg_bytes =
        sidebar_rail_rotated_label_svg_cached(rotated_text.as_ref(), text_box_height, dock_left);
    let tab_icon = if is_inspector {
        gpui_kit::component::Icon::default()
            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.35\"><rect x=\"2.1\" y=\"1.5\" width=\"4.6\" height=\"13\" rx=\"2.3\"/><circle cx=\"4.4\" cy=\"11.6\" r=\"1.35\" fill=\"currentColor\" stroke=\"none\"/><rect x=\"9.3\" y=\"1.5\" width=\"4.6\" height=\"13\" rx=\"2.3\"/><circle cx=\"11.6\" cy=\"4.4\" r=\"1.35\" fill=\"currentColor\" stroke=\"none\"/></svg>")
            .size(theme::scaled_px(17.))
            .text_color(fg_color)
    } else if is_explanations {
        gpui_kit::component::Icon::default()
            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.35\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M6 12.2h4M6.6 14h2.8M8 2a4 4 0 0 0-2.5 7.1c.5.4.8 1 .9 1.6h3.2c.1-.6.4-1.2.9-1.6A4 4 0 0 0 8 2z\"/></svg>")
            .size(theme::scaled_px(17.))
            .text_color(fg_color)
    } else if is_custom_actions {
        gpui_kit::component::Icon::default()
            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.35\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M9 1.8L4.2 8.5H8L7 14.2L11.8 7.5H8L9 1.8Z\"/></svg>")
            .size(theme::scaled_px(17.))
            .text_color(fg_color)
    } else {
        gpui_kit::component::Icon::new(icon)
            .size(theme::scaled_px(18.))
            .text_color(fg_color)
    };
    Button::new(id)
        .ghost()
        .rounded(ButtonRounded::None)
        .w_full()
        .h(theme::scaled_px(slot_height))
        .flex_shrink_0()
        .px_0()
        .py_0()
        .relative()
        .selected(selected)
        .toggled(selected)
        .disabled(disabled)
        .accessibility_label(a11y_label)
        .tooltip(tip)
        .when(selected, |b| b.bg(selected_fill).text_color(selected_fg))
        .child(
            div()
                .size_full()
                .pt(theme::scaled_px(15.))
                .flex()
                .flex_col()
                .items_center()
                .justify_start()
                .gap(theme::scaled_px(15.))
                .child(tab_icon)
                .child(
                    gpui_kit::svg()
                        .data(svg_bytes.as_ref())
                        .w(theme::scaled_px(19.))
                        .h(theme::scaled_px(text_box_height))
                        .flex_shrink_0()
                        .text_color(fg_color),
                )
                .when(has_note_dot, |col| {
                    col.child(
                        div()
                            .size(theme::scaled_px(6.))
                            .rounded(theme::scaled_px(3.))
                            .bg(theme::accent()),
                    )
                }),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(1.))
                .bg(border_bottom_color),
        )
}

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_tool_tabs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::base::Tabs {
        self.sync_active_top_suite_tab();
        let active_suite = self.extensions.active_top_suite_tab;
        let dark = theme::is_dark();
        let scroll_handle = window
            .use_keyed_state("mio-tool-tabs-scroll", cx, |_, _| {
                gpui_kit::ScrollHandle::new()
            })
            .read(cx)
            .clone();
        let inner = self.render_tool_tabs_strip(active_suite, &scroll_handle, window, cx);
        let right_actions = self.render_tool_tabs_right_actions(dark, cx);
        gpui_kit::base::Tabs::new("tool-tabs")
            .relative()
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .h(theme::tool_tab_height())
            .flex_shrink_0()
            .bg(theme::surface())
            .child(inner)
            .child(gpui_kit::base::ScrollableMask::vertical_wheel(
                &scroll_handle,
            ))
            .child(right_actions)
            .when(!dark, |tabs| {
                tabs.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bg(rgb(0xf2f2f2)),
                )
            })
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(theme::border()),
            )
    }

    #[inline(never)]
    fn render_tool_tabs_strip(
        &self,
        active_suite: Option<mio_core::Uuid>,
        scroll_handle: &gpui_kit::ScrollHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let builtin_selected = |tool: Tool| active_suite.is_none() && self.tool == tool;
        let suite_tabs = self.extensions.top_suite_tabs(cx);
        let builtin_tabs: [(&'static str, &'static str, Tool, f32, &'static str); 12] = [
            (
                "tool-dashboard",
                "Dashboard",
                Tool::Dashboard,
                87.,
                "监控扫描与攻击任务进度，查看实时被动与主动安全问题活动",
            ),
            (
                "tool-target",
                "Target",
                Tool::Target,
                59.,
                "查看目标站点地图 (Site map)、管理目标范围 (Scope) 与漏洞定义库",
            ),
            (
                "tool-proxy",
                "Proxy",
                Tool::Proxy,
                56.,
                "拦截与修改实时流量，浏览 HTTP 与 WebSocket 历史记录",
            ),
            (
                "tool-intruder",
                "Intruder",
                Tool::Intruder,
                67.,
                "配置载荷位置 (§) 与字典规则，执行自动化参数模糊测试与枚举攻击",
            ),
            (
                "tool-repeater",
                "Repeater",
                Tool::Repeater,
                76.,
                "手动编辑并重放单条 HTTP/1.1、HTTP/2 与 WebSocket 消息",
            ),
            (
                "tool-collaborator",
                "Collaborator",
                Tool::Collaborator,
                93.,
                "生成带外 (OAST) 载荷并监听带外 HTTP 与 DNS 回调交互",
            ),
            (
                "tool-sequencer",
                "Sequencer",
                Tool::Sequencer,
                85.,
                "采集会话令牌样本并执行 FIPS 140-2 与信息熵随机性统计分析",
            ),
            (
                "tool-decoder",
                "Decoder",
                Tool::Decoder,
                72.,
                "对文本与字节数据执行多级编码、解码与密码学哈希转换",
            ),
            (
                "tool-comparer",
                "Comparer",
                Tool::Comparer,
                80.,
                "按分词 (Words) 或字节 (Bytes) 粒度可视化对比两段报文差异",
            ),
            (
                "tool-logger",
                "Logger",
                Tool::Logger,
                62.,
                "统一记录并检索流经所有工具模块的 HTTP 请求与响应日志",
            ),
            (
                "tool-organiser",
                "Organizer",
                Tool::Organiser,
                78.,
                "归档待跟进的关键报文，管理研判状态、颜色高亮与审计备注",
            ),
            (
                "tool-extensions",
                "Extensions",
                Tool::Extensions,
                87.,
                "加载并管理本地 Native 扩展，控制宿主 API 权限",
            ),
        ];
        let mut inner = div()
            .id("tool-tabs-scroll")
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .track_scroll(scroll_handle);
        for (id, label, tool, width, tip) in builtin_tabs {
            inner = inner.child(
                tool_tab_with_icon(id, label, None, builtin_selected(tool), window, cx)
                    .w(theme::scaled_px(width))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                    })
                    .disabled(self.ext_import.visible)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_tool(tool, window, cx);
                    })),
            );
        }
        for (suite_ix, (instance_id, label)) in suite_tabs.into_iter().enumerate() {
            let tab_id = SharedString::from(format!("tool-plugin-{instance_id}"));
            let scroll_for_suite = scroll_handle.clone();
            inner = inner.child(
                tool_tab_with_icon(
                    tab_id,
                    label,
                    None,
                    active_suite == Some(instance_id),
                    window,
                    cx,
                )
                .px(theme::scaled_px(10.))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("切换到此扩展插件的独立工作台视图")
                        .build(window, cx)
                })
                .disabled(self.ext_import.visible)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cancel_plugin_interactions(cx);
                    this.extensions.active_top_suite_tab = Some(instance_id);
                    this.tool = Tool::Extensions;
                    scroll_for_suite.scroll_to_item(12 + suite_ix);
                    cx.notify();
                })),
            );
        }
        let scroll_for_discover = scroll_handle.clone();
        inner
            .child(
                tool_tab_with_icon(
                    "tool-discover",
                    "Discover",
                    None,
                    builtin_selected(Tool::Discover),
                    window,
                    cx,
                )
                .w(theme::scaled_px(74.))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(
                        "查看 Agent IPC 自动化接入状态与核心工作流快捷入口",
                    )
                    .build(window, cx)
                })
                .disabled(self.ext_import.visible)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_tool(Tool::Discover, window, cx);
                    scroll_for_discover.scroll_to_item(0);
                })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_tool_tabs_update_badge(
        &self,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let latest = match &self.update_status {
            mio_core::UpdateStatus::Available(info) => info.latest_version.clone(),
            _ => return None,
        };
        let label = format!("更新 v{latest}");
        Some(
            Button::new("suiteUpdateBadge")
                .ghost()
                .small()
                .compact()
                .rounded(ButtonRounded::Medium)
                .tab_stop(false)
                .h(px(22.))
                .px(px(8.))
                .bg(rgb(if dark { 0x263f2a } else { 0xe8f5e9 }))
                .border_1()
                .border_color(rgb(if dark { 0x3b6e44 } else { 0x81c784 }))
                .text_color(rgb(if dark { 0x81c784 } else { 0x1b5e20 }))
                .text_size(px(11.5))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .label(label)
                .accessibility_label(format!("发现新版本 v{latest}"))
                .tooltip(format!("发现 MIO 新版本 v{latest}；点击打开更新设置"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_settings_filtered(
                        Some(super::settings::SettingsPage::UserMisc),
                        Some(super::settings::SettingsScopeFilter::User),
                        false,
                        window,
                        cx,
                    );
                }))
                .into_any_element(),
        )
    }

    #[inline(never)]
    fn render_tool_tabs_browser_and_palette_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> [AnyElement; 2] {
        let open_browser = Button::new("suiteOpenBrowserButton")
            .ghost()
            .small()
            .compact()
            .rounded(ButtonRounded::None)
            .tab_stop(false)
            .w(px(28.))
            .h(px(18.))
            .p_0()
            .border_0()
            .bg(gpui_kit::transparent_black())
            .child(
                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Globe)
                    .size(px(16.))
                    .text_color(theme::foreground()),
            )
            .accessibility_label("Open browser")
            .tooltip("启动已配置 Proxy 监听地址与 CA 信任的浏览器")
            .disabled(
                self.ext_import.visible
                    || self.project.is_none()
                    || !self.backend_available()
                    || self.transition.is_some(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_configured_browser(cx);
            }))
            .into_any_element();
        let command_palette = Button::new("suiteCommandPaletteButton")
            .ghost()
            .small()
            .compact()
            .rounded(ButtonRounded::None)
            .tab_stop(false)
            .w(px(27.))
            .h(px(18.))
            .p_0()
            .border_0()
            .bg(gpui_kit::transparent_black())
            .child(
                gpui_kit::component::Icon::default()
                    .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.55\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.5 4.2L6.8 8L2.5 11.8\"/><path d=\"M8.5 12.2H13.5\"/></svg>")
                    .size(px(15.))
                    .text_color(theme::foreground()),
            )
            .accessibility_label("打开命令面板")
            .tooltip("搜索并执行全部工作台命令 (⌘K)")
            .on_click(cx.listener(|this, _, window, cx| this.open_command_palette(window, cx)))
            .into_any_element();
        [open_browser, command_palette]
    }

    #[inline(never)]
    fn render_tool_tabs_search_and_settings_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> [AnyElement; 2] {
        let search = Button::new("suiteSearchButton")
            .ghost()
            .small()
            .compact()
            .rounded(ButtonRounded::None)
            .tab_stop(false)
            .w(px(28.))
            .h(px(18.))
            .p_0()
            .border_0()
            .bg(gpui_kit::transparent_black())
            .child(
                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Search)
                    .size(px(15.))
                    .text_color(theme::foreground()),
            )
            .accessibility_label("打开全局搜索")
            .tooltip("搜索全部工具的 HTTP、WebSocket、备注与审计问题 (⌘⇧F)")
            .on_click(cx.listener(|this, _, window, cx| this.open_global_search(window, cx)))
            .into_any_element();
        let settings = Button::new("settingsButton")
            .ghost()
            .small()
            .compact()
            .rounded(ButtonRounded::None)
            .tab_stop(false)
            .w(px(28.))
            .h(px(18.))
            .p_0()
            .border_0()
            .bg(gpui_kit::transparent_black())
            .child(
                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                    .size(px(16.))
                    .text_color(theme::foreground()),
            )
            .accessibility_label("Open settings window")
            .tooltip("打开全局与项目设置 (⌘,)")
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_settings(window, cx);
            }))
            .into_any_element();
        [search, settings]
    }

    #[inline(never)]
    fn render_tool_tabs_mio_ai_action(&self, dark: bool, cx: &mut Context<Self>) -> AnyElement {
        Button::new("suiteMioAiButton")
            .ghost()
            .small()
            .compact()
            .rounded(ButtonRounded::Medium)
            .tab_stop(false)
            .w(px(120.))
            .h(px(25.))
            .px(px(10.))
            .bg(rgb(if dark { 0x313438 } else { 0xf1f1f1 }))
            .border_1()
            .border_color(rgb(if dark { 0x624d43 } else { 0xe5b29a }))
            .text_color(theme::foreground())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Zap)
                            .size(px(14.))
                            .text_color(rgb(0xe86a33)),
                    )
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(13.))
                            .text_color(theme::foreground())
                            .child("MioAT"),
                    ),
            )
            .accessibility_label("MioAT")
            .tooltip("打开 Agent IPC 控制中心与 Discover 导航页")
            .disabled(self.ext_import.visible)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.tool == Tool::Discover {
                    this.mio_ai_panel_open = !this.mio_ai_panel_open;
                    cx.notify();
                } else {
                    this.select_tool(Tool::Discover, window, cx);
                }
            }))
            .into_any_element()
    }

    #[inline(never)]
    fn render_tool_tabs_right_actions(
        &self,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let update_badge = self.render_tool_tabs_update_badge(dark, cx);
        let leading = self.render_tool_tabs_browser_and_palette_actions(cx);
        let trailing = self.render_tool_tabs_search_and_settings_actions(cx);
        let mio_ai = self.render_tool_tabs_mio_ai_action(dark, cx);
        div()
            .id("suiteTitleBarActionsPanel")
            .test_support()
            .ml_auto()
            .flex_shrink_0()
            .h(px(33.))
            .px(px(5.))
            .flex()
            .items_center()
            .gap(px(2.))
            .children(update_badge)
            .children(leading)
            .children(trailing)
            .child(mio_ai)
            .into_any_element()
    }
}
