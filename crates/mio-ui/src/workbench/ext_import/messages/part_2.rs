use gpui_kit::assets::IconName;

use super::*;

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench::ext_import) fn render_ext_messages(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item_mode = self
            .ext_import
            .messages
            .as_ref()
            .is_some_and(|messages| messages.items_visible);
        let content = if item_mode {
            self.render_ext_items(cx)
        } else {
            self.render_ext_message_contents(cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .child(if item_mode {
                        "外部归档条目 · 原位置与消息按条目分离"
                    } else {
                        "外部归档消息 · 当前位置保留源集合的索引"
                    })
                    .child(div().flex_1())
                    .child(
                        button(
                            "ext-browser-mode",
                            if item_mode {
                                "浏览已索引消息"
                            } else {
                                "浏览全部条目"
                            },
                        )
                        .icon(if item_mode {
                            IconName::FileSearch
                        } else {
                            IconName::Database
                        })
                        .tooltip(if item_mode {
                            "切换到扁平消息索引视图，跨条目集中浏览已恢复的请求与响应"
                        } else {
                            "切换到原始归档条目视图，按集合位置检查条目状态与 JSON 元数据"
                        })
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if item_mode {
                                    this.show_ext_messages(window, cx);
                                } else {
                                    this.show_ext_items(window, cx);
                                }
                            },
                        )),
                    )
                    .child(
                        button("ext-messages-report", "返回导入记录")
                            .icon(IconName::FileText)
                            .tooltip("返回 外部归档项目导入摘要与分类覆盖率矩阵页面")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_ext_repeater();
                                this.ext_import.browser_visible = false;
                                this.focus.focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        button("ext-messages-dismiss", "返回工作台")
                            .icon(IconName::ArrowLeft)
                            .tooltip("关闭 外部归档浏览器并返回主工作台")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_ext_repeater();
                                this.ext_import.visible = false;
                                this.focus.focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_ext_message_contents(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let messages = self
            .ext_import
            .messages
            .as_ref()
            .expect("message browser exists");
        let delegate = messages.table.read(cx).delegate();
        let offset = delegate.offset;
        let total = delegate.total;
        let count = delegate.rows.len();
        let loading = delegate.loading;
        let error = delegate.error.clone();
        let refresh_offset = if error.is_some() {
            messages.query_offset
        } else {
            offset
        };
        let previous = messages
            .previous_offsets
            .last()
            .copied()
            .filter(|_| offset != 0);
        let next_offset = delegate.next_offset;
        let filter = messages.filter.clone();
        let filters = toolbar()
            .child(
                button("ext-messages-all", "全部集合")
                    .icon(IconName::ListFilter)
                    .tooltip("清除集合筛选条件并显示所有来源集合中的已索引消息")
                    .selected(filter.is_none())
                    .disabled(loading)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.query_ext_messages(None, 0, window, cx)
                    })),
            )
            .child(
                div().w(theme::scaled_px(420.)).flex_none().child(
                    Select::new(&messages.collections)
                        .id("ext-collection-select")
                        .accessibility_label("筛选 外部来源集合")
                        .placeholder("全部集合")
                        .search_placeholder("搜索完整集合路径…")
                        .cleanable(true)
                        .disabled(loading)
                        .w_full()
                        .menu_max_h(px(300.))
                        .scroll_bounce(false)
                        .small(),
                ),
            );
        let refresh_filter = filter.clone();
        let previous_filter = filter.clone();
        let controls = toolbar()
            .child(
                button(
                    "ext-messages-refresh",
                    if error.is_some() {
                        "重试查询"
                    } else {
                        "刷新"
                    },
                )
                .icon(IconName::RefreshCw)
                .tooltip("重新从项目归档存储读取当前分页的消息摘要")
                .disabled(loading)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.query_ext_messages(refresh_filter.clone(), refresh_offset, window, cx)
                })),
            )
            .child(div().flex_1())
            .child(
                button("ext-messages-previous", "上一页")
                    .icon(IconName::ChevronLeft)
                    .tooltip("加载上一页归档消息摘要")
                    .disabled(previous.is_none() || loading)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(previous) = previous {
                            this.query_ext_messages(previous_filter.clone(), previous, window, cx);
                        }
                    })),
            )
            .child(format!(
                "{}–{} / {total}",
                if count == 0 { 0 } else { offset + 1 },
                offset + count
            ))
            .child(
                button("ext-messages-next", "下一页")
                    .icon(IconName::ChevronRight)
                    .tooltip("加载下一页归档消息摘要")
                    .disabled(next_offset.is_none() || loading)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(next) = next_offset {
                            this.query_ext_messages(filter.clone(), next, window, cx);
                        }
                    })),
            );

        let detail = if let Some(fragment) = &messages.detail {
            let message_id = fragment.summary.id;
            let previous = fragment.offset.saturating_sub(FRAGMENT_BYTES as u64);
            let next = fragment.offset + fragment.bytes.len() as u64;
            let mut view = div().size_full().flex().flex_col()
                .child(toolbar()
                    .child(format!("原消息字节 {}–{} / {} · 字段 f{} · {}", fragment.offset, next, fragment.summary.raw_bytes, fragment.summary.field_id, fragment.summary.role))
                    .child(div().flex_1())
                    .child(
                        button("ext-fragment-previous", "上一片段")
                            .icon(IconName::ChevronLeft)
                            .tooltip("读取前一段 64 KiB 原始归档报文字节")
                            .disabled(fragment.offset == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.read_ext_message(message_id, previous, window, cx)
                            })),
                    )
                    .child(
                        button("ext-fragment-next", "下一片段")
                            .icon(IconName::ChevronRight)
                            .tooltip("读取下一段 64 KiB 原始归档报文字节")
                            .disabled(next >= fragment.summary.raw_bytes || fragment.bytes.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.read_ext_message(message_id, next, window, cx)
                            })),
                    ))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .text_color(theme::muted_foreground())
                        .child("只读原始字节片段（单次上限 64 KiB，半开区间不含结束偏移）；若分段边界截断多字节 UTF-8 字符，请切换至 Hex 视图查看。"),
                );
            if let Some(diagnostic) = &fragment.summary.diagnostic {
                view = view.child(
                    div()
                        .px(px(8.))
                        .text_color(theme::danger_fg())
                        .child(diagnostic.clone()),
                );
            }
            view.child(self.message_header("ext-message", "归档原消息", messages.payload.view, cx))
                .child(
                    div().flex_1().min_h_0().child(
                        self.render_inspector_dock(
                            Editor::new(messages.payload.state())
                                .readonly(true)
                                .size_full()
                                .bordered(false)
                                .aria_label("外部归档消息片段")
                                .into_any_element(),
                            cx,
                        ),
                    ),
                )
                .child(messages.payload.render_search_bar(
                    "ext-message",
                    cx,
                    |this, action, window, cx| {
                        this.handle_message_search_action("ext-message", action, window, cx);
                    },
                ))
                .into_any_element()
        } else {
            let mut view = div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .text_color(theme::muted_foreground())
                .child(messages.detail_error.as_ref().map_or_else(
                    || {
                        if messages.selected.is_some() {
                            "正在读取归档消息片段…".into()
                        } else if messages.items_visible
                            && total == 0
                            && !loading
                            && error.is_none()
                        {
                            "当前条目没有已索引消息；未从其它历史记录拼补当前草稿。".into()
                        } else {
                            "选择消息，按需读取其原始字节；不会请求原目标。".into()
                        }
                    },
                    |error| format!("读取片段失败：{error}"),
                ));
            if messages.detail_error.is_some()
                && let Some(id) = messages.selected
            {
                let offset = messages.requested_offset;
                view = view.child(
                    button("ext-fragment-retry", "重试读取片段")
                        .icon(IconName::RefreshCw)
                        .tooltip("重新请求读取当前选中消息偏移处的字节片段")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.read_ext_message(id, offset, window, cx)
                        })),
                );
            }
            view.into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .when(!messages.items_visible, |this| this.child(filters))
            .child(controls)
            .child(self.render_ext_repeater(cx))
            .when_some(error.as_ref().filter(|_| count > 0), |this, error| {
                this.child(
                    div()
                        .px(px(8.))
                        .text_color(theme::danger_fg())
                        .child(format!("查询失败，保留上次结果：{error}")),
                )
            })
            .child(
                div()
                    .id("ext-message-table")
                    .h(px(220.))
                    .min_h(px(120.))
                    .child(
                        DataTable::new(&messages.table)
                            .with_size(theme::row_height())
                            .header_row_height(theme::row_height())
                            .bordered(false)
                            .scroll_bounce(false)
                            .accessibility_label("外部归档消息索引"),
                    ),
            )
            .child(div().flex_1().min_h_0().child(detail))
            .into_any_element()
    }
}
