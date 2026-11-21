use super::*;
use gpui_kit::assets::IconName;

impl Workbench {
    pub(in crate::workbench) fn render_new_live_task_panel(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.audit_dashboard;
        let can_operate = self.audit_can_operate();
        let draft_audit = pane.live_task_draft_passive_audit;
        let draft_crawl = pane.live_task_draft_passive_crawl;
        let draft_scope = pane.live_task_draft_scope;
        let scope_empty = !self.workspace.scope.has_enabled_includes();

        div()
            .id("dashboard-new-live-task-panel")
            .test_support()
            .flex_shrink_0()
            .px(px(16.))
            .py(px(14.))
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::toolbar_surface())
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                gpui_kit::component::Icon::new(IconName::SlidersHorizontal)
                                    .size(theme::scaled_px(15.))
                                    .text_color(theme::accent()),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(13.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::foreground())
                                    .child("新建 / 配置实时任务 (New live task)"),
                            ),
                    )
                    .child(
                        button("dashboard-live-task-close", "收起")
                            .icon(
                                gpui_kit::component::Icon::new(IconName::ChevronUp)
                                    .size(theme::scaled_px(13.)),
                            )
                            .tooltip("关闭实时任务配置面板")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.new_live_task_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .items_stretch()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.))
                            .p(px(12.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::foreground())
                                            .child("实时被动审计 (Live passive audit)"),
                                    )
                                    .child(
                                        Switch::new("dashboard-live-task-modal-audit")
                                            .color(mio_blue())
                                            .checked(draft_audit)
                                            .disabled(!can_operate)
                                            .accessibility_label("启用实时被动审计")
                                            .on_change(cx.listener(|this, checked, _, cx| {
                                                this.audit_dashboard.live_task_draft_passive_audit =
                                                    *checked;
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child("对经过 Proxy 的流量自动运行被动漏洞审计检查。"),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.))
                            .p(px(12.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::foreground())
                                            .child("实时被动爬行 (Live passive crawl)"),
                                    )
                                    .child(
                                        Switch::new("dashboard-live-task-modal-crawl")
                                            .color(mio_blue())
                                            .checked(draft_crawl)
                                            .disabled(!can_operate)
                                            .accessibility_label("启用实时被动爬行")
                                            .on_change(cx.listener(|this, checked, _, cx| {
                                                this.audit_dashboard.live_task_draft_passive_crawl =
                                                    *checked;
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child("被动发现响应中的链接与表单并自动补充站点地图 (Site map)。"),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.))
                            .p(px(12.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::foreground())
                                    .child("流量作用域 (Traffic scope)"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        button("dashboard-live-task-scope-all", "所有流量")
                                            .selected(
                                                draft_scope == LiveTaskTrafficScope::AllTraffic,
                                            )
                                            .disabled(!can_operate)
                                            .tooltip("捕获并分析经过 Proxy 的全部流量")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.audit_dashboard.live_task_draft_scope =
                                                    LiveTaskTrafficScope::AllTraffic;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        button("dashboard-live-task-scope-suite", "仅限套件范围")
                                            .selected(
                                                draft_scope == LiveTaskTrafficScope::SuiteScopeOnly,
                                            )
                                            .disabled(!can_operate)
                                            .tooltip("仅处理符合 Target > Scope 套件范围规则的流量")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.audit_dashboard.live_task_draft_scope =
                                                    LiveTaskTrafficScope::SuiteScopeOnly;
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .when(
                                draft_scope == LiveTaskTrafficScope::SuiteScopeOnly && scope_empty,
                                |card| {
                                    card.child(
                                        div()
                                            .text_size(theme::scaled_px(11.5))
                                            .text_color(theme::muted_foreground())
                                            .child("提示：当前 Target > Scope 尚未配置包含规则。"),
                                    )
                                },
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        button("dashboard-live-task-apply", "应用 / 启动")
                            .primary()
                            .icon(
                                gpui_kit::component::Icon::new(IconName::Play)
                                    .size(theme::scaled_px(13.)),
                            )
                            .tooltip("应用实时被动审计、被动爬行与作用域配置")
                            .disabled(!can_operate)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_new_live_task_modal(window, cx);
                            })),
                    )
                    .child(
                        button("dashboard-live-task-cancel", "取消")
                            .tooltip("放弃本次实时任务修改")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.new_live_task_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn render_new_scan_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.audit_dashboard.new_scan_open {
            if self.audit_dashboard.new_live_task_open {
                return Some(self.render_new_live_task_panel(cx));
            }
            return None;
        }
        let pane = &self.audit_dashboard;
        let can_operate = self.audit_can_operate();
        let pretty = pane.scan_request_editor.pretty_active();
        let view = pane.scan_request_editor.view();
        let readonly = !can_operate || pane.scan_request_editor.is_readonly_preview();

        Some(
            div()
                .id("dashboard-new-scan-panel")
                .test_support()
                .flex_shrink_0()
                .h(theme::scaled_px(280.).max(px(260.)).min(px(380.)))
                .border_b_1()
                .border_color(theme::border())
                .bg(theme::toolbar_surface())
                .flex()
                .child(
                    div()
                        .id("dashboard-new-scan-form")
                        .w(px(380.))
                        .min_w(px(300.))
                        .max_w(px(440.))
                        .flex_shrink(1.0)
                        .h_full()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p(px(12.))
                        .border_r_1()
                        .border_color(theme::border())
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("新建主动 / 被动扫描向导"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .min_w(theme::scaled_px(68.))
                                        .flex_shrink_0()
                                        .child("目标 URL"),
                                )
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&pane.scan_url_input)
                                            .id("dashboard-scan-url")
                                            .aria_label("扫描目标 URL")
                                            .small()
                                            .disabled(!can_operate),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .min_w(theme::scaled_px(68.))
                                        .flex_shrink_0()
                                        .child("任务标题"),
                                )
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&pane.scan_title_input)
                                            .id("dashboard-scan-title")
                                            .aria_label("扫描任务标题")
                                            .small()
                                            .disabled(!can_operate),
                                    ),
                                ),
                        )
                        .child(self.render_new_scan_mode_and_actions(cx)),
                )
                .child(self.render_new_scan_request_editor(pretty, view, readonly, cx))
                .into_any_element(),
        )
    }

    pub(in crate::workbench) fn render_new_scan_mode_and_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.audit_dashboard;
        let can_operate = self.audit_can_operate();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        button("dashboard-scan-mode-active", "主动 + 被动审计")
                            .tooltip("发送探测载荷执行主动漏洞扫描，并同时分析响应执行被动审计")
                            .selected(pane.scan_mode == AuditTaskMode::PassiveAndActive)
                            .disabled(!can_operate)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.scan_mode = AuditTaskMode::PassiveAndActive;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("dashboard-scan-mode-passive", "仅被动审计")
                            .tooltip("仅发送基础请求并对响应执行无侵入式被动安全检查")
                            .selected(pane.scan_mode == AuditTaskMode::PassiveOnly)
                            .disabled(!can_operate)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.scan_mode = AuditTaskMode::PassiveOnly;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("dashboard-scan-http11", "HTTP/1.1")
                            .tooltip("使用 HTTP/1.1 协议发送扫描请求")
                            .selected(pane.scan_version != HttpVersion::Http2)
                            .disabled(!can_operate)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.scan_version = HttpVersion::Http11;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("dashboard-scan-http2", "HTTP/2")
                            .tooltip("使用 HTTP/2 协议发送扫描请求")
                            .selected(pane.scan_version == HttpVersion::Http2)
                            .disabled(!can_operate)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.scan_version = HttpVersion::Http2;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Checkbox::new("audit-crawl-same-origin")
                    .label("包含同源链接发现 (Crawl)")
                    .checked(pane.crawl_same_origin)
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, value, _, cx| {
                        this.audit_dashboard.crawl_same_origin = *value;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        button("dashboard-start-scan", "开始扫描")
                            .primary()
                            .icon(
                                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Play)
                                    .size(theme::scaled_px(13.)),
                            )
                            .tooltip("按当前配置的目标与扫描模式启动安全扫描任务")
                            .disabled(!can_operate)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_new_audit_scan(window, cx);
                            })),
                    )
                    .child(
                        button("dashboard-cancel-new-scan", "收起")
                            .icon(
                                gpui_kit::component::Icon::new(IconName::ChevronUp)
                                    .size(theme::scaled_px(13.)),
                            )
                            .tooltip("收起新建扫描向导面板")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.new_scan_open = false;
                                this.audit_dashboard.scan_error = None;
                                cx.notify();
                            })),
                    ),
            )
            .when_some(pane.scan_error.clone(), |col, err| {
                col.child(
                    div()
                        .id("dashboard-scan-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(err.clone())
                        .text_size(theme::scaled_px(12.))
                        .text_color(theme::danger_fg())
                        .truncate()
                        .child(err),
                )
            })
            .into_any_element()
    }

    pub(in crate::workbench) fn render_new_scan_request_editor(
        &self,
        pretty: bool,
        view: MessageView,
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.audit_dashboard;
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .child("种子 HTTP 请求")
                    .child(
                        button("dashboard-scan-request-pretty", "Pretty")
                            .selected(pretty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_audit_editor_pretty(
                                    AuditEditorSlot::ScanRequest,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("dashboard-scan-request-raw", "Raw")
                            .selected(!pretty && view == MessageView::Raw)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_audit_editor_view(
                                    AuditEditorSlot::ScanRequest,
                                    MessageView::Raw,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("dashboard-scan-request-hex", "Hex")
                            .selected(!pretty && view == MessageView::Hex)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_audit_editor_view(
                                    AuditEditorSlot::ScanRequest,
                                    MessageView::Hex,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .id("dashboard-scan-request-editor")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .bg(theme::surface())
                    .child(
                        editing::message_editor(&pane.scan_request_editor)
                            .readonly(readonly || pane.scan_request_editor.is_readonly_preview())
                            .size_full()
                            .context_menu(self.request_context_menu(
                                pane.scan_request_editor.state(),
                                readonly,
                            ))
                            .aria_label("扫描向导种子请求编辑器"),
                    ),
            )
            .child(pane.scan_request_editor.render_search_bar(
                "dashboard-scan-request",
                cx,
                |this, action, window, cx| {
                    this.handle_audit_search_action(
                        AuditEditorSlot::ScanRequest,
                        action,
                        window,
                        cx,
                    );
                },
            ))
            .into_any_element()
    }

}
