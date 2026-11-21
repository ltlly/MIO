use super::*;

impl NativePanel {
    pub(in crate::plugin_panel) fn render_node(
        &self,
        node: &Node,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = node.id;
        let mut frame = div()
            .id(("plugin-node", id))
            .test_support()
            .min_w_0()
            .min_h_0()
            .overflow_hidden();
        if self
            .snapshot
            .root
            .as_ref()
            .is_some_and(|root| root.id == id)
        {
            frame = frame.size_full();
        }
        if let Some(layout) = &node.layout {
            frame = frame
                .min_w(px(layout.min_width))
                .min_h(px(layout.min_height))
                .p(px(layout.padding))
                .gap(px(layout.gap));
            if layout.flex > 0. {
                frame = frame.flex_grow(layout.flex).flex_basis(px(0.));
            }
        }
        match node.kind.as_ref().unwrap() {
            node::Kind::Row(container)
            | node::Kind::Column(container)
            | node::Kind::ScrollColumn(container) => {
                frame = frame
                    .flex()
                    .when(matches!(node.kind, Some(node::Kind::Row(_))), |frame| {
                        frame.overflow_x_scroll()
                    })
                    .when(matches!(node.kind, Some(node::Kind::Column(_))), |frame| {
                        frame.flex_col()
                    })
                    .when(
                        matches!(node.kind, Some(node::Kind::ScrollColumn(_))),
                        |frame| frame.flex_col().overflow_y_scroll(),
                    );
                for child in &container.children {
                    frame = frame.child(self.render_node(child, cx));
                }
            }
            node::Kind::Checkbox(checkbox) => {
                frame = frame
                    .flex_shrink_0()
                    .child(self.render_checkbox(id, checkbox, cx));
            }
            node::Kind::Select(select) => {
                frame = frame
                    .flex_shrink_0()
                    .child(self.render_select(id, select, cx));
            }
            node::Kind::Badge(badge) => {
                let fg = tone(badge.tone);
                frame = frame.flex_shrink_0().child(
                    div()
                        .id(("plugin-badge", id))
                        .test_support()
                        .px(theme::scaled_px(7.))
                        .py(px(1.))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(fg)
                        .text_size(theme::scaled_px(11.))
                        .text_color(fg)
                        .child(badge.text.clone()),
                );
            }
            node::Kind::Progress(progress) => {
                frame = frame.flex_shrink_0().child(Self::render_progress(
                    id,
                    &node.accessibility_label,
                    progress,
                ));
            }
            node::Kind::Text(text) => {
                frame = frame.child(div().text_color(tone(text.tone)).child(text.text.clone()));
            }
            node::Kind::Button(button) => {
                let expected_label = button.label.clone();
                let tip = if node.accessibility_label.is_empty() {
                    format!("执行插件操作：{}", button.label)
                } else {
                    node.accessibility_label.clone()
                };
                frame = frame.flex_shrink_0().child(
                    Button::new(("plugin-button", id))
                        .label(button.label.clone())
                        .tooltip(tip)
                        .small()
                        .disabled(!button.enabled || !self.enabled)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let matches_current = matches!(
                                this.node(id).and_then(|node| node.kind.as_ref()),
                                Some(node::Kind::Button(current))
                                    if current.enabled && current.label == expected_label
                            );
                            if !matches_current {
                                this.show_diagnostic(
                                    "插件面板控件内容已更新；旧视图中的操作未执行，请重试",
                                    cx,
                                );
                                return;
                            }
                            this.activate(id, window, cx);
                        })),
                );
            }
            node::Kind::Editor(editor) => {
                if let Some(document) = self.documents.get(&editor.document_id) {
                    frame = frame
                        .flex_1()
                        .size_full()
                        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                            this.key(id, event, cx)
                        }))
                        .child(
                            Editor::new(&document.state)
                                .aria_label(node.accessibility_label.clone())
                                .readonly(document.read_only || !self.enabled)
                                .bordered(false)
                                .size_full(),
                        );
                }
            }
            node::Kind::Tabs(tabs) => {
                let active = latest_overlay(&self.tabs, id).unwrap_or(tabs.active);
                let horizontal = tabs.axis == Axis::Horizontal as i32;
                let mut bar = div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(2.))
                    .when(!horizontal, |bar| bar.flex_col());
                for tab in &tabs.tabs {
                    let child = tab.content.as_ref().unwrap();
                    let selected = child.id;
                    let expected_title = tab.title.clone();
                    bar = bar.child(
                        Button::new(("plugin-tab", selected))
                            .label(tab.title.clone())
                            .tooltip(format!("切换到插件标签页「{}」", tab.title))
                            .small()
                            .selected(active == selected)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let matches_current = matches!(
                                    this.node(id).and_then(|node| node.kind.as_ref()),
                                    Some(node::Kind::Tabs(tabs))
                                        if tabs.tabs.iter().any(|t| {
                                            t.content.as_ref().is_some_and(|c| c.id == selected)
                                                && t.title == expected_title
                                        })
                                );
                                if !matches_current {
                                    this.show_diagnostic(
                                        "插件面板控件内容已更新；旧视图中的操作未执行，请重试",
                                        cx,
                                    );
                                    return;
                                }
                                this.cancel_interaction(cx);
                                if this.enabled {
                                    let seq = this.next_ui_seq();
                                    push_overlay(&mut this.tabs, id, selected, seq);
                                    this.remember_visible_document();
                                    this.emit_with_seq(
                                        id,
                                        seq,
                                        ui_event::Event::TabSelected(selected),
                                        cx,
                                    );
                                    this.prepare_drawings(window, cx);
                                }
                            })),
                    );
                }
                frame = frame
                    .flex()
                    .flex_1()
                    .when(horizontal, |frame| frame.flex_col())
                    .child(bar);
                if let Some(tab) = tabs
                    .tabs
                    .iter()
                    .find(|tab| tab.content.as_ref().is_some_and(|node| node.id == active))
                {
                    frame = frame.child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(self.render_node(tab.content.as_ref().unwrap(), cx)),
                    );
                }
            }
            node::Kind::Split(split) => {
                let horizontal = split.axis == Axis::Horizontal as i32;
                let fraction = latest_overlay(&self.splits, id).unwrap_or(split.fraction);
                let bounds = self.geometry[&id].clone();
                let handle = div()
                    .id(("plugin-split", id))
                    .test_support()
                    .flex_shrink_0()
                    .bg(theme::border())
                    .when(horizontal, |handle| {
                        handle.w(px(5.)).h_full().cursor_col_resize()
                    })
                    .when(!horizontal, |handle| {
                        handle.h(px(5.)).w_full().cursor_row_resize()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if this.enabled {
                                this.resize = Some(Resize {
                                    node_id: id,
                                    original: fraction,
                                });
                                cx.notify();
                            }
                        }),
                    );
                frame = frame
                    .flex()
                    .flex_1()
                    .relative()
                    .when(!horizontal, |frame| frame.flex_col())
                    .child(
                        canvas(move |geometry, _, _| bounds.set(geometry), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_basis(relative(fraction))
                            .flex_grow(fraction)
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_node(split.first.as_ref().unwrap(), cx)),
                    )
                    .child(handle)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_basis(relative(1. - fraction))
                            .flex_grow(1. - fraction)
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_node(split.second.as_ref().unwrap(), cx)),
                    );
            }
            node::Kind::VirtualTable(table) => {
                frame = frame.flex_1().child(self.render_table(node, table, cx));
            }
            node::Kind::Canvas(model) => {
                frame = frame
                    .flex_shrink_0()
                    .h(px(model.height.min(300.)))
                    .overflow_scroll()
                    .child(self.render_canvas(id, model, cx));
            }
        }
        frame.into_any_element()
    }

    fn render_checkbox(
        &self,
        id: u64,
        checkbox: &panel::CheckboxNode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let checked = latest_overlay(&self.checkboxes, id).unwrap_or(checkbox.checked);
        let expected_label = checkbox.label.clone();
        Checkbox::new(("plugin-checkbox", id))
            .label(checkbox.label.clone())
            .checked(checked)
            .disabled(!checkbox.enabled || !self.enabled)
            .small()
            .on_click(cx.listener(move |this, next, _, cx| {
                let matches_current = matches!(
                    this.node(id).and_then(|node| node.kind.as_ref()),
                    Some(node::Kind::Checkbox(current))
                        if current.enabled && current.label == expected_label
                );
                if !matches_current {
                    this.show_diagnostic(
                        "插件面板控件内容已更新；旧视图中的操作未执行，请重试",
                        cx,
                    );
                    return;
                }
                if this.enabled {
                    let seq = this.next_ui_seq();
                    push_overlay(&mut this.checkboxes, id, *next, seq);
                    this.emit_with_seq(id, seq, ui_event::Event::Toggled(*next), cx);
                }
            }))
            .into_any_element()
    }

    fn render_select(
        &self,
        id: u64,
        select: &panel::SelectNode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = latest_overlay(&self.selects, id)
            .filter(|idx| (*idx as usize) < select.options.len())
            .unwrap_or(select.selected_index);
        let current = select
            .options
            .get(selected as usize)
            .cloned()
            .unwrap_or_default();
        let expected_label = select.label.clone();
        let options = select.options.clone();
        let weak = cx.weak_entity();
        Button::new(("plugin-select", id))
            .label(format!("{}: {current}", select.label))
            .tooltip(format!("选择「{}」选项（当前：{current}）", select.label))
            .small()
            .dropdown_caret(true)
            .disabled(!select.enabled || !self.enabled)
            .dropdown_menu(move |menu, _, _| {
                options
                    .iter()
                    .enumerate()
                    .fold(menu, |menu, (index, option)| {
                        let weak = weak.clone();
                        let idx = index as u32;
                        let expected_label = expected_label.clone();
                        let expected_option = option.clone();
                        menu.item(
                            PopupMenuItem::new(option.clone())
                                .checked(selected == idx)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        if this.enabled {
                                            let matches_current = matches!(
                                                this.node(id).and_then(|node| node.kind.as_ref()),
                                                Some(node::Kind::Select(current_select))
                                                    if current_select.enabled
                                                        && current_select.label == expected_label
                                                        && current_select.options.get(idx as usize)
                                                            == Some(&expected_option)
                                            );
                                            if !matches_current {
                                                this.show_diagnostic(
                                                    "下拉选项已更新；旧菜单中的选择未执行，请重新打开菜单选择",
                                                    cx,
                                                );
                                                return;
                                            }
                                            let seq = this.next_ui_seq();
                                            push_overlay(&mut this.selects, id, idx, seq);
                                            this.emit_with_seq(
                                                id,
                                                seq,
                                                ui_event::Event::SelectedOption(idx),
                                                cx,
                                            );
                                        }
                                    });
                                }),
                        )
                    })
            })
            .into_any_element()
    }

    fn render_progress(
        id: u64,
        accessibility_label: &str,
        progress: &panel::ProgressNode,
    ) -> AnyElement {
        let pct = progress.percent.min(100);
        div()
            .id(("plugin-progress-box", id))
            .test_support()
            .flex()
            .items_center()
            .gap(px(6.))
            .when(!progress.label.is_empty(), |row| {
                row.child(
                    div()
                        .text_size(theme::scaled_px(11.))
                        .text_color(rgb(0x6b6b6b))
                        .child(progress.label.clone()),
                )
            })
            .child(
                div().w(theme::scaled_px(72.)).child(
                    Progress::new(("plugin-progress", id))
                        .value(pct as f32)
                        .small()
                        .accessibility_label(accessibility_label.to_owned()),
                ),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(11.))
                    .text_color(rgb(0x6b6b6b))
                    .child(format!("{pct}%")),
            )
            .into_any_element()
    }
}

impl Render for NativePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if let Some(id) = self
            .recovery
            .and_then(|id| self.documents.get(&id).map(|_| id))
        {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    Button::new("plugin-close-recovery")
                        .label("返回插件面板")
                        .tooltip("关闭保留内容预览并返回插件主面板")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.recovery = None;
                            cx.notify();
                        })),
                )
                .child(
                    Editor::new(&self.documents[&id].state)
                        .readonly(true)
                        .aria_label("已解除绑定的插件编辑内容")
                        .size_full(),
                )
                .into_any_element()
        } else {
            self.render_node(self.snapshot.root.as_ref().unwrap(), cx)
        };
        let mut root = div()
            .id("native-plugin-panel")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when_some(self.error.clone(), |root, error| {
                root.child(div().text_color(rgb(0xb42318)).child(error))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(body),
            );
        for (id, document) in self
            .documents
            .iter()
            .filter(|(_, document)| !document.live && document.unsaved)
        {
            let id = *id;
            root = root.child(
                Button::new(("plugin-recover", id))
                    .label(format!("保留的编辑内容：{}", document.label))
                    .tooltip(format!(
                        "以只读方式查看并复制已解绑文档「{}」中未保存的编辑内容",
                        document.label
                    ))
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.recovery = Some(id);
                        cx.notify();
                    })),
            );
        }
        if self.capture.is_some() || self.resize.is_some() {
            let weak = cx.weak_entity();
            root = root.child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let moving = weak.clone();
                        window.on_mouse_event::<MouseMoveEvent>(move |event, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = moving.update(cx, |this, cx| {
                                    this.drag_pointer(event.position, PointerPhase::Move, cx);
                                    this.resize_split(event.position, cx);
                                });
                            }
                        });
                        window.on_mouse_event::<MouseUpEvent>(move |event, phase, _, cx| {
                            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                                let _ = weak.update(cx, |this, cx| {
                                    this.drag_pointer(event.position, PointerPhase::Up, cx);
                                    this.resize_split(event.position, cx);
                                    this.resize = None;
                                    cx.notify();
                                });
                            }
                        });
                    },
                )
                .absolute()
                .size(px(0.)),
            );
        }
        root
    }
}
