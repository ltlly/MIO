use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::ButtonVariants,
    menu::{DropdownMenu, PopupMenuItem},
    popover::Popover,
};

impl Workbench {
    pub(in crate::workbench) fn is_cancelled_http_query_error(error: &str) -> bool {
        error.trim() == "HTTP 历史查询已取消"
    }

    pub(in crate::workbench) fn refresh_pending_http_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(query) = self.http_filter.pending_query() else {
            return;
        };
        let (filter, sort, enabled) = self.http_filter.requested();
        self.query_http_history_with_limit(
            filter,
            sort,
            enabled,
            query.offset,
            query.limit,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn query_http_history_filtered(
        &mut self,
        filter: HttpHistoryFilter,
        sort: HttpHistorySort,
        enabled: bool,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        if !self.http_filter.search_placeholder_initialized.get() {
            self.http_filter.search_placeholder_initialized.set(true);
            self.history_filter.update(cx, |input, cx| {
                input.set_placeholder("Search", window, cx);
            });
        }
        let loaded = if offset == 0 {
            self.history.read(cx).delegate().rows.len()
        } else {
            0
        };
        let limit = loaded.clamp(HTTP_HISTORY_BATCH_SIZE, 100_000);
        self.query_http_history_with_limit(filter, sort, enabled, offset, limit, window, cx)
    }

    pub(crate) fn load_more_http_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history_loading {
            return;
        }
        let (offset, count, total) = {
            let delegate = self.history.read(cx).delegate();
            (delegate.offset, delegate.rows.len(), delegate.total)
        };
        if offset != 0 || count >= total || count >= 100_000 {
            self.history.update(cx, |table, cx| {
                table.delegate_mut().loading = false;
                cx.notify();
            });
            return;
        }
        let next_limit = count
            .saturating_add(HTTP_HISTORY_BATCH_SIZE)
            .min(100_000);
        let (filter, sort, enabled) = self.http_filter.requested();
        if self
            .query_http_history_with_limit(filter, sort, enabled, 0, next_limit, window, cx)
            .is_none()
        {
            self.history.update(cx, |table, cx| {
                table.delegate_mut().loading = false;
                cx.notify();
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn query_http_history_with_limit(
        &mut self,
        filter: HttpHistoryFilter,
        sort: HttpHistorySort,
        enabled: bool,
        offset: usize,
        limit: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let project = self.project?;
        if !self.backend_available() || self.transition.is_some() {
            return None;
        }
        let url_filter = match editing::input_snapshot(&self.history_filter, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return None;
            }
        };
        if url_filter.len() > 4096 {
            self.fail("URL／方法筛选超过 4096 字节；当前列表保留。", cx);
            return None;
        }
        // Querying does not change the editor; the result handler guards removal of its row.
        let revision = self.query_revision + 1;
        let mut selected_ids: Vec<_> = self
            .history
            .read(cx)
            .delegate()
            .selected_ids
            .iter()
            .copied()
            .collect();
        if let Some(selected) = self.selected_history
            && !selected_ids.contains(&selected)
        {
            selected_ids.push(selected);
        }
        if selected_ids.len() > 500 {
            self.fail("HTTP 历史选择超过 500 条；当前列表保留。", cx);
            return None;
        }
        let query = HttpHistoryQuery {
            project,
            query_revision: revision,
            offset,
            limit: limit.clamp(1, 100_000),
            url_filter,
            filter: if enabled {
                filter.clone()
            } else {
                HttpHistoryFilter::default()
            },
            sort,
            selected_ids,
        };
        if !self.enqueue(
            Command::QueryHttpHistory {
                query: query.clone(),
            },
            cx,
        ) {
            return None;
        }
        let wait_revision = self
            .http_filter
            .pending
            .as_ref()
            .filter(|pending| {
                pending.filter == filter && pending.query.sort == sort && pending.enabled == enabled
            })
            .map_or(revision, |pending| pending.wait_revision);
        let inputs = self.http_filter.pending.as_ref().map_or_else(
            || self.http_filter.inputs.clone(),
            |pending| pending.inputs.clone(),
        );
        self.http_filter.history_shift = None;
        self.http_filter.pending = Some(PendingFilter {
            query,
            wait_revision,
            filter,
            enabled,
            inputs,
        });
        self.query_revision = revision;
        self.history_loading = true;
        self.history_dirty = false;
        self.history.update(cx, |state, cx| {
            state.delegate_mut().loading = true;
            cx.notify();
        });
        cx.notify();
        Some(revision)
    }

    pub(in crate::workbench) fn http_filter_query_matches(&self, query: &HttpHistoryQuery) -> bool {
        self.project == Some(query.project)
            && self.query_revision == query.query_revision
            && (self
                .http_filter
                .pending
                .as_ref()
                .is_some_and(|pending| &pending.query == query)
                || self
                    .http_filter
                    .history_shift
                    .as_ref()
                    .is_some_and(|shift| &shift.query == query))
    }

    pub(in crate::workbench) fn complete_http_filter_query(
        &mut self,
        query: &HttpHistoryQuery,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if !self.http_filter_query_matches(query) {
            return;
        }
        if let Some(shift) = self.http_filter.history_shift.as_ref()
            && shift.query_revision == query.query_revision
        {
            return;
        }
        let pending = self.http_filter.pending.take().unwrap();
        let cancelled = result
            .as_ref()
            .err()
            .is_some_and(|error| Self::is_cancelled_http_query_error(error));
        if result.is_ok() {
            self.http_filter.applied = pending.filter;
            self.http_filter.applied_url_filter = pending.query.url_filter;
            self.http_filter.sort = pending.query.sort;
            self.http_filter.enabled = pending.enabled;
            self.http_filter.inputs = pending.inputs;
        }
        self.history_loading = false;
        self.history.update(cx, |table, cx| {
            table.delegate_mut().loading = false;
            cx.notify();
        });
        if cancelled {
            self.http_filter.outcome = None;
        } else {
            self.http_filter.outcome = Some((pending.wait_revision, result.clone()));
        }
        if let Err(error) = result
            && !cancelled
        {
            self.fail(format!("HTTP 历史查询失败：{error}；当前列表保留。"), cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn reset_http_filter(&mut self, cx: &mut Context<Self>) {
        let old = std::mem::take(&mut self.http_filter);
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.columns = crate::history::HistoryDelegate::default().columns;
            delegate.apply_sort(HttpHistorySort::default());
            table.refresh(cx);
        });
        if let Some(handle) = old.window {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        if let Some(handle) = old.detached_history_window {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
    }

    pub(crate) fn sync_history_all_columns(&mut self, cx: &mut Context<Self>) {
        let scale = theme::ui_scale().max(1.0);
        let all_columns = &mut self.http_filter.all_columns;
        self.history.update(cx, |table, _| {
            let widths: Vec<Option<gpui_kit::Pixels>> = (0..table.delegate().columns.len())
                .map(|ix| table.col_width(ix))
                .collect();
            let delegate = table.delegate_mut();
            for (ix, col) in delegate.columns.iter_mut().enumerate() {
                if let Some(Some(width)) = widths.get(ix)
                    && *width > px(0.)
                {
                    col.width = px((width.as_f32() / scale).round());
                }
            }
            let visible_cols = delegate.columns.clone();
            let visible_keys: std::collections::HashSet<SharedString> =
                visible_cols.iter().map(|c| c.key.clone()).collect();
            let mut visible_iter = visible_cols.into_iter();
            for slot in all_columns.iter_mut() {
                if visible_keys.contains(&slot.key)
                    && let Some(updated) = visible_iter.next()
                {
                    *slot = updated;
                }
            }
        });
    }

    pub(crate) fn toggle_history_column_visibility(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
    ) {
        if key == "number" {
            return;
        }
        self.sync_history_all_columns(cx);
        let sort = self.http_filter.requested().1;
        let all_columns = self.http_filter.all_columns.clone();
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if delegate.columns.iter().any(|c| c.key.as_ref() == key) {
                if delegate.columns.len() > 1 {
                    delegate.columns.retain(|c| c.key.as_ref() != key);
                }
            } else if let Some((target_pos, target_col)) = all_columns
                .iter()
                .enumerate()
                .find(|(_, c)| c.key.as_ref() == key)
            {
                let insert_ix = delegate
                    .columns
                    .iter()
                    .position(|existing| {
                        all_columns
                            .iter()
                            .position(|d| d.key == existing.key)
                            .is_some_and(|pos| pos > target_pos)
                    })
                    .unwrap_or(delegate.columns.len());
                delegate.columns.insert(insert_ix, target_col.clone());
            }
            delegate.apply_sort(sort);
            table.refresh(cx);
        });
    }

    pub(crate) fn restore_default_history_columns(&mut self, cx: &mut Context<Self>) {
        let sort = self.http_filter.requested().1;
        let defaults = crate::history::HistoryDelegate::default().columns;
        self.http_filter.all_columns = crate::history::HistoryDelegate::all_columns();
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.columns = defaults;
            delegate.apply_sort(sort);
            table.refresh(cx);
        });
    }

    pub(in crate::workbench) fn http_filter_disconnected(&mut self, cx: &mut Context<Self>) {
        self.http_filter.history_shift = None;
        if let Some(pending) = self.http_filter.pending.take() {
            self.http_filter.outcome = Some((
                pending.wait_revision,
                Err("后台已断开，筛选结果未确认；当前列表和条件保留。".into()),
            ));
        }
        self.history_loading = false;
        self.history.update(cx, |table, cx| {
            table.delegate_mut().loading = false;
            cx.notify();
        });
        cx.notify();
    }

    pub(in crate::workbench) fn render_http_filter_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.http_filter.search_placeholder_initialized.get() {
            self.http_filter.search_placeholder_initialized.set(true);
            self.history_filter.update(cx, |input, cx| {
                input.set_placeholder("Search", window, cx);
            });
        }
        let label = filter_summary(&self.http_filter.applied);
        let offset = self.history.read(cx).delegate().offset;
        let owner = cx.weak_entity();
        let dark = theme::is_dark();
        let show_clear_search = {
            let filter_state = self.history_filter.read(cx);
            let applied_url = self.http_filter.applied_url_filter.as_str();
            !filter_state.has_marked_text()
                && !applied_url.is_empty()
                && filter_state.value().as_ref() == applied_url
                && self
                    .http_filter
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.query.url_filter == applied_url)
        };
        let filter_bg: gpui_kit::Hsla = if dark {
            rgb(0x35383d).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let filter_hover_bg: gpui_kit::Hsla = if dark {
            rgb(0x3f434a).into()
        } else {
            rgb(0xe2e2e2).into()
        };
        let filter_btn = flat_button("http-history-filter", false, window, cx)
            .accessibility_label(label.clone())
            .h(theme::scaled_px(23.))
            .rounded(px(3.))
            .border_1()
            .border_color(theme::border())
            .px(theme::scaled_px(8.))
            .bg(filter_bg)
            .hover(move |s| s.bg(filter_hover_bg))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_start()
            .gap(theme::scaled_px(6.))
            .overflow_hidden()
            .text_color(theme::foreground())
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "打开 HTTP 历史记录高级筛选窗口（按范围、状态码、MIME、扩展名、正则与注解过滤）",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, window, cx| this.open_http_filter(window, cx)))
            .child(
                gpui_kit::component::Icon::new(IconName::Funnel)
                    .size(theme::scaled_px(13.))
                    .text_color(theme::foreground()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_left()
                    .child(label.clone().test_text(label)),
            );
        toolbar()
            .h(theme::toolbar_height())
            .border_b_0()
            .flex_nowrap()
            .gap(theme::scaled_px(10.))
            .child(filter_btn)
            .child(
                div()
                    .w(theme::scaled_px(254.))
                    .min_w(theme::scaled_px(120.))
                    .flex_shrink(1.0)
                    .child(
                        Input::new(&self.history_filter)
                            .small()
                            .h(theme::scaled_px(23.))
                            .focus_bordered(false)
                            .suffix(
                                Button::new("history-filter")
                                    .ghost()
                                    .compact()
                                    .h(theme::scaled_px(19.))
                                    .icon(if show_clear_search {
                                        IconName::Close
                                    } else {
                                        IconName::Search
                                    })
                                    .accessibility_label(if show_clear_search {
                                        "清除筛选"
                                    } else {
                                        "筛选"
                                    })
                                    .tooltip(if show_clear_search {
                                        "清除当前 URL／方法搜索关键字并恢复列表"
                                    } else {
                                        "按输入框中的 URL 或 HTTP 方法关键字过滤历史记录列表"
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if show_clear_search {
                                            if let Err(error) = editing::input_snapshot(
                                                &this.history_filter,
                                                window,
                                                cx,
                                            ) {
                                                this.fail(error, cx);
                                                return;
                                            }
                                            this.history_filter.update(cx, |input, cx| {
                                                input.set_value("", window, cx);
                                            });
                                        }
                                        this.query_history(0, window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("http-history-filter-switch-wrap")
                    .w(theme::scaled_px(86.))
                    .h(theme::scaled_px(21.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .flex_shrink_0()
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "启用或暂停当前筛选条件（关闭时显示全部历史记录）",
                        )
                        .build(window, cx)
                    })
                    .child(
                        Switch::new("http-history-filter-enabled")
                            .small()
                            .color(rgb(0x1558b0))
                            .label(if self.http_filter.requested().2 {
                                "筛选开启"
                            } else {
                                "筛选关闭"
                            })
                            .tooltip("启用或暂停当前筛选条件（关闭时显示全部历史记录）")
                            .checked(self.http_filter.requested().2)
                            .on_change(cx.listener(|this, checked, window, cx| {
                                let (filter, sort, _) = this.http_filter.requested();
                                this.query_http_history_filtered(
                                    filter, sort, *checked, 0, window, cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .w(px(46.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_shrink_0()
                    .child(
                        Popover::new("history-help-popover")
                            .anchor(gpui_kit::Anchor::BottomRight)
                            .trigger(
                                Button::new("history-help")
                                    .small()
                                    .ghost()
                                    .compact()
                                    .size(px(18.))
                                    .p_0()
                                    .rounded(px(9.))
                                    .child(
                                        gpui_kit::component::Icon::new(IconName::CircleQuestionMark)
                                            .size(px(16.))
                                            .text_color(theme::foreground()),
                                    )
                                    .accessibility_label("帮助")
                                    .tooltip("查看 HTTP 历史记录筛选与操作说明"),
                            )
                            .content(|_, _, _| {
                                div()
                                    .id("history-help-content")
                                    .test_support()
                                    .w(px(300.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child("HTTP 历史记录")
                                    .child("点击左侧筛选栏可按范围、状态码、MIME、扩展名、正则或 脚本过滤。")
                                    .child("右侧搜索框支持按 URL 或 HTTP 方法关键字即时过滤。")
                                    .child("支持 Shift 范围多选，右键菜单可发送报文、提取链接、导出 XML 或添加备注。")
                            }),
                    )
                    .child(
                        Button::new("history-options-menu")
                            .small()
                            .ghost()
                            .compact()
                            .size(px(18.))
                            .p_0()
                            .rounded(px(3.))
                            .child(
                                gpui_kit::component::Icon::new(IconName::EllipsisVertical)
                                    .size(px(16.))
                                    .text_color(theme::foreground()),
                            )
                            .accessibility_label("历史记录选项")
                            .tooltip("HTTP 历史记录操作选项")
                            .dropdown_menu(move |menu, window, cx| {
                                let Some(entity) = owner.upgrade() else {
                                    return menu;
                                };
                                let view = entity.read(cx);
                                let loading = view.history_loading;
                                let continuous_scroll = view.http_filter.continuous_scroll;
                                let can_clear = view.project.is_some()
                                    && view.backend_available()
                                    && view.transition.is_none()
                                    && !loading
                                    && view.pending_history_delete.is_none();
                                let current_keys: Vec<SharedString> = view
                                    .history
                                    .read(cx)
                                    .delegate()
                                    .columns
                                    .iter()
                                    .map(|col| col.key.clone())
                                    .collect();
                                let all_cols = view.http_filter.all_columns.clone();
                                let layout_owner = owner.clone();
                                let layout_submenu = gpui_kit::component::menu::PopupMenu::build(
                                    window,
                                    cx,
                                    move |mut sub, _, _| {
                                        let visible_count = current_keys.len();
                                        for col in &all_cols {
                                            let key = col.key.clone();
                                            let visible = current_keys.contains(&key);
                                            let is_number = key.as_ref() == "number";
                                            let can_toggle = !is_number && (!visible || visible_count > 1);
                                            let toggle_owner = layout_owner.clone();
                                            let label = if visible {
                                                format!("✓ {}", col.name)
                                            } else {
                                                format!("   {}", col.name)
                                            };
                                            sub = sub.item(
                                                PopupMenuItem::new(label)
                                                    .disabled(!can_toggle)
                                                    .on_click(move |_, _, cx| {
                                                        let _ = toggle_owner.update(cx, |this, cx| {
                                                            this.toggle_history_column_visibility(key.as_ref(), cx);
                                                        });
                                                    }),
                                            );
                                        }
                                        let restore_owner = layout_owner.clone();
                                        sub.separator().item(
                                            PopupMenuItem::new("恢复默认列布局 (Restore default layout)")
                                                .on_click(move |_, _, cx| {
                                                    let _ = restore_owner.update(cx, |this, cx| {
                                                        this.restore_default_history_columns(cx);
                                                    });
                                                }),
                                        )
                                    },
                                );
                                let scroll_owner = owner.clone();
                                let clear_owner = owner.clone();
                                let _ = offset;
                                menu.item(PopupMenuItem::submenu(
                                    "表格列布局 (Table layout)",
                                    layout_submenu,
                                ))
                                .item(
                                    PopupMenuItem::new("连续自动滚屏 (Continuous scroll)")
                                        .checked(continuous_scroll)
                                        .on_click(move |_, _, cx| {
                                            let _ = scroll_owner.update(cx, |this, cx| {
                                                this.toggle_continuous_scroll(cx);
                                            });
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("清空历史")
                                        .disabled(!can_clear)
                                        .on_click(move |_, window, cx| {
                                            let _ = clear_owner.update(cx, |this, cx| {
                                                this.clear_live_proxy_history(window, cx);
                                            });
                                        }),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn toggle_continuous_scroll(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.http_filter.continuous_scroll;
        self.http_filter.continuous_scroll = enabled;
        self.http_filter.pending_auto_scroll = false;
        if enabled {
            let sort = self.http_filter.sort;
            self.history.update(cx, |state, cx| {
                let row_count = state.delegate().rows.len();
                if row_count > 0 {
                    let target = if sort.column == mio_core::HttpHistorySortColumn::Number
                        && sort.ascending
                    {
                        row_count - 1
                    } else {
                        0
                    };
                    state.scroll_to_row(target, cx);
                }
            });
        }
        cx.notify();
    }

    fn open_http_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project else { return };
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let owner = cx.entity();
        let owner_window = window.window_handle();
        let scale = theme::ui_scale().max(1.0);
        let target_w = px((WINDOW_WIDTH * scale).round());
        let desired_h = px((if cfg!(test) { CONTENT_HEIGHT } else { WINDOW_HEIGHT } * scale).round());
        let bounds = if let Some(display) = window.display(cx).or_else(|| cx.primary_display()) {
            let visible = display.visible_bounds();
            let fitted = size(
                target_w.min(visible.size.width),
                desired_h.min(visible.size.height),
            );
            let mut bounds = Bounds::centered(Some(display.id()), fitted, cx);
            bounds.origin.x = bounds.origin.x.clamp(
                visible.origin.x,
                (visible.right() - fitted.width).max(visible.origin.x),
            );
            bounds.origin.y = bounds.origin.y.clamp(
                visible.origin.y,
                (visible.bottom() - fitted.height).max(visible.origin.y),
            );
            bounds
        } else {
            Bounds::centered(None, size(target_w, desired_h), cx)
        };
        cx.defer(move |cx| {
            if owner.read(cx).project != Some(project) {
                return;
            }
            if let Some(existing) = owner.read(cx).http_filter.window
                && crate::chrome::activate_popup_window(existing, cx)
            {
                return;
            }
            let filter = owner.read(cx).http_filter.applied.clone();
            let inputs = owner.read(cx).http_filter.inputs.clone();
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        FilterWindow::new(
                            owner.clone(),
                            owner_window,
                            project,
                            filter,
                            inputs,
                            window,
                            cx,
                        )
                    })
                },
            );
            owner.update(cx, |view, cx| match opened {
                Ok((handle, _)) => view.http_filter.window = Some(handle),
                Err(error) => view.fail(format!("无法打开 HTTP 历史筛选：{error}"), cx),
            });
        });
    }
}
