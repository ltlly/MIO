use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonRounded;

impl Workbench {
    pub(in crate::workbench) fn refresh_history_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((tab_id, forward, menu)) = self.repeater_history.menu.clone() else {
            return;
        };
        if self
            .repeater_history
            .owner
            .is_none_or(|(_, id)| id != tab_id)
        {
            return;
        }
        let Some(page) = self.repeater_history.page.clone() else {
            return;
        };
        let owner = cx.weak_entity();
        let _ = menu.update(cx, |menu, cx| {
            menu.rebuild(window, cx, |menu, window, cx| {
                history_menu(menu, &page, (tab_id, forward), false, owner, window, cx)
            })
        });
    }

    #[allow(dead_code)]
    pub(in crate::workbench) fn render_repeater_history_controls(
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
        let back_owner = cx.weak_entity();
        let forward_owner = cx.weak_entity();
        let btn_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into();
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(
                div()
                    .w(px(47.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_0()
                    .child(
                        Button::new("repeater-history-previous")
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(30.))
                            .h(px(24.))
                            .px(px(0.))
                            .bg(btn_bg)
                            .tab_stop(false)
                            .icon(IconName::ChevronLeft)
                            .tooltip("回退到当前 Repeater 标签页的上一次发送历史记录")
                            .disabled(previous_disabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.navigate_repeater_history(tab_id, false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("repeater-history-previous-menu")
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(17.))
                            .h(px(24.))
                            .px(px(0.))
                            .bg(btn_bg)
                            .tab_stop(false)
                            .accessibility_label("前面的历史记录")
                            .tooltip("展开早于当前位置的已发送请求历史列表")
                            .child(
                                gpui_kit::component::Icon::new(
                                    gpui_kit::component::IconName::ChevronDown,
                                )
                                .size(px(11.))
                                .test_alt_text("历史下拉箭头"),
                            )
                            .disabled(previous_disabled)
                            .dropdown_menu(move |menu, window, cx| {
                                if let Some(page) = &back_page {
                                    history_menu(
                                        menu,
                                        page,
                                        (tab_id, false),
                                        true,
                                        back_owner.clone(),
                                        window,
                                        cx,
                                    )
                                } else {
                                    menu
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .w(px(47.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_0()
                    .child(
                        Button::new("repeater-history-next")
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(30.))
                            .h(px(24.))
                            .px(px(0.))
                            .bg(btn_bg)
                            .tab_stop(false)
                            .icon(IconName::ChevronRight)
                            .tooltip("前进到当前 Repeater 标签页的下一次发送历史记录")
                            .disabled(next_disabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.navigate_repeater_history(tab_id, true, window, cx)
                            })),
                    )
                    .child(
                        Button::new("repeater-history-next-menu")
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(17.))
                            .h(px(24.))
                            .px(px(0.))
                            .bg(btn_bg)
                            .tab_stop(false)
                            .accessibility_label("后面的历史记录")
                            .tooltip("展开晚于当前位置的已发送请求历史列表")
                            .child(
                                gpui_kit::component::Icon::new(
                                    gpui_kit::component::IconName::ChevronDown,
                                )
                                .size(px(11.))
                                .test_alt_text("历史下拉箭头"),
                            )
                            .disabled(next_disabled)
                            .dropdown_menu(move |menu, window, cx| {
                                if let Some(page) = &forward_page {
                                    history_menu(
                                        menu,
                                        page,
                                        (tab_id, true),
                                        true,
                                        forward_owner.clone(),
                                        window,
                                        cx,
                                    )
                                } else {
                                    menu
                                }
                            }),
                    ),
            )
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

fn history_menu(
    mut menu: PopupMenu,
    page: &RepeaterHistoryPage,
    (tab_id, forward): (TabId, bool),
    initial: bool,
    owner: gpui_kit::WeakEntity<Workbench>,
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
            position.saturating_sub((HISTORY_PAGE_SIZE - 1) as u64)
        };
        request_menu_page(tab_id, forward, start, owner, window, cx);
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
            PopupMenuItem::new(format_repeater_history_menu_label(row)).on_click(
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
                        position.saturating_sub((HISTORY_PAGE_SIZE - 1) as u64)
                    };
                    request_menu_page(tab_id, forward, start, owner.clone(), window, cx);
                })
                .detach();
                menu.item(PopupMenuItem::Label("正在读取历史…".into()))
            },
        );
    }
    menu.scrollable(true).max_h(px(500.))
}

fn format_repeater_history_menu_label(row: &mio_core::RepeaterHistoryEntry) -> String {
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

fn request_menu_page(
    tab_id: TabId,
    forward: bool,
    start: u64,
    owner: gpui_kit::WeakEntity<Workbench>,
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
