use super::*;
use gpui_kit::FontWeight;
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::{ClickEvent, MouseButton, WeakEntity, uniform_list};

fn payloads_header() -> gpui_kit::Div {
    div()
        .px(px(14.))
        .py(px(10.))
        .flex_shrink_0()
        .bg(theme::toolbar_surface())
        .border_b_1()
        .border_color(theme::border())
        .text_size(theme::scaled_px(13.5))
        .font_weight(FontWeight::BOLD)
        .child("Payloads")
}

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_intruder_payloads(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(positions) = editors.request.position_count(cx) else {
            return div()
                .size_full()
                .bg(theme::surface())
                .flex()
                .flex_col()
                .child(payloads_header())
                .child(
                    div()
                        .m(px(14.))
                        .p(px(12.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme::danger_border())
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .text_size(theme::scaled_px(12.5))
                        .child("Payload 标记未配对；请撤销编辑或使用清除 § 修复，输入仍保留。"),
                )
                .into_any_element();
        };
        if positions == 0 {
            return div()
                .size_full()
                .bg(theme::surface())
                .flex()
                .flex_col()
                .child(payloads_header())
                .child(
                    div()
                        .m(px(14.))
                        .p(px(12.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::toolbar_surface())
                        .text_color(theme::muted_foreground())
                        .text_size(theme::scaled_px(12.5))
                        .child("在消息中选择要替换的内容，再使用“添加 §”设置 payload 位置。"),
                )
                .into_any_element();
        }
        let editable = self.transition.is_none() && !editors.loading_payloads;
        div()
            .id("intruder-payloads-panel")
            .test_support()
            .size_full()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(payloads_header())
            .child(self.render_intruder_payloads_top_controls(
                id,
                spec,
                editors,
                positions,
                editable,
                cx,
            ))
            .child(self.render_intruder_payloads_list_section(
                id,
                spec,
                editors,
                editable,
                cx,
            ))
            .child(self.render_intruder_numbers_section(id, editors, editable, cx))
            .child(self.render_intruder_processing_section(id, editable, cx))
            .child(self.render_intruder_generators_cards(id, editable, cx))
            .when(editors.loading_payloads, |this| {
                this.child(
                    div()
                        .px(px(14.))
                        .pb(px(10.))
                        .text_size(theme::scaled_px(12.))
                        .text_color(theme::muted_foreground())
                        .child("正在读取 payload 文件…"),
                )
            })
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_payloads_top_controls(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        positions: usize,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = editors.payload_position;
        let weak = cx.weak_entity();
        let position_count = payload_set_count(spec, positions);
        let dropdown = button(
            "intruder-payload-position",
            selected.map_or_else(
                || "所有 payload 位置".into(),
                |index| format!("Payload 位置 {}", index + 1),
            ),
        )
        .w_full()
        .dropdown_caret(true)
        .tooltip("选择要独立配置载荷列表的标记位置，或统一编辑全部位置")
        .dropdown_menu(move |menu, _, _| {
            std::iter::once(None)
                .chain((0..position_count).map(Some))
                .fold(menu, |menu, position| {
                    let weak = weak.clone();
                    menu.item(
                        PopupMenuItem::new(position.map_or_else(
                            || "所有 payload 位置".into(),
                            |index| format!("Payload 位置 {}", index + 1),
                        ))
                        .checked(position == selected)
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |view, cx| {
                                if let Some(editors) = view.intruder.editors.get_mut(&id) {
                                    editors.payload_position = position;
                                    editors.payload_selection.clear();
                                }
                                cx.notify();
                            });
                        }),
                    )
                })
        });
        let count = displayed_payloads(spec, editors).map(<[_]>::len);
        let current_payload_type = self.current_intruder_payload_type(id, editors);
        let type_weak = cx.weak_entity();
        let payload_type_dropdown = button(
            "intruder-payload-type-dropdown",
            current_payload_type.label(),
        )
        .w_full()
        .dropdown_caret(true)
        .disabled(!editable)
        .tooltip("选择 18 种标准 Payload 生成器类型")
        .dropdown_menu(move |menu, _, _| {
            IntruderPayloadType::ALL
                .into_iter()
                .fold(menu, |menu, payload_type| {
                    let weak = type_weak.clone();
                    menu.item(
                        PopupMenuItem::new(payload_type.label())
                            .checked(payload_type == current_payload_type)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |view, cx| {
                                    view.select_intruder_payload_type(id, payload_type, window, cx);
                                });
                            }),
                    )
                })
        });
        let payload_count_text =
            count.map_or_else(|| "各位置不同".into(), |count| count.to_string());
        let request_count_text = payload_request_count(spec, positions)
            .map_or_else(|| "配置尚未完整".into(), |count| count.to_string());

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(14.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child("Payload 位置"),
                            )
                            .child(div().flex_1().min_w_0().child(dropdown)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child("Payload 类型"),
                            )
                            .child(div().flex_1().min_w_0().child(payload_type_dropdown)),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::toolbar_surface())
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .text_size(theme::scaled_px(12.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_color(theme::muted_foreground())
                                            .child("Payload 数量："),
                                    )
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(payload_count_text),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_color(theme::muted_foreground())
                                            .child("请求数量："),
                                    )
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(request_count_text),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(theme::border())
                    .px(px(14.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Payload 配置"),
                            )
                            .when(
                                current_payload_type != IntruderPayloadType::SimpleList,
                                |row| {
                                    row.child(
                                        button(
                                            "intruder-payload-type-generate",
                                            "生成并应用载荷",
                                        )
                                        .icon(IconName::Sparkles)
                                        .tooltip("按当前所选 Payload 类型与输入框参数生成载荷并应用到当前位置")
                                        .disabled(!editable)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.generate_for_intruder_payload_type(id, window, cx)
                                        })),
                                    )
                                },
                            ),
                    )
                    .child(
                        div()
                            .id("intruder-payload-type-description")
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::muted_foreground())
                            .child(current_payload_type.description()),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_payloads_list_section(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let values = displayed_payloads(spec, editors);
        let count = values.map(<[_]>::len);
        let weak = cx.weak_entity();
        let list = uniform_list(
            "intruder-payload-list",
            count.unwrap_or(0),
            move |range, _, cx| {
                let Some(entity) = weak.upgrade() else {
                    return Vec::new();
                };
                let view = entity.read(cx);
                let Some(tab) = view.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
                    return Vec::new();
                };
                let Some(editors) = view.intruder.editors.get(&id) else {
                    return Vec::new();
                };
                let Some(values) = displayed_payloads(&tab.spec, editors) else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| {
                        values.get(index).map(|value| (index, payload_label(value)))
                    })
                    .map(|(index, label)| {
                        let click_weak = weak.clone();
                        let right_weak = weak.clone();
                        let menu_weak = weak.clone();
                        div()
                            .id(("intruder-payload", index))
                            .test_support()
                            .h(theme::row_height())
                            .flex()
                            .items_center()
                            .px(px(6.))
                            .truncate()
                            .border_b_1()
                            .border_color(theme::border().opacity(0.45))
                            .bg(if editors.payload_selection.contains(&index) {
                                theme::selected()
                            } else {
                                theme::surface()
                            })
                            .child(label)
                            .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                                let _ = right_weak.update(cx, |view, cx| {
                                    if let Some(editors) = view.intruder.editors.get_mut(&id)
                                        && !editors.payload_selection.contains(&index)
                                    {
                                        editors.payload_selection.clear();
                                        editors.payload_selection.insert(index);
                                        cx.notify();
                                    }
                                });
                            })
                            .on_click(move |event: &ClickEvent, _, cx| {
                                let _ = click_weak.update(cx, |view, cx| {
                                    if let Some(editors) = view.intruder.editors.get_mut(&id) {
                                        if !event.modifiers().secondary(cx) {
                                            editors.payload_selection.clear();
                                        }
                                        if !editors.payload_selection.insert(index) {
                                            editors.payload_selection.remove(&index);
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .context_menu(move |menu, _, cx| {
                                intruder_payload_row_menu(menu_weak.clone(), id, index, menu, cx)
                            })
                    })
                    .collect::<Vec<_>>()
            },
        )
        .size_full();

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(14.))
                    .flex()
                    .gap(px(8.))
                    .min_h(theme::scaled_px(154.))
                    .child(
                        div()
                            .w(theme::scaled_px(94.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                button("intruder-payload-paste", "粘贴")
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::ClipboardPaste)
                                    .tooltip("从剪贴板按行解析并追加载荷到当前列表")
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.paste_intruder_payloads(id, window, cx)
                                    })),
                            )
                            .child(
                                button("intruder-payload-load", "加载…")
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::FolderOpen)
                                    .tooltip("从本地字典文件按行流式加载载荷（上限 32 MiB）")
                                    .disabled(
                                        !editable
                                            || self.file_picker_pending()
                                            || !self.backend_available(),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.load_intruder_payloads(id, window, cx)
                                    })),
                            )
                            .child(
                                button("intruder-payload-remove", "移除")
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::Trash)
                                    .tooltip("从当前载荷列表中移除已选中的条目")
                                    .disabled(
                                        !editable
                                            || values.is_none()
                                            || editors.payload_selection.is_empty(),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_intruder_payloads(id, window, cx)
                                    })),
                            )
                            .child(
                                button("intruder-payload-clear", "清空")
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::Eraser)
                                    .tooltip("清空当前位置（或全部位置）的所有载荷条目")
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.clear_intruder_payloads(id, window, cx)
                                    })),
                            )
                            .child(
                                button("intruder-payload-deduplicate", "去重")
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::ListFilter)
                                    .tooltip("按原顺序保留首次出现的载荷并剔除重复项")
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.deduplicate_intruder_payloads(id, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .border_1()
                            .border_color(theme::control_border())
                            .rounded(px(4.))
                            .bg(theme::surface())
                            .overflow_hidden()
                            .when(values.is_none(), |this| {
                                this.child(
                                    div()
                                        .p(px(8.))
                                        .text_size(theme::scaled_px(12.))
                                        .text_color(theme::muted_foreground())
                                        .child("各位置的列表不同；选择具体位置查看。"),
                                )
                            })
                            .when(values.is_some(), |this| this.child(list)),
                    ),
            )
            .child(
                div()
                    .px(px(14.))
                    .py(px(6.))
                    .flex()
                    .gap(px(8.))
                    .child(
                        button("intruder-payload-add", "添加")
                            .w(theme::scaled_px(94.))
                            .justify_start()
                            .icon(IconName::Plus)
                            .tooltip(
                                "将右侧输入框中的字符串作为单条载荷追加到列表（留空可插入空载荷）",
                            )
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_intruder_payload(id, window, cx)
                            })),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&editors.payload_input)
                                .small()
                                .disabled(!editable),
                        ),
                    ),
            )
            .into_any_element()
    }
}

#[inline(never)]
fn intruder_payload_row_menu(
    view: WeakEntity<Workbench>,
    id: TabId,
    index: usize,
    menu: PopupMenu,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let state = entity.read(cx);
    let Some(tab) = state
        .workspace
        .intruder
        .tabs
        .iter()
        .find(|tab| tab.id == id)
    else {
        return menu;
    };
    let Some(editors) = state.intruder.editors.get(&id) else {
        return menu;
    };
    let editable = state.transition.is_none() && !editors.loading_payloads;
    let total_len = displayed_payloads(&tab.spec, editors).map_or(0, <[_]>::len);
    let has_row = index < total_len;
    let copy_view = view.clone();
    let remove_view = view.clone();
    let up_view = view.clone();
    let down_view = view.clone();
    let clear_view = view;
    menu.item(
        PopupMenuItem::new("复制载荷")
            .disabled(!has_row)
            .on_click(move |_, _, cx| {
                let _ = copy_view.update(cx, |view, cx| view.copy_intruder_payload(id, index, cx));
            }),
    )
    .item(
        PopupMenuItem::new("删除")
            .disabled(!editable || !has_row)
            .on_click(move |_, window, cx| {
                let _ = remove_view.update(cx, |view, cx| {
                    view.remove_intruder_payload_at(id, index, window, cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("上移")
            .disabled(!editable || !has_row || index == 0)
            .on_click(move |_, window, cx| {
                let _ = up_view.update(cx, |view, cx| {
                    view.move_intruder_payload(id, index, -1, window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("下移")
            .disabled(!editable || !has_row || index + 1 >= total_len)
            .on_click(move |_, window, cx| {
                let _ = down_view.update(cx, |view, cx| {
                    view.move_intruder_payload(id, index, 1, window, cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("清空列表")
            .disabled(!editable || total_len == 0)
            .on_click(move |_, window, cx| {
                let _ =
                    clear_view.update(cx, |view, cx| view.clear_intruder_payloads(id, window, cx));
            }),
    )
}
