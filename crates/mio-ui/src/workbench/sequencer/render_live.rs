use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement;

use super::widgets::{
    sequencer_divider, sequencer_list_button, sequencer_outline_button, sequencer_primary_button,
    sequencer_radio, sequencer_section, sequencer_stack_button, sequencer_table_header_bg,
};
use super::*;

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

impl SequencerView {
    #[inline(never)]
    pub(super) fn render_live(
        &self,
        wb_entity: &Entity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (
            can_operate,
            capturing,
            analyzing,
            request_readonly,
            has_selected_request,
            collected_count,
            progress_pct,
            can_analyze_captured,
            has_capture_activity,
        ) = {
            let wb = wb_entity.read(cx);
            let pane = &wb.sequencer_pane;
            let can_operate = wb.sequencer_can_operate();
            let capturing = pane.running.is_some();
            let analyzing = pane.pending_analysis.is_some();
            let can_edit_config = can_operate && !capturing;
            let request_readonly = !can_edit_config || pane.request.is_readonly_preview();
            let has_selected_request = pane.selected_request_seq.is_some();
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
            let has_capture_activity =
                capturing || collected_count > 0 || pane.capture.requests_sent > 0;
            (
                can_operate,
                capturing,
                analyzing,
                request_readonly,
                has_selected_request,
                collected_count,
                progress_pct,
                can_analyze_captured,
                has_capture_activity,
            )
        };
        let _ = analyzing;
        let can_edit_config = can_operate && !capturing;
        let drawer_open = self.request_drawer_open;
        let can_start_capture = can_edit_config && (has_selected_request || drawer_open);

        let section_request = self.render_live_request_section(
            wb_entity,
            can_edit_config,
            drawer_open,
            has_selected_request,
            request_readonly,
            can_start_capture,
            window,
            cx,
        );
        let location_enabled = can_edit_config && (has_selected_request || drawer_open);
        let section_location = self.render_live_location_section(
            wb_entity,
            can_edit_config,
            location_enabled,
            window,
            cx,
        );
        let show_capture_panel = has_capture_activity || self.capture_panel_open;
        let section_progress = show_capture_panel.then(|| {
            self.render_live_progress_section(
                wb_entity,
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

    #[inline(never)]
    fn render_live_request_drawer(
        &self,
        wb_entity: &Entity<Workbench>,
        can_edit_config: bool,
        has_selected_request: bool,
        request_readonly: bool,
        table_h: gpui_kit::Pixels,
        table_border: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (target_url, request_version, pretty, view, editor_elem) = {
            let wb = wb_entity.read(cx);
            let pane = &wb.sequencer_pane;
            let editor_elem = editing::message_editor(&pane.request)
                .readonly(request_readonly)
                .size_full()
                .context_menu(wb.request_context_menu(pane.request.state(), request_readonly))
                .aria_label("Sequencer 采样源请求编辑器")
                .into_any_element();
            (
                pane.target_url.clone(),
                pane.request_version,
                pane.request.pretty_active(),
                pane.request.view(),
                editor_elem,
            )
        };
        let search_bar = wb_entity.update(cx, |wb, cx| {
            wb.sequencer_pane.request.render_search_bar_editable(
                "sequencer-request-editor",
                request_readonly,
                cx,
                |this, action, window, cx| {
                    this.handle_sequencer_search_action(action, window, cx);
                },
            )
        });
        let add_current_btn = (!has_selected_request).then(|| {
            button("sequencer-request-add-current", "加入列表")
                .tooltip("将当前编辑的目标 URL 与请求添加到实时捕获列表")
                .disabled(!can_edit_config)
                .on_click(cx.listener(|this, _, window, cx| {
                    let _ = this
                        .workbench
                        .update(cx, |wb, cx| wb.add_current_sequencer_request_to_list(window, cx));
                    cx.notify();
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
                            Input::new(&target_url)
                                .id("sequencer-target-url")
                                .aria_label("Sequencer 目标 URL")
                                .small()
                                .disabled(!can_edit_config),
                        ),
                    )
                    .child(
                        button("sequencer-version-http11", "HTTP/1.1")
                            .tooltip("使用 HTTP/1.1 协议发送采样请求")
                            .selected(request_version != HttpVersion::Http2)
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.workbench.update(cx, |wb, _| {
                                    wb.sequencer_pane.request_version = HttpVersion::Http11;
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        button("sequencer-version-http2", "HTTP/2")
                            .tooltip("使用 HTTP/2 协议发送采样请求")
                            .selected(request_version == HttpVersion::Http2)
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.workbench.update(cx, |wb, _| {
                                    wb.sequencer_pane.request_version = HttpVersion::Http2;
                                });
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
                                let _ = this.workbench.update(cx, |wb, cx| {
                                    wb.switch_sequencer_request_pretty(window, cx)
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        button("sequencer-request-editor-raw", "Raw")
                            .tooltip("查看或编辑采样源请求原始文本")
                            .selected(!pretty && view == MessageView::Raw)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let _ = this.workbench.update(cx, |wb, cx| {
                                    wb.switch_sequencer_request_view(MessageView::Raw, window, cx)
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        button("sequencer-request-editor-hex", "Hex")
                            .tooltip("以十六进制视图查看或编辑请求字节")
                            .selected(!pretty && view == MessageView::Hex)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let _ = this.workbench.update(cx, |wb, cx| {
                                    wb.switch_sequencer_request_view(MessageView::Hex, window, cx)
                                });
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("sequencer-delim-from-selection", "从选区生成分隔符")
                            .icon(IconName::Scissors)
                            .tooltip("根据请求编辑器当前选区前后文本自动填充起止分隔符")
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.fill_delimiters_from_selection(window, cx);
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
                    .child(editor_elem),
            )
            .child(search_bar)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn render_live_request_section(
        &self,
        wb_entity: &Entity<Workbench>,
        can_edit_config: bool,
        drawer_open: bool,
        has_selected_request: bool,
        request_readonly: bool,
        can_start_capture: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let (has_any_request, request_rows_data) = {
            let wb = wb_entity.read(cx);
            let pane = &wb.sequencer_pane;
            let live_target_text = pane.target_url.read(cx).value().to_string();
            let live_req_text = pane.request.state().read(cx).value().to_string();
            let rows: Vec<(usize, String, String, bool)> = pane
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
            (!pane.live_requests.is_empty(), rows)
        };

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
            this.remove_selected_request(window, cx);
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
            this.clear_requests(window, cx);
        }));

        let mut request_table_body = div()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .flex()
            .flex_col();
        let row_menu_owner = self.workbench.clone();
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
                        let _ = this.workbench.update(cx, |wb, cx| {
                            wb.select_sequencer_live_request(seq, window, cx)
                        });
                        cx.notify();
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
                        this.request_drawer_open = !this.request_drawer_open;
                        cx.notify();
                    }))
                    .child(if drawer_open { "◂" } else { "▸" }),
            );

        let request_drawer = drawer_open.then(|| {
            self.render_live_request_drawer(
                wb_entity,
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
            this.start_capture(window, cx);
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
    fn render_live_location_section(
        &self,
        wb_entity: &Entity<Workbench>,
        can_edit_config: bool,
        location_enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let location_kind = wb_entity.read(cx).sequencer_pane.location_kind;
        let dark = theme::is_dark();
        let is_custom = matches!(
            location_kind,
            LocationKind::BodyDelimiter | LocationKind::Regex | LocationKind::Header
        );
        let custom_summary = if !location_enabled {
            String::new()
        } else {
            match location_kind {
                LocationKind::Regex => {
                    let pattern = self.regex_pattern.read(cx).value();
                    if pattern.trim().is_empty() {
                        String::new()
                    } else {
                        format!("正则: {pattern}")
                    }
                }
                LocationKind::Header => {
                    let header = self.header_name.read(cx).value();
                    if header.trim().is_empty() {
                        String::new()
                    } else {
                        format!("响应头: {header}")
                    }
                }
                _ => {
                    let start = self.delim_start.read(cx).value();
                    let end = self.delim_end.read(cx).value();
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

        let cookie_checked = location_enabled && location_kind == LocationKind::Cookie;
        let form_field_checked = location_enabled && location_kind == LocationKind::FormField;
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
            let _ = this.workbench.update(cx, |wb, _| {
                wb.sequencer_pane.location_kind = LocationKind::Cookie;
            });
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
            let _ = this.workbench.update(cx, |wb, _| {
                wb.sequencer_pane.location_kind = LocationKind::FormField;
            });
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
            let _ = this.workbench.update(cx, |wb, _| {
                if !matches!(
                    wb.sequencer_pane.location_kind,
                    LocationKind::BodyDelimiter | LocationKind::Regex | LocationKind::Header
                ) {
                    wb.sequencer_pane.location_kind = LocationKind::BodyDelimiter;
                }
            });
            this.custom_config_open = true;
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
            let was_custom = this
                .workbench
                .update(cx, |wb, _| {
                    let is_c = matches!(
                        wb.sequencer_pane.location_kind,
                        LocationKind::BodyDelimiter | LocationKind::Regex | LocationKind::Header
                    );
                    if !is_c {
                        wb.sequencer_pane.location_kind = LocationKind::BodyDelimiter;
                    }
                    is_c
                })
                .unwrap_or(false);
            if !was_custom {
                this.custom_config_open = true;
            } else {
                this.custom_config_open = !this.custom_config_open;
            }
            cx.notify();
        }));

        let combo_input_or_idle = |active_id: &'static str,
                                   idle_id: &'static str,
                                   aria: &'static str,
                                   input: &Entity<InputState>,
                                   target_kind: LocationKind| {
            if location_enabled {
                div()
                    .w(px(340.))
                    .flex_shrink_0()
                    .child(
                        Input::new(input)
                            .id(active_id)
                            .aria_label(aria)
                            .small()
                            .disabled(!can_edit_config || location_kind != target_kind)
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
                    .id(idle_id)
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
            }
        };

        let cookie_field = combo_input_or_idle(
            "sequencer-cookie-name",
            "sequencer-cookie-combobox-idle",
            "Cookie 名称",
            &self.cookie_name,
            LocationKind::Cookie,
        );
        let form_field_box = combo_input_or_idle(
            "sequencer-form-field-name",
            "sequencer-form-field-combobox-idle",
            "表单字段名称",
            &self.form_field_name,
            LocationKind::FormField,
        );

        let custom_config_panel = (location_enabled && is_custom && self.custom_config_open)
            .then(|| self.render_custom_location_panel(location_kind, can_edit_config, cx));

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

    fn render_custom_location_panel(
        &self,
        location_kind: LocationKind,
        can_edit_config: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let labeled_input = |label: &'static str, input: &Entity<InputState>, id: &'static str, aria: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .w(theme::scaled_px(88.))
                        .flex_shrink_0()
                        .text_size(theme::scaled_px(12.5))
                        .child(label),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(input)
                            .id(id)
                            .aria_label(aria)
                            .small()
                            .disabled(!can_edit_config),
                    ),
                )
        };
        let mode_inputs = match location_kind {
            LocationKind::Regex => labeled_input(
                "正则表达式：",
                &self.regex_pattern,
                "sequencer-regex-pattern",
                "Token 提取正则表达式",
            )
            .into_any_element(),
            LocationKind::Header => labeled_input(
                "Header 名称：",
                &self.header_name,
                "sequencer-header-name",
                "Header 名称",
            )
            .into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(labeled_input(
                    "起始分隔符：",
                    &self.delim_start,
                    "sequencer-delim-start",
                    "起始分隔符",
                ))
                .child(labeled_input(
                    "结束分隔符：",
                    &self.delim_end,
                    "sequencer-delim-end",
                    "结束分隔符",
                ))
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
                            .selected(location_kind == LocationKind::BodyDelimiter)
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.workbench.update(cx, |wb, _| {
                                    wb.sequencer_pane.location_kind = LocationKind::BodyDelimiter;
                                });
                                this.custom_config_open = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("sequencer-loc-regex", "正则提取 Regex")
                            .tooltip("使用正则表达式从完整 HTTP 响应中提取首个捕获组或完整匹配")
                            .selected(location_kind == LocationKind::Regex)
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.workbench.update(cx, |wb, _| {
                                    wb.sequencer_pane.location_kind = LocationKind::Regex;
                                });
                                this.custom_config_open = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("sequencer-loc-header", "响应头 Header")
                            .tooltip("从指定 HTTP 响应头字段中提取 Token")
                            .selected(location_kind == LocationKind::Header)
                            .disabled(!can_edit_config)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.workbench.update(cx, |wb, _| {
                                    wb.sequencer_pane.location_kind = LocationKind::Header;
                                });
                                this.custom_config_open = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(mode_inputs)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn render_live_progress_section(
        &self,
        wb_entity: &Entity<Workbench>,
        can_edit_config: bool,
        capturing: bool,
        can_analyze_captured: bool,
        collected_count: usize,
        progress_pct: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let (
            requests_sent,
            extraction_misses,
            request_errors,
            file_picker_pending,
            preview_samples,
        ) = {
            let wb = wb_entity.read(cx);
            let pane = &wb.sequencer_pane;
            (
                pane.capture.requests_sent,
                pane.capture.extraction_misses,
                pane.capture.request_errors,
                wb.file_picker_pending(),
                pane.preview_samples.clone(),
            )
        };
        let stats_summary = format!(
            "已发送 {requests_sent} · 已提取 {collected_count} · 未命中 {extraction_misses} · 错误 {request_errors} · 进度 {progress_pct:.1}%"
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
            let _ = this
                .workbench
                .update(cx, |wb, cx| wb.stop_sequencer_capture(cx));
            cx.notify();
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
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.analyze_captured_sequencer_samples(window, cx)
            });
            cx.notify();
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
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.append_sequencer_captured_from_clipboard(window, cx)
            });
            cx.notify();
        }));

        let import_btn = sequencer_stack_button(
            "sequencer-import-captured-file",
            "从文件导入…",
            can_edit_config && !file_picker_pending,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("从本地文本文件按行导入 Token 样本")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.import_sequencer_samples_from_file(false, window, cx)
            });
            cx.notify();
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
            let _ = this
                .workbench
                .update(cx, |wb, cx| wb.copy_all_sequencer_samples(window, cx));
            cx.notify();
        }));

        let export_btn = sequencer_stack_button(
            "sequencer-export-captured-file",
            "导出样本…",
            collected_count > 0 && !file_picker_pending,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("将当前采集的 Token 样本保存到本地文件")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this
                .workbench
                .update(cx, |wb, cx| wb.export_sequencer_samples_to_file(window, cx));
            cx.notify();
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
                            Editor::new(&preview_samples)
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
                        .child(format!("已发送请求：{requests_sent}"))
                        .child(format!("已提取 Token：{collected_count}"))
                        .child(format!("未命中：{extraction_misses}"))
                        .child(format!("错误：{request_errors}"))
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
}
