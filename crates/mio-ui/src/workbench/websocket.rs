use std::ops::Range;

use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
    table::{Column, ColumnSort, TableDelegate},
};
use mio_core::{
    Uuid, WebSocketDirection, WebSocketHistoryFilter, WebSocketMessage, WebSocketSummary,
};

use super::*;

mod delegate;
mod empty_state;
mod filter;
mod notes;
mod selection;
mod transfer;
mod transfer_diff;

pub(super) const WS_HISTORY_BATCH_SIZE: usize = 5_000;

pub(super) struct WebSocketHistory {
    pub(in crate::workbench) owner: WeakEntity<Workbench>,
    pub(super) table: Entity<TableState<WebSocketDelegate>>,
    pub(super) all_columns: Vec<Column>,
    pub(super) search_input: Entity<InputState>,
    pub(super) applied_search: String,
    pub(super) query_search: String,
    pub(super) _search_subscription: Subscription,
    pub(super) filter: Option<FlowId>,
    pub(super) applied_filter: WebSocketHistoryFilter,
    pub(super) in_scope_only: bool,
    pub(super) filter_enabled: bool,
    pub(super) query_filter: WebSocketHistoryFilter,
    pub(super) query_in_scope_only: bool,
    pub(super) query_filter_enabled: bool,
    pub(super) filter_window: Option<gpui_kit::AnyWindowHandle>,
    pub(super) filter_outcome: Option<(u64, Result<(), String>)>,
    pub(super) selection_filter_pending: bool,
    pub(super) dirty: bool,
    pub(super) query_revision: u64,
    pub(super) query_offset: usize,
    pub(super) selection_revision: u64,
    pub(super) selected: Option<Uuid>,
    pub(super) selected_summary: Option<WebSocketSummary>,
    pub(super) pending_row: Option<usize>,
    pub(super) pending_transfer: Option<(Uuid, transfer::PendingWsTransfer)>,
    pub(super) pending_copy: Option<(Uuid, bool)>,
    pub(super) detail: Option<Box<WebSocketMessage>>,
    pub(super) detail_error: Option<String>,
    pub(super) payload: MessageEditor,
    pub(super) original_payload: Option<Vec<u8>>,
    pub(super) original_view: bool,
    pub(super) notes: notes::Notes,
}

impl WebSocketHistory {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let owner = cx.weak_entity();
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let search_subscription =
            cx.subscribe_in(&search_input, window, |view, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    view.apply_websocket_search(window, cx);
                }
            });
        Self {
            owner: owner.clone(),
            table: cx.new(|cx| {
                TableState::new(WebSocketDelegate::new(owner), window, cx)
                    .sortable(true)
                    .col_selectable(false)
                    .select_on_modifier_click(false)
            }),
            all_columns: WebSocketDelegate::default_columns(),
            search_input,
            applied_search: String::new(),
            query_search: String::new(),
            _search_subscription: search_subscription,
            filter: None,
            applied_filter: WebSocketHistoryFilter::default(),
            in_scope_only: false,
            filter_enabled: true,
            query_filter: WebSocketHistoryFilter::default(),
            query_in_scope_only: false,
            query_filter_enabled: true,
            filter_window: None,
            filter_outcome: None,
            selection_filter_pending: false,
            dirty: true,
            query_revision: 0,
            query_offset: 0,
            selection_revision: 0,
            selected: None,
            selected_summary: None,
            pending_row: None,
            pending_transfer: None,
            pending_copy: None,
            detail: None,
            detail_error: None,
            payload: MessageEditor::new(window, cx),
            original_payload: None,
            original_view: false,
            notes: notes::Notes::new(window, cx),
        }
    }

    pub(in crate::workbench) fn reset(&mut self, window: &mut Window, cx: &mut App) {
        self.filter = None;
        self.applied_search.clear();
        self.query_search.clear();
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.notes.reset(window, cx);
        self.applied_filter = WebSocketHistoryFilter::default();
        self.in_scope_only = false;
        self.filter_enabled = true;
        self.query_filter = WebSocketHistoryFilter::default();
        self.query_in_scope_only = false;
        self.query_filter_enabled = true;
        self.filter_outcome = None;
        self.selection_filter_pending = false;
        self.dirty = true;
        self.query_revision = 0;
        self.query_offset = 0;
        self.selection_revision = 0;
        self.clear_detail(window, cx);
        self.table.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            delegate.rows.clear();
            delegate.highlights.clear();
            delegate.deleted_ids.clear();
            delegate.offset = 0;
            delegate.total = 0;
            delegate.loading = false;
            delegate.error = None;
            delegate.visible = 0..0;
            delegate.reset_sort();
            state.clear_selection(cx);
            state.scroll_to_row(0, cx);
            state.refresh(cx);
        });
    }

    pub(in crate::workbench) fn clear_detail(&mut self, window: &mut Window, cx: &mut App) {
        self.selected = None;
        self.selected_summary = None;
        self.pending_row = None;
        self.pending_transfer = None;
        self.pending_copy = None;
        self.selection_filter_pending = false;
        self.detail = None;
        self.detail_error = None;
        self.original_payload = None;
        self.original_view = false;
        self.payload.load(&[], window, cx);
    }

    pub(in crate::workbench) fn backend_disconnected(&mut self, cx: &mut Context<Workbench>) {
        self.notes.disconnect();
        self.dirty = false;
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if delegate.loading {
                delegate.loading = false;
                delegate.error = Some("后台服务已断开，查询结果未确认。".into());
                table.refresh(cx);
            }
        });
        if (self.selected.is_some() || self.pending_row.is_some()) && self.detail.is_none() {
            self.detail_error = Some("后台服务已断开，正文读取未确认。".into());
        }
        cx.notify();
    }
}

pub(super) struct WebSocketDelegate {
    pub(super) owner: WeakEntity<Workbench>,
    pub(super) rows: Vec<WebSocketSummary>,
    pub(super) highlights: HashMap<Uuid, u8>,
    pub(super) deleted_ids: std::collections::HashSet<Uuid>,
    pub(super) offset: usize,
    pub(super) total: usize,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) visible: Range<usize>,
    pub(super) columns: Vec<Column>,
}

impl Workbench {
    fn show_websocket_version(
        &mut self,
        original: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.websocket.detail.as_deref() else {
            return;
        };
        let show_original = original && self.websocket.original_payload.is_some();
        self.websocket.original_view = show_original;
        let bytes = match (show_original, self.websocket.original_payload.as_deref()) {
            (true, Some(original)) => original,
            _ => &record.payload,
        };
        self.websocket.payload.load(bytes, window, cx);
        cx.notify();
    }

    fn should_show_websocket_empty_state(&self, cx: &App) -> bool {
        let delegate = self.websocket.table.read(cx).delegate();
        delegate.total == 0
            && delegate.rows.is_empty()
            && !delegate.loading
            && delegate.error.is_none()
            && self.websocket.selected.is_none()
            && self.websocket.pending_row.is_none()
            && self.websocket.filter.is_none()
            && self.websocket.filter_window.is_none()
            && self.websocket.filter_enabled
            && self.websocket.query_filter_enabled
            && !self.websocket.in_scope_only
            && !self.websocket.query_in_scope_only
            && self.websocket.applied_filter == WebSocketHistoryFilter::default()
            && self.websocket.query_filter == WebSocketHistoryFilter::default()
            && self.websocket.applied_search.is_empty()
            && self.websocket.query_search.is_empty()
    }

    fn render_websocket_message_header(
        &self,
        can_send_to_repeater: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const SLOT: crate::workbench::wb_editors::EditorSlot =
            crate::workbench::wb_editors::EditorSlot::WebsocketPayload;
        let dark = theme::is_dark();
        let pretty = self.is_message_pretty(SLOT);
        let non_printable = self.is_message_non_printable(SLOT);
        let soft_wrap = self.is_message_soft_wrap(SLOT);
        let view = self.websocket.payload.view();
        let raw_selected = !pretty && view == MessageView::Raw;
        let hex_selected = !pretty && view == MessageView::Hex;
        let title_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        };
        let has_original =
            self.websocket.detail.is_some() && self.websocket.original_payload.is_some();
        let original_view = self.websocket.original_view;
        let menu_owner = cx.weak_entity();

        let view_tab = |id: &'static str, label: &'static str, width: f32, selected: bool| {
            Button::new(id)
                .ghost()
                .small()
                .rounded(ButtonRounded::None)
                .relative()
                .self_start()
                .w(px(width))
                .h(px(37.))
                .p_0()
                .bg(theme::surface())
                .text_color(if selected {
                    title_fg
                } else if dark {
                    theme::muted_foreground()
                } else {
                    rgb(0x1f1f1f).into()
                })
                .selected(selected)
                .toggled(selected)
                .accessibility_label(label)
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(if selected {
                            gpui_kit::FontWeight::BOLD
                        } else {
                            gpui_kit::FontWeight::NORMAL
                        })
                        .child(label),
                )
                .when(selected, |tab| {
                    tab.child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(3.))
                            .bg(rgb(0xdb612f)),
                    )
                })
        };

        let version_toggle = |id: &'static str, label: &'static str, selected: bool, tip: &'static str| {
            let active_bg: gpui_kit::Hsla = rgb(0x1558b0).into();
            let inactive_bg: gpui_kit::Hsla = if dark {
                rgb(0x35383d).into()
            } else {
                rgb(0xeeeeee).into()
            };
            Button::new(id)
                .ghost()
                .small()
                .compact()
                .rounded(ButtonRounded::None)
                .h(px(21.))
                .px(px(8.))
                .py_0()
                .bg(if selected { active_bg } else { inactive_bg })
                .text_color(if selected {
                    rgb(0xffffff).into()
                } else {
                    theme::foreground()
                })
                .selected(selected)
                .toggled(selected)
                .accessibility_label(label)
                .tooltip(tip)
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(if selected {
                            gpui_kit::FontWeight::SEMIBOLD
                        } else {
                            gpui_kit::FontWeight::NORMAL
                        })
                        .child(label),
                )
        };

        div()
            .w_full()
            .h(px(77.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                div()
                    .w_full()
                    .h(px(39.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .pl(px(15.))
                    .pr(px(10.))
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(title_fg)
                            .child("Message"),
                    )
                    .when(has_original, |row| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .rounded(px(3.))
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme::border())
                                .child(
                                    version_toggle(
                                        "websocket-original",
                                        "原始消息",
                                        original_view,
                                        "查看未经自动替换或手动拦截修改的原始 WebSocket 载荷",
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_websocket_version(true, window, cx);
                                    })),
                                )
                                .child(
                                    version_toggle(
                                        "websocket-final",
                                        "最终消息",
                                        !original_view,
                                        "查看实际向对端发送或转发的最终 WebSocket 载荷",
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_websocket_version(false, window, cx);
                                    })),
                                ),
                        )
                    })
                    .child(
                        div().ml_auto().flex().items_center().gap(px(4.)).child(
                            Button::new("websocket-to-repeater")
                                .ghost()
                                .small()
                                .compact()
                                .rounded(ButtonRounded::Size(px(3.)))
                                .w(px(24.))
                                .h(px(24.))
                                .p_0()
                                .icon(
                                    gpui_kit::component::Icon::new(IconName::Repeat)
                                        .size(px(14.))
                                        .text_color(theme::foreground()),
                                )
                                .accessibility_label("发送到 Repeater")
                                .tooltip("将所选 WebSocket 消息及其连接上下文复制到 WebSocket Repeater")
                                .disabled(!can_send_to_repeater)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let Some(message_id) = this.websocket.selected else {
                                        return;
                                    };
                                    let use_original = this.websocket.detail.is_some()
                                        && this.websocket.original_view;
                                    this.send_websocket_to_repeater(
                                        message_id,
                                        use_original,
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(38.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        view_tab("websocket-payload-pretty", "Pretty", 64., pretty)
                            .tooltip("格式化预览视图（自动美化 JSON / XML / HTML / 表单，只读）")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_pretty_view(SLOT, window, cx)
                            })),
                    )
                    .child(
                        view_tab("websocket-payload-raw", "Raw", 56., raw_selected)
                            .tooltip("原始字节流文本视图（按字节展示完整载荷）")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_message_view(SLOT, MessageView::Raw, window, cx)
                            })),
                    )
                    .child(
                        view_tab("websocket-payload-hex", "Hex", 53., hex_selected)
                            .tooltip("十六进制字节视图（按精确字节偏移查看载荷）")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_message_view(SLOT, MessageView::Hex, window, cx)
                            })),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .pr(px(10.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                Button::new("websocket-payload-wrap")
                                    .ghost()
                                    .small()
                                    .rounded(ButtonRounded::None)
                                    .w(px(19.))
                                    .h(px(19.))
                                    .p_0()
                                    .bg(if soft_wrap {
                                        rgb(0x26649d).into()
                                    } else {
                                        gpui_kit::transparent_black()
                                    })
                                    .text_color(if soft_wrap {
                                        rgb(0xffffff).into()
                                    } else {
                                        theme::foreground()
                                    })
                                    .icon(
                                        gpui_kit::component::Icon::new(IconName::TextWrap)
                                            .size(px(15.)),
                                    )
                                    .accessibility_label("自动换行")
                                    .selected(soft_wrap)
                                    .toggled(soft_wrap)
                                    .tooltip(if soft_wrap {
                                        "自动换行已开启（点击切换为单行水平滚动）"
                                    } else {
                                        "自动换行已关闭（点击开启长行自动折行显示）"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_message_soft_wrap(SLOT, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("websocket-payload-nonprintable")
                                    .ghost()
                                    .small()
                                    .rounded(ButtonRounded::None)
                                    .w(px(19.))
                                    .h(px(19.))
                                    .p_0()
                                    .bg(if non_printable {
                                        rgb(0x26649d).into()
                                    } else {
                                        gpui_kit::transparent_black()
                                    })
                                    .text_color(if non_printable {
                                        rgb(0xffffff).into()
                                    } else {
                                        theme::foreground()
                                    })
                                    .child({
                                        let icon_color: gpui_kit::Hsla = if non_printable {
                                            rgb(0xffffff).into()
                                        } else {
                                            theme::foreground()
                                        };
                                        gpui_kit::canvas(
                                            |_, _, _| {},
                                            move |b, _, window, _| {
                                                let mut builder =
                                                    gpui_kit::PathBuilder::stroke(px(1.5));
                                                builder.move_to(gpui_kit::point(
                                                    b.left() + px(2.0),
                                                    b.top() + px(2.5),
                                                ));
                                                builder.line_to(gpui_kit::point(
                                                    b.left() + px(6.0),
                                                    b.top() + px(11.5),
                                                ));
                                                builder.move_to(gpui_kit::point(
                                                    b.left() + px(7.8),
                                                    b.top() + px(5.2),
                                                ));
                                                builder.line_to(gpui_kit::point(
                                                    b.left() + px(7.8),
                                                    b.top() + px(11.5),
                                                ));
                                                builder.move_to(gpui_kit::point(
                                                    b.left() + px(7.8),
                                                    b.top() + px(7.0),
                                                ));
                                                builder.curve_to(
                                                    gpui_kit::point(
                                                        b.left() + px(11.8),
                                                        b.top() + px(7.0),
                                                    ),
                                                    gpui_kit::point(
                                                        b.left() + px(9.8),
                                                        b.top() + px(4.6),
                                                    ),
                                                );
                                                builder.line_to(gpui_kit::point(
                                                    b.left() + px(11.8),
                                                    b.top() + px(11.5),
                                                ));
                                                if let Ok(path) = builder.build() {
                                                    window.paint_path(path, icon_color);
                                                }
                                            },
                                        )
                                        .w(px(14.))
                                        .h(px(14.))
                                    })
                                    .accessibility_label("显示非打印字符")
                                    .selected(non_printable)
                                    .toggled(non_printable)
                                    .tooltip(if non_printable {
                                        "显示非打印字符已开启（可视化呈现 \\r\\n、制表符与控制字符，点击恢复原始视图）"
                                    } else {
                                        "显示非打印字符（Show non-printable characters：可视化呈现 \\r\\n、制表符与控制字符）"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_message_non_printable(SLOT, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("websocket-payload-message-actions")
                                    .ghost()
                                    .small()
                                    .rounded(ButtonRounded::None)
                                    .w(px(17.))
                                    .h(px(17.))
                                    .p_0()
                                    .icon(
                                        gpui_kit::component::Icon::default()
                                            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\"><path d=\"M2.5 4h11M2.5 8h11M2.5 12h11\"/></svg>")
                                            .size(px(14.))
                                            .text_color(theme::foreground()),
                                    )
                                    .accessibility_label("Message actions")
                                    .tooltip("Message actions")
                                    .dropdown_menu(move |menu, _, cx| {
                                        let Some(entity) = menu_owner.upgrade() else {
                                            return menu;
                                        };
                                        let view = entity.read(cx);
                                        let enabled = view.project.is_some()
                                            && view.backend_available()
                                            && view.transition.is_none()
                                            && view.websocket.selected.is_some();
                                        let has_url = enabled
                                            && view
                                                .websocket
                                                .selected_summary
                                                .as_ref()
                                                .is_some_and(|row| !row.url.is_empty());
                                        let notes_open = view.websocket.selected.is_some()
                                            && view.websocket.notes.open;
                                        let o_rep = menu_owner.clone();
                                        let o_dec = menu_owner.clone();
                                        let o_cmp = menu_owner.clone();
                                        let o_notes = menu_owner.clone();
                                        let o_url = menu_owner.clone();
                                        let o_text = menu_owner.clone();
                                        let o_hex = menu_owner.clone();
                                        let o_in = menu_owner.clone();
                                        let o_ex = menu_owner.clone();
                                        menu.item(
                                            PopupMenuItem::new("发送到 Repeater")
                                                .action(Box::new(OpenInRepeater))
                                                .disabled(!enabled)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_rep.update(cx, |this, cx| {
                                                        this.send_selected_websocket_to_repeater(
                                                            window, cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new("发送到 Decoder")
                                                .action(Box::new(crate::SendToDecoder))
                                                .disabled(!enabled)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_dec.update(cx, |this, cx| {
                                                        this.send_websocket_to_decoder(
                                                            None, window, cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new("发送到 Comparer")
                                                .action(Box::new(crate::SendToComparerRequest))
                                                .disabled(!enabled)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_cmp.update(cx, |this, cx| {
                                                        this.send_websocket_to_comparer(
                                                            None, window, cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("编辑备注 (Notes)")
                                                .checked(notes_open)
                                                .disabled(!enabled)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_notes.update(cx, |this, cx| {
                                                        if this.websocket.notes.open {
                                                            if this.leave_websocket_notes(
                                                                window, cx,
                                                            ) {
                                                                this.websocket.notes.open = false;
                                                                cx.notify();
                                                            }
                                                        } else {
                                                            this.open_websocket_notes_for(
                                                                this.websocket.selected,
                                                                window,
                                                                cx,
                                                            );
                                                        }
                                                    });
                                                }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("复制 URL")
                                                .action(Box::new(CopyUrl))
                                                .disabled(!has_url)
                                                .on_click(move |_, _, cx| {
                                                    let _ = o_url.update(cx, |this, cx| {
                                                        this.copy_websocket_url(cx);
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new("复制载荷文本")
                                                .disabled(!enabled)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_text.update(cx, |this, cx| {
                                                        this.copy_websocket_payload(
                                                            this.websocket.selected,
                                                            false,
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new("复制载荷 Hex")
                                                .disabled(!enabled)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_hex.update(cx, |this, cx| {
                                                        this.copy_websocket_payload(
                                                            this.websocket.selected,
                                                            true,
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("添加到范围")
                                                .action(Box::new(crate::AddToScope))
                                                .disabled(!has_url)
                                                .on_click(move |_, _, cx| {
                                                    let _ = o_in.update(cx, |this, cx| {
                                                        this.modify_websocket_scope(true, cx);
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new("从范围排除")
                                                .action(Box::new(crate::ExcludeFromScope))
                                                .disabled(!has_url)
                                                .on_click(move |_, _, cx| {
                                                    let _ = o_ex.update(cx, |this, cx| {
                                                        this.modify_websocket_scope(false, cx);
                                                    });
                                                }),
                                        )
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn render_websocket_history(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.should_show_websocket_empty_state(cx) {
            return self.render_websocket_empty_state(cx);
        }
        let error = self.websocket.table.read(cx).delegate().error.clone();
        let flow_id = self.websocket.filter;
        let filter = self.websocket.query_filter.clone();
        let in_scope_only = self.websocket.query_in_scope_only;
        let enabled = self.websocket.query_filter_enabled;
        let retry_offset = self.websocket.query_offset;
        let can_send_to_repeater = self.project.is_some()
            && self.backend_available()
            && self.transition.is_none()
            && self.websocket.selected.is_some();
        let detail_body = if self.websocket.detail.is_some() {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div().flex_1().min_h_0().child(
                        editing::message_editor(&self.websocket.payload)
                            .readonly(true)
                            .context_menu(
                                self.websocket_payload_context_menu(
                                    self.websocket.payload.state(),
                                    true,
                                ),
                            )
                            .size_full()
                            .aria_label("WebSocket 消息正文"),
                    ),
                )
                .child(self.websocket.payload.render_search_bar(
                    "websocket-payload",
                    cx,
                    |this, action, window, cx| {
                        this.handle_message_search_action(
                            crate::workbench::wb_editors::EditorSlot::WebsocketPayload,
                            action,
                            window,
                            cx,
                        );
                    },
                ))
                .into_any_element()
        } else {
            div()
                .size_full()
                .bg(theme::surface())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .text_color(theme::muted_foreground())
                .child(self.websocket.detail_error.as_ref().map_or_else(
                    || {
                        if self.websocket.selected.is_some() || self.websocket.pending_row.is_some()
                        {
                            "正在读取 WebSocket 消息…".into()
                        } else {
                            "选择一条 WebSocket 消息查看正文".into()
                        }
                    },
                    |error| format!("无法读取消息：{error}"),
                ))
                .when(
                    self.websocket.detail_error.is_some() && self.websocket.selected.is_some(),
                    |this| {
                        this.child(
                            button("websocket-detail-retry", "重试读取")
                                .icon(IconName::RefreshCw)
                                .tooltip("从项目存储重新读取当前所选 WebSocket 帧的载荷正文")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(id) = this.websocket.selected {
                                        this.select_websocket(id, window, cx);
                                    }
                                })),
                        )
                    },
                )
                .into_any_element()
        };
        let details = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(self.render_websocket_message_header(can_send_to_repeater, cx))
            .child(div().flex_1().min_h_0().child(detail_body))
            .into_any_element();
        let history = div()
            .id("websocket-history-table")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .when_some(error, |this, error| {
                this.child(
                    toolbar()
                        .text_color(theme::danger_fg())
                        .child(format!("查询失败，已读取内容仍保留：{error}"))
                        .child(
                            button("websocket-refresh", "重试查询")
                                .icon(IconName::RefreshCw)
                                .tooltip("按当前筛选条件重新查询 WebSocket 历史记录")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.query_websocket_filtered_with_scope(
                                        flow_id,
                                        retry_offset,
                                        filter.clone(),
                                        in_scope_only,
                                        enabled,
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.websocket.table)
                        .with_size(theme::row_height())
                        .header_row_height(theme::table_header_height())
                        .stripe(theme::table_zebra_stripe())
                        .scrollbar_mode(gpui_kit::component::scroll::ScrollbarMode::Always)
                        .bordered(false)
                        .scroll_bounce(false)
                        .accessibility_label("WebSocket 历史记录"),
                ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &OpenInRepeater, window, cx| {
                this.send_selected_websocket_to_repeater(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::SendToDecoder, window, cx| {
                this.send_websocket_to_decoder(None, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::SendToComparerRequest, window, cx| {
                    this.send_websocket_to_comparer(None, window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &CopyUrl, _, cx| {
                this.copy_websocket_url(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::AddToScope, _, cx| {
                this.modify_websocket_scope(true, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ExcludeFromScope, _, cx| {
                this.modify_websocket_scope(false, cx);
            }))
            .child(self.render_websocket_filter_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        v_resizable("websocket-history-split")
                            .child(
                                resizable_panel()
                                    .size(px(265.))
                                    .size_range(px(110.)..px(900.))
                                    .child(history),
                            )
                            .child(
                                resizable_panel()
                                    .size_range(px(180.)..px(10000.))
                                    .child(self.render_websocket_notes(details, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::workbench::websocket) fn open_websocket_notes_for(
        &mut self,
        target_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let Some(message_id) = target_id.or(self.websocket.selected) else {
            self.fail("请先选择一条 WebSocket 消息。", cx);
            return;
        };
        if self.websocket.selected != Some(message_id) {
            self.select_websocket(message_id, window, cx);
        }
        if self.websocket.selected != Some(message_id) {
            return;
        }
        if !self.dismiss_inspector_for_current_selection(window, cx) {
            return;
        }
        self.inspector.open = false;
        self.websocket.notes.open = true;
        self.select_websocket_notes(window, cx);
        if self.websocket.notes.stored.is_some() {
            self.websocket
                .notes
                .editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn copy_websocket_payload(
        &mut self,
        target_id: Option<Uuid>,
        as_hex: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let Some(message_id) = target_id.or(self.websocket.selected) else {
            self.fail("请先选择一条 WebSocket 消息。", cx);
            return;
        };
        if self.websocket.selected == Some(message_id)
            && let Some(detail) = self
                .websocket
                .detail
                .as_deref()
                .filter(|record| record.id == message_id)
        {
            let bytes = if self.websocket.original_view
                && let Some(original) = &self.websocket.original_payload
            {
                original.as_slice()
            } else {
                detail.payload.as_slice()
            };
            let text = if as_hex {
                editing::hex_encode(bytes)
            } else {
                String::from_utf8_lossy(bytes).into_owned()
            };
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
            self.error = None;
            self.status = if as_hex {
                "已复制 WebSocket 载荷 Hex 到剪贴板。".into()
            } else {
                "已复制 WebSocket 载荷文本到剪贴板。".into()
            };
            cx.notify();
            return;
        }
        self.select_websocket(message_id, window, cx);
        if self.websocket.selected == Some(message_id) {
            self.websocket.pending_copy = Some((message_id, as_hex));
        }
    }

    pub(in crate::workbench::websocket) fn modify_websocket_url_scope(
        &mut self,
        url: &str,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let Some(prefix) = websocket_url_to_http_prefix(url) else {
            self.fail("WebSocket URL 格式无效，无法更新范围规则。", cx);
            return;
        };
        if self.workspace.scope.advanced_mode {
            self.add_advanced_scope_rule_from_prefix(prefix, include, cx);
        } else {
            self.add_scope_rule(self.project, include, prefix, cx);
        }
    }

    pub(in crate::workbench::websocket) fn set_websocket_highlight(
        &mut self,
        message_id: Uuid,
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || color > 9 {
            return;
        }
        self.websocket.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if color == 0 {
                delegate.highlights.remove(&message_id);
            } else {
                delegate.highlights.insert(message_id, color);
            }
            table.refresh(cx);
        });
        self.error = None;
        self.status = if color == 0 {
            "已清除 WebSocket 消息高亮。".into()
        } else {
            "已更新 WebSocket 消息高亮颜色。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn send_websocket_to_organiser(
        &mut self,
        target_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        let Some(message_id) = target_id.or(self.websocket.selected) else {
            self.fail("请先选择一条 WebSocket 消息。", cx);
            return;
        };
        if self.websocket.selected == Some(message_id)
            && let Some(detail) = self
                .websocket
                .detail
                .as_deref()
                .filter(|record| record.id == message_id)
        {
            let payload = if self.websocket.original_view
                && let Some(original) = &self.websocket.original_payload
            {
                original.clone()
            } else {
                detail.payload.clone()
            };
            let direction = detail.direction;
            let kind = detail.kind;
            let observed = detail.observed_unix_ms;
            self.commit_websocket_organiser_item(
                message_id, direction, kind, observed, &payload, window, cx,
            );
            return;
        }
        self.select_websocket(message_id, window, cx);
        if self.websocket.selected != Some(message_id) {
            return;
        }
        let project = self.project;
        let owner_window = self.owner_window;
        let self_entity = cx.entity();
        let sub_slot: std::rc::Rc<std::cell::RefCell<Option<Subscription>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let sub_obs = sub_slot.clone();
        let sub_timeout = sub_slot.clone();
        let observer = cx.observe(&self_entity, move |view, entity, cx| {
            if view.project != project
                || !view.backend_available()
                || view.websocket.selected != Some(message_id)
                || view.websocket.detail_error.is_some()
            {
                *sub_obs.borrow_mut() = None;
                return;
            }
            if let Some(detail) = view
                .websocket
                .detail
                .as_deref()
                .filter(|record| record.id == message_id)
            {
                let payload = detail.payload.clone();
                let direction = detail.direction;
                let kind = detail.kind;
                let observed = detail.observed_unix_ms;
                *sub_obs.borrow_mut() = None;
                let weak = entity.downgrade();
                cx.defer(move |cx| {
                    let _ = owner_window.update(cx, |_, window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.commit_websocket_organiser_item(
                                message_id, direction, kind, observed, &payload, window, cx,
                            );
                        });
                    });
                });
            }
        });
        *sub_slot.borrow_mut() = Some(observer);
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_secs(15));
        cx.spawn(async move |_, cx| {
            timer.await;
            let _ = cx.update(|_| {
                *sub_timeout.borrow_mut() = None;
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_websocket_organiser_item(
        &mut self,
        message_id: Uuid,
        direction: WebSocketDirection,
        kind: mio_core::WebSocketKind,
        observed_unix_ms: i64,
        payload: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() || !self.flush_workspace(window, cx) {
            return;
        }
        let number = match mio_core::next_organiser_number(&self.workspace.organiser_items) {
            Ok(n) => n,
            Err(err) => {
                self.fail(err.to_string(), cx);
                return;
            }
        };
        let summary = self
            .websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .find(|r| r.id == message_id)
            .cloned()
            .or_else(|| {
                self.websocket
                    .selected_summary
                    .clone()
                    .filter(|r| r.id == message_id)
            });
        let raw_url = summary
            .as_ref()
            .map(|r| r.url.clone())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "ws://localhost/".to_owned());
        let target = websocket_url_to_http_prefix(&raw_url).unwrap_or_default();
        let host = raw_url
            .split_once("://")
            .map(|(_, tail)| tail.split(['/', '?', '#']).next().unwrap_or("localhost"))
            .unwrap_or("localhost");
        let path = raw_url
            .split_once("://")
            .and_then(|(_, tail)| tail.find('/').map(|idx| &tail[idx..]))
            .unwrap_or("/");
        let kind_label = match kind {
            mio_core::WebSocketKind::Text => "Text",
            mio_core::WebSocketKind::Binary => "Binary",
            _ => "Message",
        };
        let mut request_raw = format!(
            "WS {path} HTTP/1.1\r\nHost: {host}\r\nX-WebSocket-Direction: {}\r\nX-WebSocket-Kind: {kind_label}\r\n\r\n",
            direction_label(direction)
        )
        .into_bytes();
        let max_payload = (4 * 1024 * 1024usize).saturating_sub(request_raw.len());
        request_raw.extend_from_slice(&payload[..payload.len().min(max_payload)]);
        let highlight = self
            .websocket
            .table
            .read(cx)
            .delegate()
            .highlights
            .get(&message_id)
            .copied()
            .filter(|c| (1..=9).contains(c));
        let notes = summary
            .as_ref()
            .map(|r| r.notes_preview.clone())
            .unwrap_or_default();
        let item = mio_core::OrganiserItem {
            id: Uuid::new_v4(),
            number,
            added_unix_ms: observed_unix_ms.max(0),
            source_tool: "Proxy".into(),
            target,
            method: "WS".into(),
            url: raw_url,
            status_code: None,
            response_len: 0,
            request_raw,
            response_raw: None,
            http_version: HttpVersion::Http11,
            workflow_status: mio_core::OrganiserStatus::Unknown,
            notes,
            highlight,
        };
        let item_id = item.id;
        self.workspace.organiser_items.push(item);
        if let Err(err) = mio_core::validate_organiser_items(&self.workspace.organiser_items)
            .and_then(|()| self.workspace.validate_structure())
        {
            self.workspace.organiser_items.pop();
            self.fail(format!("未添加到 Organizer：{err}"), cx);
            return;
        }
        if self.organiser.selected_id.is_none() {
            self.organiser.selected_id = Some(item_id);
            self.sync_organiser_table(window, cx);
            self.reload_selected_organiser_detail(window, cx);
        } else {
            self.sync_organiser_table(window, cx);
        }
        self.save_workspace(cx);
        self.error = None;
        self.status = format!("已将 WebSocket 消息添加到 Organizer (#{number})。");
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn delete_websocket_history_entry(
        &mut self,
        message_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        if self.websocket.selected == Some(message_id) {
            if !self.leave_websocket_notes(window, cx) {
                return;
            }
            self.clear_websocket_detail(window, cx);
        }
        self.websocket.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.deleted_ids.insert(message_id);
            let before = delegate.rows.len();
            delegate.rows.retain(|row| row.id != message_id);
            delegate.highlights.remove(&message_id);
            let removed = before.saturating_sub(delegate.rows.len());
            delegate.total = delegate.total.saturating_sub(removed);
            table.clear_selection(cx);
            table.refresh(cx);
        });
        self.error = None;
        self.status = "已删除选中的 WebSocket 历史条目。".into();
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn clear_websocket_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        if !self.leave_websocket_notes(window, cx) {
            return;
        }
        self.clear_websocket_detail(window, cx);
        self.websocket.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.rows.clear();
            delegate.highlights.clear();
            delegate.offset = 0;
            delegate.total = 0;
            delegate.visible = 0..0;
            table.clear_selection(cx);
            table.scroll_to_row(0, cx);
            table.refresh(cx);
        });
        self.error = None;
        self.status = "已清空 WebSocket 历史记录。".into();
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn save_websocket_item_to_file(
        &mut self,
        target_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let Some(message_id) = target_id.or(self.websocket.selected) else {
            self.fail("请先选择一条 WebSocket 消息。", cx);
            return;
        };
        if self.websocket.selected == Some(message_id)
            && let Some(detail) = self
                .websocket
                .detail
                .as_deref()
                .filter(|record| record.id == message_id)
        {
            let payload = if self.websocket.original_view
                && let Some(original) = &self.websocket.original_payload
            {
                original.clone()
            } else {
                detail.payload.clone()
            };
            let direction = detail.direction;
            let kind = detail.kind;
            let observed = detail.observed_unix_ms;
            self.prompt_save_websocket_item(
                message_id, direction, kind, observed, &payload, window, cx,
            );
            return;
        }
        self.select_websocket(message_id, window, cx);
        if self.websocket.selected != Some(message_id) {
            return;
        }
        let project = self.project;
        let owner_window = self.owner_window;
        let self_entity = cx.entity();
        let sub_slot: std::rc::Rc<std::cell::RefCell<Option<Subscription>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let sub_obs = sub_slot.clone();
        let sub_timeout = sub_slot.clone();
        let observer = cx.observe(&self_entity, move |view, entity, cx| {
            if view.project != project
                || !view.backend_available()
                || view.websocket.selected != Some(message_id)
                || view.websocket.detail_error.is_some()
            {
                *sub_obs.borrow_mut() = None;
                return;
            }
            if let Some(detail) = view
                .websocket
                .detail
                .as_deref()
                .filter(|record| record.id == message_id)
            {
                let payload = detail.payload.clone();
                let direction = detail.direction;
                let kind = detail.kind;
                let observed = detail.observed_unix_ms;
                *sub_obs.borrow_mut() = None;
                let weak = entity.downgrade();
                cx.defer(move |cx| {
                    let _ = owner_window.update(cx, |_, window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.prompt_save_websocket_item(
                                message_id, direction, kind, observed, &payload, window, cx,
                            );
                        });
                    });
                });
            }
        });
        *sub_slot.borrow_mut() = Some(observer);
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_secs(15));
        cx.spawn(async move |_, cx| {
            timer.await;
            let _ = cx.update(|_| {
                *sub_timeout.borrow_mut() = None;
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn prompt_save_websocket_item(
        &mut self,
        message_id: Uuid,
        direction: WebSocketDirection,
        kind: mio_core::WebSocketKind,
        observed_unix_ms: i64,
        payload: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_picker_pending() || !self.backend_available() {
            return;
        }
        let summary = self
            .websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .find(|r| r.id == message_id)
            .cloned()
            .or_else(|| {
                self.websocket
                    .selected_summary
                    .clone()
                    .filter(|r| r.id == message_id)
            });
        let url = summary
            .as_ref()
            .map(|r| r.url.clone())
            .unwrap_or_default();
        let conn_id = summary.as_ref().map_or(0, |r| r.connection_id);
        let notes = summary
            .as_ref()
            .map(|r| r.notes_preview.clone())
            .unwrap_or_default();
        let xml_bytes = match build_websocket_item_xml(
            &url,
            direction,
            kind,
            observed_unix_ms,
            conn_id,
            &notes,
            payload,
        ) {
            Ok(xml) => xml.into_bytes(),
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some("websocket-item.xml"),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，本次 WebSocket 导出未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            if path.extension().is_none() {
                                path.set_extension("xml");
                            }
                            match std::fs::write(&path, &xml_bytes) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存 WebSocket 条目到文件：{}",
                                        path.display()
                                    );
                                    cx.notify();
                                }
                                Err(err) => view.fail(format!("写入文件失败：{err}"), cx),
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => view.fail(format!("无法选择保存路径：{err}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }
}

fn build_websocket_item_xml(
    url: &str,
    direction: WebSocketDirection,
    kind: mio_core::WebSocketKind,
    observed_unix_ms: i64,
    connection_id: u64,
    notes: &str,
    payload: &[u8],
) -> Result<String, String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event as XmlEvent};

    let mut writer = quick_xml::Writer::new(Vec::new());
    writer
        .write_event(XmlEvent::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(|e| e.to_string())?;
    let mut root = BytesStart::new("websocket-items");
    root.push_attribute(("mioVersion", env!("CARGO_PKG_VERSION")));
    writer
        .write_event(XmlEvent::Start(root))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(XmlEvent::Start(BytesStart::new("item")))
        .map_err(|e| e.to_string())?;
    let kind_str = match kind {
        mio_core::WebSocketKind::Text => "text",
        mio_core::WebSocketKind::Binary => "binary",
        _ => "message",
    };
    let dir_str = match direction {
        WebSocketDirection::ClientToServer => "client_to_server",
        WebSocketDirection::ServerToClient => "server_to_client",
    };
    let time_str = local_time(observed_unix_ms);
    for (name, value) in [
        ("url", url.to_owned()),
        ("direction", dir_str.to_owned()),
        ("kind", kind_str.to_owned()),
        ("time", time_str),
        ("connection", connection_id.to_string()),
        ("length", payload.len().to_string()),
        ("notes", notes.to_owned()),
    ] {
        writer
            .create_element(name)
            .write_text_content(BytesText::new(&value))
            .map_err(|e| e.to_string())?;
    }
    writer
        .create_element("payload")
        .with_attribute(("base64", "true"))
        .write_text_content(BytesText::new(&BASE64_STANDARD.encode(payload)))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(XmlEvent::End(BytesEnd::new("item")))
        .map_err(|e| e.to_string())?;
    writer
        .write_event(XmlEvent::End(BytesEnd::new("websocket-items")))
        .map_err(|e| e.to_string())?;
    String::from_utf8(writer.into_inner()).map_err(|e| e.to_string())
}

pub(in crate::workbench::websocket) fn websocket_url_in_scope(
    scope: &mio_core::TargetScope,
    url: &str,
) -> bool {
    if !scope.has_enabled_includes() {
        if scope.advanced_mode {
            return !scope
                .advanced_exclude
                .iter()
                .any(|rule| rule.matches_url(url));
        }
        return !scope.exclude.iter().any(|rule| rule.matches_url(url));
    }
    if scope.is_url_in_scope(url) {
        return true;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let alt = if scheme.eq_ignore_ascii_case("wss") {
        Some(format!("https://{rest}"))
    } else if scheme.eq_ignore_ascii_case("ws") {
        Some(format!("http://{rest}"))
    } else {
        None
    };
    alt.as_deref()
        .is_some_and(|mapped| scope.is_url_in_scope(mapped))
}

fn websocket_url_to_http_prefix(url: &str) -> Option<String> {
    let (scheme, tail) = url.trim().split_once("://")?;
    let http_scheme = match scheme.to_ascii_lowercase().as_str() {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        _ => return None,
    };
    let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    let authority = &tail[..authority_end];
    (!authority.is_empty()).then(|| format!("{http_scheme}://{authority}"))
}

fn direction_label(direction: WebSocketDirection) -> &'static str {
    match direction {
        WebSocketDirection::ClientToServer => "→ 服务器",
        WebSocketDirection::ServerToClient => "← 客户端",
    }
}

fn local_time(observed_unix_ms: i64) -> String {
    let seconds = observed_unix_ms.div_euclid(1000);
    let Some(local) = crate::local_time::from_unix_seconds(seconds) else {
        return "时间未知".into();
    };
    format!(
        "{:02}:{:02}:{:02} {} {}月 {}",
        local.hour, local.min, local.sec, local.day, local.month, local.year,
    )
}

#[cfg(test)]
use mio_core::WebSocketKind;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_extra_actions {
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;
    use mio_core::{ScopeRule, WebSocketDirection, WebSocketKind, WebSocketMessage};

    use super::super::tests::{drain, fixture};
    use super::*;

    #[gpui_kit::test]
    fn websocket_history_supports_in_scope_filter_notes_sidebar_and_payload_copy(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);

        f.view.update(cx, |view, cx| {
            view.workspace.scope.include.push(ScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                prefix: "https://scoped.example.com".into(),
                include_subdomains: false,
            });
            view.proxy_page = ProxyPage::WebSocketHistory;
            cx.notify();
        });
        let flow_id = Uuid::new_v4();
        let msg_in = WebSocketMessage {
            id: Uuid::new_v4(),
            revision: 1,
            flow_id,
            sequence: 1,
            direction: WebSocketDirection::ClientToServer,
            kind: WebSocketKind::Text,
            payload: b"hello-ws".to_vec(),
            observed_unix_ms: 100,
        };
        let msg_out = WebSocketMessage {
            id: Uuid::new_v4(),
            revision: 1,
            flow_id,
            sequence: 2,
            direction: WebSocketDirection::ServerToClient,
            kind: WebSocketKind::Text,
            payload: b"other-ws".to_vec(),
            observed_unix_ms: 101,
        };
        let sum_in = WebSocketSummary {
            id: msg_in.id,
            ordinal: 1,
            connection_id: 1,
            listener_port: Some(8080),
            revision: 1,
            flow_id,
            sequence: 1,
            direction: msg_in.direction,
            kind: msg_in.kind,
            payload_bytes: 8,
            observed_unix_ms: 100,
            url: "wss://scoped.example.com/ws".into(),
            edited: false,
            notes_revision: 0,
            notes_preview: String::new(),
        };
        let sum_out = WebSocketSummary {
            id: msg_out.id,
            ordinal: 2,
            connection_id: 2,
            listener_port: Some(8080),
            revision: 1,
            flow_id,
            sequence: 2,
            direction: msg_out.direction,
            kind: msg_out.kind,
            payload_bytes: 8,
            observed_unix_ms: 101,
            url: "wss://unscoped.example.com/ws".into(),
            edited: false,
            notes_revision: 0,
            notes_preview: String::new(),
        };

        let q_rev = cx
            .update_window(f.window, |_, window, cx| {
                f.view.update(cx, |view, cx| {
                    view.query_websocket_filtered_with_scope(
                        None,
                        0,
                        WebSocketHistoryFilter::default(),
                        true,
                        true,
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
            .expect("query revision");
        drain(&mut f.commands);

        f.events
            .try_send(Event::WebSocketHistoryPage {
                project: f.project,
                query_revision: q_rev,
                flow_id: None,
                filter: WebSocketHistoryFilter::default(),
                selected_match: None,
                offset: 0,
                total: 2,
                rows: vec![sum_in.clone(), sum_out],
            })
            .unwrap();
        cx.run_until_parked();

        cx.read(|cx| {
            let delegate = f.view.read(cx).websocket.table.read(cx).delegate();
            assert_eq!(delegate.rows.len(), 1);
            assert_eq!(delegate.rows[0].id, sum_in.id);
        });

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.open_websocket_notes_for(Some(sum_in.id), window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.read(|cx| {
            assert!(f.view.read(cx).websocket.notes.open);
            assert_eq!(f.view.read(cx).websocket.selected, Some(sum_in.id));
        });
        let read_rev = drain(&mut f.commands)
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::ReadWebSocket {
                    selection_revision,
                    message_id,
                    ..
                } if message_id == sum_in.id => Some(selection_revision),
                _ => None,
            })
            .expect("ReadWebSocket command");
        f.events
            .try_send(Event::WebSocketDetail {
                project: f.project,
                selection_revision: read_rev,
                record: Box::new(msg_in),
                original_payload: None,
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.copy_websocket_payload(Some(sum_in.id), false, window, cx);
            });
        })
        .unwrap();
        assert_eq!(
            cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
            Some("hello-ws".into())
        );

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.copy_websocket_payload(Some(sum_in.id), true, window, cx);
            });
        })
        .unwrap();
        assert_eq!(
            cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
            Some(editing::hex_encode(b"hello-ws"))
        );
    }
}
