use super::*;
use gpui_kit::assets::IconName;

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_websocket_repeater_editor(
        &self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter()
            .find(|tab| !tab.closed && tab.id == tab_id)
        else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("WebSocket Repeater 标签已关闭")
                .into_any_element();
        };
        let Some(editors) = self.websocket_repeater.editors.get(&tab_id) else {
            return div().size_full().into_any_element();
        };
        let can_operate =
            self.project.is_some() && self.backend_available() && self.transition.is_none();
        let is_standalone = matches!(
            editors.mode,
            WebSocketRepeaterConnectionMode::Standalone { .. }
        );
        let effective_direction = if is_standalone {
            WebSocketDirection::ClientToServer
        } else {
            tab.direction
        };
        let busy = editors.sending.is_some() || editors.connecting.is_some();
        let status_badge = mode_label(editors.mode);
        let footer_text = editors
            .result
            .clone()
            .unwrap_or_else(|| status_badge.into());
        let selected_history = editors
            .selected_history
            .filter(|selected| tab.history.iter().any(|item| item.id == *selected));
        let headers_expanded = editors.headers_expanded;
        let auto_follow = editors.auto_follow_latest;
        let header_count = tab.handshake_headers.len();

        let history_list_pane = self.render_websocket_repeater_history_list(
            tab_id,
            tab,
            editors,
            selected_history,
            can_operate,
            auto_follow,
            cx,
        );
        let history_detail_pane = self.render_websocket_repeater_history_detail(editors, cx);
        let main_split = self.render_websocket_repeater_split(
            can_operate,
            editors,
            history_list_pane,
            history_detail_pane,
            cx,
        );
        let docked = self.render_repeater_inspector_dock(main_split, window, cx);
        let header = self.render_websocket_repeater_connection_toolbar(
            tab_id,
            editors,
            can_operate,
            busy,
            status_badge,
            header_count,
            headers_expanded,
            cx,
        );
        let frame_toolbar = self.render_websocket_repeater_frame_toolbar(
            tab_id,
            tab,
            effective_direction,
            can_operate,
            busy,
            is_standalone,
            cx,
        );
        let mut root = div()
            .key_context("MioRepeater")
            .size_full()
            .flex()
            .flex_col()
            .child(header);
        if headers_expanded {
            root = root.child(self.render_websocket_repeater_headers_panel(
                tab_id,
                tab,
                editors,
                can_operate,
                busy,
                cx,
            ));
        }
        root.child(frame_toolbar)
            .child(div().flex_1().min_h_0().child(docked))
            .child(toolbar().child(footer_text))
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_websocket_repeater_history_list(
        &self,
        tab_id: TabId,
        tab: &WebSocketRepeaterTab,
        editors: &WsRepeaterEditors,
        selected_history: Option<Uuid>,
        can_operate: bool,
        auto_follow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {

        let mut history_rows = div()
            .id("ws-repeater-history-list")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if tab.history.is_empty() {
            history_rows = history_rows.child(
                div()
                    .size_full()
                    .min_h(px(72.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::muted_foreground())
                    .child("本会话尚无收发记录"),
            );
        } else {
            for (index, item) in tab.history.iter().enumerate() {
                let item_id = item.id;
                let selected = selected_history == Some(item_id);
                let arrow = match item.direction {
                    WebSocketDirection::ClientToServer => "→",
                    WebSocketDirection::ServerToClient => "←",
                };
                let kind = kind_label(item.kind);
                let length = format!("{} B", item.payload.len());
                let preview = preview_payload(&item.payload);
                let row_owner = cx.weak_entity();
                history_rows = history_rows.child(
                    div()
                        .id(("ws-repeater-history-row", index))
                        .test_support()
                        .min_h(theme::scaled_px(24.))
                        .px(px(8.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .border_b_1()
                        .border_color(theme::border())
                        .cursor_pointer()
                        .bg(if selected {
                            theme::selected()
                        } else {
                            theme::surface()
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_websocket_repeater_history(tab_id, item_id, window, cx);
                        }))
                        .context_menu(move |menu, _, cx| {
                            let Some(entity) = row_owner.upgrade() else {
                                return menu;
                            };
                            let view = entity.read(cx);
                            let editable = view.project.is_some()
                                && view.backend_available()
                                && view.transition.is_none()
                                && view
                                    .websocket_repeater
                                    .editors
                                    .get(&tab_id)
                                    .is_some_and(|ed| ed.sending.is_none());
                            let load_draft = row_owner.clone();
                            let to_decoder = row_owner.clone();
                            let to_comparer = row_owner.clone();
                            let copy_text = row_owner.clone();
                            let copy_hex = row_owner.clone();
                            let save_all = row_owner.clone();
                            menu.item(
                                PopupMenuItem::new("载入到发送编辑器")
                                    .disabled(!editable)
                                    .on_click(move |_, window, cx| {
                                        let _ = load_draft.update(cx, |this, cx| {
                                            this.load_specific_history_into_websocket_draft(
                                                tab_id, item_id, window, cx,
                                            );
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("发送到 Decoder").on_click(
                                    move |_, window, cx| {
                                        let _ = to_decoder.update(cx, |this, cx| {
                                            this.send_ws_repeater_history_to_decoder(
                                                tab_id, item_id, window, cx,
                                            );
                                        });
                                    },
                                ),
                            )
                            .item(
                                PopupMenuItem::new("发送到 Comparer").on_click(
                                    move |_, window, cx| {
                                        let _ = to_comparer.update(cx, |this, cx| {
                                            this.send_ws_repeater_history_to_comparer(
                                                tab_id, item_id, window, cx,
                                            );
                                        });
                                    },
                                ),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("复制载荷文本").on_click(move |_, _, cx| {
                                    let _ = copy_text.update(cx, |this, cx| {
                                        this.copy_ws_repeater_history_payload(
                                            tab_id, item_id, false, cx,
                                        );
                                    });
                                }),
                            )
                            .item(
                                PopupMenuItem::new("复制载荷 Hex").on_click(move |_, _, cx| {
                                    let _ = copy_hex.update(cx, |this, cx| {
                                        this.copy_ws_repeater_history_payload(
                                            tab_id, item_id, true, cx,
                                        );
                                    });
                                }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("保存完整历史 (Save entire history)…")
                                    .disabled(!editable)
                                    .on_click(move |_, window, cx| {
                                        let _ = save_all.update(cx, |this, cx| {
                                            this.save_repeater_entire_history(window, cx);
                                        });
                                    }),
                            )
                        })
                        .child(
                            div()
                                .w(theme::scaled_px(38.))
                                .flex_shrink_0()
                                .truncate()
                                .child(format!("#{}", index + 1)),
                        )
                        .child(div().w(theme::scaled_px(32.)).flex_shrink_0().child(arrow))
                        .child(
                            div()
                                .w(theme::scaled_px(54.))
                                .flex_shrink_0()
                                .truncate()
                                .child(kind),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(68.))
                                .flex_shrink_0()
                                .truncate()
                                .child(length),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(preview)),
                );
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .child("会话历史")
                    .child(format!("（{}）", tab.history.len()))
                    .child(div().flex_1())
                    .child(
                        button("ws-repeater-auto-follow", "自动跟随最新消息")
                            .icon(IconName::ArrowDown)
                            .tooltip("收到或发送新 WebSocket 帧时自动选中并展示最新消息")
                            .selected(auto_follow)
                            .toggled(auto_follow)
                            .tab_stop(false)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_websocket_repeater_auto_follow(tab_id, window, cx);
                            })),
                    )
                    .child(
                        button("ws-repeater-use-history", "载入到发送框")
                            .icon(IconName::CornerDownRight)
                            .tooltip("将所选历史帧的载荷内容与方向载入左侧待发送编辑器")
                            .disabled(
                                !can_operate
                                    || selected_history.is_none()
                                    || editors.sending.is_some(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.load_history_into_websocket_draft(tab_id, window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .min_h(theme::row_height())
                    .px(px(8.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(12.))
                    .child(div().w(theme::scaled_px(38.)).flex_shrink_0().child("#"))
                    .child(div().w(theme::scaled_px(32.)).flex_shrink_0().child("方向"))
                    .child(div().w(theme::scaled_px(54.)).flex_shrink_0().child("类型"))
                    .child(div().w(theme::scaled_px(68.)).flex_shrink_0().child("长度"))
                    .child(div().flex_1().min_w_0().child("内容预览")),
            )
            .child(history_rows)
            .into_any_element()
    }

    #[inline(never)]
    fn render_websocket_repeater_history_detail(
        &self,
        editors: &WsRepeaterEditors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.message_header("ws-repeater-history", "历史消息", editors.history.view, cx))
            .child(
                div().flex_1().min_h_0().child(
                    editing::message_editor(editors.history.state())
                        .readonly(true)
                        .size_full()
                        .context_menu(
                            self.request_context_menu(editors.history.state().clone(), true),
                        )
                        .aria_label("WebSocket 历史消息编辑器"),
                ),
            )
            .child(editors.history.render_search_bar(
                "ws-repeater-history",
                cx,
                |this, action, window, cx| {
                    this.handle_message_search_action("ws-repeater-history", action, window, cx);
                },
            ))
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_websocket_repeater_connection_toolbar(
        &self,
        tab_id: TabId,
        editors: &WsRepeaterEditors,
        can_operate: bool,
        busy: bool,
        status_badge: &'static str,
        header_count: usize,
        headers_expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
                toolbar()
                    .child(
                        button("ws-repeater-send", "发送")
                            .primary()
                            .icon(IconName::SendHorizontal)
                            .tooltip(if cfg!(target_os = "macos") {
                                "通过当前连接按指定方向与帧类型发送待发送消息（⌘Enter）"
                            } else {
                                "通过当前连接按指定方向与帧类型发送待发送消息（Ctrl+Enter）"
                            })
                            .disabled(!can_operate || busy || self.pending_records > 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.send_websocket_repeater(tab_id, window, cx);
                            })),
                    )
                    .child(
                        button(
                            "ws-repeater-connect",
                            if editors.mode.is_connected() {
                                "重连"
                            } else {
                                "连接"
                            },
                        )
                        .icon(if editors.mode.is_connected() {
                            IconName::RotateCw
                        } else {
                            IconName::Plug
                        })
                        .tooltip(if editors.mode.is_connected() {
                            "重新握手并建立新的独立 WebSocket 会话"
                        } else {
                            "向目标 URL 发起 WebSocket 握手并建立独立测试连接"
                        })
                        .disabled(
                            !can_operate || editors.connecting.is_some() || editors.disconnecting,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.connect_websocket_repeater(tab_id, window, cx);
                            },
                        )),
                    )
                    .child(
                        button("ws-repeater-disconnect", "断开")
                            .icon(IconName::Unplug)
                            .tooltip("关闭当前 WebSocket 连接会话")
                            .disabled(
                                !can_operate
                                    || (!editors.mode.is_connected()
                                        && editors.connecting.is_none())
                                    || editors.disconnecting,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.disconnect_websocket_repeater(tab_id, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("ws-repeater-status")
                            .test_support()
                            .aria_label(status_badge)
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if editors.mode.is_connected() {
                                rgb(if theme::is_dark() { 0x2a5a41 } else { 0xb7e4c7 }).into()
                            } else {
                                theme::border()
                            })
                            .bg(rgb(if editors.mode.is_connected() {
                                if theme::is_dark() { 0x1e3a2b } else { 0xe7f5ec }
                            } else if theme::is_dark() {
                                0x35383d
                            } else {
                                0xf4f5f7
                            }))
                            .child(status_badge),
                    )
                    .child("目标：")
                    .child(
                        div().flex_1().min_w(px(140.)).child(
                            Input::new(&editors.url)
                                .id("ws-repeater-url")
                                .aria_label("WebSocket 目标 URL")
                                .small()
                                .disabled(!can_operate || busy),
                        ),
                    )
                    .child(
                        button(
                            "ws-repeater-headers-toggle",
                            format!("握手标头 ({header_count})"),
                        )
                        .icon(IconName::SlidersHorizontal)
                        .tooltip("展开或折叠 WebSocket 握手请求标头管理面板（重连时生效）")
                        .selected(headers_expanded)
                        .toggled(headers_expanded)
                        .tab_stop(false)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_websocket_repeater_headers_panel(tab_id, cx);
                        })),
                    )
                    .into_any_element()
    }

    #[inline(never)]
    fn render_websocket_repeater_headers_panel(
        &self,
        tab_id: TabId,
        tab: &WebSocketRepeaterTab,
        editors: &WsRepeaterEditors,
        can_operate: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
                let mut panel = div()
                    .id("ws-repeater-headers-panel")
                    .test_support()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .px(px(10.))
                    .py(px(6.))
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div().w(px(200.)).child(
                                    Input::new(&editors.header_name)
                                        .id("ws-repeater-header-name")
                                        .aria_label("握手标头名称")
                                        .small()
                                        .disabled(!can_operate || busy),
                                ),
                            )
                            .child(
                                div().flex_1().min_w(px(160.)).child(
                                    Input::new(&editors.header_value)
                                        .id("ws-repeater-header-value")
                                        .aria_label("握手标头内容")
                                        .small()
                                        .disabled(!can_operate || busy),
                                ),
                            )
                            .child(
                                button("ws-repeater-header-add", "添加握手标头")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .tooltip("保存自定义 HTTP 握手标头，建立或重连独立 WebSocket 会话时发送")
                                    .disabled(!can_operate || busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.add_websocket_repeater_handshake_header(
                                            tab_id, window, cx,
                                        );
                                    })),
                            ),
                    );
                if tab.handshake_headers.is_empty() {
                    panel = panel.child(
                        div()
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::muted_foreground())
                            .child("当前未配置额外握手标头；可输入名称与内容（或在名称框输入 Name: Value）后添加。"),
                    );
                } else {
                    let mut list = div()
                        .id("ws-repeater-headers-list")
                        .test_support()
                        .max_h(px(130.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(px(2.));
                    for (idx, header) in tab.handshake_headers.iter().enumerate() {
                        let val_str = String::from_utf8_lossy(&header.value).into_owned();
                        list = list.child(
                            div()
                                .id(("ws-repeater-header-row", idx))
                                .test_support()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(3.))
                                .bg(theme::surface())
                                .border_1()
                                .border_color(theme::border())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(theme::scaled_px(12.))
                                        .child(format!("{}: {}", header.name, val_str)),
                                )
                                .child(
                                    button(("ws-repeater-header-del", idx), "删除")
                                        .icon(IconName::Trash)
                                        .tooltip("移除此握手标头")
                                        .disabled(!can_operate || busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.remove_websocket_repeater_handshake_header(
                                                tab_id, idx, window, cx,
                                            );
                                        })),
                                ),
                        );
                    }
                    panel = panel.child(list);
                }
                panel.into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_websocket_repeater_frame_toolbar(
        &self,
        tab_id: TabId,
        tab: &WebSocketRepeaterTab,
        effective_direction: WebSocketDirection,
        can_operate: bool,
        busy: bool,
        is_standalone: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
                toolbar()
                    .child("方向：")
                    .child(
                        button("ws-repeater-dir-c2s", "客户端 → 服务器")
                            .icon(IconName::ArrowRight)
                            .tooltip("向远端目标服务器注入 WebSocket 数据帧（Client-to-server）")
                            .selected(effective_direction == WebSocketDirection::ClientToServer)
                            .disabled(!can_operate || busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_websocket_repeater_direction(
                                    tab_id,
                                    WebSocketDirection::ClientToServer,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("ws-repeater-dir-s2c", "服务器 → 客户端")
                            .icon(IconName::ArrowLeft)
                            .tooltip("在复用代理连接模式下向客户端注入 WebSocket 数据帧（Server-to-client）")
                            .selected(effective_direction == WebSocketDirection::ServerToClient)
                            .disabled(!can_operate || busy || is_standalone)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_websocket_repeater_direction(
                                    tab_id,
                                    WebSocketDirection::ServerToClient,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child("帧类型：")
                    .child(
                        button("ws-repeater-kind-text", "Text")
                            .icon(IconName::FileText)
                            .tooltip("以 UTF-8 文本帧（Opcode 0x1）编码发送载荷")
                            .selected(tab.draft_kind == WebSocketKind::Text)
                            .disabled(!can_operate || busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_websocket_repeater_kind(
                                    tab_id,
                                    WebSocketKind::Text,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("ws-repeater-kind-binary", "Binary")
                            .icon(IconName::Binary)
                            .tooltip("以原始二进制帧（Opcode 0x2）编码发送载荷")
                            .selected(tab.draft_kind == WebSocketKind::Binary)
                            .disabled(!can_operate || busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_websocket_repeater_kind(
                                    tab_id,
                                    WebSocketKind::Binary,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("ws-repeater-kind-ping", "Ping")
                            .icon(IconName::Activity)
                            .tooltip("以心跳探测控制帧（Opcode 0x9）编码发送载荷")
                            .selected(tab.draft_kind == WebSocketKind::Ping)
                            .disabled(!can_operate || busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_websocket_repeater_kind(
                                    tab_id,
                                    WebSocketKind::Ping,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("ws-repeater-kind-pong", "Pong")
                            .icon(IconName::Radio)
                            .tooltip("以心跳应答控制帧（Opcode 0xA）编码发送载荷")
                            .selected(tab.draft_kind == WebSocketKind::Pong)
                            .disabled(!can_operate || busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_websocket_repeater_kind(
                                    tab_id,
                                    WebSocketKind::Pong,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("ws-repeater-kind-close", "Close")
                            .icon(IconName::Ban)
                            .tooltip("以关闭握手控制帧（Opcode 0x8）编码发送载荷")
                            .selected(tab.draft_kind == WebSocketKind::Close)
                            .disabled(!can_operate || busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_websocket_repeater_kind(
                                    tab_id,
                                    WebSocketKind::Close,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .into_any_element()
    }

    #[inline(never)]
    fn render_websocket_repeater_split(
        &self,
        can_operate: bool,
        editors: &WsRepeaterEditors,
        history_list_pane: AnyElement,
        history_detail_pane: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(self.message_header(
                        "ws-repeater-draft",
                        "待发送消息",
                        editors.draft.view,
                        cx,
                    ))
                    .child(
                        div().flex_1().min_h_0().child(
                            editing::message_editor(editors.draft.state())
                                .readonly(!can_operate || editors.draft.is_readonly_preview())
                                .size_full()
                                .context_menu(self.request_context_menu(
                                    editors.draft.state().clone(),
                                    !can_operate || editors.draft.is_readonly_preview(),
                                ))
                                .aria_label("WebSocket 待发送消息编辑器"),
                        ),
                    )
                    .child(editors.draft.render_search_bar_editable(
                        "ws-repeater-draft",
                        !can_operate || editors.draft.is_readonly_preview(),
                        cx,
                        |this, action, window, cx| {
                            this.handle_message_search_action(
                                "ws-repeater-draft",
                                action,
                                window,
                                cx,
                            );
                        },
                    )),
            )
            .child(
                div().flex_1().min_w_0().child(
                    v_resizable("ws-repeater-history-split")
                        .child(
                            resizable_panel()
                                .size(px(210.))
                                .size_range(px(120.)..px(700.))
                                .child(history_list_pane),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(120.)..px(10000.))
                                .child(history_detail_pane),
                        ),
                ),
            )
            .into_any_element()
    }
}
