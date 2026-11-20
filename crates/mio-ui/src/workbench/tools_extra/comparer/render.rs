use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};

use super::*;

fn comparer_editor_context_menu(
    input: Entity<EditorState>,
    readonly: bool,
) -> impl Fn(
    gpui_kit::component::native_menu::NativeMenu,
    &mut Window,
    &mut gpui_kit::App,
) -> gpui_kit::component::native_menu::NativeMenu
+ 'static {
    use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
    move |menu, window, cx| {
        input.update(cx, |state, cx| state.focus(window, cx));
        let selected = !input.read(cx).selected_range().is_empty();
        let empty = input.read(cx).value().is_empty();
        menu.menu_with_disabled("剪切", readonly || !selected, Box::new(Cut))
            .menu_with_disabled("复制", !selected, Box::new(Copy))
            .menu_with_disabled("粘贴", readonly, Box::new(Paste))
            .separator()
            .menu_with_disabled("全选", empty, Box::new(SelectAll))
    }
}

fn comparer_item_preview(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let max_chars = theme::table_cell_preview_chars();
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            let mut out = String::new();
            let mut prev_ws = false;
            let mut count = 0;
            for ch in text.chars() {
                if count >= max_chars {
                    out.push('…');
                    break;
                }
                if ch == '\r' || ch == '\n' || ch == '\t' || ch.is_control() {
                    if !prev_ws && !out.is_empty() {
                        out.push(' ');
                        count += 1;
                        prev_ws = true;
                    }
                } else {
                    out.push(ch);
                    count += 1;
                    prev_ws = false;
                }
            }
            out
        }
        Err(_) => {
            let sample_len = bytes.len().min(64);
            let mut hex = editing::hex_encode(&bytes[..sample_len]);
            if bytes.len() > sample_len {
                hex.push('…');
            }
            hex
        }
    }
}

fn comparer_action_button(
    id: &'static str,
    label: &'static str,
    tip: &'static str,
    available: bool,
    neutral_fill: bool,
    selected: bool,
) -> gpui_kit::base::Button {
    let dark = theme::is_dark();
    let (bg, hover_bg, active_bg, border, fg): (
        gpui_kit::Hsla,
        gpui_kit::Hsla,
        gpui_kit::Hsla,
        gpui_kit::Hsla,
        gpui_kit::Hsla,
    ) = if selected {
        (
            rgb(if dark { 0x3e4247 } else { 0xe2e2e2 }).into(),
            rgb(if dark { 0x464b52 } else { 0xdadada }).into(),
            rgb(if dark { 0x4e545c } else { 0xd0d0d0 }).into(),
            theme::accent(),
            rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e }).into(),
        )
    } else if neutral_fill {
        (
            rgb(if dark { 0x33363a } else { 0xececec }).into(),
            rgb(if dark { 0x3e4247 } else { 0xe2e2e2 }).into(),
            rgb(if dark { 0x4b5056 } else { 0xd8d8d8 }).into(),
            rgb(if dark { 0x4e535b } else { 0xd0d0d0 }).into(),
            rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e }).into(),
        )
    } else {
        (
            rgb(if dark { 0x2b2d30 } else { 0xffffff }).into(),
            rgb(if dark { 0x36393f } else { 0xf2f2f2 }).into(),
            rgb(if dark { 0x41454c } else { 0xe6e6e6 }).into(),
            rgb(if dark { 0x4e535b } else { 0xd0d0d0 }).into(),
            rgb(if dark { 0xcfd3d8 } else { 0x2b2b2b }).into(),
        )
    };
    gpui_kit::base::Button::new(id)
        .accessibility_label(label)
        .disabled(!available)
        .w_full()
        .h(theme::scaled_px(26.).max(px(22.)))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(fg)
        .hover(move |s| if available { s.bg(hover_bg) } else { s })
        .active(move |s| if available { s.bg(active_bg) } else { s })
        .styles(|styles| styles.disabled(|s| s.opacity(0.45)))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .child(label)
}

fn comparer_help_popover(title_color: gpui_kit::Hsla) -> Popover {
    Popover::new("comparer-help-popover")
        .anchor(gpui_kit::Anchor::BottomRight)
        .trigger(
            Button::new("comparer-help")
                .ghost()
                .compact()
                .p_0()
                .size(theme::scaled_px(22.))
                .tab_stop(false)
                .accessibility_label("Comparer 帮助")
                .tooltip("查看 Comparer 使用说明")
                .child(
                    gpui_kit::component::Icon::new(IconName::CircleQuestionMark)
                        .size(theme::scaled_px(18.))
                        .text_color(title_color),
                ),
        )
        .content(|_, _, _| {
            div()
                .id("comparer-help-content")
                .test_support()
                .w(theme::scaled_px(320.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_size(theme::ui_font_size())
                .text_color(theme::foreground())
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .child("Comparer"),
                )
                .child("在上方两个列表中分别选中条目 1 与条目 2，点击右侧“字词”或“字节”即可按分词或十六进制字节粒度执行对比。")
                .child("可通过“粘贴”、“加载”或从 Proxy、Repeater、Intruder、Organiser、Logger 等工具右键“发送到 Comparer”添加对比项。")
        })
}

impl Workbench {
    fn render_comparer_selection_table(
        &self,
        second: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xd4d4d4).into()
        };
        let header_bg: gpui_kit::Hsla = if dark {
            rgb(0x25272b).into()
        } else {
            rgb(0xf1f1f1).into()
        };
        let header_fg: gpui_kit::Hsla = if dark {
            rgb(0xd4d4d4).into()
        } else {
            rgb(0x141414).into()
        };
        let col_sep: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xdcdcdc).into()
        };
        let cell_sep: gpui_kit::Hsla = if dark {
            rgb(0x33363a).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let row_border: gpui_kit::Hsla = if dark {
            rgb(0x33363a).into()
        } else {
            rgb(0xefefef).into()
        };
        let table_bg = theme::surface();
        let even_row_bg: gpui_kit::Hsla = if dark {
            rgb(0x25272b).into()
        } else {
            rgb(0xfafafa).into()
        };
        let row_hover_bg: gpui_kit::Hsla = if dark {
            rgb(0x26334a).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let text_color: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x1e1e1e).into()
        };

        let header = div()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .bg(header_bg)
            .border_b_1()
            .border_color(table_border)
            .flex()
            .items_center()
            .text_size(theme::ui_font_size())
            .text_color(header_fg)
            .child(
                div()
                    .w(gpui_kit::relative(0.24))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(7.))
                    .border_r_1()
                    .border_color(col_sep)
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child("#"),
            )
            .child(
                div()
                    .w(gpui_kit::relative(0.24))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(7.))
                    .border_r_1()
                    .border_color(col_sep)
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child("Length"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child("Data"),
            );

        let list_id = if second {
            "comparer-items-2"
        } else {
            "comparer-items"
        };
        let row_id_prefix = if second {
            "comparer-item2-row"
        } else {
            "comparer-item-row"
        };
        let selected_idx = if second {
            self.extra_tools.right_index
        } else {
            self.extra_tools.left_index
        };

        let mut rows = div()
            .id(list_id)
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .bg(table_bg)
            .flex()
            .flex_col();

        for (index, item) in self.extra_tools.comparer_items.iter().enumerate() {
            let selected = selected_idx == index;
            let row_bg = if selected {
                theme::selected()
            } else if index % 2 == 1 {
                even_row_bg
            } else {
                table_bg
            };
            let preview = comparer_item_preview(&item.bytes);
            let aria = format!("#{} ({} 字节) {}", item.id, item.bytes.len(), preview);
            rows = rows.child(
                div()
                    .id((row_id_prefix, index))
                    .test_support()
                    .aria_label(aria)
                    .aria_selected(selected)
                    .w_full()
                    .h(theme::scaled_px(21.).max(px(19.)))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(row_bg)
                    .when(!selected, |row| row.hover(move |s| s.bg(row_hover_bg)))
                    .border_b_1()
                    .border_color(row_border)
                    .text_size(theme::ui_font_size())
                    .text_color(text_color)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if second {
                            this.extra_tools.right_index = index;
                        } else {
                            this.extra_tools.left_index = index;
                        }
                        this.extra_tools.comparer_selected = index;
                        this.refresh_comparer(window, cx);
                    }))
                    .child(
                        div()
                            .w(gpui_kit::relative(0.24))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(7.))
                            .border_r_1()
                            .border_color(cell_sep)
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .child(div().truncate().child(item.id.to_string())),
                    )
                    .child(
                        div()
                            .w(gpui_kit::relative(0.24))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(7.))
                            .border_r_1()
                            .border_color(cell_sep)
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .child(div().truncate().child(item.bytes.len().to_string())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(7.))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .child(div().truncate().child(preview)),
                    ),
            );
        }

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .border_1()
            .border_color(table_border)
            .bg(table_bg)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(rows)
            .into_any_element()
    }

    fn render_comparer_diff_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let border_color: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xd4d4d4).into()
        };
        let left_id = self
            .extra_tools
            .comparer_items
            .get(self.extra_tools.left_index)
            .map_or(1, |item| item.id);
        let right_id = self
            .extra_tools
            .comparer_items
            .get(self.extra_tools.right_index)
            .map_or(2, |item| item.id);
        let mode_label = if self.extra_tools.byte_mode {
            "字节 (Bytes)"
        } else {
            "字词 (Words)"
        };

        let diff_toolbar = toolbar()
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(format!("对比结果 · {mode_label} (#{left_id} ↔ #{right_id})")),
            )
            .child(
                button("comparer-view-unified", "统一视图")
                    .icon(IconName::Rows2)
                    .tooltip("在单栏视图内合并展示增删改差异")
                    .selected(!self.extra_tools.comparer_side_by_side)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.extra_tools.comparer_side_by_side = false;
                        this.refresh_comparer(window, cx);
                    })),
            )
            .child(
                button("comparer-view-side-by-side", "并排视图")
                    .icon(IconName::Columns2)
                    .tooltip("左右双栏并排对照展示 #A 与 #B 差异")
                    .selected(self.extra_tools.comparer_side_by_side)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.extra_tools.comparer_side_by_side = true;
                        this.refresh_comparer(window, cx);
                    })),
            )
            .child(
                Checkbox::new("comparer-sync-views")
                    .label("同步视图 (Sync views)")
                    .tooltip("并排视图下联动同步左右编辑器滚动位置")
                    .checked(self.extra_tools.comparer_sync_views)
                    .disabled(!self.extra_tools.comparer_side_by_side)
                    .on_click(cx.listener(|this, checked, _, cx| {
                        this.extra_tools.comparer_sync_views = *checked;
                        if *checked && this.extra_tools.comparer_side_by_side {
                            let offset =
                                this.extra_tools.comparer_left_view.read(cx).scroll_offset();
                            if this.extra_tools.comparer_right_view.read(cx).scroll_offset()
                                != offset
                            {
                                this.extra_tools
                                    .comparer_right_view
                                    .update(cx, |right_ed, cx| {
                                        right_ed.set_scroll_offset(offset, cx)
                                    });
                            }
                        }
                        cx.notify();
                    })),
            )
            .child(
                button("comparer-prev-diff", "上一处差异")
                    .icon(IconName::ChevronUp)
                    .disabled(self.comparer_diff_count() == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.step_comparer_diff(false, cx))),
            )
            .child(
                button("comparer-next-diff", "下一处差异")
                    .icon(IconName::ChevronDown)
                    .disabled(self.comparer_diff_count() == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.step_comparer_diff(true, cx))),
            )
            .child(
                div()
                    .id("comparer-diff-count")
                    .test_support()
                    .text_color(theme::muted_foreground())
                    .child(format!(
                        "{}/{} 处差异",
                        self.extra_tools
                            .comparer_diff_cursor
                            .min(self.comparer_diff_count()),
                        self.comparer_diff_count()
                    )),
            );

        let diff_body = if self.extra_tools.comparer_side_by_side {
            let left_title = self
                .extra_tools
                .comparer_items
                .get(self.extra_tools.left_index)
                .map(|item| format!("左侧 (#A) · {}", item.label))
                .unwrap_or_else(|| "左侧 (#A)".into());
            let right_title = self
                .extra_tools
                .comparer_items
                .get(self.extra_tools.right_index)
                .map(|item| format!("右侧 (#B) · {}", item.label))
                .unwrap_or_else(|| "右侧 (#B)".into());
            let left_pane = div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(theme::surface())
                .child(toolbar().child(left_title))
                .child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&self.extra_tools.comparer_left_view)
                            .readonly(true)
                            .bordered(false)
                            .aria_label("Comparer 左侧 (#A)")
                            .context_menu(comparer_editor_context_menu(
                                self.extra_tools.comparer_left_view.clone(),
                                true,
                            ))
                            .size_full(),
                    ),
                );
            let right_pane = div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(theme::surface())
                .child(toolbar().child(right_title))
                .child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&self.extra_tools.comparer_right_view)
                            .readonly(true)
                            .bordered(false)
                            .aria_label("Comparer 右侧 (#B)")
                            .context_menu(comparer_editor_context_menu(
                                self.extra_tools.comparer_right_view.clone(),
                                true,
                            ))
                            .size_full(),
                    ),
                );
            div()
                .id("comparer-side-by-side")
                .test_support()
                .size_full()
                .min_h_0()
                .child(
                    h_resizable("comparer-side-hsplit")
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(left_pane),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(right_pane),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .min_h_0()
                .bg(theme::surface())
                .child(
                    Editor::new(&self.extra_tools.comparer_view)
                        .readonly(true)
                        .bordered(false)
                        .aria_label("Comparer 对比结果")
                        .context_menu(comparer_editor_context_menu(
                            self.extra_tools.comparer_view.clone(),
                            true,
                        ))
                        .size_full(),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .border_t_1()
            .border_color(border_color)
            .child(diff_toolbar)
            .child(div().flex_1().min_h_0().child(diff_body))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_comparer(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let page_bg: gpui_kit::Hsla = if dark {
            theme::background()
        } else {
            rgb(0xffffff).into()
        };
        let title_color: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        };
        let text_color: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x1e1e1e).into()
        };
        let button_col_w = theme::scaled_px(96.).max(px(88.));
        let has_diff = self.extra_tools.comparer_items.len() >= 2;
        let section_gap = if has_diff { px(10.) } else { px(14.) };

        let header = div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("comparer-heading")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Heading)
                    .aria_label("Comparer")
                    .text_size(theme::scaled_px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(title_color)
                    .child("Comparer"),
            )
            .child(comparer_help_popover(title_color));

        let subtitle = div()
            .mt(px(6.))
            .flex_shrink_0()
            .text_size(theme::ui_font_size())
            .text_color(text_color)
            .child("此功能支持对两份数据进行字词级或字节级对比。你可以加载、粘贴或从其他工具将数据发送到此处，然后选择要执行的对比方式。");

        let top_actions = div()
            .w(button_col_w)
            .ml(px(10.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .justify_start()
            .gap(px(5.))
            .child(
                comparer_action_button(
                    "comparer-paste-button",
                    "粘贴",
                    "从剪贴板粘贴文本并新增为对比项",
                    true,
                    true,
                    false,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    match cx.read_from_clipboard().and_then(|item| item.text()) {
                        Some(text) => {
                            this.add_comparer_item("剪贴板", text.into_bytes(), window, cx)
                        }
                        None => this.fail("剪贴板没有可比较的文本。", cx),
                    }
                })),
            )
            .child(
                comparer_action_button(
                    "comparer-load-file",
                    "加载",
                    "从本地文件读取内容并新增为对比项",
                    !self.file_picker_pending(),
                    true,
                    false,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.load_comparer_from_file(window, cx);
                })),
            )
            .child(
                comparer_action_button(
                    "comparer-remove-selected",
                    "移除",
                    "从对比列表移除当前选中的条目",
                    true,
                    true,
                    false,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.remove_selected_comparer_item(window, cx);
                })),
            )
            .child(
                comparer_action_button(
                    "comparer-clear-button",
                    "清空",
                    "清空全部待对比条目与差异结果",
                    true,
                    true,
                    false,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.extra_tools.comparer_items.clear();
                    this.extra_tools.left_index = 0;
                    this.extra_tools.right_index = 0;
                    this.extra_tools.comparer_selected = 0;
                    this.refresh_comparer(window, cx);
                })),
            );

        let compare_actions = div()
            .w(button_col_w)
            .ml(px(10.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .justify_end()
            .gap(px(5.))
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .mb(px(1.))
                    .text_size(theme::ui_font_size())
                    .text_color(text_color)
                    .child("对比…"),
            )
            .child(
                comparer_action_button(
                    "comparer-mode-words",
                    "字词",
                    "按文本分词 (Words) 粒度对比选中的两份数据",
                    true,
                    has_diff,
                    has_diff && !self.extra_tools.byte_mode,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.extra_tools.byte_mode = false;
                    this.refresh_comparer(window, cx);
                })),
            )
            .child(
                comparer_action_button(
                    "comparer-mode-bytes",
                    "字节",
                    "按十六进制字节 (Bytes) 粒度逐字节对比选中的两份数据",
                    true,
                    has_diff,
                    has_diff && self.extra_tools.byte_mode,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.extra_tools.byte_mode = true;
                    this.refresh_comparer(window, cx);
                })),
            );

        let section_1 = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(theme::ui_font_size())
                    .text_color(text_color)
                    .child("选择条目 1："),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_stretch()
                    .child(self.render_comparer_selection_table(false, cx))
                    .child(top_actions),
            );

        let section_2 = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(theme::ui_font_size())
                    .text_color(text_color)
                    .child("选择条目 2："),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_stretch()
                    .child(self.render_comparer_selection_table(true, cx))
                    .child(compare_actions),
            );

        let selection_view = div()
            .size_full()
            .min_h_0()
            .bg(page_bg)
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .child(header)
            .child(subtitle)
            .child(
                div()
                    .mt(section_gap)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(section_gap)
                    .child(section_1)
                    .child(section_2),
            );

        if has_diff {
            div()
                .size_full()
                .min_h_0()
                .bg(page_bg)
                .child(
                    v_resizable("comparer-main-vsplit")
                        .child(
                            resizable_panel()
                                .size(theme::scaled_px(384.))
                                .size_range(px(332.)..px(640.))
                                .child(selection_view),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(140.)..px(10000.))
                                .child(self.render_comparer_diff_pane(cx)),
                        ),
                )
                .into_any_element()
        } else {
            selection_view.into_any_element()
        }
    }
}
