use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::size;
use mio_core::IntruderRunState;

use super::*;
use crate::workbench::wb_render_chrome::{sidebar_rail, sidebar_rail_tab, tool_tab_with_icon};

impl IntruderResultsWindow {
    fn change_result_pool_form(
        &mut self,
        action: resource_pools::PoolFormAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pool_create_op.is_some() || !self.pool_form.creating {
            return;
        }
        if self.pool_form.has_composition(window, cx) {
            self.pool_create_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        match action {
            resource_pools::PoolFormAction::Flag(flag, value) => match flag {
                resource_pools::PoolFlag::Cap => self.pool_form.cap_enabled = value,
                resource_pools::PoolFlag::Delay => self.pool_form.delay_enabled = value,
                resource_pools::PoolFlag::Auto => self.pool_form.automatic = value,
                resource_pools::PoolFlag::Status429 => self.pool_form.status_429 = value,
                resource_pools::PoolFlag::Status503 => self.pool_form.status_503 = value,
                resource_pools::PoolFlag::Other => self.pool_form.other_enabled = value,
            },
            resource_pools::PoolFormAction::Delay(kind) => self.pool_form.delay_kind = kind,
        }
        self.pool_create_error = None;
        cx.notify();
    }

    fn submit_new_result_pool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pool_create_op.is_some() || !self.pool_form.creating {
            return;
        }
        let Some(owner) = self.target.workbench.upgrade() else {
            return;
        };
        if !owner.read(cx).can_change_intruder_pool(self.target.view_id) {
            return;
        }
        let pool = match self.pool_form.snapshot_preserving_inputs(window, cx) {
            Ok(pool) => pool,
            Err(err) => {
                self.pool_create_error = Some(err);
                cx.notify();
                return;
            }
        };
        let operation = Uuid::new_v4();
        let pool_id = pool.id;
        let result = owner.update(cx, |wb, cx| {
            wb.submit_result_window_new_pool(self.target.project, operation, pool, cx)
        });
        match result {
            Ok(()) => {
                self.pool_create_op = Some((operation, pool_id));
                self.pool_create_error = None;
            }
            Err(err) => self.pool_create_error = Some(err),
        }
        cx.notify();
    }

    pub(in crate::workbench) fn render_result_pool(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(owner) = self.target.workbench.upgrade() else {
            return div().into_any_element();
        };
        let owner = owner.read(cx);
        let pools = &owner.intruder.resource_pools;
        let view = &owner.intruder.results.views[&self.target.view_id];
        let legacy = view
            .summary
            .as_ref()
            .is_some_and(|run| run.resource_pool.is_none());
        let can_change = owner.can_change_intruder_pool(self.target.view_id)
            && self.pool_create_op.is_none()
            && !pools.management.mutation_pending();
        let create = self.pool_form.creating;
        let callback = cx.weak_entity();
        let change: resource_pools::PoolFormChange = std::rc::Rc::new(move |action, window, cx| {
            let _ = callback.update(cx, |this, cx| {
                this.change_result_pool_form(action, window, cx)
            });
        });
        div()
            .id("intruder-result-pool-panel")
            .test_support()
            .w(px(420.))
            .min_w(px(280.))
            .max_w(px(480.))
            .flex_shrink(1.0)
            .h_full()
            .bg(theme::surface())
            .border_l_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(14.))
                    .py(px(8.))
                    .flex_shrink_0()
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.5))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("Resource pool"),
                    )
                    .child(
                        button("intruder-result-pool-refresh", "刷新")
                            .icon(IconName::RefreshCw)
                            .tooltip("重新从后台读取当前项目的资源池目录与并发配置")
                            .disabled(!owner.backend_available() || pools.pending.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.target.update(window, cx, |wb, _, cx| {
                                    wb.query_intruder_resource_pools(cx);
                                });
                            })),
                    ),
            )
            .child(
                div()
                    .id("intruder-result-pool-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::muted_foreground())
                            .child("多个任务共享资源池的并发与请求间隔设置。"),
                    )
                    .child(
                        resource_pools::pool_radio(
                            "intruder-result-existing-pool",
                            "使用现有资源池",
                            !create,
                            can_change,
                        )
                        .on_change({
                            let callback = cx.weak_entity();
                            move |checked, _, window, cx| {
                                if checked {
                                    let _ = callback.update(cx, |this, cx| {
                                        if this.pool_form.has_composition(window, cx) {
                                            this.pool_create_error =
                                                Some(editing::COMPOSITION_PENDING.into());
                                        } else {
                                            this.pool_form.creating = false;
                                            this.pool_create_error = None;
                                        }
                                        cx.notify();
                                    });
                                }
                            }
                        }),
                    )
                    .when(legacy, |this| {
                        this.child(
                            div()
                                .text_color(theme::muted_foreground())
                                .child("此旧任务保持独立并发；选择资源池后应用共享设置。"),
                        )
                    })
                    .when(pools.pending.is_some(), |this| {
                        this.child(
                            div()
                                .text_color(theme::muted_foreground())
                                .child("正在读取资源池…"),
                        )
                    })
                    .when_some(pools.error.clone(), |this, error| {
                        this.child(div().text_color(theme::danger_fg()).child(error))
                    })
                    .child(
                        div()
                            .h(theme::scaled_px(190.).max(px(190.)))
                            .flex_shrink_0()
                            .child(
                                DataTable::new(&self.pool_table)
                                    .with_size(theme::row_height())
                                    .header_row_height(theme::table_header_height())
                                    .bordered(true)
                                    .scroll_bounce(false)
                                    .accessibility_label("Intruder 资源池"),
                            ),
                    )
                    .when(owner.intruder_pool_change_pending(view.run_id), |this| {
                        this.child(
                            div()
                                .text_color(theme::muted_foreground())
                                .child("正在确认资源池切换…"),
                        )
                    })
                    .when_some(view.pool_error.clone(), |this, error| {
                        this.child(
                            div()
                                .id("intruder-pool-error")
                                .test_support()
                                .role(gpui_kit::accesskit::Role::Alert)
                                .aria_label(error.clone())
                                .text_color(theme::danger_fg())
                                .child(error),
                        )
                    })
                    .child(
                        resource_pools::pool_radio(
                            "intruder-result-create-pool",
                            "创建新资源池",
                            create,
                            can_change,
                        )
                        .on_change({
                            let callback = cx.weak_entity();
                            move |checked, _, window, cx| {
                                if checked {
                                    let _ = callback.update(cx, |this, cx| {
                                        if this.pool_form.has_composition(window, cx) {
                                            this.pool_create_error =
                                                Some(editing::COMPOSITION_PENDING.into());
                                        } else {
                                            this.pool_form.creating = true;
                                            this.pool_create_error = None;
                                            this.pool_form.id = Uuid::new_v4();
                                        }
                                        cx.notify();
                                    });
                                }
                            }
                        }),
                    )
                    .when(create, |this| {
                        this.child(resource_pools::render_pool_form(
                            &self.pool_form,
                            can_change,
                            false,
                            change,
                        ))
                        .child(
                            button("intruder-result-create-pool-submit", "创建并绑定资源池")
                                .primary()
                                .icon(IconName::Plus)
                                .tooltip("保存新资源池配置并将当前攻击会话绑定到此资源池")
                                .disabled(!can_change)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_new_result_pool(window, cx)
                                })),
                        )
                    })
                    .when(self.pool_create_op.is_some(), |this| {
                        this.child(
                            div()
                                .text_color(theme::muted_foreground())
                                .child("正在创建并确认资源池…"),
                        )
                    })
                    .when_some(self.pool_create_error.clone(), |this, error| {
                        this.child(
                            div()
                                .id("intruder-result-create-pool-error")
                                .test_support()
                                .role(gpui_kit::accesskit::Role::Alert)
                                .aria_label(error.clone())
                                .text_color(theme::danger_fg())
                                .child(error),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_result_inspector(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.target.view(cx) else {
            return div().into_any_element();
        };
        let header = div()
            .px(px(14.))
            .py(px(10.))
            .flex_shrink_0()
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .text_size(theme::scaled_px(13.5))
            .font_weight(gpui_kit::FontWeight::BOLD)
            .child("Inspector");
        let mut body = div()
            .id("intruder-results-inspector-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.));
        let Some(detail) = view.detail.as_ref().filter(|_| view.selected.is_some()) else {
            return div()
                .id("intruder-results-inspector-panel")
                .test_support()
                .w(px(340.))
                .min_w(px(260.))
                .max_w(px(420.))
                .flex_shrink(1.0)
                .h_full()
                .bg(theme::surface())
                .border_l_1()
                .border_color(theme::border())
                .flex()
                .flex_col()
                .child(header)
                .child(
                    body.child(
                        div()
                            .text_size(theme::scaled_px(12.5))
                            .text_color(theme::muted_foreground())
                            .child("选择攻击结果查看请求属性、参数、Cookies 与协议头字段。"),
                    ),
                )
                .into_any_element();
        };
        let request_raw = request_to_raw(&detail.request);
        let response_raw = detail.response.as_ref().map(response_to_raw);
        let data = mio_core::inspect_http_messages(&request_raw, response_raw.as_deref());
        let groups = [
            ("请求属性", data.attributes),
            ("Query 参数", data.query_params),
            ("Body 参数", data.body_params),
            ("Cookies", data.cookies),
            ("请求头", data.request_headers),
            ("响应头", data.response_headers),
        ];
        for (idx, (label, items)) in groups.into_iter().enumerate() {
            let mut section = div().flex().flex_col().gap(px(3.)).child(
                button(
                    ("intruder-results-inspector-section", idx),
                    format!("{label} ({})", items.len()),
                )
                .w_full()
                .justify_start()
                .selected(self.inspector_expanded[idx])
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.inspector_expanded[idx] = !this.inspector_expanded[idx];
                    cx.notify();
                })),
            );
            if self.inspector_expanded[idx] {
                for (item_idx, item) in items.into_iter().enumerate() {
                    let name_copy = item.name.clone();
                    let value_copy = item.value.clone();
                    let pair_copy = format!("{}: {}", item.name, item.value);
                    section = section.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .p(px(5.))
                            .border_b_1()
                            .border_color(theme::border())
                            .child(div().text_color(theme::muted_foreground()).child(item.name))
                            .child(div().whitespace_normal().child(item.value))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .child(
                                        button(
                                            SharedString::from(format!(
                                                "intruder-inspector-copy-name-{idx}-{item_idx}"
                                            )),
                                            "复制名称",
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        name_copy.clone(),
                                                    ),
                                                )
                                            },
                                        ),
                                    )
                                    .child(
                                        button(
                                            SharedString::from(format!(
                                                "intruder-inspector-copy-value-{idx}-{item_idx}"
                                            )),
                                            "复制内容",
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        value_copy.clone(),
                                                    ),
                                                )
                                            },
                                        ),
                                    )
                                    .child(
                                        button(
                                            SharedString::from(format!(
                                                "intruder-inspector-copy-pair-{idx}-{item_idx}"
                                            )),
                                            "复制键值",
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        pair_copy.clone(),
                                                    ),
                                                )
                                            },
                                        ),
                                    ),
                            ),
                    );
                }
            }
            body = body.child(section);
        }
        let candidate = editing::pick_selection_candidate(
            [
                view.request.selection_candidate(false, None, cx),
                view.response.selection_candidate(false, None, cx),
            ]
            .into_iter()
            .flatten(),
        );
        if let Some(candidate) = candidate {
            let (decoded, chain_label) = match std::str::from_utf8(&candidate.raw_bytes) {
                Ok(text) => {
                    let res = mio_core::decode_inspector_selection(text);
                    let chain = res.chain_label();
                    (res.decoded, chain)
                }
                Err(_) => match editing::smart_decode_bytes(&candidate.raw_bytes, 512 * 1024) {
                    Ok((bytes, steps)) => {
                        let text = std::str::from_utf8(&bytes)
                            .map(str::to_owned)
                            .unwrap_or_else(|_| editing::hex_encode(&bytes));
                        (text, steps.join(" → "))
                    }
                    Err(err) => (err, "解码错误".into()),
                },
            };
            let copy_decoded = decoded.clone();
            body = body.child(
                div()
                    .mt(px(8.))
                    .border_t_1()
                    .border_color(theme::border())
                    .pt(px(8.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(format!("选区自动解码 · {chain_label}")),
                    )
                    .child(div().whitespace_normal().child(decoded))
                    .child(
                        button("intruder-results-inspector-copy-decoded", "复制解码内容").on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    copy_decoded.clone(),
                                ))
                            },
                        ),
                    ),
            );
        }
        div()
            .id("intruder-results-inspector-panel")
            .test_support()
            .w(px(340.))
            .min_w(px(260.))
            .max_w(px(420.))
            .flex_shrink(1.0)
            .h_full()
            .bg(theme::surface())
            .border_l_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn save_result_sidebar_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ordinal, revision)) = self.notes_loaded_target else {
            return;
        };
        if self.notes_pending.is_some() {
            return;
        }
        if self.notes_input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            self.notes_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let text = self.notes_input.read(cx).value().to_string();
        if self.notes_truncated && text == self.notes_initial {
            self.notes_error = Some("当前显示备注预览；请修改内容或清空后保存。".into());
            cx.notify();
            return;
        }
        let mut result = Err("结果窗口所属工作台已关闭。".into());
        self.target.update(window, cx, |wb, _, cx| {
            result =
                wb.submit_intruder_result_notes(self.target.view_id, ordinal, revision, text, cx);
        });
        match result {
            Ok(operation) => {
                self.notes_pending = Some(operation);
                self.notes_error = None;
            }
            Err(err) => self.notes_error = Some(err),
        }
        cx.notify();
    }

    fn render_result_notes(&self, cx: &mut Context<Self>) -> AnyElement {
        let annotate = self.notes_loaded_target.is_some_and(|(ord, _)| {
            self.target.workbench.upgrade().is_some_and(|owner| {
                results::results_annotations::can_annotate_result(owner.read(cx), &self.target, ord)
            })
        }) && self.notes_pending.is_none();
        div()
            .id("intruder-results-notes-panel")
            .test_support()
            .w(px(340.))
            .min_w(px(260.))
            .max_w(px(420.))
            .flex_shrink(1.0)
            .h_full()
            .bg(theme::surface())
            .border_l_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .flex_shrink_0()
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(13.5))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("Notes"),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when(self.notes_loaded_target.is_none(), |this| {
                        this.child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child("选择攻击结果行查看和保存备注。"),
                        )
                    })
                    .when(self.notes_truncated, |this| {
                        this.child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child("当前仅显示备注预览；修改后保存会替换完整备注。"),
                        )
                    })
                    .child(
                        div()
                            .id("intruder-result-notes-input")
                            .test_support()
                            .flex_1()
                            .min_h(px(120.))
                            .child(
                                gpui_kit::component::input::Textarea::new(&self.notes_input)
                                    .readonly(!annotate)
                                    .size_full()
                                    .bordered(true)
                                    .aria_label("当前攻击结果备注编辑器"),
                            ),
                    )
                    .when_some(self.notes_error.clone(), |this, error| {
                        this.child(
                            div()
                                .id("intruder-result-notes-error")
                                .test_support()
                                .role(gpui_kit::accesskit::Role::Alert)
                                .aria_label(error.clone())
                                .text_color(theme::danger_fg())
                                .child(error),
                        )
                    })
                    .child(
                        button("intruder-result-notes-save", "保存备注")
                            .primary()
                            .icon(IconName::Save)
                            .disabled(!annotate)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_result_sidebar_notes(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for IntruderResultsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.target.view(cx) else {
            return div().into_any_element();
        };
        let focus = view.focus.clone();
        let summary = view.summary.clone();
        let status = if view.discarded {
            "已丢弃".to_owned()
        } else {
            summary.as_ref().map_or_else(
                || {
                    if view.pending.is_some() {
                        "正在读取运行状态…"
                    } else {
                        "运行状态未确认"
                    }
                    .to_owned()
                },
                run_progress,
            )
        };
        let pending = view.control_pending.is_some();
        let stopped = view.stop_requested;
        let error = view.control_error.clone();
        let close_status = view.close_status;
        let close_error = view.close_error.clone();
        let paused = summary
            .as_ref()
            .is_some_and(|run| run.state == IntruderRunState::Paused);
        let enabled = self.target.workbench.upgrade().is_some_and(|workbench| {
            let workbench = workbench.read(cx);
            !workbench.backend_disconnected
                && workbench.transition.is_none()
                && !workbench.intruder_result_close_pending(self.target.run_id)
                && workbench
                    .intruder
                    .runs
                    .get(&self.target.run_id)
                    .is_some_and(|run| {
                        !run.cancel.is_cancelled()
                            && matches!(
                                run.summary.state,
                                IntruderRunState::Running | IntruderRunState::Paused
                            )
                    })
        }) && !pending
            && !stopped;
        let window_title = summary
            .as_ref()
            .filter(|run| !run.title.is_empty())
            .map_or_else(|| "Intruder — 结果".to_owned(), |run| run.title.clone());
        let close_target = self.target.clone();
        let title_bar = crate::chrome::render_title_bar(
            "mio-intruder-results",
            window_title,
            crate::chrome::TitleBarControls::FULL,
            move |window, cx| close_target.request_close(window, cx),
            window,
            cx,
        );
        let content = if self.show_positions {
            self.render_positions(cx)
        } else {
            self.render_results(window, cx)
        };
        div()
            .id("intruder-results-window")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_color(theme::foreground())
            .text_size(theme::ui_font_size())
            .key_context("MioWorkspace")
            .track_focus(&focus)
            .on_action(cx.listener(|this, _: &OpenInIntruder, window, cx| {
                cx.stop_propagation();
                this.target.transfer_selected(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenInRepeater, window, cx| {
                cx.stop_propagation();
                this.target.transfer_selected(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Search, window, cx| {
                cx.stop_propagation();
                if this.show_positions {
                    return;
                }
                let view_id = this.target.view_id;
                let response = this.show_response;
                this.target.update(window, cx, |workbench, window, cx| {
                    let result = workbench.intruder.results.views.get_mut(&view_id).unwrap();
                    if result.selected.is_none() {
                        return;
                    }
                    let editor = if response {
                        &mut result.response
                    } else {
                        &mut result.request
                    };
                    result.detail_error = editor.focus_search(window, cx).err();
                    cx.notify();
                });
            }))
            .on_action(
                cx.listener(|this, action: &TransferIntruderResult, window, cx| {
                    cx.stop_propagation();
                    if action.project != this.target.project
                        || action.view_id != this.target.view_id
                        || action.run_id != this.target.run_id
                        || action.owner != this.target.owner
                        || action.editor_id != this.target.editor_id
                    {
                        return;
                    }
                    match &action.entry {
                        Ok(entry) => {
                            this.target
                                .transfer_result(entry.clone(), action.intruder, window, cx)
                        }
                        Err(error) => this.target.update(window, cx, |workbench, _, cx| {
                            workbench.fail(error.clone(), cx)
                        }),
                    }
                }),
            )
            .on_action(
                cx.listener(|this, action: &ResultDetailAction, window, cx| {
                    cx.stop_propagation();
                    if action.project != this.target.project
                        || action.view_id != this.target.view_id
                        || action.run_id != this.target.run_id
                        || action.owner != this.target.owner
                        || action.editor_id != this.target.editor_id
                    {
                        return;
                    }
                    let op = match action.kind {
                        ResultDetailActionKind::Decoder => result_table::menu::RowOp::Decoder,
                        ResultDetailActionKind::ComparerRequest => {
                            result_table::menu::RowOp::Comparer(false)
                        }
                        ResultDetailActionKind::ComparerResponse => {
                            result_table::menu::RowOp::Comparer(true)
                        }
                        ResultDetailActionKind::Organiser => result_table::menu::RowOp::Organiser,
                        ResultDetailActionKind::RecordIssue => {
                            result_table::menu::RowOp::RecordIssue
                        }
                        ResultDetailActionKind::SendToMioAi => {
                            result_table::menu::RowOp::SendToMioAi
                        }
                        ResultDetailActionKind::CopyUrl => result_table::menu::RowOp::CopyUrl,
                        ResultDetailActionKind::CopyCurl => result_table::menu::RowOp::CopyCurl,
                        ResultDetailActionKind::GenerateCsrfPoc => {
                            result_table::menu::RowOp::GenerateCsrfPoc
                        }
                        ResultDetailActionKind::SaveItem => result_table::menu::RowOp::SaveItem,
                    };
                    result_table::menu::run_row_op(this.target.clone(), None, op, window, cx);
                }),
            )
            .when(self.detached, |this| {
                this.on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                    this.target.request_close(window, cx);
                }))
            })
            .child(title_bar)
            .child(
                toolbar().gap_0().px_0().children(
                    [
                        (
                            false,
                            "结果",
                            IconName::List,
                            "浏览攻击请求结果列表、Grep 提取命中与请求/响应详情",
                        ),
                        (
                            true,
                            "Positions",
                            IconName::Crosshair,
                            "查看启动本次攻击时冻结的请求模板与 § 载荷标记位快照",
                        ),
                    ]
                    .into_iter()
                    .map(|(positions, label, icon, tip)| {
                        tool_tab_with_icon(
                            if positions {
                                "intruder-results-positions"
                            } else {
                                "intruder-results-results"
                            },
                            label,
                            Some(icon),
                            self.show_positions == positions,
                            window,
                            cx,
                        )
                        .h(theme::toolbar_height())
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                        })
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.select_page(positions, window, cx),
                        ))
                    }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(content),
                    )
                    .when(self.show_settings, |this| {
                        this.child(self.render_result_settings(cx))
                    })
                    .when(self.show_pool, |this| {
                        this.child(self.render_result_pool(cx))
                    })
                    .when(self.show_inspector, |this| {
                        this.child(self.render_result_inspector(cx))
                    })
                    .when(self.show_notes, |this| {
                        this.child(self.render_result_notes(cx))
                    })
                    .child(
                        sidebar_rail()
                            .child(
                                sidebar_rail_tab(
                                    "intruder-results-resource-pool",
                                    "资源池",
                                    "资源池",
                                    IconName::SlidersHorizontal,
                                    self.show_pool,
                                    false,
                                    "展开或收起当前攻击任务的运行时资源池切换面板",
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        if let Some(view) = this.target.view(cx) {
                                            view.focus.clone().focus(window, cx);
                                        }
                                        this.show_pool = !this.show_pool;
                                        this.show_settings = false;
                                        this.show_inspector = false;
                                        this.show_notes = false;
                                        if this.show_pool {
                                            this.target.update(window, cx, |view, _, cx| {
                                                view.query_intruder_resource_pools(cx)
                                            });
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                sidebar_rail_tab(
                                    "intruder-results-settings",
                                    "设置",
                                    "设置",
                                    IconName::Settings,
                                    self.show_settings,
                                    false,
                                    "展开或收起当前攻击任务的项目持久化与基准请求设置面板",
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        if let Some(view) = this.target.view(cx) {
                                            view.focus.clone().focus(window, cx);
                                        }
                                        this.show_settings = !this.show_settings;
                                        this.show_pool = false;
                                        this.show_inspector = false;
                                        this.show_notes = false;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                sidebar_rail_tab(
                                    "intruder-results-inspector",
                                    "Inspector",
                                    "Inspector",
                                    IconName::SlidersHorizontal,
                                    self.show_inspector,
                                    false,
                                    "查看当前选中结果请求属性、参数、Cookies 与选区自动解码",
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        if let Some(view) = this.target.view(cx) {
                                            view.focus.clone().focus(window, cx);
                                        }
                                        this.show_inspector = !this.show_inspector;
                                        this.show_settings = false;
                                        this.show_pool = false;
                                        this.show_notes = false;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                sidebar_rail_tab(
                                    "intruder-results-notes",
                                    "Notes",
                                    "Notes",
                                    IconName::FileText,
                                    self.show_notes,
                                    false,
                                    "查看与保存当前选中攻击结果行的备注",
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.show_notes = !this.show_notes;
                                        this.show_settings = false;
                                        this.show_pool = false;
                                        this.show_inspector = false;
                                        this.refresh(window, cx);
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                toolbar()
                    .border_b_0()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        Button::new("intruder-result-pause")
                            .small()
                            .ghost()
                            .icon(if paused {
                                IconName::Play
                            } else {
                                IconName::Pause
                            })
                            .accessibility_label(if paused {
                                "恢复 Intruder"
                            } else {
                                "暂停 Intruder"
                            })
                            .tooltip(if paused {
                                "恢复发送剩余的 Intruder 攻击请求"
                            } else {
                                "暂停后续载荷派发（已在途的请求将继续接收响应）"
                            })
                            .disabled(!enabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let view_id = this.target.view_id;
                                this.target.update(window, cx, |view, _, cx| {
                                    view.set_intruder_paused(view_id, !paused, cx)
                                });
                            })),
                    )
                    .when(enabled, |bar| {
                        bar.child(
                            button("intruder-result-stop", "停止攻击")
                                .icon(IconName::Square)
                                .tooltip("停止当前攻击派发并取消仍在执行的请求")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let run_id = this.target.run_id;
                                    this.target.update(window, cx, |view, _, cx| {
                                        view.stop_intruder_run(run_id, cx);
                                    });
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("intruder-run-progress")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Status)
                            .aria_label(status.clone())
                            .child(status),
                    )
                    .when(pending, |this| this.child("等待运行控制确认…"))
                    .when_some(close_status, |this, status| this.child(status)),
            )
            .when_some(error, |this, error| {
                this.child(toolbar().text_color(theme::danger_fg()).child(error))
            })
            .when_some(close_error, |this, error| {
                this.child(toolbar().text_color(theme::danger_fg()).child(error))
            })
            .into_any_element()
    }
}

impl Workbench {
    pub(in crate::workbench) fn open_intruder_results_window(
        &mut self,
        run_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let weak = cx.weak_entity();
        cx.defer(move |cx| {
            let Some(workbench) = weak.upgrade() else {
                return;
            };
            if workbench.read(cx).project != Some(project) {
                return;
            }
            if let Some(view) = workbench.read(cx).intruder.results.views.get(&run_id) {
                let owner = view.owner;
                if crate::chrome::activate_popup_window(owner, cx) {
                    return;
                }
                workbench.update(cx, |view, cx| view.forget_intruder_results(run_id, cx));
            }
            let scale = theme::ui_scale();
            let target_w = if scale > 1.0 {
                theme::scaled_px(1100.)
            } else {
                px(1100.)
            };
            let content_h = if scale > 1.0 {
                theme::scaled_px(760.)
            } else {
                px(760.)
            };
            let desired_h = if cfg!(test) {
                content_h
            } else {
                let bar_h = if scale > 1.0 {
                    theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                        .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
                } else {
                    px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                };
                content_h + bar_h
            };
            let bounds = crate::fit_window_bounds(size(target_w, desired_h), cx);
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, true, true),
                cx,
                |window, cx| {
                    cx.new(|cx| IntruderResultsWindow::new(&workbench, run_id, window, cx))
                },
            );
            if let Err(error) = opened {
                workbench.update(cx, |view, cx| {
                    view.fail(
                        format!("Intruder 结果窗口未打开：{error}；运行继续由后台管理。"),
                        cx,
                    );
                });
            }
        });
    }
}
