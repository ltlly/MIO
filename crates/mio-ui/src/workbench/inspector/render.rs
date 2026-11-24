use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};

mod detail_drawer;
mod notes_dock;

fn move_id_prefixes(section: InspectorSection) -> (&'static str, &'static str) {
    match section {
        InspectorSection::RequestAttributes => {
            ("inspector-move-up-attributes", "inspector-move-down-attributes")
        }
        InspectorSection::QueryParams => ("inspector-move-up-query", "inspector-move-down-query"),
        InspectorSection::BodyParams => ("inspector-move-up-body", "inspector-move-down-body"),
        InspectorSection::Cookies => ("inspector-move-up-cookies", "inspector-move-down-cookies"),
        InspectorSection::RequestHeaders => (
            "inspector-move-up-request-headers",
            "inspector-move-down-request-headers",
        ),
        InspectorSection::ResponseHeaders => (
            "inspector-move-up-response-headers",
            "inspector-move-down-response-headers",
        ),
    }
}

fn request_attributes_split(
    section: InspectorSection,
    items: &[InspectorItem],
) -> (Option<&InspectorItem>, &[InspectorItem]) {
    if section == InspectorSection::RequestAttributes
        && let Some(proto) = items
            .get(2)
            .filter(|item| item.name.eq_ignore_ascii_case("protocol"))
    {
        (Some(proto), &items[..2])
    } else {
        (None, items)
    }
}

fn section_items_slice(data: &InspectorData, section: InspectorSection) -> &[InspectorItem] {
    let items = match section {
        InspectorSection::RequestAttributes => &data.attributes,
        InspectorSection::QueryParams => &data.query_params,
        InspectorSection::BodyParams => &data.body_params,
        InspectorSection::Cookies => &data.cookies,
        InspectorSection::RequestHeaders => &data.request_headers,
        InspectorSection::ResponseHeaders => &data.response_headers,
    };
    request_attributes_split(section, items).1
}

impl Workbench {
    #[inline(never)]
    fn render_inspector_protocol_row(
        &self,
        proto_item: &InspectorItem,
        editable: bool,
        _subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let proto_trimmed = proto_item.value.trim();
        let is_http2 = proto_trimmed.eq_ignore_ascii_case("HTTP/2")
            || proto_trimmed.eq_ignore_ascii_case("HTTP/2.0");
        let is_http1 = !is_http2
            && (proto_trimmed.eq_ignore_ascii_case("HTTP/1")
                || proto_trimmed.eq_ignore_ascii_case("HTTP/1.1")
                || proto_trimmed.eq_ignore_ascii_case("HTTP/1.0"));
        let inactive_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xdfdfdf }).into();
        let active_bg: gpui_kit::Hsla = rgb(0x26649d).into();
        let active_fg: gpui_kit::Hsla = rgb(0xffffff).into();
        div()
            .id("inspector-protocol-row")
            .test_support()
            .w_full()
            .h(px(41.))
            .pb(px(20.))
            .bg(theme::toolbar_surface())
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(67.))
                    .flex_shrink_0()
                    .text_size(px(13.))
                    .font_weight(gpui_kit::FontWeight::NORMAL)
                    .text_color(theme::foreground())
                    .child("Protocol"),
            )
            .child(
                div()
                    .w(px(112.))
                    .h(px(21.))
                    .flex()
                    .items_center()
                    .gap_0()
                    .child(
                        Button::new("inspector-protocol-HTTP/1")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(56.))
                            .h(px(21.))
                            .p_0()
                            .bg(if is_http1 { active_bg } else { inactive_bg })
                            .text_color(if is_http1 { active_fg } else { theme::foreground() })
                            .label("HTTP/1")
                            .accessibility_label("HTTP/1")
                            .tooltip(if editable {
                                "切换请求协议为 HTTP/1.1"
                            } else {
                                "当前请求协议：HTTP/1（只读）"
                            })
                            .selected(is_http1)
                            .toggled(is_http1)
                            .disabled(!editable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_inspector_request_protocol("HTTP/1.1", window, cx);
                            })),
                    )
                    .child(
                        Button::new("inspector-protocol-HTTP/2")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(56.))
                            .h(px(21.))
                            .p_0()
                            .bg(if is_http2 { active_bg } else { inactive_bg })
                            .text_color(if is_http2 { active_fg } else { theme::foreground() })
                            .label("HTTP/2")
                            .accessibility_label("HTTP/2")
                            .tooltip(if editable {
                                "切换请求协议为 HTTP/2"
                            } else {
                                "当前请求协议：HTTP/2（只读）"
                            })
                            .selected(is_http2)
                            .toggled(is_http2)
                            .disabled(!editable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_inspector_request_protocol("HTTP/2", window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_inspector_section(
        &self,
        section: InspectorSection,
        items: &[InspectorItem],
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idx = section_index(section);
        let is_expanded = self.inspector.expanded[idx];
        let wrap_enabled = self.inspector.section_wrap[idx];
        let (header_id, title, item_id_prefix, remove_id_prefix, add_target_id) =
            section_meta(section);
        let (move_up_prefix, move_down_prefix) = move_id_prefixes(section);
        let can_remove = editable && section != InspectorSection::RequestAttributes;
        let can_select = editable;
        let (protocol_item, table_items) = request_attributes_split(section, items);
        let count = table_items.len();
        let has_items = protocol_item.is_some() || !table_items.is_empty();
        let add_target = add_target_id.filter(|_| editable);
        let can_expand = has_items || add_target.is_some();
        let show_body = is_expanded && can_expand;
        let subtle_divider: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x34373c } else { 0xebeef2 }).into();
        let pill_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into();
        let header_bar =
                div()
                    .w_full()
                    .h(if show_body { px(38.) } else { px(39.) })
                    .min_h(if show_body { px(38.) } else { px(39.) })
                    .flex_shrink_0()
                    .bg(theme::toolbar_surface())
                    .when(!show_body, |b| b.border_b_1().border_color(theme::border()))
                    .flex()
                    .items_center()
                    .child(
                        Button::new(header_id)
                            .ghost()
                            .rounded(ButtonRounded::None)
                            .flex_1()
                            .min_w_0()
                            .h(px(38.))
                            .px(px(10.))
                            .accessibility_label(format!("{title} ({count})"))
                            .tooltip(if !can_expand {
                                SharedString::from(format!("{title}（当前无条目）"))
                            } else if show_body {
                                SharedString::from(format!("折叠 {title}"))
                            } else if has_items {
                                SharedString::from(format!("展开 {title}（共 {count} 项）"))
                            } else {
                                SharedString::from(format!("展开 {title}（可新增条目）"))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.inspector.auto_opened_by_selection = false;
                                if can_expand {
                                    this.inspector.expanded[idx] = !this.inspector.expanded[idx];
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w_full()
                                    .relative()
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .pr(px(80.))
                                            .truncate()
                                            .text_size(px(13.))
                                            .font_weight(gpui_kit::FontWeight::NORMAL)
                                            .text_color(theme::foreground())
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .top_0()
                                            .flex()
                                            .h(px(18.))
                                            .items_center()
                                            .gap(px(20.))
                                            .child(
                                                div()
                                                    .w(if count >= 10 { px(32.) } else { px(25.) })
                                                    .h(px(18.))
                                                    .flex_shrink_0()
                                                    .px(px(6.))
                                                    .rounded(px(9.))
                                                    .bg(pill_bg)
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(13.))
                                                    .text_color(theme::foreground())
                                                    .child(count.to_string()),
                                            )
                                            .child(
                                                div()
                                                    .w(px(18.))
                                                    .h(px(18.))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        gpui_kit::component::Icon::new(if show_body {
                                                            IconName::ChevronUp
                                                        } else {
                                                            IconName::ChevronDown
                                                        })
                                                        .size(px(14.))
                                                        .text_color(
                                                            if can_expand {
                                                                theme::foreground()
                                                            } else {
                                                                theme::muted_foreground().opacity(0.45)
                                                            },
                                                        ),
                                                    ),
                                            ),
                                    ),
                            ),
                    );
        let mut container = div().flex_shrink_0().flex().flex_col().child(header_bar);
        if show_body {
            let mut body = div()
                .w_full()
                .px(px(10.))
                .pb(px(10.))
                .bg(theme::toolbar_surface())
                .border_b_1()
                .border_color(theme::border())
                .flex()
                .flex_col();
            if let Some(proto_item) = protocol_item {
                body = body.child(self.render_inspector_protocol_row(
                    proto_item,
                    editable,
                    subtle_divider,
                    cx,
                ));
            }
            if !table_items.is_empty() {
                let table_header_bg: gpui_kit::Hsla =
                    rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into();
                let table_header_fg: gpui_kit::Hsla = theme::foreground();
                body = body.child(
                    div()
                        .id(SharedString::from(format!("{header_id}-table-header")))
                        .test_support()
                        .w_full()
                        .h(px(27.))
                        .bg(table_header_bg)
                        .border_b_1()
                        .border_color(subtle_divider)
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .w(px(143.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .border_r_1()
                                .border_color(subtle_divider)
                                .text_left()
                                .font_family(theme::ui_font_family())
                                .text_size(px(13.))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(table_header_fg)
                                .child("Name"),
                        )
                        .child(
                            div()
                                .w(px(142.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .border_r_1()
                                .border_color(subtle_divider)
                                .text_left()
                                .font_family(theme::ui_font_family())
                                .text_size(px(13.))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(table_header_fg)
                                .child("Value"),
                        )
                        .child(
                            Button::new(SharedString::from(format!("{header_id}-wrap-toggle")))
                                .ghost()
                                .small()
                                .compact()
                                .rounded(ButtonRounded::None)
                                .w(px(28.))
                                .h_full()
                                .p_0()
                                .selected(wrap_enabled)
                                .toggled(wrap_enabled)
                                .when(wrap_enabled, |b| b.bg(rgb(0x26649d)))
                                .accessibility_label(format!("切换 {title} 表格自动换行"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.inspector.auto_opened_by_selection = false;
                                    this.inspector.section_wrap[idx] =
                                        !this.inspector.section_wrap[idx];
                                    cx.notify();
                                }))
                                .child(
                                    gpui_kit::component::Icon::default()
                                        .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2 4h12M2 8h9a2 2 0 0 1 0 4H8m2-2-2 2 2 2M2 12h3\"/></svg>")
                                        .size(px(14.))
                                        .text_color(if wrap_enabled {
                                            gpui_kit::white()
                                        } else {
                                            table_header_fg
                                        }),
                                ),
                        ),
                );
            }
            for (item_idx, item) in table_items.iter().enumerate() {
                body = body.child(self.render_inspector_section_row(
                    section,
                    item,
                    item_idx,
                    count,
                    can_select,
                    can_remove,
                    wrap_enabled,
                    item_id_prefix,
                    move_up_prefix,
                    move_down_prefix,
                    remove_id_prefix,
                    subtle_divider,
                    cx,
                ));
            }
            if let Some(target_id) = add_target {
                body = body.child(self.render_inspector_section_add_bar(
                    section,
                    idx,
                    title,
                    target_id,
                    !table_items.is_empty(),
                    subtle_divider,
                    cx,
                ));
            }
            container = container.child(body);
        }
        container.into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_section_add_bar(
        &self,
        section: InspectorSection,
        section_idx: usize,
        title: &'static str,
        target_id: &'static str,
        has_items: bool,
        subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open_add_drawer = cx.listener(move |this, _, window, cx| {
            if this
                .inspector
                .name_input
                .update(cx, |state, cx| state.marked_text_range(window, cx).is_some())
                || this
                    .inspector
                    .value_input
                    .update(cx, |state, cx| state.marked_text_range(window, cx).is_some())
            {
                this.fail(editing::COMPOSITION_PENDING, cx);
                return;
            }
            this.inspector.auto_opened_by_selection = false;
            this.inspector.editing_section = section;
            this.inspector.editing_index = None;
            this.inspector.editor_open = true;
            this.inspector.detail_url_decoded = false;
            this.inspector.expanded[section_idx] = true;
            this.inspector
                .name_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            this.inspector
                .value_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            cx.notify();
        });
        if has_items {
            div()
                .w_full()
                .py(px(5.))
                .bg(theme::surface())
                .flex()
                .items_center()
                .justify_end()
                .child(
                    Button::new(target_id)
                        .ghost()
                        .small()
                        .compact()
                        .h(px(23.))
                        .px(px(8.))
                        .icon(IconName::Plus)
                        .label("Add")
                        .accessibility_label(format!("新增 {title}"))
                        .tooltip(format!("向 {title} 新增键值对条目"))
                        .on_click(open_add_drawer),
                )
                .into_any_element()
        } else {
            div()
                .w_full()
                .py(px(10.))
                .bg(theme::surface())
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::muted_foreground())
                        .border_b_1()
                        .border_color(subtle_divider)
                        .pb(px(4.))
                        .child("It's empty in here"),
                )
                .child(
                    Button::new(target_id)
                        .small()
                        .compact()
                        .h(px(23.))
                        .px(px(10.))
                        .icon(IconName::Plus)
                        .label("Add")
                        .accessibility_label(format!("新增 {title}"))
                        .tooltip(format!("向 {title} 新增键值对条目"))
                        .on_click(open_add_drawer),
                )
                .into_any_element()
        }
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_row_main_button(
        &self,
        section: InspectorSection,
        item_idx: usize,
        name: String,
        value: String,
        can_select: bool,
        is_selected: bool,
        wrap_enabled: bool,
        item_id_prefix: &'static str,
        subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = format!("{name}: {value}");
        let tip = if can_select {
            format!("{name}: {value}\n（点击在底部编辑此条目，再次点击收起）")
        } else {
            format!("{name}: {value}\n（点击查看完整键值或复制，再次点击收起）")
        };
        let name_display = name.clone();
        let value_display = value.clone();
        Button::new(SharedString::from(format!("{item_id_prefix}-{item_idx}")))
            .ghost()
            .rounded(ButtonRounded::None)
            .flex_1()
            .min_w_0()
            .map(|this| {
                if wrap_enabled {
                    this.h_auto().min_h(px(25.))
                } else {
                    this.h(px(25.))
                }
            })
            .px_0()
            .selected(is_selected)
            .when(is_selected, |b| b.bg(theme::selected()))
            .accessibility_label(label)
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.inspector.name_input.update(cx, |s, cx| {
                    s.marked_text_range(window, cx).is_some()
                }) || this.inspector.value_input.update(cx, |s, cx| {
                    s.marked_text_range(window, cx).is_some()
                }) {
                    this.fail(editing::COMPOSITION_PENDING, cx);
                    return;
                }
                this.inspector.auto_opened_by_selection = false;
                if is_selected {
                    this.unfocus_inspector_detail_inputs(window, cx);
                    this.inspector.editing_index = None;
                    this.inspector.editor_open = false;
                    this.inspector.detail_url_decoded = false;
                    cx.notify();
                    return;
                }
                this.inspector.editing_section = section;
                this.inspector.editing_index = Some(item_idx);
                this.inspector.editor_open = true;
                this.inspector.detail_url_decoded = false;
                let name = name.clone();
                let value = value.clone();
                this.inspector
                    .name_input
                    .update(cx, |state, cx| state.set_value(name, window, cx));
                this.inspector
                    .value_input
                    .update(cx, |state, cx| state.set_value(value, window, cx));
                cx.notify();
            }))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .map(|this| {
                        if wrap_enabled {
                            this.min_h(px(25.)).items_stretch()
                        } else {
                            this.h_full().items_center()
                        }
                    })
                    .flex()
                    .child(
                        div()
                            .w(px(143.))
                            .flex_shrink_0()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(subtle_divider)
                            .flex()
                            .items_center()
                            .map(|this| {
                                if wrap_enabled {
                                    this.min_h(px(25.)).py(px(4.)).whitespace_normal()
                                } else {
                                    this.h_full().truncate()
                                }
                            })
                            .text_left()
                            .font_family(theme::ui_font_family())
                            .text_size(px(13.))
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(theme::foreground())
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .map(|this| {
                                        if wrap_enabled {
                                            this.whitespace_normal()
                                        } else {
                                            this.truncate()
                                        }
                                    })
                                    .child(name_display),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(4.))
                            .pr(px(2.))
                            .flex()
                            .items_center()
                            .map(|this| {
                                if wrap_enabled {
                                    this.min_h(px(25.)).py(px(4.)).whitespace_normal()
                                } else {
                                    this.h_full().truncate()
                                }
                            })
                            .text_left()
                            .font_family(theme::ui_font_family())
                            .text_size(px(13.))
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(theme::foreground())
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .map(|this| {
                                        if wrap_enabled {
                                            this.whitespace_normal()
                                        } else {
                                            this.truncate()
                                        }
                                    })
                                    .child(value_display),
                            ),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_row_actions(
        &self,
        section: InspectorSection,
        item_idx: usize,
        count: usize,
        can_remove: bool,
        item_id_prefix: &'static str,
        move_up_prefix: &'static str,
        move_down_prefix: &'static str,
        remove_id_prefix: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_first = item_idx == 0;
        let is_last = item_idx + 1 == count;
        let mut actions_col = div()
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_end();
        if can_remove {
            actions_col = actions_col
                .child(
                    Button::new(SharedString::from(format!(
                        "{move_up_prefix}-{item_idx}"
                    )))
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(20.))
                    .h(theme::scaled_px(20.))
                    .icon(IconName::ChevronUp)
                    .accessibility_label("上移")
                    .tooltip("将此条目向上移动一行并同步调整原始报文字节顺序")
                    .disabled(is_first)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.inspector.auto_opened_by_selection = false;
                        if item_idx > 0 {
                            this.move_inspector_item(section, item_idx, item_idx - 1, window, cx);
                        }
                    })),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "{move_down_prefix}-{item_idx}"
                    )))
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(20.))
                    .h(theme::scaled_px(20.))
                    .icon(IconName::ChevronDown)
                    .accessibility_label("下移")
                    .tooltip("将此条目向下移动一行并同步调整原始报文字节顺序")
                    .disabled(is_last)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.inspector.auto_opened_by_selection = false;
                        if !is_last {
                            this.move_inspector_item(section, item_idx, item_idx + 1, window, cx);
                        }
                    })),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "{remove_id_prefix}-{item_idx}"
                    )))
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(22.))
                    .h(theme::scaled_px(22.))
                    .icon(IconName::Trash)
                    .accessibility_label("删除")
                    .tooltip("从当前报文中移除此条目并同步更新原始字节")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.inspector.auto_opened_by_selection = false;
                        this.remove_inspector_item(section, item_idx, window, cx);
                    })),
                );
        } else {
            actions_col = actions_col.w(px(28.));
        }
        actions_col
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{item_id_prefix}-chevron-{item_idx}"
                    )))
                    .test_support()
                    .w(px(14.))
                    .h(px(14.))
                    .ml(px(2.))
                    .mr(px(6.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui_kit::component::Icon::new(IconName::ChevronRight)
                            .size(px(14.))
                            .text_color(theme::foreground()),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_section_row(
        &self,
        section: InspectorSection,
        item: &InspectorItem,
        item_idx: usize,
        count: usize,
        can_select: bool,
        can_remove: bool,
        wrap_enabled: bool,
        item_id_prefix: &'static str,
        move_up_prefix: &'static str,
        move_down_prefix: &'static str,
        remove_id_prefix: &'static str,
        subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_selected = self.inspector.editor_open
            && self.inspector.editing_section == section
            && self.inspector.editing_index == Some(item_idx);
        div()
            .w_full()
            .map(|this| {
                if wrap_enabled {
                    this.min_h(px(25.))
                } else {
                    this.h(px(25.))
                }
            })
            .flex()
            .items_center()
            .bg(if is_selected {
                theme::selected()
            } else {
                theme::surface()
            })
            .child(self.render_inspector_row_main_button(
                section,
                item_idx,
                item.name.clone(),
                item.value.clone(),
                can_select,
                is_selected,
                wrap_enabled,
                item_id_prefix,
                subtle_divider,
                cx,
            ))
            .child(self.render_inspector_row_actions(
                section,
                item_idx,
                count,
                can_remove,
                item_id_prefix,
                move_up_prefix,
                move_down_prefix,
                remove_id_prefix,
                cx,
            ))
            .into_any_element()
    }

    #[inline(never)]
    fn render_inspector_title_bar(&self, show_http: bool, cx: &mut Context<Self>) -> AnyElement {
        let dock_left = self.inspector.dock_left;
        let inactive_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xdfdfdf }).into();
        div()
            .id("inspector-title-bar")
            .test_support()
            .w_full()
            .h(px(40.))
            .min_h(px(40.))
            .max_h(px(40.))
            .px(px(10.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme::foreground())
                    .child("Inspector"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w(px(50.))
                    .h(px(19.))
                    .flex()
                    .items_center()
                    .gap_0()
                    .child(
                        Button::new("inspector-dock-left")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(25.))
                            .h(px(19.))
                            .p_0()
                            .bg(if dock_left { rgb(0x26649d).into() } else { inactive_bg })
                            .text_color(if dock_left { rgb(0xffffff).into() } else { theme::foreground() })
                            .icon(
                                gpui_kit::component::Icon::default()
                                    .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 15 13\" fill=\"none\"><rect x=\"0.75\" y=\"1.25\" width=\"13.5\" height=\"10.5\" rx=\"1\" stroke=\"currentColor\" stroke-width=\"1.2\"/><rect x=\"1.2\" y=\"1.7\" width=\"5.2\" height=\"9.6\" fill=\"currentColor\"/></svg>")
                                    .size(px(14.))
                                    .text_color(if dock_left { rgb(0xffffff).into() } else { theme::foreground() }),
                            )
                            .accessibility_label("Dock on the left")
                            .tooltip("Dock on the left")
                            .selected(dock_left)
                            .toggled(dock_left)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector.dock_left = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("inspector-dock-right")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(25.))
                            .h(px(19.))
                            .p_0()
                            .bg(if !dock_left { rgb(0x26649d).into() } else { inactive_bg })
                            .text_color(if !dock_left { rgb(0xffffff).into() } else { theme::foreground() })
                            .icon(
                                gpui_kit::component::Icon::default()
                                    .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 15 13\" fill=\"none\"><rect x=\"0.75\" y=\"1.25\" width=\"13.5\" height=\"10.5\" rx=\"1\" stroke=\"currentColor\" stroke-width=\"1.2\"/><rect x=\"8.6\" y=\"1.7\" width=\"5.2\" height=\"9.6\" fill=\"currentColor\"/></svg>")
                                    .size(px(14.))
                                    .text_color(if !dock_left { rgb(0xffffff).into() } else { theme::foreground() }),
                            )
                            .accessibility_label("Dock on the right")
                            .tooltip("Dock on the right")
                            .selected(!dock_left)
                            .toggled(!dock_left)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector.dock_left = false;
                                cx.notify();
                            })),
                    ),
            )
            .when(show_http, |bar| {
                bar.child(
                    Button::new("inspector-expand-all")
                        .ghost()
                        .small()
                        .min_w(px(18.))
                        .max_w(px(18.))
                        .min_h(px(18.))
                        .max_h(px(18.))
                        .w(px(18.))
                        .h(px(18.))
                        .p_0()
                        .child(
                            gpui_kit::component::Icon::default()
                                .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.45\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.5 2.5h11M2.5 13.5h11M8 4.8v6.4M5.6 7.1L8 4.8l2.4 2.3M5.6 8.9L8 11.2l2.4-2.3\"/></svg>")
                                .size(px(15.))
                                .text_color(theme::foreground()),
                        )
                        .accessibility_label("全部展开")
                        .tooltip("展开全部非空检视分组")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.inspector.auto_opened_by_selection = false;
                            let parsed = if let Some((ws_data, _)) =
                                this.websocket_repeater_inspector_data(cx)
                            {
                                ws_data
                            } else {
                                this.inspect_current_http(cx).0
                            };
                            this.inspector.expanded = [
                                !parsed.attributes.is_empty(),
                                !parsed.query_params.is_empty(),
                                !parsed.body_params.is_empty(),
                                !parsed.cookies.is_empty(),
                                !parsed.request_headers.is_empty(),
                                !parsed.response_headers.is_empty(),
                            ];
                            this.inspector.selection_expanded = true;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("inspector-collapse-all")
                        .ghost()
                        .small()
                        .min_w(px(18.))
                        .max_w(px(18.))
                        .min_h(px(18.))
                        .max_h(px(18.))
                        .w(px(18.))
                        .h(px(18.))
                        .p_0()
                        .child(
                            gpui_kit::component::Icon::default()
                                .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.45\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.5 8h11M8 2.2v3.6M5.8 3.8L8 5.8l2.2-2M8 13.8v-3.6M5.8 12.2L8 10.2l2.2 2\"/></svg>")
                                .size(px(15.))
                                .text_color(theme::foreground()),
                        )
                        .accessibility_label("全部收起")
                        .tooltip("折叠全部检视分组")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.inspector.auto_opened_by_selection = false;
                            this.inspector.expanded = [false; 6];
                            this.inspector.selection_expanded = false;
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("inspector-close")
                    .ghost()
                    .small()
                    .min_w(px(18.))
                    .max_w(px(18.))
                    .min_h(px(18.))
                    .max_h(px(18.))
                    .w(px(18.))
                    .h(px(18.))
                    .p_0()
                    .child(
                        gpui_kit::component::Icon::default()
                            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.65\" stroke-linecap=\"round\"><path d=\"M3.2 3.2l9.6 9.6M12.8 3.2l-9.6 9.6\"/></svg>")
                            .size(px(14.))
                            .text_color(theme::foreground()),
                    )
                    .accessibility_label("收起 Inspector")
                    .tooltip("收起 Inspector 检视面板")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.dismiss_inspector_for_current_selection(window, cx) {
                            cx.notify();
                        }
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_sections_area(
        &self,
        parsed_http: Option<&InspectorData>,
        is_ws: bool,
        req_editable: bool,
        resp_editable: bool,
        any_editable: bool,
        has_selection_preview: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut sections = div()
            .id("inspector-sections")
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.inspector.sections_scroll_handle)
            .flex()
            .flex_col()
            .bg(theme::toolbar_surface())
            .child(self.render_inspector_selection_section(any_editable, cx));

        if is_ws && let Some(data) = parsed_http {
            sections = sections
                .when(req_editable || !data.attributes.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestAttributes,
                        &data.attributes,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.request_headers.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestHeaders,
                        &data.request_headers,
                        req_editable,
                        cx,
                    ))
                });
        } else if let Some(data) = parsed_http {
            sections = sections
                .when(req_editable || !data.attributes.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestAttributes,
                        &data.attributes,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.query_params.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::QueryParams,
                        &data.query_params,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.body_params.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::BodyParams,
                        &data.body_params,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.cookies.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::Cookies,
                        &data.cookies,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.request_headers.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestHeaders,
                        &data.request_headers,
                        req_editable,
                        cx,
                    ))
                })
                .when(resp_editable || !data.response_headers.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::ResponseHeaders,
                        &data.response_headers,
                        resp_editable,
                        cx,
                    ))
                });
        } else if !has_selection_preview {
            sections = sections.child(
                div()
                    .p(px(14.))
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child("在编辑器中框选文本或字节，即可在此实时解码检视。"),
            );
        }

        let section_visible_and_expanded = |idx: usize, has_items: bool, has_add: bool| -> bool {
            self.inspector.expanded[idx] && (has_items || has_add)
        };
        let any_section_expanded = if is_ws {
            parsed_http.is_some_and(|data| {
                section_visible_and_expanded(0, !data.attributes.is_empty(), false)
                    || section_visible_and_expanded(4, !data.request_headers.is_empty(), req_editable)
            })
        } else {
            parsed_http.is_some_and(|data| {
                section_visible_and_expanded(0, !data.attributes.is_empty(), false)
                    || section_visible_and_expanded(1, !data.query_params.is_empty(), req_editable)
                    || section_visible_and_expanded(2, !data.body_params.is_empty(), req_editable)
                    || section_visible_and_expanded(3, !data.cookies.is_empty(), req_editable)
                    || section_visible_and_expanded(4, !data.request_headers.is_empty(), req_editable)
                    || section_visible_and_expanded(5, !data.response_headers.is_empty(), resp_editable)
            })
        };
        let any_expanded =
            (has_selection_preview && self.inspector.selection_expanded) || any_section_expanded;
        let scrollbar_track_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x2b2d30 } else { 0xf7f7f7 }).into();
        let scrollbar_thumb_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x5c626b } else { 0xc8c8c8 }).into();
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .bg(theme::toolbar_surface())
            .child(sections)
            .when(any_expanded, |row| {
                row.child(
                    div()
                        .id("inspector-sections-scrollbar")
                        .test_support()
                        .relative()
                        .w(px(10.))
                        .h_full()
                        .flex_shrink_0()
                        .bg(scrollbar_track_bg)
                        .child(
                            gpui_kit::component::scroll::Scrollbar::vertical(
                                &self.inspector.sections_scroll_handle,
                            )
                            .viewport_from_layout()
                            .mode(gpui_kit::component::scroll::ScrollbarMode::Always)
                            .styles(move |styles| {
                                styles
                                    .track(|s| s.width(px(10.)).bg(scrollbar_track_bg))
                                    .track_hover(|s| s.width(px(10.)).bg(scrollbar_track_bg))
                                    .track_active(|s| s.width(px(10.)).bg(scrollbar_track_bg))
                                    .thumb(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(scrollbar_thumb_bg)
                                    })
                                    .thumb_hover(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(scrollbar_thumb_bg)
                                    })
                                    .thumb_active(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(scrollbar_thumb_bg)
                                    })
                            })
                            .max_fps(60),
                        ),
                )
            })
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_inspector_sidebar(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let show_http = self.inspector_supports_http_sections();
        let ws_inspector = self.websocket_repeater_inspector_data(cx);
        let (parsed_http, req_editable, resp_editable) = if show_http {
            if let Some((ws_data, ws_editable)) = &ws_inspector {
                (Some(ws_data.clone()), *ws_editable, false)
            } else {
                let (data, req_ed, resp_ed) = self.inspect_current_http(cx);
                (Some(data), req_ed, resp_ed)
            }
        } else {
            (None, false, false)
        };
        let selection_writable = self
            .inspector
            .selection_identity
            .as_ref()
            .is_some_and(|id| id.writable);
        let any_editable = req_editable || resp_editable || selection_writable;
        let has_selection_preview = self.inspector.selection_preview.is_some();
        let detail_drawer = self.render_inspector_detail_drawer(
            any_editable,
            req_editable,
            resp_editable,
            parsed_http.as_deref(),
            cx,
        );
        let title_bar = self.render_inspector_title_bar(show_http, cx);
        let sections_with_scrollbar = self.render_inspector_sections_area(
            parsed_http.as_deref(),
            ws_inspector.is_some(),
            req_editable,
            resp_editable,
            any_editable,
            has_selection_preview,
            cx,
        );

        div()
            .id("inspector-sidebar")
            .test_support()
            .w(px(343.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::toolbar_surface())
            .child(title_bar)
            .child(sections_with_scrollbar)
            .when_some(detail_drawer, |sidebar, drawer| sidebar.child(drawer))
            .into_any_element()
    }
}
