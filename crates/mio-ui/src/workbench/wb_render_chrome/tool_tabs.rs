use gpui_kit::component::button::{ButtonRounded, ButtonVariants};

use super::*;

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
                        Some(crate::workbench::settings::SettingsPage::SuiteUpdates),
                        Some(crate::workbench::settings::SettingsScopeFilter::User),
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
