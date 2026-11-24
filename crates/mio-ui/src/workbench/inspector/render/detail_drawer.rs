use super::super::section_singular_label;
use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

impl Workbench {
    fn add_inspector_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match editing::input_snapshot(&self.inspector.name_input, window, cx) {
            Ok(name) => name.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let mut value = match editing::input_snapshot(&self.inspector.value_input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let section = self.effective_inspector_section(cx);
        if self.inspector.detail_url_decoded
            && matches!(
                section,
                InspectorSection::QueryParams
                    | InspectorSection::BodyParams
                    | InspectorSection::Cookies
            )
            && let Ok(encoded) = editing::convert_selection(editing::ConvertKind::UrlEncodeKey, &value)
        {
            value = encoded;
        }
        self.inspector.editing_section = section;
        let _ = self.handle_inspector_mutation(
            InspectorMutation::Add {
                section,
                name,
                value,
            },
            window,
            cx,
        );
    }

    fn update_inspector_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let section = self.effective_inspector_section(cx);
        if self.inspector.editing_section != section {
            return;
        }
        let Some(index) = self.inspector.editing_index else {
            return;
        };
        let name = match editing::input_snapshot(&self.inspector.name_input, window, cx) {
            Ok(name) => name.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let mut value = match editing::input_snapshot(&self.inspector.value_input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if self.inspector.detail_url_decoded
            && matches!(
                section,
                InspectorSection::QueryParams
                    | InspectorSection::BodyParams
                    | InspectorSection::Cookies
            )
            && let Ok(encoded) = editing::convert_selection(editing::ConvertKind::UrlEncodeKey, &value)
        {
            value = encoded;
        }
        let _ = self.handle_inspector_mutation(
            InspectorMutation::Update {
                section,
                index,
                name,
                value,
            },
            window,
            cx,
        );
    }

    pub(super) fn move_inspector_item(
        &mut self,
        section: InspectorSection,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from == to {
            return;
        }
        if self
            .inspector
            .name_input
            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .inspector
                .value_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        if self.handle_inspector_mutation(
            InspectorMutation::Move { section, from, to },
            window,
            cx,
        ) && self.inspector.editing_section == section
            && let Some(sel) = self.inspector.editing_index
        {
            if sel == from {
                self.inspector.editing_index = Some(to);
            } else if from < to && sel > from && sel <= to {
                self.inspector.editing_index = Some(sel - 1);
            } else if to < from && sel >= to && sel < from {
                self.inspector.editing_index = Some(sel + 1);
            }
        }
    }

    fn step_inspector_detail_item(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .inspector
            .name_input
            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .inspector
                .value_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let Some(cur_idx) = self.inspector.editing_index else {
            return;
        };
        let parsed = if let Some((ws_data, _)) = self.websocket_repeater_inspector_data(cx) {
            ws_data
        } else {
            self.inspect_current_http(cx).0
        };
        let items = section_items_slice(&parsed, self.inspector.editing_section);
        let next_idx = cur_idx.checked_add_signed(delta);
        let Some(target_idx) = next_idx.filter(|&i| i < items.len()) else {
            return;
        };
        let item = items[target_idx].clone();
        self.inspector.editing_index = Some(target_idx);
        self.inspector.detail_url_decoded = false;
        self.inspector
            .name_input
            .update(cx, |s, cx| s.set_value(item.name, window, cx));
        self.inspector
            .value_input
            .update(cx, |s, cx| s.set_value(item.value, window, cx));
        cx.notify();
    }

    pub(super) fn remove_inspector_item(
        &mut self,
        section: InspectorSection,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.inspector.editing_section == section
            && self.inspector.editing_index == Some(index)
            && (self
                .inspector
                .name_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
                || self
                    .inspector
                    .value_input
                    .update(cx, |s, cx| s.marked_text_range(window, cx).is_some()))
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        if self.handle_inspector_mutation(InspectorMutation::Remove { section, index }, window, cx)
            && self.inspector.editing_section == section
        {
            match self.inspector.editing_index {
                Some(k) if k == index => {
                    self.unfocus_inspector_detail_inputs(window, cx);
                    self.inspector.editing_index = None;
                    self.inspector.editor_open = false;
                }
                Some(k) if k > index => {
                    self.inspector.editing_index = Some(k - 1);
                }
                _ => {}
            }
        }
    }

    pub(in crate::workbench::inspector) fn switch_inspector_request_protocol(
        &mut self,
        protocol: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .inspector
            .name_input
            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .inspector
                .value_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        self.inspector.auto_opened_by_selection = false;
        let _ = self.handle_inspector_mutation(
            InspectorMutation::Update {
                section: InspectorSection::RequestAttributes,
                index: 2,
                name: "Protocol".into(),
                value: protocol.to_owned(),
            },
            window,
            cx,
        );
    }

    pub(super) fn render_inspector_detail_drawer(
        &self,
        any_editable: bool,
        req_editable: bool,
        resp_editable: bool,
        parsed_data: Option<&InspectorData>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.inspector.editor_open {
            return None;
        }
        let section = if self.inspector.editing_index.is_some() || !any_editable {
            self.inspector.editing_section
        } else {
            self.effective_inspector_section_for(req_editable, resp_editable)
        };
        let section_len = parsed_data.map_or(0, |d| section_items_slice(d, section).len());
        let has_selection = self.inspector.editing_section == section
            && self
                .inspector
                .editing_index
                .is_some_and(|idx| idx < section_len);
        let section_editable = match section {
            InspectorSection::ResponseHeaders => resp_editable,
            _ => req_editable,
        };
        if !section_editable && !has_selection {
            return None;
        }
        let title = if section_editable {
            if has_selection {
                format!("编辑 · {}", section_singular_label(section))
            } else {
                format!("新增 · {}", section_singular_label(section))
            }
        } else {
            format!("键值详情 · {}", section_singular_label(section))
        };
        let current_idx = self.inspector.editing_index.filter(|_| has_selection);
        let can_prev = current_idx.is_some_and(|i| i > 0);
        let can_next = current_idx.is_some_and(|i| i + 1 < section_len);
        let cur_name_val = self.inspector.name_input.read(cx).value().to_string();
        let cur_value_val = self.inspector.value_input.read(cx).value().to_string();
        let raw_item_value = current_idx
            .and_then(|idx| parsed_data.and_then(|d| section_items_slice(d, section).get(idx)))
            .map(|it| it.value.clone());
        let supports_url_toggle = matches!(
            section,
            InspectorSection::QueryParams | InspectorSection::BodyParams | InspectorSection::Cookies
        ) && (self.inspector.detail_url_decoded
            || cur_value_val.contains('%')
            || cur_value_val.contains('+')
            || raw_item_value
                .as_deref()
                .is_some_and(|v| v.contains('%') || v.contains('+')));
        let readonly_box_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x232529 } else { 0xf6f7f9 }).into();
        let subtle_border: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x34373c } else { 0xdde1e7 }).into();
        Some(
            div()
                .id("inspector-detail-drawer")
                .test_support()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(theme::border())
                .bg(theme::surface())
                .when(section_editable, |drawer| {
                    drawer.child(
                        div()
                            .w_full()
                            .min_h(theme::scaled_px(26.))
                            .px(px(8.))
                            .bg(theme::toolbar_surface())
                            .border_b_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(theme::scaled_px(11.5))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(theme::foreground())
                                    .child(title),
                            )
                            .when(has_selection && section_len > 1, |bar| {
                                bar.child(
                                    Button::new("inspector-prev-item")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .w(theme::scaled_px(20.))
                                        .h(theme::scaled_px(20.))
                                        .icon(IconName::ChevronLeft)
                                        .accessibility_label("上一项")
                                        .tooltip("切换到当前分组中的上一项")
                                        .disabled(!can_prev)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.step_inspector_detail_item(-1, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("inspector-next-item")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .w(theme::scaled_px(20.))
                                        .h(theme::scaled_px(20.))
                                        .icon(IconName::ChevronRight)
                                        .accessibility_label("下一项")
                                        .tooltip("切换到当前分组中的下一项")
                                        .disabled(!can_next)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.step_inspector_detail_item(1, window, cx);
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .when(supports_url_toggle, |bar| {
                                let url_decoded = self.inspector.detail_url_decoded;
                                bar.child(
                                    Button::new("inspector-detail-url-decode-toggle")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .h(theme::scaled_px(21.))
                                        .px(px(5.))
                                        .label("URL 解码")
                                        .selected(url_decoded)
                                        .tooltip(if url_decoded {
                                            "切换回 URL 原始编码视图"
                                        } else {
                                            "将当前参数值按 URL 解码展示（保存时自动重编码）"
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let Ok(current) = editing::input_snapshot(
                                                &this.inspector.value_input,
                                                window,
                                                cx,
                                            ) else {
                                                this.fail(editing::COMPOSITION_PENDING, cx);
                                                return;
                                            };
                                            let current = current.to_string();
                                            if this.inspector.detail_url_decoded {
                                                if let Ok(enc) = editing::convert_selection(
                                                    editing::ConvertKind::UrlEncodeKey,
                                                    &current,
                                                ) {
                                                    this.inspector.value_input.update(
                                                        cx,
                                                        |s, cx| s.set_value(enc, window, cx),
                                                    );
                                                    this.inspector.detail_url_decoded = false;
                                                }
                                            } else if let Ok(dec) = editing::convert_selection(
                                                editing::ConvertKind::UrlDecode,
                                                &current,
                                            ) {
                                                this.inspector
                                                    .value_input
                                                    .update(cx, |s, cx| s.set_value(dec, window, cx));
                                                this.inspector.detail_url_decoded = true;
                                            }
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(self.render_inspector_detail_action_buttons(cx)),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .p(px(8.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .w(theme::scaled_px(40.))
                                        .flex_shrink_0()
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::muted_foreground())
                                        .child("Name"),
                                )
                                .child(div().flex_1().min_w_0().child(if section_editable {
                                    Input::new(&self.inspector.name_input)
                                        .id("inspector-name-input")
                                        .aria_label("名称")
                                        .disabled(section == InspectorSection::RequestAttributes)
                                        .small()
                                        .into_any_element()
                                } else {
                                    div()
                                        .id("inspector-readonly-name")
                                        .test_support()
                                        .w_full()
                                        .max_h(theme::scaled_px(48.))
                                        .overflow_y_scroll()
                                        .px(px(7.))
                                        .py(px(4.))
                                        .rounded(px(4.))
                                        .bg(readonly_box_bg)
                                        .border_1()
                                        .border_color(subtle_border)
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::foreground())
                                        .child(cur_name_val.clone())
                                        .into_any_element()
                                }))
                                .when(!section_editable, |row| {
                                    row.child(self.render_inspector_detail_action_buttons(cx))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .w(theme::scaled_px(40.))
                                        .pt(px(4.))
                                        .flex_shrink_0()
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::muted_foreground())
                                        .child("Value"),
                                )
                                .child(div().flex_1().min_w_0().child(if section_editable {
                                    Input::new(&self.inspector.value_input)
                                        .id("inspector-value-input")
                                        .aria_label("值")
                                        .small()
                                        .into_any_element()
                                } else {
                                    div()
                                        .id("inspector-readonly-value")
                                        .test_support()
                                        .w_full()
                                        .max_h(theme::scaled_px(132.))
                                        .overflow_y_scroll()
                                        .px(px(7.))
                                        .py(px(5.))
                                        .rounded(px(4.))
                                        .bg(readonly_box_bg)
                                        .border_1()
                                        .border_color(subtle_border)
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::foreground())
                                        .child(cur_value_val.clone())
                                        .into_any_element()
                                })),
                        )
                        .when(section_editable, |form| {
                            form.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap(px(6.))
                                    .pt(px(2.))
                                    .when(section != InspectorSection::RequestAttributes, |row| {
                                        row.child(
                                            button("inspector-add-button", "新增")
                                                .icon(IconName::Plus)
                                                .tooltip("作为新条目追加到当前报文分组")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.add_inspector_item(window, cx)
                                                })),
                                        )
                                    })
                                    .child(
                                        button("inspector-apply-button", "保存修改")
                                            .primary()
                                            .icon(IconName::Check)
                                            .tooltip("将修改后的名称与值写回报文中当前选中的条目")
                                            .disabled(!has_selection)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.update_inspector_item(window, cx)
                                            })),
                                    ),
                            )
                        }),
                )
                .into_any_element()
        )
    }

    #[inline(never)]
    fn render_inspector_detail_action_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                Button::new("inspector-copy-name")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(21.))
                    .px(px(5.))
                    .icon(IconName::Copy)
                    .label("复制名称")
                    .tooltip("复制当前条目的完整 Name 到剪贴板")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let name = this.inspector.name_input.read(cx).value().to_string();
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(name));
                        this.status = "已复制 Inspector 条目名称到剪贴板。".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("inspector-copy-value")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(21.))
                    .px(px(5.))
                    .icon(IconName::Copy)
                    .label("复制值")
                    .tooltip("复制当前条目的完整 Value 到剪贴板")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let val = this.inspector.value_input.read(cx).value().to_string();
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(val));
                        this.status = "已复制 Inspector 条目值到剪贴板。".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("inspector-editor-close")
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(20.))
                    .h(theme::scaled_px(20.))
                    .icon(IconName::Close)
                    .accessibility_label("关闭条目面板")
                    .tooltip("收起底部键值面板")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.inspector.name_input.update(cx, |s, cx| {
                            s.marked_text_range(window, cx).is_some()
                        }) || this.inspector.value_input.update(cx, |s, cx| {
                            s.marked_text_range(window, cx).is_some()
                        }) {
                            this.fail(editing::COMPOSITION_PENDING, cx);
                            return;
                        }
                        this.unfocus_inspector_detail_inputs(window, cx);
                        this.inspector.editor_open = false;
                        this.inspector.editing_index = None;
                        this.inspector.detail_url_decoded = false;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
