use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use mio_core::RepeaterHistoryPage;

use super::*;

const REPEATER_HISTORY_PAGE_SIZE: usize = 64;

impl Workbench {
    #[inline(never)]
    pub(super) fn render_repeater_settings_menu(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        Button::new("repeater-settings-menu")
            .small()
            .rounded(ButtonRounded::Size(px(3.)))
            .w(px(24.))
            .h(px(24.))
            .p_0()
            .bg(rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }))
            .border_color(theme::control_border())
            .tab_stop(false)
            .accessibility_label("Repeater 快捷设置")
            .tooltip("配置 Repeater 标头规范化、响应解包与重定向策略")
            .disabled(!enabled)
            .child(
                gpui_kit::component::Icon::new(IconName::Settings)
                    .size(px(17.))
                    .text_color(if enabled {
                        theme::foreground()
                    } else {
                        theme::muted_foreground()
                    }),
            )
            .dropdown_menu(move |menu, _, cx| {
                let Some(entity) = owner.upgrade() else {
                    return menu;
                };
                let view = entity.read(cx);
                let project = view.project;
                let editable =
                    project.is_some() && view.backend_available() && view.transition.is_none();
                let settings = view.workspace.repeater_settings.clone();
                let update_cl = owner.clone();
                let unpack = owner.clone();
                let norm_crlf = owner.clone();
                let h1_close = owner.clone();
                let h2_strip = owner.clone();
                let open_full = owner.clone();
                let mut menu = menu
                    .item(
                        PopupMenuItem::new("自动更新 Content-Length")
                            .disabled(!editable)
                            .checked(settings.update_content_length)
                            .on_click(move |_, window, cx| {
                                let _ = update_cl.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_update_content_length(
                                            project,
                                            !settings.update_content_length,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("自动解包压缩响应 (Gzip / Brotli / Deflate)")
                            .disabled(!editable)
                            .checked(settings.unpack_compressed_responses)
                            .on_click(move |_, window, cx| {
                                let _ = unpack.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_unpack(
                                            project,
                                            !settings.unpack_compressed_responses,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("规范化 HTTP/1 换行符 (\\r\\n)")
                            .disabled(!editable)
                            .checked(settings.normalize_http1_line_endings)
                            .on_click(move |_, window, cx| {
                                let _ = norm_crlf.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_normalize_http1_line_endings(
                                            project,
                                            !settings.normalize_http1_line_endings,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("HTTP/1 强制 Connection: close")
                            .disabled(!editable)
                            .checked(settings.enforce_http1_connection_close)
                            .on_click(move |_, window, cx| {
                                let _ = h1_close.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_enforce_http1_connection_close(
                                            project,
                                            !settings.enforce_http1_connection_close,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("HTTP/2 自动剥离逐跳连接头")
                            .disabled(!editable)
                            .checked(settings.strip_connection_headers_over_http2)
                            .on_click(move |_, window, cx| {
                                let _ = h2_strip.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_strip_http2_connection_headers(
                                            project,
                                            !settings.strip_connection_headers_over_http2,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .separator();
                for (label, mode) in [
                    ("从不跟随重定向", RepeaterRedirectMode::Never),
                    ("仅同站跟随重定向", RepeaterRedirectMode::OnSiteOnly),
                    ("仅范围内跟随重定向", RepeaterRedirectMode::InScopeOnly),
                    ("始终跟随重定向", RepeaterRedirectMode::Always),
                ] {
                    let redirect_owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .disabled(!editable)
                            .checked(settings.follow_redirects == mode)
                            .on_click(move |_, window, cx| {
                                let _ = redirect_owner.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_follow_redirects(project, mode, cx);
                                    }
                                });
                            }),
                    );
                }
                menu.separator().item(
                    PopupMenuItem::new("打开完整 Repeater 设置…").on_click(
                        move |_, window, cx| {
                            let _ = open_full.update(cx, |this, cx| {
                                this.open_settings_filtered(
                                    Some(crate::workbench::settings::SettingsPage::Repeater),
                                    None,
                                    false,
                                    window,
                                    cx,
                                );
                            });
                        },
                    ),
                )
            })
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_repeater_history_nav_pair(
        &self,
        tab_id: TabId,
        forward: bool,
        disabled: bool,
        page: Option<RepeaterHistoryPage>,
        btn_bg: gpui_kit::Hsla,
        btn_hover_bg: gpui_kit::Hsla,
        btn_border: gpui_kit::Hsla,
        icon_fg: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let (btn_id, menu_id, nav_tip, menu_label, menu_tip, nav_icon) = if forward {
            (
                "repeater-history-next",
                "repeater-history-next-menu",
                "前进到当前 Repeater 标签页的下一次发送历史记录",
                "后面的历史记录",
                "展开晚于当前位置的已发送请求历史列表",
                IconName::ChevronRight,
            )
        } else {
            (
                "repeater-history-previous",
                "repeater-history-previous-menu",
                "回退到当前 Repeater 标签页的上一次发送历史记录",
                "前面的历史记录",
                "展开早于当前位置的已发送请求历史列表",
                IconName::ChevronLeft,
            )
        };
        div()
            .w(px(47.))
            .h(px(24.))
            .flex()
            .items_center()
            .gap_0()
            .child(
                Button::new(btn_id)
                    .ghost()
                    .small()
                    .rounded(ButtonRounded::None)
                    .rounded_tl(px(3.))
                    .rounded_bl(px(3.))
                    .w(px(30.))
                    .h(px(24.))
                    .p_0()
                    .bg(btn_bg)
                    .border_1()
                    .border_color(btn_border)
                    .when(!disabled, |b| b.hover(move |s| s.bg(btn_hover_bg)))
                    .tab_stop(false)
                    .tooltip(nav_tip)
                    .disabled(disabled)
                    .child(
                        gpui_kit::component::Icon::new(nav_icon)
                            .size(px(14.))
                            .text_color(icon_fg),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate_repeater_history(tab_id, forward, window, cx)
                    })),
            )
            .child(
                Button::new(menu_id)
                    .ghost()
                    .small()
                    .rounded(ButtonRounded::None)
                    .rounded_tr(px(3.))
                    .rounded_br(px(3.))
                    .w(px(17.))
                    .h(px(24.))
                    .p_0()
                    .bg(btn_bg)
                    .border_t_1()
                    .border_r_1()
                    .border_b_1()
                    .border_color(btn_border)
                    .when(!disabled, |b| b.hover(move |s| s.bg(btn_hover_bg)))
                    .tab_stop(false)
                    .accessibility_label(menu_label)
                    .tooltip(menu_tip)
                    .child(
                        gpui_kit::component::Icon::new(IconName::ChevronDown)
                            .size(px(11.))
                            .text_color(icon_fg)
                            .test_alt_text("历史下拉箭头"),
                    )
                    .disabled(disabled)
                    .dropdown_menu(move |menu, window, cx| {
                        if let Some(page) = &page {
                            repeater_toolbar_history_menu(
                                menu,
                                page,
                                (tab_id, forward),
                                true,
                                owner.clone(),
                                window,
                                cx,
                            )
                        } else {
                            menu
                        }
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(super) fn render_repeater_toolbar_history_controls(
        &self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = self
            .repeater_history
            .owner
            .is_some_and(|(_, id)| id == tab_id);
        let page = owner
            .then_some(self.repeater_history.page.as_ref())
            .flatten();
        let current = page.and_then(|page| page.current_position);
        let busy = self.repeater_history.pending.contains_key(&tab_id)
            || self.repeater_history.querying
            || !self.backend_available()
            || self.transition.is_some();
        let previous_disabled = busy || current.is_none_or(|position| position == 0);
        let next_disabled = busy
            || current.is_none_or(|position| page.is_none_or(|page| position + 1 >= page.total));
        let back_page = page.cloned();
        let forward_page = page.cloned();
        let dark = theme::is_dark();
        let btn_bg: gpui_kit::Hsla = rgb(if dark { 0x2b2d30 } else { 0xffffff }).into();
        let btn_hover_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xf2f2f2 }).into();
        let btn_border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xcccccc }).into();
        let chevron_color = |disabled: bool| -> gpui_kit::Hsla {
            if dark {
                if disabled {
                    theme::muted_foreground()
                } else {
                    theme::foreground()
                }
            } else if disabled {
                rgb(0x666666).into()
            } else {
                rgb(0x444444).into()
            }
        };
        let prev_fg = chevron_color(previous_disabled);
        let next_fg = chevron_color(next_disabled);
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(self.render_repeater_history_nav_pair(
                tab_id,
                false,
                previous_disabled,
                back_page,
                btn_bg,
                btn_hover_bg,
                btn_border,
                prev_fg,
                cx,
            ))
            .child(self.render_repeater_history_nav_pair(
                tab_id,
                true,
                next_disabled,
                forward_page,
                btn_bg,
                btn_hover_bg,
                btn_border,
                next_fg,
                cx,
            ))
            .when_some(
                self.repeater_history.error.as_ref().filter(|_| owner),
                |this, error| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_color(theme::danger_fg())
                            .child(error.clone())
                            .child(
                                button("repeater-history-retry", "重试读取历史")
                                    .icon(IconName::RefreshCw)
                                    .tooltip("重新从项目存储读取当前标签页的发送历史")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.query_repeater_history(
                                            this.repeater_history.query_start,
                                            cx,
                                        )
                                    })),
                            ),
                    )
                },
            )
            .into_any_element()
    }
}

fn repeater_toolbar_history_menu(
    mut menu: PopupMenu,
    page: &RepeaterHistoryPage,
    (tab_id, forward): (TabId, bool),
    initial: bool,
    owner: WeakEntity<Workbench>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(current) = page.current_position else {
        return menu;
    };
    let adjacent = if forward {
        current
            .checked_add(1)
            .filter(|position| *position < page.total)
    } else {
        current.checked_sub(1)
    };
    if initial
        && let Some(position) = adjacent
        && !page.rows.iter().any(|row| row.position == position)
    {
        let start = if forward {
            position
        } else {
            position.saturating_sub((REPEATER_HISTORY_PAGE_SIZE - 1) as u64)
        };
        request_repeater_toolbar_history_page(tab_id, forward, start, owner, window, cx);
        return menu.item(PopupMenuItem::Label("正在读取历史…".into()));
    }
    let mut rows: Vec<_> = page
        .rows
        .iter()
        .filter(|row| {
            if forward {
                row.position > current
            } else {
                row.position < current
            }
        })
        .collect();
    if !forward {
        rows.reverse();
    }
    let next = if forward {
        rows.last()
            .and_then(|row| row.position.checked_add(1))
            .filter(|next| *next < page.total)
            .or_else(|| {
                rows.is_empty()
                    .then_some(current + 1)
                    .filter(|next| *next < page.total)
            })
    } else {
        rows.last()
            .and_then(|row| row.position.checked_sub(1))
            .or_else(|| rows.is_empty().then(|| current.checked_sub(1)).flatten())
    };
    for row in rows {
        let owner = owner.clone();
        let position = row.position;
        let id = row.id;
        menu = menu.item(
            PopupMenuItem::new(format_repeater_toolbar_history_label(row)).on_click(
                move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.select_history_position(tab_id, position, Some(id), window, cx)
                    });
                },
            ),
        );
    }
    if let Some(position) = next {
        menu = menu.submenu(
            "更多历史记录…",
            window,
            cx,
            move |menu, window, cx| {
                let owner = owner.clone();
                let focus = menu.focus_handle(cx);
                cx.on_focus(&focus, window, move |_, window, cx| {
                    let start = if forward {
                        position
                    } else {
                        position.saturating_sub((REPEATER_HISTORY_PAGE_SIZE - 1) as u64)
                    };
                    request_repeater_toolbar_history_page(
                        tab_id,
                        forward,
                        start,
                        owner.clone(),
                        window,
                        cx,
                    );
                })
                .detach();
                menu.item(PopupMenuItem::Label("正在读取历史…".into()))
            },
        );
    }
    menu.scrollable(true).max_h(px(500.))
}

fn format_repeater_toolbar_history_label(row: &mio_core::RepeaterHistoryEntry) -> String {
    let mut label = format!("{}. ", row.position + 1);
    if let Some(method) = row.method.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        label.push_str(method);
        label.push(' ');
    }
    label.push_str(
        row.target
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("URL 未确认"),
    );
    match (
        row.status,
        row.diagnostic
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty()),
    ) {
        (Some(status), Some(diag)) => {
            label.push_str(&format!(" [{status} · {diag}]"));
        }
        (Some(status), None) => {
            label.push_str(&format!(" [{status}]"));
        }
        (None, Some(diag)) => {
            label.push_str(&format!(" [{diag}]"));
        }
        (None, None) => {}
    }
    label
}

fn request_repeater_toolbar_history_page(
    tab_id: TabId,
    forward: bool,
    start: u64,
    owner: WeakEntity<Workbench>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) {
    let popup = cx.weak_entity();
    let handle = window.window_handle();
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, _, cx| {
            let _ = owner.update(cx, |view, cx| {
                if view
                    .repeater_history
                    .owner
                    .is_some_and(|(_, id)| id == tab_id)
                {
                    view.repeater_history.menu = Some((tab_id, forward, popup));
                    view.query_repeater_history(start, cx);
                }
            });
        });
    });
}
