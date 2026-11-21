use gpui_kit::assets::IconName;

use super::*;

impl Workbench {
    pub(in crate::workbench::ext_import::messages) fn render_ext_items(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = self
            .ext_import
            .messages
            .as_ref()
            .and_then(|messages| messages.items.as_ref())
            .expect("item browser exists");
        let delegate = items.table.read(cx).delegate();
        let offset = delegate.offset;
        let count = delegate.rows.len();
        let total = delegate.total;
        let loading = delegate.loading;
        let error = delegate.error.clone();
        let refresh_offset = if error.is_some() {
            items.query_offset
        } else {
            offset
        };
        let previous = items
            .previous_offsets
            .last()
            .copied()
            .filter(|_| offset != 0);
        let next = delegate.next_offset;

        let metadata = self.render_ext_item_metadata(items, cx);
        let messages = if items.selected.is_some() {
            self.render_ext_message_contents(cx)
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::muted_foreground())
                .child("选择条目后仅显示该槽位关联的归档消息；不会从其它历史记录拼补当前草稿。")
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .child(
                div()
                    .w(px(500.))
                    .flex_none()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(toolbar().child("全部条目 · 保留 null、failed、empty 与重复引用的位置"))
                    .child(self.render_ext_items_filter_bar(items, loading, cx))
                    .child(self.render_ext_items_pagination_bar(
                        items,
                        offset,
                        count,
                        total,
                        loading,
                        error.as_deref(),
                        refresh_offset,
                        previous,
                        next,
                        cx,
                    ))
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
                            .id("ext-items-table")
                            .h(px(220.))
                            .min_h(px(120.))
                            .child(
                                DataTable::new(&items.table)
                                    .with_size(theme::row_height())
                                    .header_row_height(theme::row_height())
                                    .bordered(false)
                                    .scroll_bounce(false)
                                    .accessibility_label("External 全部归档条目"),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(metadata)),
            )
            .child(div().flex_1().min_w_0().min_h_0().child(messages))
            .into_any_element()
    }

    fn render_ext_item_metadata(&self, items: &Items, cx: &mut Context<Self>) -> AnyElement {
        if let Some(item) = &items.detail {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(toolbar().child(format!(
                    "原位置 {} · {} · 原始 JSON",
                    item.index, item.state
                )))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .text_color(theme::muted_foreground())
                        .child("只读字段观察，最多 256 KiB；未恢复的语义见导入记录。"),
                )
                .child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&items.metadata)
                            .readonly(true)
                            .size_full()
                            .bordered(false)
                            .aria_label("外部归档条目原始元数据 JSON"),
                    ),
                )
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
                .child(items.detail_error.as_ref().map_or_else(
                    || {
                        if items.selected.is_some() {
                            "正在读取条目元数据与消息摘要…".into()
                        } else {
                            "选择条目，按需查看原始元数据。".into()
                        }
                    },
                    |error| format!("读取条目失败：{error}"),
                ));
            if items.detail_error.is_some()
                && let Some(id) = items.selected
            {
                let offset = items.requested_message_offset;
                view = view.child(
                    button("ext-item-retry", "重试读取条目")
                        .icon(IconName::RefreshCw)
                        .tooltip("重新从项目归档存储读取该条目的 JSON 元数据与关联消息列表")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.read_ext_item(id, offset, window, cx)
                        })),
                );
            }
            view.into_any_element()
        }
    }

    fn render_ext_items_filter_bar(
        &self,
        items: &Items,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        toolbar()
            .child(
                button("ext-items-all", "全部集合")
                    .icon(IconName::ListFilter)
                    .tooltip("清除集合筛选条件并显示所有来源集合中的原始归档条目")
                    .selected(items.filter.is_none())
                    .disabled(loading)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.query_ext_items(None, 0, window, cx)
                    })),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Select::new(&items.collections)
                        .id("ext-item-collection-select")
                        .accessibility_label("筛选 外部归档条目来源集合")
                        .placeholder("全部集合")
                        .search_placeholder("搜索完整集合路径…")
                        .cleanable(true)
                        .disabled(loading)
                        .w_full()
                        .menu_max_h(px(300.))
                        .scroll_bounce(false)
                        .small(),
                ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_ext_items_pagination_bar(
        &self,
        items: &Items,
        offset: usize,
        count: usize,
        total: usize,
        loading: bool,
        error: Option<&str>,
        refresh_offset: usize,
        previous: Option<usize>,
        next: Option<usize>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let refresh_filter = items.filter.clone();
        let previous_filter = items.filter.clone();
        let next_filter = items.filter.clone();
        toolbar()
            .child(
                button(
                    "ext-items-refresh",
                    if error.is_some() {
                        "重试查询"
                    } else {
                        "刷新"
                    },
                )
                .icon(IconName::RefreshCw)
                .tooltip("重新从项目归档存储查询当前分页的条目列表")
                .disabled(loading)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.query_ext_items(refresh_filter.clone(), refresh_offset, window, cx)
                })),
            )
            .child(div().flex_1())
            .child(
                button("ext-items-previous", "上一页")
                    .icon(IconName::ChevronLeft)
                    .tooltip("加载上一页归档条目摘要")
                    .disabled(previous.is_none() || loading)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(offset) = previous {
                            this.query_ext_items(previous_filter.clone(), offset, window, cx);
                        }
                    })),
            )
            .child(format!(
                "{}–{} / {total}",
                if count == 0 { 0 } else { offset + 1 },
                offset + count
            ))
            .child(
                button("ext-items-next", "下一页")
                    .icon(IconName::ChevronRight)
                    .tooltip("加载下一页归档条目摘要")
                    .disabled(next.is_none() || loading)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(offset) = next {
                            this.query_ext_items(next_filter.clone(), offset, window, cx);
                        }
                    })),
            )
    }
}
