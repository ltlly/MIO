use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

use super::*;
use crate::workbench::wb_render_chrome::tool_tab_with_icon;

fn find_echoed_payload_labels(
    response_bytes: &[u8],
    payloads: &[IntruderPayloadPreview],
) -> Vec<String> {
    const MAX_SCAN: usize = mio_core::intruder::MAX_REQUEST_BYTES;
    let raw = &response_bytes[..response_bytes.len().min(MAX_SCAN)];
    let mut seen = HashSet::new();
    let mut echoed = Vec::new();
    for payload in payloads {
        if payload.raw_bytes == 0 || payload.prefix.is_empty() {
            continue;
        }
        let needle = payload.prefix.as_slice();
        if !seen.insert(needle.to_vec()) {
            continue;
        }
        if needle.len() <= raw.len() && raw.windows(needle.len()).any(|window| window == needle) {
            echoed.push(payload_preview(payload));
        }
    }
    echoed
}

fn format_combined_grep_and_echo_summary(
    grep_summary: Option<String>,
    echoed: &[String],
) -> Option<String> {
    match (grep_summary, echoed.is_empty()) {
        (None, true) => None,
        (Some(summary), true) => Some(summary),
        (Some(summary), false) => Some(format!("{summary} | 回显: {}", echoed.join(", "))),
        (None, false) => Some(format!("载荷回显 (Grep - Payloads): {}", echoed.join(", "))),
    }
}

impl IntruderResultsWindow {
    fn render_highlight_swatches(
        &self,
        ordinal: Option<u64>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (current, can_annotate) = ordinal
            .and_then(|ord| {
                let wb = self.target.workbench.upgrade()?;
                let wb_ref = wb.read(cx);
                let view = wb_ref.intruder.results.views.get(&self.target.view_id)?;
                let hl = view
                    .query
                    .view_row(ord)
                    .and_then(|row| row.highlight)
                    .filter(|c| (1..=9).contains(c))
                    .unwrap_or(0);
                let ok =
                    results::results_annotations::can_annotate_result(wb_ref, &self.target, ord);
                Some((hl, ok))
            })
            .unwrap_or((0, false));
        let mut row = div().flex().items_center().gap(px(4.));
        for color in 0u8..=9 {
            let name = results::results_annotations::HIGHLIGHT_NAMES[usize::from(color)];
            let tip = if color == 0 {
                "清除当前选中结果行的颜色高亮 (None)".to_owned()
            } else {
                format!("将当前选中结果行标记为 {name} 高亮")
            };
            let target = self.target.clone();
            let active = ordinal.is_some() && current == color;
            let swatch = Button::new(("intruder-result-swatch", usize::from(color)))
                .ghost()
                .compact()
                .w(px(16.))
                .h(px(16.))
                .p_0()
                .rounded(px(8.))
                .accessibility_label(format!("Swatch {name}"))
                .tooltip(tip)
                .disabled(!can_annotate)
                .border_1()
                .when(color == 0 && !active, |this| {
                    this.border_dashed().border_color(theme::control_border())
                })
                .border_color(if active {
                    theme::accent()
                } else if color == 0 {
                    theme::control_border()
                } else {
                    theme::border()
                })
                .when_some(
                    crate::history::highlight_background(color),
                    |this, background| this.bg(background),
                )
                .on_click(move |_, window, cx| {
                    if let Some(ord) = ordinal {
                        target.update(window, cx, |workbench, _, cx| {
                            workbench.set_intruder_result_highlight(target.view_id, ord, color, cx);
                        });
                    }
                });
            row = row.child(swatch);
        }
        row.into_any_element()
    }

    fn copy_results_tsv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((tsv, count)) = self.table.read(cx).delegate().export_loaded_rows_tsv(cx) else {
            self.target.update(window, cx, |workbench, _, cx| {
                workbench.fail("当前没有可导出的已加载 Intruder 结果行。", cx);
            });
            return;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(tsv));
        self.target.update(window, cx, |workbench, _, cx| {
            workbench.status = format!("已复制 {count} 条 Intruder 结果（TSV）到剪贴板。");
            cx.notify();
        });
    }

    fn export_results_to_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((tsv, _)) = self
            .table
            .read(cx)
            .delegate()
            .export_loaded_rows_delimited(false, cx)
        else {
            self.target.update(window, cx, |workbench, _, cx| {
                workbench.fail("当前没有可导出的已加载 Intruder 结果行。", cx);
            });
            return;
        };
        let csv_pair = self
            .table
            .read(cx)
            .delegate()
            .export_loaded_rows_delimited(true, cx);
        let Some(owner) = self.target.workbench.upgrade() else {
            return;
        };
        let (can_open, directory, project) = {
            let wb = owner.read(cx);
            (
                wb.backend_available() && wb.transition.is_none() && !wb.file_picker_pending(),
                wb.default_directory(),
                wb.project,
            )
        };
        if !can_open {
            return;
        }
        let picker = Uuid::new_v4();
        owner.update(cx, |wb, cx| {
            wb.file_picker = Some(picker);
            cx.notify();
        });
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &directory,
            Some("intruder-results.csv"),
        );
        let target = self.target.clone();
        cx.spawn_in(window, async move |_, cx| {
            let result = prompt.await;
            let _ = target.owner.update(cx, |_, window, cx| {
                target.update(window, cx, |wb, _, cx| {
                    if !wb.finish_file_picker(picker, cx) || !wb.backend_available() {
                        return;
                    }
                    if wb.project != project {
                        wb.fail("项目已切换，本次结果导出未写入。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(path))) => {
                            let use_csv = path
                                .extension()
                                .is_none_or(|ext| !ext.eq_ignore_ascii_case("tsv"));
                            let (payload, count) = if use_csv {
                                csv_pair.unwrap_or((tsv, 0))
                            } else {
                                let cnt = tsv.lines().count().saturating_sub(1);
                                (tsv, cnt)
                            };
                            match mio_core::ui_files::write_export_file(&path, payload.as_bytes()) {
                                Ok(()) => {
                                    wb.error = None;
                                    wb.status = format!(
                                        "已导出 {count} 条 Intruder 结果到 {}。",
                                        path.display()
                                    );
                                }
                                Err(err) => {
                                    wb.fail(format!("导出 Intruder 结果失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => wb.fail(format!("无法选择结果导出路径：{err}"), cx),
                        Err(_) => wb.fail("文件选择器已意外关闭。", cx),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn render_results(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (
            state,
            message_view,
            pretty_active,
            render_active,
            non_printable_active,
            soft_wrap_active,
            render_card,
            detail_error,
            page_error,
            retention,
            selected,
            filter_label,
            grep_summary,
            can_copy_tsv,
            search_bar,
        ) = {
            let Some(view) = self.target.view(cx) else {
                return div().into_any_element();
            };
            let view_id = self.target.view_id;
            let response = self.show_response;
            let message = if self.show_response {
                &view.response
            } else {
                &view.request
            };
            let state = message.state();
            let message_view = message.view();
            let pretty_active = message.pretty_active();
            let render_active = response && message.render_active;
            let non_printable_active = message.show_non_printable();
            let soft_wrap_active = message.soft_wrap;
            let detail_error = view.detail_error.clone();
            let page_error = view.query.error.clone().or_else(|| view.error.clone());
            let retention = view
                .summary
                .as_ref()
                .filter(|_| !view.discarded)
                .and_then(|run| retention_note(run.retention, view.persisted));
            let selected = view.selected;
            let filter_label = result_filter_label(&view.query.applied);
            let echoed_inputs = match (selected, view.detail_response_raw.as_deref()) {
                (Some(ordinal), Some(raw)) => view
                    .query
                    .view_row(ordinal)
                    .map(|row| find_echoed_payload_labels(raw, &row.payloads))
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            let grep_summary = format_combined_grep_and_echo_summary(
                view.detail_grep_summary.clone(),
                &echoed_inputs,
            );
            let can_copy_tsv = !view.discarded
                && view
                    .query
                    .page
                    .as_ref()
                    .is_some_and(|page| !page.rows.is_empty());
            let search_input = message.search.clone();
            let case_sensitive = message.case_sensitive();
            let regex = message.regex();
            let has_error =
                message.has_search_error() || message.search_status_label(cx) == "正则无效";
            let status = message.search_status_label(cx);
            (
                state,
                message_view,
                pretty_active,
                render_active,
                non_printable_active,
                soft_wrap_active,
                None::<AnyElement>,
                detail_error,
                page_error,
                retention,
                selected,
                filter_label,
                grep_summary,
                can_copy_tsv,
                (
                    view_id,
                    response,
                    search_input,
                    case_sensitive,
                    regex,
                    has_error,
                    status,
                ),
            )
        };
        let render_card = if render_active && selected.is_some() {
            let view_id = self.target.view_id;
            self.target
                .workbench
                .upgrade()
                .and_then(|wb| {
                    wb.update(cx, |workbench, cx| {
                        let result = workbench.intruder.results.views.get(&view_id)?;
                        Some(workbench.render_response_visual_card(
                            "intruder-result-response",
                            &result.response,
                            cx,
                        ))
                    })
                })
                .or(render_card)
        } else {
            None
        };
        let target = self.target.clone();
        let mut body =
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    toolbar()
                        .child(
                            div().flex_1().min_w_0().child(
                                button("intruder-result-view-filter", filter_label)
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::ListFilter)
                                    .tooltip("按 HTTP 状态码类别、搜索关键词/正则表达式、高亮颜色或备注筛选结果行")
                                    .on_click(move |_, window, cx| {
                                        target.update(window, cx, |workbench, window, cx| {
                                            workbench.open_intruder_result_filter(
                                                target.view_id,
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            ),
                        )
                        .child(self.render_highlight_swatches(selected, cx))
                        .child(
                            button("intruder-copy-results-tsv", "复制结果 (TSV)")
                                .icon(IconName::Copy)
                                .tooltip("将当前已加载的 Intruder 结果行导出为制表符分隔文本 (TSV) 复制到剪贴板")
                                .disabled(!can_copy_tsv)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.copy_results_tsv(window, cx)
                                })),
                        )
                        .child(
                            button("intruder-export-results-file", "导出结果到文件…")
                                .icon(IconName::Download)
                                .tooltip("将当前已加载的攻击结果行导出保存为本地 CSV 或 TSV 文件")
                                .disabled(!can_copy_tsv)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.export_results_to_file(window, cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .id("intruder-results-table")
                        .test_support()
                        .h(px(260.))
                        .min_h(px(100.))
                        .child(
                            DataTable::new(&self.table)
                                .with_size(theme::row_height())
                                .header_row_height(theme::table_header_height())
                                .bordered(false)
                                .scroll_bounce(false)
                                .accessibility_label("Intruder 结果"),
                        ),
                );
        if let Some(error) = page_error {
            body = body.child(
                toolbar()
                    .id("intruder-result-read-error")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Alert)
                    .aria_label(error.clone())
                    .text_color(theme::danger_fg())
                    .child(error),
            );
        }
        if let Some(note) = retention {
            body = body.child(
                div()
                    .id("intruder-retention-note")
                    .test_support()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(8.))
                    .py(px(6.))
                    .role(gpui_kit::accesskit::Role::Status)
                    .aria_label(note)
                    .child(note.test_text(note)),
            );
        }
        body.child(
            toolbar()
                .gap_0()
                .px_0()
                .border_t_1()
                .border_color(theme::border())
                .children(
                [
                    (
                        false,
                        "请求",
                        IconName::ArrowUpRight,
                        "查看所选攻击结果实际发出的展开后 HTTP 请求报文",
                    ),
                    (
                        true,
                        "响应",
                        IconName::ArrowDownLeft,
                        "查看所选攻击结果收到的目标服务器 HTTP 响应报文",
                    ),
                ]
                .into_iter()
                .map(|(response, label, icon, tip)| {
                    tool_tab_with_icon(
                        if response {
                            "intruder-result-response"
                        } else {
                            "intruder-result-request"
                        },
                        label,
                        Some(icon),
                        self.show_response == response,
                        window,
                        cx,
                    )
                    .h(theme::toolbar_height())
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(view) = this.target.view(cx) {
                            view.focus.clone().focus(window, cx);
                        }
                        this.show_response = response;
                        cx.notify();
                    }))
                }),
            ),
        )
        .child(
            toolbar()
                .child(
                    button(
                        if self.show_response {
                            "intruder-result-response-pretty"
                        } else {
                            "intruder-result-request-pretty"
                        },
                        "Pretty",
                    )
                    .icon(IconName::Sparkles)
                    .tooltip("自动美化并结构化展示 JSON、XML 或表单报文正文")
                    .selected(pretty_active && !render_active)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let view_id = this.target.view_id;
                        let response = this.show_response;
                        this.target.update(window, cx, |workbench, window, cx| {
                            let result =
                                workbench.intruder.results.views.get_mut(&view_id).unwrap();
                            let editor = if response {
                                &mut result.response
                            } else {
                                &mut result.request
                            };
                            result.detail_error = editor.switch_pretty(window, cx).err();
                            cx.notify();
                        });
                    })),
                )
                .children(
                    [
                        (
                            MessageView::Raw,
                            "Raw",
                            IconName::Code,
                            "查看未经格式化的原始 HTTP 报文文本",
                        ),
                        (
                            MessageView::Hex,
                            "Hex",
                            IconName::Binary,
                            "以十六进制字节偏移视图检查原始报文数据",
                        ),
                    ]
                    .into_iter()
                    .map(|(view, label, icon, tip)| {
                        button(
                            SharedString::from(format!(
                                "intruder-result-{}",
                                label.to_ascii_lowercase()
                            )),
                            label,
                        )
                        .icon(icon)
                        .tooltip(tip)
                        .selected(!pretty_active && !render_active && message_view == view)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let view_id = this.target.view_id;
                                let response = this.show_response;
                                this.target.update(window, cx, |workbench, window, cx| {
                                    let result =
                                        workbench.intruder.results.views.get_mut(&view_id).unwrap();
                                    let editor = if response {
                                        &mut result.response
                                    } else {
                                        &mut result.request
                                    };
                                    result.detail_error = editor.switch(view, window, cx).err();
                                    cx.notify();
                                });
                            },
                        ))
                    }),
                )
                .when(self.show_response, |bar| {
                    bar.child(
                        button("intruder-result-response-render", "Render")
                            .icon(IconName::Eye)
                            .tooltip("安全离线页面渲染预览（提取 HTML 结构、超链接、表单控件与图片元数据，绝不发起外部请求）")
                            .selected(render_active)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let view_id = this.target.view_id;
                                this.target.update(window, cx, |workbench, window, cx| {
                                    if let Some(result) =
                                        workbench.intruder.results.views.get_mut(&view_id)
                                    {
                                        result.detail_error =
                                            result.response.switch_render(window, cx).err();
                                        cx.notify();
                                    }
                                });
                            })),
                    )
                })
                .child(
                    button(
                        if self.show_response {
                            "intruder-result-response-nonprintable"
                        } else {
                            "intruder-result-request-nonprintable"
                        },
                        "\\r\\n",
                    )
                    .selected(non_printable_active)
                    .tooltip("切换可视化显示非打印字符（如 \\r\\n、制表符与控制字符）")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let view_id = this.target.view_id;
                        let response = this.show_response;
                        this.target.update(window, cx, |workbench, window, cx| {
                            if let Some(result) =
                                workbench.intruder.results.views.get_mut(&view_id)
                            {
                                let editor = if response {
                                    &mut result.response
                                } else {
                                    &mut result.request
                                };
                                result.detail_error = editor.toggle_non_printable(window, cx).err();
                                cx.notify();
                            }
                        });
                    })),
                )
                .child(
                    Button::new(if self.show_response {
                        "intruder-result-response-wrap"
                    } else {
                        "intruder-result-request-wrap"
                    })
                    .small()
                    .h(theme::control_height())
                    .px(theme::scaled_px(7.))
                    .icon(IconName::TextWrap)
                    .accessibility_label("自动换行")
                    .selected(soft_wrap_active)
                    .tooltip("切换结果详情编辑器长行自动软换行显示")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let view_id = this.target.view_id;
                        let response = this.show_response;
                        this.target.update(window, cx, |workbench, window, cx| {
                            if let Some(result) =
                                workbench.intruder.results.views.get_mut(&view_id)
                            {
                                let editor = if response {
                                    &mut result.response
                                } else {
                                    &mut result.request
                                };
                                editor.toggle_soft_wrap(window, cx);
                                cx.notify();
                            }
                        });
                    })),
                ),
        )
        .when_some(detail_error, |this, error| {
            this.child(toolbar().text_color(theme::danger_fg()).child(error))
        })
        .when_some(grep_summary, |this, summary| {
            this.child(
                toolbar()
                    .id("intruder-result-grep-summary")
                    .aria_label(summary.clone())
                    .test_support()
                    .child(summary),
            )
        })
        .child(div().flex_1().min_h_0().child(if selected.is_some() {
            let (view_id, response, search_input, case_sensitive, regex, has_error, status) =
                search_bar;
            let id = if response {
                "intruder-result-response"
            } else {
                "intruder-result-request"
            };
            let run_action = move |this: &mut Self,
                                   action: editing::SearchBarAction,
                                   window: &mut Window,
                                   cx: &mut Context<Self>| {
                this.target.update(window, cx, |workbench, window, cx| {
                    let result = workbench.intruder.results.views.get_mut(&view_id).unwrap();
                    let editor = if response {
                        &mut result.response
                    } else {
                        &mut result.request
                    };
                    result.detail_error = editor.handle_search_bar_action(action, window, cx).err();
                    cx.notify();
                });
            };
            let search_bar = div()
                .id(SharedString::from(format!("{id}-search-bar")))
                .min_h(theme::scaled_px(34.).max(px(34.)))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .border_t_1()
                .border_color(theme::border())
                .bg(theme::toolbar_surface())
                .child(
                    button(SharedString::from(format!("{id}-search-prev")), "‹")
                        .tooltip("定位到报文中的上一个搜索匹配项")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            run_action(this, editing::SearchBarAction::Previous, window, cx);
                        })),
                )
                .child(
                    button(SharedString::from(format!("{id}-search-next")), "›")
                        .tooltip("定位到报文中的下一个搜索匹配项")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            run_action(this, editing::SearchBarAction::Next, window, cx);
                        })),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-search-input")))
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&search_input).small()),
                )
                .child(
                    button(SharedString::from(format!("{id}-search-case")), "Aa")
                        .tooltip("切换报文搜索是否区分大小写 (Case sensitive)")
                        .selected(case_sensitive)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            run_action(
                                this,
                                editing::SearchBarAction::ToggleCaseSensitive,
                                window,
                                cx,
                            );
                        })),
                )
                .child(
                    button(SharedString::from(format!("{id}-search-regex")), ".*")
                        .tooltip("切换报文搜索是否启用正则表达式匹配 (Regex)")
                        .selected(regex)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            run_action(this, editing::SearchBarAction::ToggleRegex, window, cx);
                        })),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-search-matches")))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Status)
                        .aria_label(status.clone())
                        .text_color(if has_error {
                            theme::danger_fg()
                        } else {
                            theme::foreground()
                        })
                        .child(status),
                );
            div()
                .size_full()
                .bg(theme::surface())
                .flex()
                .flex_col()
                .child(
                    div().flex_1().min_h_0().child(if let Some(card) = render_card {
                        card
                    } else {
                        Editor::new(&state)
                            .readonly(true)
                            .size_full()
                            .context_menu(self.target.editor_context_menu(state.clone()))
                            .aria_label(if response {
                                "Intruder 结果响应消息编辑器"
                            } else {
                                "Intruder 结果请求消息编辑器"
                            })
                            .into_any_element()
                    }),
                )
                .child(search_bar)
                .into_any_element()
        } else {
            div()
                .size_full()
                .bg(theme::surface())
                .p(px(16.))
                .text_color(theme::muted_foreground())
                .child("选择结果查看请求和响应。正文按需读取。")
                .into_any_element()
        }))
        .into_any_element()
    }
}
