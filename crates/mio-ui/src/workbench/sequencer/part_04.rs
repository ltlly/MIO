use super::*;
use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement;

#[derive(Clone, Copy, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
struct ClearSequencerManualEditor;

fn sequencer_manual_editor_context_menu(
    input: Entity<EditorState>,
    readonly: bool,
) -> impl Fn(
    gpui_kit::component::native_menu::NativeMenu,
    &mut Window,
    &mut App,
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
            .menu_with_disabled(
                "清空",
                readonly || empty,
                Box::new(ClearSequencerManualEditor),
            )
    }
}

fn sequencer_live_request_row_menu(
    view: WeakEntity<Workbench>,
    seq: usize,
    menu: PopupMenu,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(owner) = view.upgrade() else {
        return menu;
    };
    let state = owner.read(cx);
    let can_operate = state.sequencer_can_operate();
    let capturing = state.sequencer_pane.running.is_some();
    let exists = state
        .sequencer_pane
        .live_requests
        .iter()
        .any(|item| item.seq == seq);
    let set_target_view = view.clone();
    let repeater_view = view.clone();
    let intruder_view = view.clone();
    let copy_url_view = view;
    menu.item(
        PopupMenuItem::new("设为目标请求")
            .disabled(!can_operate || capturing || !exists)
            .on_click(move |_, window, cx| {
                let _ = set_target_view.update(cx, |v, cx| {
                    v.set_sequencer_target_request(seq, window, cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("发送到 Repeater")
            .disabled(!can_operate || !exists)
            .on_click(move |_, window, cx| {
                let _ = repeater_view.update(cx, |v, cx| {
                    v.send_sequencer_live_request_to_repeater(seq, window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Intruder")
            .disabled(!can_operate || !exists)
            .on_click(move |_, window, cx| {
                let _ = intruder_view.update(cx, |v, cx| {
                    v.send_sequencer_live_request_to_intruder(seq, window, cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("复制 URL")
            .disabled(!exists)
            .on_click(move |_, window, cx| {
                let _ = copy_url_view.update(cx, |v, cx| {
                    v.copy_sequencer_live_request_url(seq, window, cx)
                });
            }),
    )
}

impl Workbench {
    #[inline(never)]
    fn render_sequencer_live_request_drawer(
        &self,
        can_edit_config: bool,
        has_selected_request: bool,
        request_readonly: bool,
        table_h: gpui_kit::Pixels,
        table_border: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.sequencer_pane;
        let pretty = pane.request.pretty_active;
        let view = pane.request.view;
        let add_current_btn = (!has_selected_request).then(|| {
            button("sequencer-request-add-current", "加入列表")
                .tooltip("将当前编辑的目标 URL 与请求添加到实时捕获列表")
                .disabled(!can_edit_config)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.add_current_sequencer_request_to_list(window, cx);
                }))
        });

        div()
            .id("sequencer-request-drawer")
            .test_support()
            .flex_1()
            .min_w(px(360.))
            .h(table_h)
            .border_1()
            .border_color(table_border)
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .w_full()
                    .min_h(theme::scaled_px(30.))
                    .px(px(8.))
                    .py(px(3.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::foreground())
                            .child("目标 URL："),
                    )
                    .child(
                        div().flex_1().min_w(px(120.)).child(
                            Input::new(&pane.target_url)
                                .id("sequencer-target-url")
                                .aria_label("Sequencer 目标 URL")
                                .small()
                                .disabled(!can_edit_config),
                        ),
                    )
                    .child(
                        button("sequencer-version-http11", "HTTP/1.1")
                            .tooltip("使用 HTTP/1.1 协议发送采样请求")
                            .selected(pane.request_version != HttpVersion::Http2)
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sequencer_pane.request_version = HttpVersion::Http11;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("sequencer-version-http2", "HTTP/2")
                            .tooltip("使用 HTTP/2 协议发送采样请求")
                            .selected(pane.request_version == HttpVersion::Http2)
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sequencer_pane.request_version = HttpVersion::Http2;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_h(theme::scaled_px(26.))
                    .px(px(8.))
                    .py(px(2.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .bg(theme::surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        button("sequencer-request-editor-pretty", "Pretty")
                            .tooltip("格式化预览采样源请求报文")
                            .selected(pretty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_sequencer_request_pretty(window, cx);
                            })),
                    )
                    .child(
                        button("sequencer-request-editor-raw", "Raw")
                            .tooltip("查看或编辑采样源请求原始文本")
                            .selected(!pretty && view == MessageView::Raw)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_sequencer_request_view(MessageView::Raw, window, cx);
                            })),
                    )
                    .child(
                        button("sequencer-request-editor-hex", "Hex")
                            .tooltip("以十六进制视图查看或编辑请求字节")
                            .selected(!pretty && view == MessageView::Hex)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_sequencer_request_view(MessageView::Hex, window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("sequencer-delim-from-selection", "从选区生成分隔符")
                            .icon(IconName::Scissors)
                            .tooltip("根据请求编辑器当前选区前后文本自动填充起止分隔符")
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.fill_sequencer_delimiters_from_selection(window, cx);
                            })),
                    )
                    .children(add_current_btn),
            )
            .child(
                div()
                    .id("sequencer-request-editor")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .bg(theme::surface())
                    .child(
                        editing::message_editor(pane.request.state())
                            .readonly(request_readonly)
                            .size_full()
                            .context_menu(self.request_context_menu(
                                pane.request.state().clone(),
                                request_readonly,
                            ))
                            .aria_label("Sequencer 采样源请求编辑器"),
                    ),
            )
            .child(pane.request.render_search_bar_editable(
                "sequencer-request-editor",
                request_readonly,
                cx,
                |this, action, window, cx| {
                    this.handle_sequencer_search_action(action, window, cx);
                },
            ))
            .into_any_element()
    }

    #[inline(never)]
    fn render_sequencer_live_request_section(
        &self,
        can_edit_config: bool,
        drawer_open: bool,
        has_selected_request: bool,
        request_readonly: bool,
        can_start_capture: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.sequencer_pane;
        let dark = theme::is_dark();
        let has_any_request = !pane.live_requests.is_empty();
        let live_target_text = pane.target_url.read(cx).value().to_string();
        let live_req_text = pane.request.state().read(cx).value().to_string();
        let request_rows_data: Vec<(usize, String, String, bool)> = pane
            .live_requests
            .iter()
            .map(|item| {
                let selected = pane.selected_request_seq == Some(item.seq);
                if selected {
                    let host =
                        extract_live_request_host(&live_target_text, live_req_text.as_bytes());
                    let summary = extract_live_request_summary(live_req_text.as_bytes());
                    (item.seq, host, summary, true)
                } else {
                    (
                        item.seq,
                        item.host.clone(),
                        item.request_summary.clone(),
                        false,
                    )
                }
            })
            .collect();

        let table_h = if drawer_open { px(200.) } else { px(164.) };
        let table_w = if drawer_open { px(380.) } else { px(560.) };
        let host_col_w = if drawer_open { px(136.) } else { px(188.) };
        let table_border: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xd0d0d0).into()
        };
        let toggle_hover_bg: gpui_kit::Hsla = if dark {
            rgb(0x34373c).into()
        } else {
            rgb(0xeeeeee).into()
        };

        let remove_btn = sequencer_list_button(
            "sequencer-request-remove",
            "移除",
            can_edit_config && has_selected_request,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("移除当前选中的实时捕获请求")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.remove_selected_sequencer_request(window, cx);
        }));

        let clear_btn = sequencer_list_button(
            "sequencer-request-clear",
            "清空",
            can_edit_config && has_any_request,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("清空实时捕获请求列表").build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.clear_sequencer_requests(window, cx);
        }));

        let mut request_table_body = div()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .flex()
            .flex_col();
        let row_menu_owner = cx.weak_entity();
        for (seq, host, summary, selected) in request_rows_data {
            let row_hover_bg: gpui_kit::Hsla = if selected {
                theme::selected()
            } else if dark {
                rgb(0x31343a).into()
            } else {
                rgb(0xf5f7fa).into()
            };
            let menu_weak = row_menu_owner.clone();
            request_table_body = request_table_body.child(
                div()
                    .id(("sequencer-request-row", seq))
                    .test_support()
                    .role(Role::Row)
                    .aria_label(format!("#{seq} {host} {summary}"))
                    .w_full()
                    .h(theme::scaled_px(23.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(if selected {
                        theme::selected()
                    } else {
                        theme::surface()
                    })
                    .hover(move |s| s.bg(row_hover_bg))
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(12.5))
                    .text_color(theme::foreground())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_sequencer_live_request(seq, window, cx);
                    }))
                    .context_menu(move |menu, _, cx| {
                        sequencer_live_request_row_menu(menu_weak.clone(), seq, menu, cx)
                    })
                    .child(
                        div()
                            .w(px(68.))
                            .h_full()
                            .px(px(6.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(theme::border())
                            .truncate()
                            .child(seq.to_string()),
                    )
                    .child(
                        div()
                            .w(host_col_w)
                            .h_full()
                            .px(px(6.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(theme::border())
                            .truncate()
                            .child(host),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .truncate()
                            .child(summary),
                    ),
            );
        }

        let request_table = div()
            .id("sequencer-request-table")
            .test_support()
            .w(table_w)
            .max_w_full()
            .h(table_h)
            .flex_shrink_0()
            .border_1()
            .border_color(table_border)
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .w_full()
                    .h(theme::scaled_px(24.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(sequencer_table_header_bg())
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(12.5))
                    .text_color(theme::foreground())
                    .child(
                        div()
                            .w(px(68.))
                            .h_full()
                            .px(px(6.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .border_r_1()
                            .border_color(theme::border())
                            .child("#")
                            .child(
                                Icon::new(IconName::ChevronUp)
                                    .size(px(11.))
                                    .text_color(theme::foreground()),
                            ),
                    )
                    .child(
                        div()
                            .w(host_col_w)
                            .h_full()
                            .px(px(6.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(theme::border())
                            .child("Host"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .child("Request"),
                    ),
            )
            .child(request_table_body);

        let toggle_drawer_btn = div()
            .h(table_h)
            .ml(px(2.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .child(
                flat_button("sequencer-toggle-request-editor", false, window, cx)
                    .accessibility_label("展开或折叠采样源请求编辑器")
                    .w(px(12.))
                    .h(px(22.))
                    .rounded(px(2.))
                    .flex()
                    .items_center()
                    .justify_start()
                    .text_size(px(18.))
                    .line_height(px(18.))
                    .text_color(if dark {
                        theme::foreground()
                    } else {
                        rgb(0x000000).into()
                    })
                    .hover(move |s| s.bg(toggle_hover_bg))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if drawer_open {
                            "折叠采样源请求编辑器"
                        } else {
                            "展开采样源请求编辑器（查看或手动编辑目标 URL 与 HTTP 报文）"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sequencer_pane.request_drawer_open =
                            !this.sequencer_pane.request_drawer_open;
                        cx.notify();
                    }))
                    .child(if drawer_open { "◂" } else { "▸" }),
            );

        let request_drawer = drawer_open.then(|| {
            self.render_sequencer_live_request_drawer(
                can_edit_config,
                has_selected_request,
                request_readonly,
                table_h,
                table_border,
                cx,
            )
        });

        let start_capture_btn = sequencer_primary_button(
            "sequencer-start-capture",
            "开始实时采样",
            can_start_capture,
            window,
            cx,
        )
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(if can_start_capture {
                "发送并发采样请求并实时提取 Token"
            } else {
                "请先从其他工具发送请求到 Sequencer，或点击表格右侧箭头手动编辑请求"
            })
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.start_sequencer_capture(window, cx);
        }));

        sequencer_section(
            "sequencer-help-request",
            "选择实时捕获请求帮助",
            "从 Proxy、Target 或 Repeater 等工具将请求发送到 Sequencer，选中请求后即可发起实时采样。",
            "选择实时捕获请求",
            Some("从其他工具将请求发送到此处以配置实时捕获。选择要使用的请求并配置下方选项，然后点击“开始实时采样”。"),
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(90.))
                                .flex_shrink_0()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(remove_btn)
                                .child(clear_btn),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .child(request_table)
                                .child(toggle_drawer_btn),
                        )
                        .children(request_drawer),
                )
                .child(div().flex().items_center().child(start_capture_btn))
                .into_any_element(),
        )
    }

    #[inline(never)]
    fn render_sequencer_live_location_section(
        &self,
        can_edit_config: bool,
        location_enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.sequencer_pane;
        let dark = theme::is_dark();
        let is_custom = matches!(
            pane.location_kind,
            LocationKind::BodyDelimiter | LocationKind::Regex | LocationKind::Header
        );
        let custom_summary = if !location_enabled {
            String::new()
        } else {
            match pane.location_kind {
                LocationKind::Regex => {
                    let pattern = pane.regex_pattern.read(cx).value();
                    if pattern.trim().is_empty() {
                        String::new()
                    } else {
                        format!("正则: {pattern}")
                    }
                }
                LocationKind::Header => {
                    let header = pane.header_name.read(cx).value();
                    if header.trim().is_empty() {
                        String::new()
                    } else {
                        format!("响应头: {header}")
                    }
                }
                _ => {
                    let start = pane.delim_start.read(cx).value();
                    let end = pane.delim_end.read(cx).value();
                    if start.is_empty() && end.is_empty() {
                        String::new()
                    } else {
                        format!("分隔符: {start} … {end}")
                    }
                }
            }
        };

        let combo_border: gpui_kit::Hsla = if dark {
            rgb(0x454950).into()
        } else {
            rgb(0xdadada).into()
        };
        let combo_divider: gpui_kit::Hsla = if dark {
            rgb(0x3f434a).into()
        } else {
            rgb(0xe4e4e4).into()
        };
        let chevron_fg: gpui_kit::Hsla = if dark {
            theme::muted_foreground()
        } else {
            rgb(0x3c3c3c).into()
        };

        let cookie_checked = location_enabled && pane.location_kind == LocationKind::Cookie;
        let form_field_checked = location_enabled && pane.location_kind == LocationKind::FormField;
        let custom_checked = !location_enabled || is_custom;

        let cookie_radio = sequencer_radio(
            "sequencer-loc-cookie",
            "Cookie：",
            cookie_checked,
            location_enabled,
            window,
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.sequencer_pane.location_kind = LocationKind::Cookie;
            cx.notify();
        }));

        let form_field_radio = sequencer_radio(
            "sequencer-loc-form-field",
            "表单字段：",
            form_field_checked,
            location_enabled,
            window,
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.sequencer_pane.location_kind = LocationKind::FormField;
            cx.notify();
        }));

        let custom_radio = sequencer_radio(
            "sequencer-loc-custom",
            "自定义位置：",
            custom_checked,
            location_enabled,
            window,
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            if !matches!(
                this.sequencer_pane.location_kind,
                LocationKind::BodyDelimiter | LocationKind::Regex | LocationKind::Header
            ) {
                this.sequencer_pane.location_kind = LocationKind::BodyDelimiter;
            }
            this.sequencer_pane.custom_config_open = true;
            cx.notify();
        }));

        let configure_btn = sequencer_outline_button(
            "sequencer-custom-configure",
            "配置",
            location_enabled,
            window,
            cx,
        )
        .w(px(96.))
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "配置响应体起止分隔符、正则表达式或响应头提取规则",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            if !matches!(
                this.sequencer_pane.location_kind,
                LocationKind::BodyDelimiter | LocationKind::Regex | LocationKind::Header
            ) {
                this.sequencer_pane.location_kind = LocationKind::BodyDelimiter;
                this.sequencer_pane.custom_config_open = true;
            } else {
                this.sequencer_pane.custom_config_open = !this.sequencer_pane.custom_config_open;
            }
            cx.notify();
        }));

        let cookie_field = if location_enabled {
            div()
                .w(px(340.))
                .flex_shrink_0()
                .child(
                    Input::new(&pane.cookie_name)
                        .id("sequencer-cookie-name")
                        .aria_label("Cookie 名称")
                        .small()
                        .disabled(!can_edit_config || pane.location_kind != LocationKind::Cookie)
                        .suffix(
                            div()
                                .h(px(20.))
                                .pl(px(6.))
                                .border_l_1()
                                .border_color(combo_divider)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::ChevronDown)
                                        .size(px(13.))
                                        .text_color(chevron_fg),
                                ),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .id("sequencer-cookie-combobox-idle")
                .w(px(340.))
                .h(px(26.))
                .flex_shrink_0()
                .rounded(px(3.))
                .border_1()
                .border_color(combo_border)
                .bg(theme::surface())
                .flex()
                .items_center()
                .child(div().flex_1().h_full())
                .child(
                    div()
                        .w(px(26.))
                        .h_full()
                        .flex_shrink_0()
                        .border_l_1()
                        .border_color(combo_divider)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::ChevronDown)
                                .size(px(13.))
                                .text_color(chevron_fg),
                        ),
                )
                .into_any_element()
        };

        let form_field_box = if location_enabled {
            div()
                .w(px(340.))
                .flex_shrink_0()
                .child(
                    Input::new(&pane.form_field_name)
                        .id("sequencer-form-field-name")
                        .aria_label("表单字段名称")
                        .small()
                        .disabled(!can_edit_config || pane.location_kind != LocationKind::FormField)
                        .suffix(
                            div()
                                .h(px(20.))
                                .pl(px(6.))
                                .border_l_1()
                                .border_color(combo_divider)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::ChevronDown)
                                        .size(px(13.))
                                        .text_color(chevron_fg),
                                ),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .id("sequencer-form-field-combobox-idle")
                .w(px(340.))
                .h(px(26.))
                .flex_shrink_0()
                .rounded(px(3.))
                .border_1()
                .border_color(combo_border)
                .bg(theme::surface())
                .flex()
                .items_center()
                .child(div().flex_1().h_full())
                .child(
                    div()
                        .w(px(26.))
                        .h_full()
                        .flex_shrink_0()
                        .border_l_1()
                        .border_color(combo_divider)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::ChevronDown)
                                .size(px(13.))
                                .text_color(chevron_fg),
                        ),
                )
                .into_any_element()
        };

        let custom_config_panel = (location_enabled && is_custom && pane.custom_config_open).then(
            || {
                let mode_inputs = match pane.location_kind {
                    LocationKind::Regex => div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(theme::scaled_px(88.))
                                .flex_shrink_0()
                                .text_size(theme::scaled_px(12.5))
                                .child("正则表达式："),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&pane.regex_pattern)
                                    .id("sequencer-regex-pattern")
                                    .aria_label("Token 提取正则表达式")
                                    .small()
                                    .disabled(!can_edit_config),
                            ),
                        )
                        .into_any_element(),
                    LocationKind::Header => div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(theme::scaled_px(88.))
                                .flex_shrink_0()
                                .text_size(theme::scaled_px(12.5))
                                .child("Header 名称："),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&pane.header_name)
                                    .id("sequencer-header-name")
                                    .aria_label("Header 名称")
                                    .small()
                                    .disabled(!can_edit_config),
                            ),
                        )
                        .into_any_element(),
                    _ => div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .w(theme::scaled_px(88.))
                                        .flex_shrink_0()
                                        .text_size(theme::scaled_px(12.5))
                                        .child("起始分隔符："),
                                )
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&pane.delim_start)
                                            .id("sequencer-delim-start")
                                            .aria_label("起始分隔符")
                                            .small()
                                            .disabled(!can_edit_config),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .w(theme::scaled_px(88.))
                                        .flex_shrink_0()
                                        .text_size(theme::scaled_px(12.5))
                                        .child("结束分隔符："),
                                )
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&pane.delim_end)
                                            .id("sequencer-delim-end")
                                            .aria_label("结束分隔符")
                                            .small()
                                            .disabled(!can_edit_config),
                                    ),
                                ),
                        )
                        .into_any_element(),
                };

                div()
                    .id("sequencer-custom-config-panel")
                    .test_support()
                    .ml(theme::scaled_px(124.))
                    .w(px(444.))
                    .max_w_full()
                    .p(px(10.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                button("sequencer-loc-body", "响应体分隔符 Body")
                                    .tooltip("通过起止分隔符从响应体文本中截取 Token")
                                    .selected(pane.location_kind == LocationKind::BodyDelimiter)
                                    .disabled(!can_edit_config)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sequencer_pane.location_kind =
                                            LocationKind::BodyDelimiter;
                                        this.sequencer_pane.custom_config_open = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button("sequencer-loc-regex", "正则提取 Regex")
                                    .tooltip(
                                        "使用正则表达式从完整 HTTP 响应中提取首个捕获组或完整匹配",
                                    )
                                    .selected(pane.location_kind == LocationKind::Regex)
                                    .disabled(!can_edit_config)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sequencer_pane.location_kind = LocationKind::Regex;
                                        this.sequencer_pane.custom_config_open = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button("sequencer-loc-header", "响应头 Header")
                                    .tooltip("从指定 HTTP 响应头字段中提取 Token")
                                    .selected(pane.location_kind == LocationKind::Header)
                                    .disabled(!can_edit_config)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sequencer_pane.location_kind = LocationKind::Header;
                                        this.sequencer_pane.custom_config_open = true;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(mode_inputs)
            },
        );

        sequencer_section(
            "sequencer-help-location",
            "响应中的 Token 位置帮助",
            "指定从响应 Cookie、HTML 表单字段、响应体分隔符、正则表达式或响应头中提取待分析的 Token。",
            "响应中的 Token 位置",
            Some("选择响应中 Token 出现的位置。"),
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(cookie_radio)
                        .child(cookie_field),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(form_field_radio)
                        .child(form_field_box),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(custom_radio)
                        .child(
                            div()
                                .id("sequencer-custom-summary")
                                .w(px(340.))
                                .h(px(26.))
                                .px(px(8.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(combo_border)
                                .bg(theme::surface())
                                .text_size(theme::scaled_px(12.5))
                                .text_color(theme::foreground())
                                .child(div().flex_1().min_w_0().truncate().child(custom_summary)),
                        )
                        .child(configure_btn),
                )
                .children(custom_config_panel)
                .into_any_element(),
        )
    }

    #[inline(never)]
    fn render_sequencer_live_progress_section(
        &self,
        can_edit_config: bool,
        capturing: bool,
        can_analyze_captured: bool,
        collected_count: usize,
        progress_pct: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.sequencer_pane;
        let dark = theme::is_dark();
        let stats_summary = format!(
            "已发送 {} · 已提取 {} · 未命中 {} · 错误 {} · 进度 {:.1}%",
            pane.capture.requests_sent,
            collected_count,
            pane.capture.extraction_misses,
            pane.capture.request_errors,
            progress_pct
        );

        let stop_btn = sequencer_stack_button(
            "sequencer-stop-capture",
            "停止采样",
            capturing,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("停止当前正在运行的实时采样任务")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.stop_sequencer_capture(cx);
        }));

        let analyze_btn = if can_analyze_captured {
            sequencer_primary_button(
                "sequencer-analyze-captured",
                "分析已采样本",
                true,
                window,
                cx,
            )
            .h(theme::scaled_px(26.))
            .min_w(px(0.))
            .px(theme::scaled_px(12.))
        } else {
            sequencer_stack_button(
                "sequencer-analyze-captured",
                "分析已采样本",
                false,
                window,
                cx,
            )
        }
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "对已采集的 Token 样本执行 FIPS 140-2 与熵统计分析",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.analyze_captured_sequencer_samples(window, cx);
        }));

        let append_btn = sequencer_stack_button(
            "sequencer-append-captured",
            "从剪贴板追加",
            can_edit_config,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "从剪贴板按行追加 Token 样本到已采集列表",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.append_sequencer_captured_from_clipboard(window, cx);
        }));

        let import_btn = sequencer_stack_button(
            "sequencer-import-captured-file",
            "从文件导入…",
            can_edit_config && !self.file_picker_pending(),
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("从本地文本文件按行导入 Token 样本")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.import_sequencer_samples_from_file(false, window, cx);
        }));

        let copy_btn = sequencer_stack_button(
            "sequencer-copy-captured",
            "复制全部样本",
            collected_count > 0,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("复制全部已采集的 Token 样本到剪贴板")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.copy_all_sequencer_samples(window, cx);
        }));

        let export_btn = sequencer_stack_button(
            "sequencer-export-captured-file",
            "导出样本…",
            collected_count > 0 && !self.file_picker_pending(),
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("将当前采集的 Token 样本保存到本地文件")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.export_sequencer_samples_to_file(window, cx);
        }));

        let preview_section = (capturing || collected_count > 0).then(|| {
            div()
                .mt(px(10.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(theme::scaled_px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .child("最近提取样本预览（只读）"),
                )
                .child(
                    div()
                        .id("sequencer-preview-box")
                        .test_support()
                        .h(px(120.))
                        .max_w(px(658.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .child(
                            Editor::new(&pane.preview_samples)
                                .readonly(true)
                                .bordered(false)
                                .aria_label("Sequencer 最近提取样本预览")
                                .size_full(),
                        ),
                )
        });

        sequencer_section(
            "sequencer-help-progress",
            "实时采样进度与样本帮助",
            "查看实时采样请求计数、提取进度，以及停止采样、执行分析或导入/导出样本操作。",
            "实时采样进度与样本",
            None,
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .id("sequencer-capture-metrics")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(stats_summary.clone())
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(14.))
                        .text_size(theme::scaled_px(12.5))
                        .text_color(theme::foreground())
                        .child(format!("已发送请求：{}", pane.capture.requests_sent))
                        .child(format!("已提取 Token：{collected_count}"))
                        .child(format!("未命中：{}", pane.capture.extraction_misses))
                        .child(format!("错误：{}", pane.capture.request_errors))
                        .child(format!("进度：{progress_pct:.1}%")),
                )
                .child(
                    div()
                        .w_full()
                        .max_w(px(658.))
                        .h(px(5.))
                        .rounded(px(2.))
                        .bg(rgb(if dark { 0x35383d } else { 0xe8e8e8 }))
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(gpui_kit::relative((progress_pct / 100.0) as f32))
                                .bg(theme::accent()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .child(stop_btn)
                        .child(analyze_btn)
                        .child(append_btn)
                        .child(import_btn)
                        .child(copy_btn)
                        .child(export_btn),
                )
                .children(preview_section)
                .into_any_element(),
        )
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_sequencer_live(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.sequencer_pane;
        let can_operate = self.sequencer_can_operate();
        let capturing = pane.running.is_some();
        let analyzing = pane.pending_analysis.is_some();
        let can_edit_config = can_operate && !capturing;
        let request_readonly = !can_edit_config || pane.request.is_readonly_preview();
        let drawer_open = pane.request_drawer_open;
        let has_selected_request = pane.selected_request_seq.is_some();
        let can_start_capture = can_edit_config && (has_selected_request || drawer_open);

        let target_total = pane.capture.target_samples.max(1);
        let collected_count = if capturing {
            pane.capture.tokens_collected
        } else {
            pane.captured_samples
                .len()
                .max(pane.capture.tokens_collected)
        };
        let progress_pct =
            ((collected_count as f64 / target_total as f64) * 100.0).clamp(0.0, 100.0);
        let can_analyze_captured = can_operate
            && !capturing
            && !analyzing
            && pane.captured_samples.len() >= MIN_SAMPLE_COUNT;

        let section_request = self.render_sequencer_live_request_section(
            can_edit_config,
            drawer_open,
            has_selected_request,
            request_readonly,
            can_start_capture,
            window,
            cx,
        );
        let location_enabled = can_edit_config && (has_selected_request || drawer_open);
        let section_location = self.render_sequencer_live_location_section(
            can_edit_config,
            location_enabled,
            window,
            cx,
        );
        let show_capture_panel = capturing
            || collected_count > 0
            || pane.capture.requests_sent > 0
            || pane.capture_panel_open;
        let section_progress = show_capture_panel.then(|| {
            self.render_sequencer_live_progress_section(
                can_edit_config,
                capturing,
                can_analyze_captured,
                collected_count,
                progress_pct,
                window,
                cx,
            )
        });

        div()
            .id("sequencer-live-controls")
            .test_support()
            .size_full()
            .bg(theme::surface())
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .child(section_request)
            .child(sequencer_divider())
            .child(section_location)
            .when_some(section_progress, |col, progress| {
                col.child(sequencer_divider()).child(progress)
            })
            .into_any_element()
    }

    pub(in crate::workbench) fn render_sequencer_manual(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.sequencer_pane;
        let dark = theme::is_dark();
        let can_operate = self.sequencer_can_operate();
        let analyzing = pane.pending_analysis.is_some();
        let editable = can_operate && !analyzing;
        let raw_text = pane.manual_editor.read(cx).value();
        let stats = compute_manual_stats(raw_text.as_ref());
        let summary = format!(
            "有效行数：{} · 最小长度：{} B · 最大长度：{} B · 总计：{} B",
            stats.count, stats.min_len, stats.max_len, stats.total_bytes
        );

        let paste_btn = sequencer_stack_button(
            "sequencer-paste-clipboard",
            "粘贴",
            editable,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("从剪贴板按行追加样本").build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.paste_sequencer_manual_clipboard(window, cx);
        }));

        let load_btn = sequencer_stack_button(
            "sequencer-import-manual-file",
            "从文件加载…",
            editable && !self.file_picker_pending(),
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("从本地文件加载 Token 样本到手动列表")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.import_sequencer_samples_from_file(true, window, cx);
        }));

        let copy_btn = sequencer_stack_button(
            "sequencer-copy-manual",
            "复制全部",
            editable && stats.count > 0,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("复制全部手动样本到剪贴板").build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.copy_all_sequencer_samples(window, cx);
        }));

        let export_btn = sequencer_stack_button(
            "sequencer-export-manual-file",
            "导出样本…",
            editable && stats.count > 0 && !self.file_picker_pending(),
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("将手动样本保存到本地文本文件")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.export_sequencer_samples_to_file(window, cx);
        }));

        let clear_btn = sequencer_stack_button(
            "sequencer-clear-samples",
            "清空",
            editable,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("清空手动样本列表").build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.clear_sequencer_manual_samples(window, cx);
        }));

        let analyze_btn = sequencer_primary_button(
            "sequencer-analyze-manual",
            "立即统计分析",
            editable,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("对当前样本执行随机性统计分析")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.analyze_manual_sequencer_samples(window, cx);
        }));

        let editor_border: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xd0d0d0).into()
        };

        div()
            .id("sequencer-manual-root")
            .test_support()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(theme::surface())
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(20.))
                            .pt(px(1.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(sequencer_help_icon(
                                "sequencer-help-manual",
                                "手动加载帮助",
                                "每行输入一条 Token（自动忽略首尾空行），支持从剪贴板或文本文件批量加载样本并执行统计分析。",
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(theme::scaled_px(14.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if dark {
                                        theme::foreground()
                                    } else {
                                        rgb(0x000000).into()
                                    })
                                    .child("手动加载 Token 样本"),
                            )
                            .child(
                                div()
                                    .mt(px(5.))
                                    .text_size(theme::ui_font_size())
                                    .text_color(theme::foreground())
                                    .child("从剪贴板粘贴或从文件加载待测 Token（每行一条，自动忽略首尾空行），然后点击“立即统计分析”。"),
                            )
                            .child(
                                div()
                                    .mt(px(10.))
                                    .flex_1()
                                    .min_h(px(240.))
                                    .w_full()
                                    .flex()
                                    .items_stretch()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(px(104.))
                                            .flex_shrink_0()
                                            .flex()
                                            .flex_col()
                                            .gap(px(6.))
                                            .child(paste_btn)
                                            .child(load_btn)
                                            .child(copy_btn)
                                            .child(export_btn)
                                            .child(clear_btn),
                                    )
                                    .child(
                                        div()
                                            .id("sequencer-manual-editor")
                                            .test_support()
                                            .flex_1()
                                            .min_w_0()
                                            .min_h_0()
                                            .border_1()
                                            .border_color(editor_border)
                                            .bg(theme::surface())
                                            .on_action(cx.listener(
                                                |this, _: &ClearSequencerManualEditor, window, cx| {
                                                    cx.stop_propagation();
                                                    this.clear_sequencer_manual_samples(window, cx);
                                                },
                                            ))
                                            .child(
                                                Editor::new(&pane.manual_editor)
                                                    .readonly(!editable)
                                                    .bordered(false)
                                                    .aria_label("Sequencer 手动加载样本编辑器")
                                                    .context_menu(
                                                        sequencer_manual_editor_context_menu(
                                                            pane.manual_editor.clone(),
                                                            !editable,
                                                        ),
                                                    )
                                                    .size_full(),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .mt(px(12.))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(14.))
                                    .child(analyze_btn)
                                    .child(
                                        div()
                                            .id("sequencer-manual-stats")
                                            .test_support()
                                            .role(Role::Status)
                                            .aria_label(summary.clone())
                                            .text_size(theme::scaled_px(12.5))
                                            .text_color(theme::muted_foreground())
                                            .child(summary),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
