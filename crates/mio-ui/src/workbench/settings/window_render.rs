use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::{
    Icon,
    button::ButtonVariants,
    menu::{DropdownMenu, PopupMenuItem},
    table::{Column, TableDelegate},
};

use super::*;
use crate::CloseSettings;

const EXPAND_ALL_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 2h14"/><path d="M3 18h14"/><path d="M10 4.5v11"/><path d="m6.5 8 3.5-3.5L13.5 8"/><path d="m6.5 12 3.5 3.5 3.5-3.5"/></svg>"#;
const COLLAPSE_ALL_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10h14"/><path d="M10 1.8v5.2"/><path d="m6.5 4.2 3.5 3 3.5-3"/><path d="M10 18.2V13"/><path d="m6.5 15.8 3.5-3 3.5 3"/></svg>"#;

pub(super) struct HttpStreamingScopeTable {
    pub(super) owner: WeakEntity<Settings>,
}

const HTTP_STREAMING_COLUMNS: [(&str, f32); 3] = [
    ("Enabled", 74.),
    ("Prefix", 410.),
    ("Include subdomains", 148.),
];

impl TableDelegate for HttpStreamingScopeTable {
    fn columns_count(&self, _: &App) -> usize {
        HTTP_STREAMING_COLUMNS.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), HTTP_STREAMING_COLUMNS[index].0)
            .width((HTTP_STREAMING_COLUMNS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner.upgrade().map_or(0, |owner| {
            owner.read(cx).state.network_http.streaming_scope_rules.len()
        })
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(
            HTTP_STREAMING_COLUMNS[col_ix].0,
            HTTP_STREAMING_COLUMNS[col_ix].1,
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("settings-http-streaming-row", row)).border_b_0()
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (rule, enabled, project, wb) = {
            let settings = owner.read(cx);
            let Some(rule) = settings
                .state
                .network_http
                .streaming_scope_rules
                .get(row)
                .cloned()
            else {
                return div().into_any_element();
            };
            (
                rule,
                settings.state.enabled,
                settings.state.project,
                settings.owner.clone(),
            )
        };
        let id = rule.id;
        match column {
            0 => settings_table_checkbox_cell(
                listeners_tls::indexed_checkbox(
                    ("settings-http-streaming-enabled", row),
                    "",
                    rule.enabled,
                    enabled,
                )
                .p_0()
                .gap_0()
                .on_change(move |st, _, _, cx| {
                    let checked = st == CheckboxState::Checked;
                    let _ = wb.update(cx, |view, cx| {
                        view.update_network_http_settings(
                            project,
                            |s| {
                                if let Some(item) =
                                    s.streaming_scope_rules.iter_mut().find(|r| r.id == id)
                                {
                                    item.enabled = checked;
                                }
                            },
                            cx,
                        )
                    });
                }),
            )
            .into_any_element(),
            1 => div()
                .px(px(6.))
                .truncate()
                .child(rule.prefix)
                .into_any_element(),
            _ => settings_table_checkbox_cell(
                listeners_tls::indexed_checkbox(
                    ("settings-http-streaming-subdomains", row),
                    "",
                    rule.include_subdomains,
                    enabled,
                )
                .p_0()
                .gap_0()
                .on_change(move |st, _, _, cx| {
                    let checked = st == CheckboxState::Checked;
                    let _ = wb.update(cx, |view, cx| {
                        view.update_network_http_settings(
                            project,
                            |s| {
                                if let Some(item) =
                                    s.streaming_scope_rules.iter_mut().find(|r| r.id == id)
                                {
                                    item.include_subdomains = checked;
                                }
                            },
                            cx,
                        )
                    });
                }),
            )
            .into_any_element(),
        }
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let settings = owner.read(cx);
        let rule = settings
            .state
            .network_http
            .streaming_scope_rules
            .get(row)?;
        Some(match column {
            0 => if rule.enabled { "Yes" } else { "No" }.into(),
            1 => rule.prefix.clone().into(),
            _ => if rule.include_subdomains { "Yes" } else { "No" }.into(),
        })
    }
}

impl Settings {
    fn render_nav_group_header(
        &self,
        id: &'static str,
        label: &'static str,
        group_idx: usize,
        collapsed: bool,
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let hover_bg = rgb(if dark { 0x34373c } else { 0xf2f4f7 });
        let chevron_color = rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e });
        div()
            .id(id)
            .test_support()
            .role(Role::Heading)
            .aria_label(label)
            .when(!first, |this| this.mt(px(4.)))
            .flex()
            .items_center()
            .gap(px(5.))
            .min_h(theme::scaled_px(26.).max(px(26.)))
            .px(px(6.))
            .py(px(3.))
            .rounded(px(4.))
            .cursor_pointer()
            .text_color(theme::foreground())
            .hover(move |style| style.bg(hover_bg))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_nav_group(group_idx, cx)))
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(px(13.5))
                .text_color(chevron_color),
            )
            .child(label)
            .into_any_element()
    }

    fn render_sidebar_nav(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = self.search_input.focus_handle(cx).tab_stop(false);
        let query = self.current_search_query(cx);
        let searching = !query.trim().is_empty();
        let scope = self.scope_filter;
        let vis_proxy = Self::page_matches(SettingsPage::Proxy, scope, &query);
        let vis_intruder = Self::page_matches(SettingsPage::Intruder, scope, &query);
        let vis_repeater = Self::page_matches(SettingsPage::Repeater, scope, &query);
        let vis_tools = vis_proxy || vis_intruder || vis_repeater;
        let vis_tasks = Self::page_matches(SettingsPage::ProjectTasks, scope, &query);
        let vis_scope = Self::page_matches(SettingsPage::ProjectScope, scope, &query);
        let vis_project = vis_tasks || vis_scope;
        let vis_sessions = Self::page_matches(SettingsPage::Sessions, scope, &query);
        let vis_conn = Self::page_matches(SettingsPage::NetworkConnections, scope, &query);
        let vis_http = Self::page_matches(SettingsPage::NetworkHttp, scope, &query);
        let vis_tls = Self::page_matches(SettingsPage::NetworkTls, scope, &query);
        let vis_network = vis_conn || vis_http || vis_tls;
        let vis_display = Self::page_matches(SettingsPage::UserDisplay, scope, &query);
        let vis_editor = Self::page_matches(SettingsPage::UserMessageEditor, scope, &query);
        let vis_misc = Self::page_matches(SettingsPage::UserMisc, scope, &query);
        let vis_user = vis_display || vis_editor || vis_misc;
        let any_visible = vis_tools || vis_project || vis_sessions || vis_network || vis_user;

        let tools_collapsed = !searching && self.collapsed_groups[0];
        let project_collapsed = !searching && self.collapsed_groups[1];
        let network_collapsed = !searching && self.collapsed_groups[3];
        let user_collapsed = !searching && self.collapsed_groups[4];

        let intruder_page = self.page == SettingsPage::Intruder;
        let repeater_page = self.page == SettingsPage::Repeater;

        let dark = theme::is_dark();
        let search_icon_color = rgb(if dark { 0xc5c9d1 } else { 0x333333 });
        let toolbar_icon_color = rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e });
        let search_bar = div()
            .h(theme::scaled_px(42.).max(px(42.)))
            .flex_shrink_0()
            .px(px(10.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div().w_full().child(
                    Input::new(&self.search_input)
                        .id("settings-search-input")
                        .aria_label("搜索设置")
                        .small()
                        .cleanable(true)
                        .suffix(
                            Icon::new(IconName::Search)
                                .size(px(14.5))
                                .text_color(search_icon_color),
                        ),
                ),
            );

        let filter_row = div()
            .flex_shrink_0()
            .px(px(10.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .justify_between()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        settings_pill_button(
                            "settings-filter-all",
                            "全部",
                            scope == SettingsScopeFilter::All,
                        )
                        .accessibility_label("筛选全部设置")
                        .tooltip("显示全部项目级与用户级设置分组")
                        .tab_stop(false)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_scope_filter(SettingsScopeFilter::All, cx)
                        })),
                    )
                    .child(
                        settings_pill_button(
                            "settings-filter-user",
                            "用户",
                            scope == SettingsScopeFilter::User,
                        )
                        .accessibility_label("筛选 User 设置")
                        .tooltip("仅显示跨项目生效的用户级偏好设置项")
                        .tab_stop(false)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_scope_filter(SettingsScopeFilter::User, cx)
                        })),
                    )
                    .child(
                        settings_pill_button(
                            "settings-filter-project",
                            "项目",
                            scope == SettingsScopeFilter::Project,
                        )
                        .accessibility_label("筛选 Project 设置")
                        .tooltip("仅显示当前项目专属的设置项")
                        .tab_stop(false)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_scope_filter(SettingsScopeFilter::Project, cx)
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(
                        Button::new("settings-nav-expand-all")
                            .ghost()
                            .size(px(24.))
                            .p_0()
                            .tab_stop(false)
                            .accessibility_label("展开全部设置分组")
                            .tooltip("展开全部设置分组")
                            .child(
                                Icon::default()
                                    .data(EXPAND_ALL_SVG)
                                    .size(px(17.))
                                    .text_color(toolbar_icon_color),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.expand_all_nav_groups(cx)
                            })),
                    )
                    .child(
                        Button::new("settings-nav-collapse-all")
                            .ghost()
                            .size(px(24.))
                            .p_0()
                            .tab_stop(false)
                            .accessibility_label("折叠全部设置分组")
                            .tooltip("折叠全部设置分组")
                            .child(
                                Icon::default()
                                    .data(COLLAPSE_ALL_SVG)
                                    .size(px(17.))
                                    .text_color(toolbar_icon_color),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.collapse_all_nav_groups(cx)
                            })),
                    ),
            );

        let nav_tree_inner = div()
            .px(px(8.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .when(!any_visible, |this| {
                this.child(
                    div()
                        .id("settings-search-empty")
                        .test_support()
                        .px(px(8.))
                        .py(px(8.))
                        .text_color(theme::muted_foreground())
                        .child("无匹配的设置项"),
                )
            })
            .when(vis_tools, |this| {
                this.child(self.render_nav_group_header(
                    "settings-tools",
                    "Tools",
                    0,
                    tools_collapsed,
                    true,
                    cx,
                ))
            })
            .when(vis_proxy && !tools_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-proxy",
                        "Proxy",
                        self.page == SettingsPage::Proxy,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::Proxy, window, cx)
                    })),
                )
            })
            .when(vis_intruder && !tools_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-intruder",
                        "Intruder",
                        intruder_page,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = SettingsPage::Intruder;
                        cx.notify();
                    })),
                )
            })
            .when(vis_repeater && !tools_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-repeater",
                        "Repeater",
                        repeater_page,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = SettingsPage::Repeater;
                        cx.notify();
                    })),
                )
            })
            .when(vis_project, |this| {
                this.child(self.render_nav_group_header(
                    "settings-project",
                    "Project",
                    1,
                    project_collapsed,
                    !vis_tools,
                    cx,
                ))
            })
            .when(vis_scope && !project_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-project-scope-nav",
                        "Scope",
                        self.page == SettingsPage::ProjectScope,
                        window,
                        cx,
                    )
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::ProjectScope, window, cx)
                    })),
                )
            })
            .when(vis_tasks && !project_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-project-tasks",
                        "Tasks",
                        self.page == SettingsPage::ProjectTasks,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::ProjectTasks, window, cx)
                    })),
                )
            })
            .when(vis_sessions, |this| {
                this.child(
                    div().mt(px(4.)).child(
                        settings_navigation(
                            "settings-sessions-nav",
                            "Sessions",
                            self.page == SettingsPage::Sessions,
                            window,
                            cx,
                        )
                        .pl(px(24.))
                        .tab_stop(false)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.select_page(SettingsPage::Sessions, window, cx)
                        })),
                    ),
                )
            })
            .when(vis_network, |this| {
                this.child(self.render_nav_group_header(
                    "settings-network",
                    "Network",
                    3,
                    network_collapsed,
                    false,
                    cx,
                ))
            })
            .when(vis_conn && !network_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-network-connections",
                        "Connections",
                        self.page == SettingsPage::NetworkConnections,
                        window,
                        cx,
                    )
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::NetworkConnections, window, cx)
                    })),
                )
            })
            .when(vis_tls && !network_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-network-tls",
                        "TLS",
                        self.page == SettingsPage::NetworkTls,
                        window,
                        cx,
                    )
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::NetworkTls, window, cx)
                    })),
                )
            })
            .when(vis_http && !network_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-network-http",
                        "HTTP",
                        self.page == SettingsPage::NetworkHttp,
                        window,
                        cx,
                    )
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::NetworkHttp, window, cx)
                    })),
                )
            })
            .when(vis_user, |this| {
                this.child(self.render_nav_group_header(
                    "settings-user",
                    "User interface",
                    4,
                    user_collapsed,
                    false,
                    cx,
                ))
            })
            .when(vis_editor && !user_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-user-message-editor",
                        "Message editor",
                        self.page == SettingsPage::UserMessageEditor,
                        window,
                        cx,
                    )
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::UserMessageEditor, window, cx)
                    })),
                )
            })
            .when(vis_misc && !user_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-user-misc",
                        "Hotkeys",
                        self.page == SettingsPage::UserMisc,
                        window,
                        cx,
                    )
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::UserMisc, window, cx)
                    })),
                )
            })
            .when(vis_display && !user_collapsed, |this| {
                this.child(
                    settings_navigation(
                        "settings-user-display",
                        "Display",
                        self.page == SettingsPage::UserDisplay,
                        window,
                        cx,
                    )
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_page(SettingsPage::UserDisplay, window, cx)
                    })),
                )
            });

        div()
            .w(theme::scaled_px(312.).clamp(px(296.), px(340.)))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(search_bar)
            .child(filter_row)
            .child(settings_scrollable_pane(
                "settings-nav-scroll",
                &self.nav_scroll,
                nav_tree_inner,
            ))
            .into_any_element()
    }

    fn render_content_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let breadcrumb: (&'static str, Option<&'static str>) = match self.page {
            SettingsPage::Proxy => ("Tools", Some("Proxy")),
            SettingsPage::Intruder => ("Tools", Some("Intruder")),
            SettingsPage::Repeater => ("Tools", Some("Repeater")),
            SettingsPage::Sessions => ("Sessions", None),
            SettingsPage::ProjectScope => ("Project", Some("Scope")),
            SettingsPage::ProjectTasks => ("Project", Some("Tasks")),
            SettingsPage::NetworkConnections => ("Network", Some("Connections")),
            SettingsPage::NetworkHttp => ("Network", Some("HTTP")),
            SettingsPage::NetworkTls => ("Network", Some("TLS")),
            SettingsPage::UserDisplay => ("User interface", Some("Display")),
            SettingsPage::UserMessageEditor => ("User interface", Some("Message editor")),
            SettingsPage::UserMisc => ("User interface", Some("Hotkeys")),
        };
        let enabled = self.state.enabled;
        let owner = cx.weak_entity();
        div()
            .h(theme::scaled_px(42.).max(px(42.)))
            .flex_shrink_0()
            .px(px(22.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_color(theme::foreground())
                    .child(breadcrumb.0)
                    .when_some(breadcrumb.1, |this, sub| {
                        this.child(
                            Icon::new(IconName::ChevronRight)
                                .size(px(13.))
                                .text_color(theme::muted_foreground()),
                        )
                        .child(sub)
                    }),
            )
            .child(
                Button::new("settings-manage-global")
                    .ghost()
                    .small()
                    .tab_stop(false)
                    .px(px(6.))
                    .text_color(theme::foreground())
                    .accessibility_label("管理全局设置")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child("管理全局设置")
                            .child(
                                Icon::new(IconName::EllipsisVertical)
                                    .size(px(15.))
                                    .text_color(theme::foreground()),
                            ),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        let o_exp = owner.clone();
                        let o_imp = owner.clone();
                        let o_open = owner.clone();
                        let o_close = owner.clone();
                        menu.item(
                            PopupMenuItem::new("复制项目设置 JSON")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = o_exp.update(cx, |this, cx| {
                                        this.export_project_settings_to_clipboard(window, cx)
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("导入项目设置 JSON")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = o_imp.update(cx, |this, cx| {
                                        this.import_project_settings_from_clipboard(window, cx)
                                    });
                                }),
                        )
                        .separator()
                        .item(PopupMenuItem::new("展开全部设置分组").on_click(
                            move |_, _, cx| {
                                let _ = o_open
                                    .update(cx, |this, cx| this.expand_all_nav_groups(cx));
                            },
                        ))
                        .item(PopupMenuItem::new("折叠全部设置分组").on_click(
                            move |_, _, cx| {
                                let _ = o_close
                                    .update(cx, |this, cx| this.collapse_all_nav_groups(cx));
                            },
                        ))
                    }),
            )
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.owner.upgrade().is_none() {
            window.remove_window();
            return div().into_any_element();
        }
        let sidebar = self.render_sidebar_nav(window, cx);
        let content_header = self.render_content_header(cx);
        let intruder_page = self.page == SettingsPage::Intruder;
        let repeater_page = self.page == SettingsPage::Repeater;
        let title_bar = crate::chrome::render_title_bar(
            "mio-settings",
            "设置",
            crate::chrome::TitleBarControls::FULL,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        div()
            .id("mio-settings")
            .test_support()
            .key_context("MioSettings")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::surface())
            .on_action(cx.listener(|_, _: &CloseSettings, window, _| window.remove_window()))
            .on_action(
                cx.listener(|_, _: &crate::OpenSettings, window, _| window.activate_window()),
            )
            .on_action(|_: &crate::Quit, _, cx| {
                cx.global::<crate::native_terminate::NativeTerminate>()
                    .request();
            })
            .on_action(cx.listener(|this, _: &SaveProject, _, cx| this.save(false, cx)))
            .on_action(cx.listener(|this, _: &SaveProjectAs, _, cx| this.save(true, cx)))
            .child(title_bar)
            .child(
                div().flex_1().min_h_0().flex().child(sidebar).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .bg(theme::surface())
                        .child(content_header)
                        .when_some(self.project_json_feedback.clone(), |this, (is_err, msg)| {
                            this.child(
                                div()
                                    .id("settings-project-json-feedback")
                                    .test_support()
                                    .role(if is_err { Role::Alert } else { Role::Status })
                                    .aria_label(msg.clone())
                                    .px(px(22.))
                                    .py(px(6.))
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .bg(if is_err {
                                        theme::danger_bg()
                                    } else {
                                        theme::toolbar_surface()
                                    })
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(if is_err {
                                        theme::danger_fg()
                                    } else {
                                        theme::foreground()
                                    })
                                    .child(msg),
                            )
                        })
                        .when(intruder_page, |this| {
                            this.child(self.render_intruder_preferences(window, cx))
                        })
                        .when(self.page == SettingsPage::ProjectTasks, |this| {
                            this.child(self.render_pool_page(window, cx))
                        })
                        .when(self.page == SettingsPage::ProjectScope, |this| {
                            this.child(self.render_scope(window, cx))
                        })
                        .when(self.page == SettingsPage::NetworkConnections, |this| {
                            this.child(self.render_connections(window, cx))
                        })
                        .when(self.page == SettingsPage::NetworkHttp, |this| {
                            this.child(self.render_network_http(window, cx))
                        })
                        .when(self.page == SettingsPage::NetworkTls, |this| {
                            this.child(self.render_tls(window, cx))
                        })
                        .when(self.page == SettingsPage::Proxy, |this| {
                            this.child(self.render_proxy_rules(window, cx))
                        })
                        .when(repeater_page, |this| {
                            this.child(self.render_repeater_page(window, cx))
                        })
                        .when(self.page == SettingsPage::Sessions, |this| {
                            this.child(self.render_sessions(window, cx))
                        })
                        .when(self.page == SettingsPage::UserDisplay, |this| {
                            this.child(self.render_user_display(window, cx))
                        })
                        .when(self.page == SettingsPage::UserMessageEditor, |this| {
                            this.child(self.render_user_message_editor(window, cx))
                        })
                        .when(self.page == SettingsPage::UserMisc, |this| {
                            this.child(self.render_user_misc(window, cx))
                        }),
                ),
            )
            .into_any_element()
    }
}

impl Settings {
    pub(in crate::workbench) fn render_network_http(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.state.project;
        let enabled = self.state.enabled;
        let nh = &self.state.network_http;
        let section = |id: &'static str, title: &'static str, desc: &'static str, first: bool| {
            div()
                .when(!first, |this| {
                    this.pt(px(10.)).border_t_1().border_color(theme::border())
                })
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(settings_section_heading(id, title, project_scope_badge()))
                .child(settings_desc_row(desc))
        };
        let cb = |id: &'static str, label: &'static str, checked: bool| {
            listeners_tls::indexed_checkbox(id, label, checked, enabled).py(px(1.))
        };
        let o1 = self.owner.clone();
        let o2 = self.owner.clone();
        let o3 = self.owner.clone();
        let o4 = self.owner.clone();
        let o5 = self.owner.clone();
        let o6 = self.owner.clone();
        let o7 = self.owner.clone();
        let o8 = self.owner.clone();
        let o9 = self.owner.clone();
        let o_r1 = self.owner.clone();
        let o_r2 = self.owner.clone();
        let o_r3 = self.owner.clone();
        let o_r4 = self.owner.clone();
        let o_r5 = self.owner.clone();
        let o_adv = self.owner.clone();
        let e_sub = cx.entity();
        let has_sel = self
            .http_streaming_scope_selected
            .is_some_and(|idx| idx < nh.streaming_scope_rules.len());

        let streaming_actions = div()
            .w(theme::scaled_px(86.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                settings_action_button("settings-streaming-scope-add", "添加", !enabled)
                    .w_full()
                    .h(px(22.))
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.http_streaming_scope_editing = None;
                        this.http_streaming_scope_subdomains = false;
                        this.http_streaming_scope_editor_open = true;
                        this.http_streaming_scope_input.update(cx, |inp, cx| {
                            inp.set_value("", window, cx);
                            inp.focus(window, cx);
                        });
                        cx.notify();
                    })),
            )
            .child(
                settings_action_button(
                    "settings-streaming-scope-edit",
                    "编辑",
                    !enabled || !has_sel,
                )
                .w_full()
                .h(px(22.))
                .tab_stop(false)
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Some(idx) = this.http_streaming_scope_selected
                        && let Some(rule) = this
                            .state
                            .network_http
                            .streaming_scope_rules
                            .get(idx)
                            .cloned()
                    {
                        this.http_streaming_scope_editing = Some(rule.id);
                        this.http_streaming_scope_editor_open = true;
                        this.http_streaming_scope_subdomains = rule.include_subdomains;
                        this.http_streaming_scope_input.update(cx, |inp, cx| {
                            inp.set_value(rule.prefix, window, cx);
                            inp.focus(window, cx);
                        });
                        cx.notify();
                    }
                })),
            )
            .child(
                settings_action_button(
                    "settings-streaming-scope-remove",
                    "移除",
                    !enabled || !has_sel,
                )
                .w_full()
                .h(px(22.))
                .tab_stop(false)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(idx) = this.http_streaming_scope_selected
                        && let Some(rule_id) = this
                            .state
                            .network_http
                            .streaming_scope_rules
                            .get(idx)
                            .map(|r| r.id)
                    {
                        if this.http_streaming_scope_editing == Some(rule_id) {
                            this.http_streaming_scope_editing = None;
                            this.http_streaming_scope_editor_open = false;
                        }
                        this.http_streaming_scope_selected = None;
                        let _ = this.owner.update(cx, |v, cx| {
                            v.update_network_http_settings(
                                project,
                                |s| s.streaming_scope_rules.retain(|r| r.id != rule_id),
                                cx,
                            )
                        });
                        cx.notify();
                    }
                })),
            )
            .child(
                settings_action_button("settings-streaming-scope-paste", "粘贴 URL", !enabled)
                    .w_full()
                    .h(px(22.))
                    .tab_stop(false)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            let trimmed = text.trim().to_owned();
                            if !trimmed.is_empty()
                                && trimmed.len() <= 2048
                                && !trimmed.chars().any(char::is_control)
                            {
                                let _ = this.owner.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| {
                                            if s.streaming_scope_rules.len() < 256 {
                                                s.streaming_scope_rules.push(mio_core::ScopeRule {
                                                    id: mio_core::Uuid::new_v4(),
                                                    enabled: true,
                                                    prefix: trimmed,
                                                    include_subdomains: false,
                                                });
                                            }
                                        },
                                        cx,
                                    )
                                });
                            }
                        }
                    })),
            )
            .child(
                settings_action_button("settings-streaming-scope-load", "加载...", !enabled)
                    .w_full()
                    .h(px(22.))
                    .tab_stop(false)
                    .on_click(cx.listener(move |_this, _, window, cx| {
                        let paths = crate::native_terminate::prompt_for_paths(
                            window,
                            cx,
                            gpui_kit::PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple: false,
                                prompt: Some("加载流式响应 URL 前缀列表".into()),
                            },
                        );
                        cx.spawn_in(window, async move |this, cx| {
                            let picked = match paths.await {
                                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                                Ok(Ok(None)) => return,
                                Ok(Err(err)) => {
                                    let _ = this.update_in(cx, |this, _, cx| {
                                        this.settings_error(format!("打开文件选择器失败：{err}"), cx);
                                    });
                                    return;
                                }
                                Err(_) => {
                                    let _ = this.update_in(cx, |this, _, cx| {
                                        this.settings_error("文件选择器通道意外关闭。", cx);
                                    });
                                    return;
                                }
                            };
                            let Some(path) = picked else {
                                return;
                            };
                            let result: Result<Vec<String>, String> = cx
                                .background_executor()
                                .spawn(async move {
                                    let meta = std::fs::metadata(&path)
                                        .map_err(|err| format!("读取文件元数据失败：{err}"))?;
                                    if !meta.is_file() {
                                        return Err("所选路径不是普通文件。".into());
                                    }
                                    if meta.len() > 256 * 1024 {
                                        return Err("规则文件超过 256 KiB 上限，未导入。".into());
                                    }
                                    let text = std::fs::read_to_string(&path)
                                        .map_err(|err| format!("读取 UTF-8 规则文件失败：{err}"))?;
                                    let entries: Vec<String> = text
                                        .lines()
                                        .map(str::trim)
                                        .filter(|line| {
                                            !line.is_empty()
                                                && line.len() <= 2048
                                                && !line.chars().any(char::is_control)
                                        })
                                        .map(str::to_owned)
                                        .collect();
                                    if entries.is_empty() {
                                        return Err("文件中未找到有效的 URL 前缀规则。".into());
                                    }
                                    Ok(entries)
                                })
                                .await;
                            let _ = this.update_in(cx, |this, _, cx| {
                                match result {
                                    Ok(entries) => {
                                        let _ = this.owner.update(cx, |v, cx| {
                                            let mut added = 0usize;
                                            let mut skipped = 0usize;
                                            v.update_network_http_settings(
                                                project,
                                                |s| {
                                                    for prefix in entries {
                                                        if s.streaming_scope_rules.len() >= 256 {
                                                            skipped += 1;
                                                            continue;
                                                        }
                                                        s.streaming_scope_rules.push(
                                                            mio_core::ScopeRule {
                                                                id: mio_core::Uuid::new_v4(),
                                                                enabled: true,
                                                                prefix,
                                                                include_subdomains: false,
                                                            },
                                                        );
                                                        added += 1;
                                                    }
                                                },
                                                cx,
                                            );
                                            if skipped > 0 {
                                                v.status = format!(
                                                    "已导入 {added} 条规则；已达 256 条上限，跳过 {skipped} 条。"
                                                );
                                            }
                                        });
                                    }
                                    Err(err) => this.settings_error(err, cx),
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    })),
            );

        let content = div()
            .px(px(22.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                section(
                    "settings-network-http1-heading",
                    "HTTP/1",
                    "配置 HTTP/1 连接复用与行尾规范化行为。",
                    true,
                )
                .child(
                    div()
                        .pl(px(31.))
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(1.))
                        .child(
                            cb(
                                "settings-http1-keep-alive",
                                "Use keep-alive for HTTP/1 if the server supports it",
                                !self.state.enforce_http1_connection_close,
                            )
                            .on_change(move |st, _, _, cx| {
                                let _ = o1.update(cx, |v, cx| {
                                    v.set_repeater_enforce_http1_connection_close(
                                        project,
                                        st != CheckboxState::Checked,
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            cb(
                                "settings-http1-normalize-line-endings",
                                "Normalize HTTP/1 line endings",
                                self.state.normalize_http1_line_endings,
                            )
                            .on_change(move |st, _, _, cx| {
                                let _ = o2.update(cx, |v, cx| {
                                    v.set_repeater_normalize_http1_line_endings(
                                        project,
                                        st == CheckboxState::Checked,
                                        cx,
                                    )
                                });
                            }),
                        ),
                ),
            )
            .child(
                section(
                    "settings-network-http2-heading",
                    "HTTP/2",
                    "配置目标服务器支持 HTTP/2 时的协议协商与连接标头处理。",
                    false,
                )
                .child(
                    div()
                        .pl(px(31.))
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(1.))
                        .child(
                            cb(
                                "settings-http2-default",
                                "Default to HTTP/2 if the server supports it",
                                nh.default_to_http2,
                            )
                            .on_change(move |st, _, _, cx| {
                                let _ = o3.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| s.default_to_http2 = st == CheckboxState::Checked,
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            cb(
                                "settings-http2-strip-connection",
                                "Strip Connection header over HTTP/2",
                                self.state.strip_connection_headers_over_http2,
                            )
                            .on_change(move |st, _, _, cx| {
                                let _ = o4.update(cx, |v, cx| {
                                    v.set_repeater_strip_http2_connection_headers(
                                        project,
                                        st == CheckboxState::Checked,
                                        cx,
                                    )
                                });
                            }),
                        ),
                ),
            )
            .child(
                section(
                    "settings-network-redirects-heading",
                    "允许的重定向类型",
                    "控制配置为跟随重定向时识别的重定向类型。",
                    false,
                )
                .child(
                    div()
                        .pl(px(31.))
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(1.))
                        .child(div().mb(px(2.)).child("跟随重定向时，识别以下类型："))
                        .child(
                            cb(
                                "settings-http-redir-3xx",
                                "带 Location 头的 3xx 状态码",
                                nh.redirect_3xx,
                            )
                            .on_change(move |st, _, _, cx| {
                                let checked = st == CheckboxState::Checked;
                                let _ = o_r1.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| s.redirect_3xx = checked,
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            cb(
                                "settings-http-redir-refresh",
                                "Refresh 头",
                                nh.redirect_refresh_header,
                            )
                            .on_change(move |st, _, _, cx| {
                                let checked = st == CheckboxState::Checked;
                                let _ = o_r2.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| s.redirect_refresh_header = checked,
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            cb(
                                "settings-http-redir-meta",
                                "Meta refresh 标签",
                                nh.redirect_meta_refresh,
                            )
                            .on_change(move |st, _, _, cx| {
                                let checked = st == CheckboxState::Checked;
                                let _ = o_r3.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| s.redirect_meta_refresh = checked,
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            cb(
                                "settings-http-redir-js",
                                "JavaScript 驱动",
                                nh.redirect_javascript,
                            )
                            .on_change(move |st, _, _, cx| {
                                let checked = st == CheckboxState::Checked;
                                let _ = o_r4.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| s.redirect_javascript = checked,
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            cb(
                                "settings-http-redir-any-location",
                                "带 Location 头的任意状态码",
                                nh.redirect_any_location,
                            )
                            .on_change(move |st, _, _, cx| {
                                let checked = st == CheckboxState::Checked;
                                let _ = o_r5.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| s.redirect_any_location = checked,
                                        cx,
                                    )
                                });
                            }),
                        ),
                ),
            )
            .child(
                section(
                    "settings-network-streaming-heading",
                    "流式响应",
                    "指定哪些 URL 返回不会自动终止的「流式」响应。",
                    false,
                )
                .child(
                    div()
                        .pl(px(31.))
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .text_size(theme::scaled_px(12.5))
                                .child("MIO 对不同工具处理流式响应的方式如下：")
                                .child("• Proxy 在收到数据时直接将响应透传给客户端。")
                                .child("• Repeater 在收到数据时实时更新响应面板；可在 Tools > Repeater > 流式响应 中设置超时。")
                                .child("• 其他工具忽略流式响应并关闭连接。"),
                        )
                        .child(
                            cb(
                                "settings-streaming-adv-scope",
                                "使用高级范围控制",
                                nh.streaming_advanced_scope,
                            )
                            .on_change(move |st, _, _, cx| {
                                let checked = st == CheckboxState::Checked;
                                let _ = o_adv.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| s.streaming_advanced_scope = checked,
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(9.))
                                .h(theme::scaled_px(122.).max(px(122.)))
                                .child(streaming_actions)
                                .child(
                                    settings_table_frame(
                                        DataTable::new(&self.http_streaming_scope_table)
                                            .with_size(px(22.))
                                            .header_row_height(px(24.))
                                            .bordered(false)
                                            .stripe(false)
                                            .scrollbar_visible(true, false)
                                            .scroll_bounce(false)
                                            .accessibility_label("流式响应 URL 范围"),
                                        true,
                                    )
                                    .max_w(theme::scaled_px(645.).max(px(645.))),
                                ),
                        )
                        .when(self.http_streaming_scope_editor_open, |this| {
                            this.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(8.))
                                    .p(px(6.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .bg(theme::background())
                                    .child(
                                        div().w(px(320.)).child(
                                            Input::new(&self.http_streaming_scope_input)
                                                .id("settings-streaming-scope-input")
                                                .small(),
                                        ),
                                    )
                                    .child(
                                        listeners_tls::indexed_checkbox(
                                            "settings-streaming-scope-sub-cb",
                                            "Include subdomains",
                                            self.http_streaming_scope_subdomains,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e_sub.update(cx, |this, cx| {
                                                this.http_streaming_scope_subdomains =
                                                    st == CheckboxState::Checked;
                                                cx.notify();
                                            });
                                        }),
                                    )
                                    .child(
                                        settings_action_button(
                                            "settings-streaming-scope-ok",
                                            "确定",
                                            !enabled,
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let prefix = match editing::input_snapshot(
                                                &this.http_streaming_scope_input,
                                                window,
                                                cx,
                                            ) {
                                                Ok(val) => val.trim().to_owned(),
                                                Err(err) => {
                                                    this.settings_error(err, cx);
                                                    return;
                                                }
                                            };
                                            if prefix.is_empty()
                                                || prefix.len() > 2048
                                                || prefix.chars().any(char::is_control)
                                            {
                                                this.settings_error(
                                                    "流式响应 URL 前缀不能为空、包含控制字符或超过 2048 字节。",
                                                    cx,
                                                );
                                                return;
                                            }
                                            let editing_id = this.http_streaming_scope_editing;
                                            let include_subdomains =
                                                this.http_streaming_scope_subdomains;
                                            let _ = this.owner.update(cx, |v, cx| {
                                                v.update_network_http_settings(
                                                    project,
                                                    |s| {
                                                        if let Some(id) = editing_id
                                                            && let Some(existing) = s
                                                                .streaming_scope_rules
                                                                .iter_mut()
                                                                .find(|r| r.id == id)
                                                        {
                                                            existing.prefix = prefix;
                                                            existing.include_subdomains =
                                                                include_subdomains;
                                                        } else if s.streaming_scope_rules.len()
                                                            < 256
                                                        {
                                                            s.streaming_scope_rules.push(
                                                                mio_core::ScopeRule {
                                                                    id: mio_core::Uuid::new_v4(),
                                                                    enabled: true,
                                                                    prefix,
                                                                    include_subdomains,
                                                                },
                                                            );
                                                        }
                                                    },
                                                    cx,
                                                )
                                            });
                                            this.http_streaming_scope_editor_open = false;
                                            this.http_streaming_scope_editing = None;
                                            this.http_streaming_scope_subdomains = false;
                                            this.http_streaming_scope_input
                                                .update(cx, |i, cx| i.set_value("", window, cx));
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        settings_action_button(
                                            "settings-streaming-scope-cancel",
                                            "取消",
                                            false,
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.http_streaming_scope_editor_open = false;
                                            this.http_streaming_scope_editing = None;
                                            this.http_streaming_scope_subdomains = false;
                                            this.http_streaming_scope_input
                                                .update(cx, |i, cx| i.set_value("", window, cx));
                                            cx.notify();
                                        })),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_start()
                                .gap(px(1.))
                                .child(
                                    cb(
                                        "settings-streaming-store",
                                        "存储流式响应（可能生成较大的临时文件）",
                                        nh.store_streaming_responses,
                                    )
                                    .on_change(move |st, _, _, cx| {
                                        let _ = o6.update(cx, |v, cx| {
                                            v.update_network_http_settings(
                                                project,
                                                |s| {
                                                    s.store_streaming_responses =
                                                        st == CheckboxState::Checked
                                                },
                                                cx,
                                            )
                                        });
                                    }),
                                )
                                .child(
                                    cb(
                                        "settings-streaming-strip-chunked",
                                        "剥离流式响应中的分块编码元数据",
                                        nh.strip_chunked_metadata_in_streaming,
                                    )
                                    .on_change(move |st, _, _, cx| {
                                        let _ = o7.update(cx, |v, cx| {
                                            v.update_network_http_settings(
                                                project,
                                                |s| {
                                                    s.strip_chunked_metadata_in_streaming =
                                                        st == CheckboxState::Checked
                                                },
                                                cx,
                                            )
                                        });
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .child(
                                            listeners_tls::indexed_checkbox(
                                                "settings-streaming-event-stream",
                                                "将",
                                                nh.treat_event_stream_as_streaming,
                                                enabled,
                                            )
                                            .accessibility_label(
                                                "将 text/event-stream MIME 类型的响应视为流式响应",
                                            )
                                            .py(px(1.))
                                            .on_change(move |st, _, _, cx| {
                                                let _ = o5.update(cx, |v, cx| {
                                                    v.update_network_http_settings(
                                                        project,
                                                        |s| {
                                                            s.treat_event_stream_as_streaming =
                                                                st == CheckboxState::Checked
                                                        },
                                                        cx,
                                                    )
                                                });
                                            }),
                                        )
                                        .child(
                                            div()
                                                .px(px(4.))
                                                .rounded(px(3.))
                                                .bg(rgb(if theme::is_dark() {
                                                    0x34373c
                                                } else {
                                                    0xf1f1f1
                                                }))
                                                .font_family(theme::mono_font_family())
                                                .text_size(theme::scaled_px(12.))
                                                .child("text/event-stream"),
                                        )
                                        .child("MIME 类型的响应视为流式响应"),
                                ),
                        ),
                ),
            )
            .child(
                section(
                    "settings-network-100-continue-heading",
                    "Status 100 响应处理",
                    "配置 100 Continue 临时状态响应及 Expect 请求头处理。",
                    false,
                )
                .child(
                    div()
                        .pl(px(31.))
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(1.))
                        .child(
                            cb(
                                "settings-100-understand",
                                "Understand 100 Continue responses",
                                nh.understand_100_continue,
                            )
                            .on_change(move |st, _, _, cx| {
                                let _ = o8.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| {
                                            s.understand_100_continue =
                                                st == CheckboxState::Checked
                                        },
                                        cx,
                                    )
                                });
                            }),
                        )
                        .child(
                            cb(
                                "settings-100-remove-headers",
                                "Remove 100 Continue headers",
                                nh.remove_100_continue_headers,
                            )
                            .on_change(move |st, _, _, cx| {
                                let _ = o9.update(cx, |v, cx| {
                                    v.update_network_http_settings(
                                        project,
                                        |s| {
                                            s.remove_100_continue_headers =
                                                st == CheckboxState::Checked
                                        },
                                        cx,
                                    )
                                });
                            }),
                        ),
                ),
            );
        settings_scrollable_pane("settings-network-http-page", &self.content_scroll, content)
            .into_any_element()
    }
}
