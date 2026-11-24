use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{
    Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render, Window,
    uniform_list,
};

use super::*;

impl Render for HexGridView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = crate::theme::is_dark();
        let selected_range = self.selected_range.clone();
        let (total_bytes, status_summary) = {
            let shared = self.shared.borrow();
            let total_bytes = shared.wire_bytes.len();
            let summary = if let Some(ref range) = selected_range {
                let len = range.len();
                if len == 1
                    && let Some(&b) = shared.wire_bytes.get(range.start)
                {
                    format!(
                        "Offset: {:08X} · Byte: 0x{b:02X} ({b}) · Total: {total_bytes} B",
                        range.start
                    )
                } else {
                    format!(
                        "Range: {:08X}..{:08X} ({len} B / 0x{len:x}) · Total: {total_bytes} B",
                        range.start, range.end
                    )
                }
            } else {
                format!("Total: {total_bytes} bytes (0x{total_bytes:x})")
            };
            (total_bytes, summary)
        };
        let row_count = total_bytes.div_ceil(16).max(1);
        let readonly = self.readonly;
        let insert_mode = self.insert_mode;
        let can_undo = !readonly && !self.undo_stack.is_empty();
        let can_redo = !readonly && !self.redo_stack.is_empty();
        let has_selection = self.selected_byte_range().is_some()
            || self.active_cursor.is_some_and(|c| c < total_bytes);
        let offset_bg = rgb(if dark { 0x232529 } else { 0xf3f4f6 });
        let header_bg = rgb(if dark { 0x25272b } else { 0xededf0 });
        let border_col = crate::theme::border();
        let mono_font = crate::theme::mono_font_family();
        let hex_entity_id = self.hex_editor.entity_id();

        let toolbar = div()
            .id(("hex-grid-toolbar", hex_entity_id))
            .test_support()
            .w_full()
            .h(px(27.))
            .px(px(8.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(header_bg)
            .border_b_1()
            .border_color(border_col)
            .child(
                div()
                    .font_family(mono_font)
                    .text_size(px(11.5))
                    .text_color(crate::theme::muted_foreground())
                    .truncate()
                    .child(status_summary),
            )
            .child(div().flex_1())
            .when(!readonly, |bar| {
                bar.child(
                    div()
                        .w(px(74.))
                        .h(px(20.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(border_col)
                        .bg(crate::theme::surface())
                        .overflow_hidden()
                        .child(
                            Input::new(&self.byte_input)
                                .id(("hex-grid-byte-input", hex_entity_id))
                                .small()
                                .appearance(false)
                                .h(px(18.))
                                .px(px(4.)),
                        ),
                )
                .child(
                    Button::new(("hex-grid-mode-toggle", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .selected(insert_mode)
                        .label(if insert_mode { "插入" } else { "覆写" })
                        .tooltip("切换键盘直接键入时的覆写/插入字节模式")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_insert_mode(cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-edit-byte", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("修改字节")
                        .tooltip("修改选中字节 (Edit byte)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.edit_selected_byte(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-insert-before", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("前插字节")
                        .tooltip("在选中位置前插入字节 (Insert byte before)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.insert_byte_before(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-append-byte", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("追加字节")
                        .tooltip("在报文末尾追加字节 (Append byte)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.append_byte(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-delete-byte", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("删除字节")
                        .tooltip("删除选中字节 (Delete byte)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.delete_selected_bytes(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-undo", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .disabled(!can_undo)
                        .label("撤销")
                        .tooltip("撤销上次字节修改")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.undo(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-redo", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .disabled(!can_redo)
                        .label("重做")
                        .tooltip("重做上次撤销的字节修改")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.redo(window, cx);
                        })),
                )
            });

        let mut col_header = div()
            .id(("hex-grid-column-header", hex_entity_id))
            .test_support()
            .w_full()
            .h(px(21.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(offset_bg)
            .border_b_1()
            .border_color(border_col)
            .font_family(mono_font)
            .text_size(px(11.5))
            .text_color(crate::theme::muted_foreground())
            .child(
                div()
                    .w(px(68.))
                    .h_full()
                    .px(px(5.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_r_1()
                    .border_color(border_col)
                    .child("Offset"),
            );

        let mut hex_cols_hdr = div()
            .px(px(5.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(1.5))
            .border_r_1()
            .border_color(border_col);
        for col in 0..16usize {
            hex_cols_hdr = hex_cols_hdr.child(
                div()
                    .w(px(18.))
                    .when(col == 7, |el| el.mr(px(4.)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(format!("{col:02X}")),
            );
        }
        col_header = col_header.child(hex_cols_hdr).child(
            div()
                .px(px(6.))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .child("ASCII (0..F)"),
        );

        let weak = cx.weak_entity();
        let list = uniform_list(
            ("hex-grid-rows", hex_entity_id),
            row_count,
            move |range, _, cx| {
                let Some(entity) = weak.upgrade() else {
                    return Vec::new();
                };
                let grid = entity.read(cx);
                let bytes = grid.bytes();
                let total = bytes.len();
                let sel = grid.selected_range.clone();
                let search_hit = grid.search_match_range.clone();
                let hov = grid.hovered_byte;
                let cur = grid.active_cursor;
                let low_nib = grid.low_nibble_pending;
                let ascii_focus = grid.editing_ascii;
                let sel_primary_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x265e9c } else { 0x3b82f6 }).into();
                let sel_sync_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x2d4163 } else { 0xdbeafe }).into();
                let search_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x6b5314 } else { 0xfde047 }).into();
                let hover_sync_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x32363e } else { 0xe5e7eb }).into();

                range
                    .map(|row_ix| {
                        let base_offset = row_ix * 16;
                        let offset_cell = div()
                            .id(("hex-row-offset", row_ix))
                            .test_support()
                            .w(px(68.))
                            .h_full()
                            .px(px(5.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .bg(offset_bg)
                            .border_r_1()
                            .border_color(border_col)
                            .text_color(crate::theme::muted_foreground())
                            .child(format!("{base_offset:08X}"));

                        let mut hex_cells = div()
                            .px(px(5.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(1.5))
                            .border_r_1()
                            .border_color(border_col);

                        let mut ascii_cells =
                            div().px(px(6.)).h_full().flex_shrink_0().flex().items_center();

                        for col in 0..16usize {
                            let byte_ix = base_offset + col;
                            if byte_ix < total {
                                let b = bytes[byte_ix];
                                let is_sel =
                                    sel.as_ref().is_some_and(|r| r.contains(&byte_ix));
                                let is_search =
                                    search_hit.as_ref().is_some_and(|r| r.contains(&byte_ix));
                                let is_hov = hov == Some(byte_ix);
                                let is_cur_nibble =
                                    cur == Some(byte_ix) && low_nib && !ascii_focus;
                                let hex_bg = if is_sel {
                                    if ascii_focus {
                                        sel_sync_bg
                                    } else {
                                        sel_primary_bg
                                    }
                                } else if is_search {
                                    search_bg
                                } else if is_hov {
                                    hover_sync_bg
                                } else {
                                    gpui_kit::transparent_black()
                                };
                                let hex_fg = if is_sel && !ascii_focus {
                                    rgb(0xffffff).into()
                                } else {
                                    crate::theme::foreground()
                                };
                                let ascii_bg = if is_sel {
                                    if ascii_focus {
                                        sel_primary_bg
                                    } else {
                                        sel_sync_bg
                                    }
                                } else if is_search {
                                    search_bg
                                } else if is_hov {
                                    hover_sync_bg
                                } else {
                                    gpui_kit::transparent_black()
                                };
                                let is_printable = (0x20..=0x7E).contains(&b);
                                let ascii_fg = if is_sel && ascii_focus {
                                    rgb(0xffffff).into()
                                } else if is_printable {
                                    crate::theme::foreground()
                                } else {
                                    crate::theme::muted_foreground()
                                };
                                let ascii_ch = if is_printable {
                                    char::from(b).to_string()
                                } else {
                                    ".".to_owned()
                                };

                                let w_down_hex = weak.clone();
                                let w_move_hex = weak.clone();
                                let w_down_ascii = weak.clone();
                                let w_move_ascii = weak.clone();

                                hex_cells = hex_cells.child(
                                    div()
                                        .id(("hex-byte-cell", byte_ix))
                                        .test_support()
                                        .w(px(18.))
                                        .h(px(18.))
                                        .when(col == 7, |el| el.mr(px(4.)))
                                        .rounded(px(2.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(hex_bg)
                                        .text_color(hex_fg)
                                        .when(is_cur_nibble, |el| {
                                            el.border_b_2().border_color(crate::theme::accent())
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |ev: &MouseDownEvent, window, cx| {
                                                let _ = w_down_hex.update(cx, |this, cx| {
                                                    this.dragging = true;
                                                    this.select_byte(
                                                        byte_ix,
                                                        ev.modifiers.shift,
                                                        false,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            },
                                        )
                                        .on_mouse_move(
                                            move |ev: &MouseMoveEvent, window, cx| {
                                                let _ = w_move_hex.update(cx, |this, cx| {
                                                    if ev.pressed_button != Some(MouseButton::Left) {
                                                        this.dragging = false;
                                                    }
                                                    if this.hovered_byte != Some(byte_ix) {
                                                        this.hovered_byte = Some(byte_ix);
                                                        cx.notify();
                                                    }
                                                    if ev.pressed_button == Some(MouseButton::Left)
                                                        && this.dragging
                                                    {
                                                        this.select_byte(
                                                            byte_ix, true, false, window, cx,
                                                        );
                                                    }
                                                });
                                            },
                                        )
                                        .child(format!("{b:02X}")),
                                );

                                ascii_cells = ascii_cells.child(
                                    div()
                                        .id(("hex-ascii-cell", byte_ix))
                                        .test_support()
                                        .w(px(8.5))
                                        .h(px(18.))
                                        .rounded(px(2.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(ascii_bg)
                                        .text_color(ascii_fg)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |ev: &MouseDownEvent, window, cx| {
                                                let _ = w_down_ascii.update(cx, |this, cx| {
                                                    this.dragging = true;
                                                    this.select_byte(
                                                        byte_ix,
                                                        ev.modifiers.shift,
                                                        true,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            },
                                        )
                                        .on_mouse_move(
                                            move |ev: &MouseMoveEvent, window, cx| {
                                                let _ = w_move_ascii.update(cx, |this, cx| {
                                                    if ev.pressed_button != Some(MouseButton::Left) {
                                                        this.dragging = false;
                                                    }
                                                    if this.hovered_byte != Some(byte_ix) {
                                                        this.hovered_byte = Some(byte_ix);
                                                        cx.notify();
                                                    }
                                                    if ev.pressed_button == Some(MouseButton::Left)
                                                        && this.dragging
                                                    {
                                                        this.select_byte(
                                                            byte_ix, true, true, window, cx,
                                                        );
                                                    }
                                                });
                                            },
                                        )
                                        .child(ascii_ch),
                                );
                            } else {
                                hex_cells = hex_cells.child(
                                    div()
                                        .w(px(18.))
                                        .h(px(18.))
                                        .when(col == 7, |el| el.mr(px(4.))),
                                );
                                ascii_cells = ascii_cells.child(div().w(px(8.5)).h(px(18.)));
                            }
                        }

                        div()
                            .id(("hex-grid-row", row_ix))
                            .test_support()
                            .w_full()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .font_family(mono_font)
                            .text_size(crate::theme::editor_font_size())
                            .child(offset_cell)
                            .child(hex_cells)
                            .child(ascii_cells)
                            .into_any_element()
                    })
                    .collect()
            },
        )
        .track_scroll(&self.scroll_handle)
        .size_full();

        let v_scrollbar = gpui_kit::component::scroll::Scrollbar::vertical(&self.scroll_handle)
            .id(("hex-grid-vscroll", hex_entity_id))
            .viewport_from_layout()
            .mode(gpui_kit::component::scroll::ScrollbarMode::Always);

        let caller_menu = self.context_menu.clone();
        let menu_focus = self.focus_handle.clone();
        let grid_body = div()
            .id(("hex-grid-body", hex_entity_id))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .flex()
            .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, window, cx| {
                let mut menu = if let Some(ref builder) = caller_menu {
                    builder(NativeMenu::new(), window, cx)
                } else {
                    NativeMenu::new()
                };
                // Keep HexGridView focused so keyboard & menu actions dispatch to hex handlers.
                menu_focus.focus(window, cx);
                if !menu.is_empty() {
                    menu = menu.separator();
                }
                menu = menu.submenu(
                    "Hex 字节编辑",
                    NativeMenu::new()
                        .menu_with_disabled("复制 Hex", !has_selection, Box::new(HexCopyHex))
                        .menu_with_disabled("复制 ASCII", !has_selection, Box::new(HexCopyAscii))
                        .menu_with_disabled("粘贴字节", readonly, Box::new(HexPasteBytes))
                        .separator()
                        .menu_with_disabled(
                            "修改选中字节",
                            readonly || total_bytes == 0,
                            Box::new(HexEditSelectedByte),
                        )
                        .menu_with_disabled("前插字节", readonly, Box::new(HexInsertBefore))
                        .menu_with_disabled("追加字节", readonly, Box::new(HexAppendByte))
                        .menu_with_disabled(
                            "删除选中字节",
                            readonly || total_bytes == 0,
                            Box::new(HexDeleteSelectedBytes),
                        )
                        .separator()
                        .menu_with_disabled("撤销字节修改", !can_undo, Box::new(HexUndo))
                        .menu_with_disabled("重做字节修改", !can_redo, Box::new(HexRedo))
                        .menu_with_check(
                            "插入模式（否则为覆写）",
                            insert_mode,
                            Box::new(HexToggleInsertMode),
                        ),
                );
                menu.show(ev.position, window, cx);
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(list),
            )
            .child(
                div()
                    .relative()
                    .w(px(12.))
                    .h_full()
                    .flex_shrink_0()
                    .bg(offset_bg)
                    .border_l_1()
                    .border_color(border_col)
                    .child(v_scrollbar),
            );

        div()
            .id(("hex-grid-view", hex_entity_id))
            .test_support()
            .key_context("HexGrid")
            .track_focus(&self.focus_handle)
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(crate::theme::surface())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.dragging = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.dragging = false;
                }),
            )
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_action(cx.listener(|this, _: &HexCopyHex, _, cx| {
                this.copy_selection(Some(false), cx);
            }))
            .on_action(cx.listener(|this, _: &HexCopyAscii, _, cx| {
                this.copy_selection(Some(true), cx);
            }))
            .on_action(cx.listener(|this, _: &HexPasteBytes, window, cx| {
                this.paste_clipboard(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexEditSelectedByte, window, cx| {
                this.edit_selected_byte(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexInsertBefore, window, cx| {
                this.insert_byte_before(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexAppendByte, window, cx| {
                this.append_byte(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexDeleteSelectedBytes, window, cx| {
                this.delete_selected_bytes(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexUndo, window, cx| {
                this.undo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexRedo, window, cx| {
                this.redo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexToggleInsertMode, _, cx| {
                this.toggle_insert_mode(cx);
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                this.copy_selection(None, cx);
            }))
            .on_action(cx.listener(|this, _: &Cut, window, cx| {
                this.cut_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Paste, window, cx| {
                this.paste_clipboard(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                this.undo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Redo, window, cx| {
                this.redo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.select_all(cx);
            }))
            .child(toolbar)
            .child(col_header)
            .child(grid_body)
    }
}
