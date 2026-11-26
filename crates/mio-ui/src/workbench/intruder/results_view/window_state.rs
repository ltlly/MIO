use gpui_kit::assets::IconName;
use gpui_kit::component::{checkbox::Checkbox, input::Textarea, table::Column};
use mio_core::{IntruderResultFilter, IntruderRetention};

use super::*;

pub(in crate::workbench::intruder) struct IntruderResultsWindow {
    pub(super) target: ResultTarget,
    pub(super) detached: bool,
    pub(super) table: Entity<TableState<ResultTable>>,
    pub(super) pool_table: Entity<TableState<ResultPoolTable>>,
    pub(super) pool_form: resource_pools::CreatePoolForm,
    pub(super) pool_create_op: Option<(Uuid, Uuid)>,
    pub(super) pool_create_error: Option<String>,
    pub(super) positions: positions::PositionsEditor,
    pub(super) positions_revision: Option<u64>,
    pub(super) show_positions: bool,
    pub(super) show_response: bool,
    pub(super) show_settings: bool,
    pub(super) show_pool: bool,
    pub(super) show_inspector: bool,
    pub(super) show_notes: bool,
    pub(super) inspector_expanded: [bool; 6],
    pub(super) notes_input: Entity<gpui_kit::component::input::TextareaState>,
    pub(super) notes_loaded_target: Option<(u64, u64)>,
    pub(super) notes_truncated: bool,
    pub(super) notes_initial: String,
    pub(super) notes_pending: Option<Uuid>,
    pub(super) notes_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

pub(super) fn result_filter_label(filter: &IntruderResultFilter) -> String {
    let mut parts = Vec::new();
    if filter.status_classes != [true; 4] {
        parts.push("按状态码".to_owned());
    }
    if filter.only_with_notes {
        parts.push("有备注".to_owned());
    }
    if filter.only_highlighted {
        parts.push("已高亮".to_owned());
    }
    if filter.only_grep_matched {
        parts.push("仅显示 Grep 命中项".to_owned());
    }
    if !filter.search_term.is_empty() {
        parts.push(if filter.negative_search {
            format!("排除表达式 {}", filter.search_term)
        } else {
            format!("匹配表达式 {}", filter.search_term)
        });
    }
    if parts.is_empty() {
        "查看筛选：显示全部".into()
    } else {
        format!("查看筛选：{}", parts.join("；"))
    }
}

impl IntruderResultsWindow {
    pub(super) fn new(
        workbench: &Entity<Workbench>,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_view(workbench, run_id, run_id, true, window, cx)
    }

    pub(in crate::workbench::intruder) fn embedded(
        workbench: &Entity<Workbench>,
        view_id: Uuid,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        assert_ne!(view_id, run_id);
        Self::with_view(workbench, view_id, run_id, false, window, cx)
    }

    fn with_view(
        workbench: &Entity<Workbench>,
        view_id: Uuid,
        run_id: Uuid,
        detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (project, editor_id, focus) = workbench.update(cx, |view, cx| {
            if detached {
                view.attach_intruder_result_window(run_id, window, cx);
            } else {
                view.attach_intruder_result_view(view_id, run_id, window, cx);
            }
            let result = &view.intruder.results.views[&view_id];
            (
                view.project.unwrap(),
                result.request.raw.entity_id(),
                result.focus.clone(),
            )
        });
        let target = ResultTarget {
            workbench: workbench.downgrade(),
            project,
            view_id,
            run_id,
            owner: window.window_handle(),
            editor_id,
        };
        let table = cx.new(|cx| {
            TableState::new(
                ResultTable {
                    target: target.clone(),
                    columns: ResultTable::base_columns(),
                },
                window,
                cx,
            )
            .col_selectable(false)
        });
        let observation = cx.observe_in(workbench, window, |this, _, window, cx| {
            this.refresh(window, cx);
        });
        let pool_table = cx.new(|cx| {
            TableState::new(
                ResultPoolTable {
                    target: target.clone(),
                    columns: vec![
                        Column::new("selected", "选中").width(44.),
                        Column::new("name", "资源池").width(160.),
                        Column::new("concurrent", "并发请求").width(92.),
                        Column::new("delay", "请求间隔").width(92.),
                        Column::new("random", "随机间隔").width(88.),
                        Column::new("increment", "间隔增量").width(92.),
                        Column::new("automatic", "自动节流").width(88.),
                    ],
                },
                window,
                cx,
            )
            .sortable(false)
            .col_selectable(false)
            .row_selectable(false)
        });
        let selection = cx.subscribe_in(&table, window, |this, _, event, window, cx| {
            if let TableEvent::SelectRow(row)
            | TableEvent::DoubleClickedRow(row)
            | TableEvent::RightClickedRow(Some(row)) = event
            {
                let view_id = this.target.view_id;
                this.target.update(window, cx, |view, window, cx| {
                    view.select_intruder_result_rank(view_id, *row, window, cx);
                });
            }
        });
        if detached {
            focus.focus(window, cx);
            let title = target
                .view(cx)
                .and_then(|view| view.summary.as_ref())
                .filter(|run| !run.title.is_empty())
                .map_or("Intruder — 结果", |run| run.title.as_str());
            window.set_window_title(title);
            let close_target = target.clone();
            window.on_window_should_close(cx, move |window, cx| {
                close_target.request_close(window, cx);
                false
            });
        } else {
            cx.on_release(|this, cx| {
                let target = this.target.clone();
                cx.defer(move |cx| {
                    let _ = target.workbench.update(cx, |view, cx| {
                        if target.owns(view) {
                            view.forget_intruder_results(target.view_id, cx);
                        }
                    });
                });
            })
            .detach();
        }
        let notes_input = cx.new(|cx| {
            gpui_kit::component::input::TextareaState::new(window, cx)
                .placeholder("输入当前选中攻击结果行的备注…")
                .soft_wrap(true)
                .autocorrect(false)
        });
        Self {
            target,
            detached,
            table,
            pool_table,
            pool_form: resource_pools::CreatePoolForm::new(None, window, cx),
            pool_create_op: None,
            pool_create_error: None,
            positions: positions::PositionsEditor::new(window, cx),
            positions_revision: None,
            show_positions: false,
            show_response: false,
            show_settings: false,
            show_pool: false,
            show_inspector: false,
            show_notes: false,
            inspector_expanded: [true; 6],
            notes_input,
            notes_loaded_target: None,
            notes_truncated: false,
            notes_initial: String::new(),
            notes_pending: None,
            notes_error: None,
            _subscriptions: vec![observation, selection],
        }
    }

    pub(super) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (
            selected,
            discarded,
            applied_sort,
            detached_title,
            spec,
            max_positions,
            grep_cfg,
            pool_status,
            notes_outcome,
            sync_notes,
        ) = {
            let Some(view) = self.target.view(cx) else {
                if self.detached {
                    window.remove_window();
                } else {
                    cx.notify();
                }
                return;
            };
            let selected = view.selected;
            let discarded = view.discarded;
            let applied_sort = view.query.requested().1;
            let detached_title = self.detached.then(|| {
                view.summary
                    .as_ref()
                    .filter(|run| !run.title.is_empty())
                    .map_or_else(|| "Intruder — 结果".to_owned(), |run| run.title.clone())
            });
            let spec = view
                .spec
                .as_ref()
                .filter(|spec| {
                    self.show_positions && self.positions_revision != Some(spec.revision)
                })
                .cloned();
            let wb_opt = self.target.workbench.upgrade();
            let mut max_pos = view.spec.as_ref().map_or(0, |s| s.positions.len());
            if let Some(page) = &view.query.page {
                for row in &page.rows {
                    max_pos = max_pos.max(row.payloads.len());
                }
                if let Some(sel_row) = &page.selected_row {
                    max_pos = max_pos.max(sel_row.payloads.len());
                }
            }
            let grep_cfg = wb_opt.as_ref().and_then(|wb| {
                let tab_id = view.summary.as_ref()?.tab_id;
                wb.read(cx)
                    .workspace
                    .intruder
                    .grep_configs
                    .get(&tab_id)
                    .cloned()
            });
            let pool_status = match (wb_opt.as_ref(), self.pool_create_op) {
                (Some(wb), Some((op_id, new_pool_id))) => {
                    let pools = &wb.read(cx).intruder.resource_pools;
                    let found = pools
                        .catalog
                        .as_ref()
                        .is_some_and(|c| c.find(new_pool_id).is_some());
                    let still_mine = pools.management.pending_operation() == Some(op_id);
                    let my_outcome = pools.management.outcome_for(op_id);
                    let fallback_err = (!still_mine && !pools.management.mutation_pending())
                        .then(|| pools.error.clone())
                        .flatten();
                    Some((found, still_mine, my_outcome, fallback_err))
                }
                _ => None,
            };
            let notes_outcome = self
                .notes_pending
                .and_then(|op| view.notes_outcomes.get(&op).cloned().map(|res| (op, res)));
            let sync_notes = selected.and_then(|ord| {
                let row = view.query.view_row(ord)?;
                let wb = wb_opt.as_ref()?.read(cx);
                let cached = wb
                    .intruder
                    .results
                    .cached_notes
                    .get(&(view.run_id, ord))
                    .filter(|(rev, _)| *rev == row.notes_revision)
                    .map(|(_, text)| text.clone());
                let truncated = cached.is_none()
                    && (row.notes_preview.len() >= 256 || row.notes_preview.contains('\u{fffd}'));
                let text = cached.unwrap_or_else(|| row.notes_preview.clone());
                Some((ord, row.notes_revision, text, truncated))
            });
            (
                selected,
                discarded,
                applied_sort,
                detached_title,
                spec,
                max_pos,
                grep_cfg,
                pool_status,
                notes_outcome,
                sync_notes,
            )
        };
        if let Some((_, new_pool_id)) = self.pool_create_op
            && let Some((pool_created, still_mine, my_outcome, fallback_err)) = pool_status
        {
            if pool_created {
                self.pool_create_op = None;
                self.pool_form.creating = false;
                self.pool_form.existing = Some(new_pool_id);
                let view_id = self.target.view_id;
                let mut bound = false;
                self.target.update(window, cx, |wb, window, cx| {
                    bound = wb.change_intruder_result_pool(view_id, new_pool_id, window, cx);
                });
                self.pool_create_error = if bound {
                    None
                } else {
                    Some("资源池已创建，但当前结果窗口忙碌，请手动选择该资源池。".into())
                };
            } else if let Some(Err(err)) = my_outcome {
                self.pool_create_op = None;
                self.pool_create_error = Some(err);
            } else if !still_mine {
                self.pool_create_op = None;
                self.pool_create_error = Some(
                    fallback_err.unwrap_or_else(|| {
                        "资源池创建未确认，请检查名称或并发设置后重试。".into()
                    }),
                );
            }
        }
        if let Some(pending_op) = self.notes_pending
            && let Some((actual_op, res)) = notes_outcome
            && actual_op == pending_op
        {
            self.notes_pending = None;
            let target = self.target.clone();
            cx.defer(move |cx| {
                let _ = target.workbench.update(cx, |wb, _| {
                    if let Some(view) = wb.intruder.results.views.get_mut(&target.view_id) {
                        view.notes_outcomes.remove(&actual_op);
                    }
                });
            });
            match res {
                Ok(()) => {
                    self.notes_error = None;
                }
                Err(err) => {
                    self.notes_error = Some(err);
                }
            }
        }
        let notes_composing = self.notes_input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        });
        if !notes_composing && self.notes_pending.is_none() {
            match sync_notes {
                Some((ord, rev, text, truncated)) => {
                    if self.notes_loaded_target != Some((ord, rev)) {
                        let same_ord = self
                            .notes_loaded_target
                            .is_some_and(|(loaded_ord, _)| loaded_ord == ord);
                        let local_dirty = same_ord
                            && self.notes_input.read(cx).value().as_ref() != self.notes_initial;
                        if !local_dirty {
                            self.notes_initial = text.clone();
                            self.notes_truncated = truncated;
                            self.notes_input
                                .update(cx, |state, cx| state.set_value(text, window, cx));
                        }
                        self.notes_loaded_target = Some((ord, rev));
                        if !same_ord {
                            self.notes_error = None;
                        }
                    }
                }
                None => {
                    if self.notes_loaded_target.take().is_some() {
                        self.notes_initial.clear();
                        self.notes_truncated = false;
                        self.notes_error = None;
                        self.notes_input
                            .update(cx, |state, cx| state.set_value("", window, cx));
                    }
                }
            }
        }
        if let Some(title) = detached_title
            && window.window_title() != title.as_str()
        {
            window.set_window_title(&title);
        }
        if discarded && self.positions_revision.take().is_some() {
            self.positions.load(&[], &[], window, cx);
        }
        if let Some(spec) = spec {
            self.positions
                .load(&spec.draft.raw, &spec.positions, window, cx);
            self.positions_revision = Some(spec.revision);
        }
        let range = self.table.read(cx).visible_range().rows().clone();
        self.table.update(cx, |table, cx| {
            table
                .delegate_mut()
                .sync_dynamic_columns(max_positions, grep_cfg.as_ref());
            table.delegate_mut().apply_sort(applied_sort);
            table.refresh(cx);
            if selected.is_none() && table.selected_row().is_some() {
                table.clear_selection(cx);
            } else if let Some(ordinal) = selected {
                let visual = table.delegate().visual_row(ordinal, cx);
                if let Some(visual) = visual {
                    if table.selected_row() != Some(visual) {
                        if table.right_clicked_row().is_some() {
                            table.set_selected_row_preserving_context(visual, cx);
                        } else {
                            table.set_selected_row(visual, cx);
                        }
                    }
                } else if table.selected_row().is_some() {
                    table.clear_selection(cx);
                }
            }
            cx.notify();
        });
        self.pool_table.update(cx, |table, cx| {
            table.refresh(cx);
            cx.notify();
        });
        if !self.show_positions && !range.is_empty() {
            let view_id = self.target.view_id;
            self.target.update(window, cx, |view, window, cx| {
                view.ensure_intruder_result_range(view_id, range, window, cx)
            });
        }
        cx.notify();
    }

    pub(super) fn select_page(
        &mut self,
        positions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_positions = positions;
        let view_id = self.target.view_id;
        self.target.update(window, cx, |workbench, window, cx| {
            let view = &workbench.intruder.results.views[&view_id];
            view.focus.focus(window, cx);
            if positions {
                workbench.select_intruder_run(view_id, window, cx);
            } else if view.pending.is_none() && view.error.is_some() {
                workbench.query_intruder_results(view_id, None, window, cx);
            } else if view.query.pending.is_none() && view.query.error.is_some() {
                let (filter, sort) = view.query.requested();
                let offset = view.query.page.as_ref().map_or(0, |page| page.offset);
                workbench.query_intruder_result_view(view_id, offset, filter, sort, window, cx);
            }
        });
        self.refresh(window, cx);
    }

    pub(super) fn render_positions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.target.view(cx) else {
            return div().into_any_element();
        };
        let mut body = div().flex_1().min_h_0().flex().flex_col();
        if let Some(spec) = &view.spec {
            body = body.child(
                toolbar()
                    .child(render::attack_label(spec.attack_type))
                    .child(spec.draft.target.clone()),
            );
        }
        let error = view.spec_error.clone();
        if view.spec_pending.is_some() {
            body = body.child(toolbar().child("正在读取冻结配置…"));
        }
        if let Some(error) = error {
            return body
                .child(toolbar().text_color(theme::danger_fg()).child(error))
                .into_any_element();
        }
        body.child(
            toolbar().children(
                [
                    (
                        MessageView::Raw,
                        "Raw",
                        IconName::Code,
                        "以原始文本视图查看冻结的请求模板与 § 载荷位置标记",
                    ),
                    (
                        MessageView::Hex,
                        "Hex",
                        IconName::Binary,
                        "以十六进制字节视图查看冻结的请求模板与 § 载荷位置标记",
                    ),
                ]
                .into_iter()
                .map(|(view, label, icon, tip)| {
                    button(
                        SharedString::from(format!(
                            "intruder-frozen-{}",
                            label.to_ascii_lowercase()
                        )),
                        label,
                    )
                    .icon(icon)
                    .tooltip(tip)
                    .selected(self.positions.view == view)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Err(error) = this.positions.switch(view, window, cx) {
                            this.positions.position_error = Some(error);
                        }
                        cx.notify();
                    }))
                }),
            ),
        )
        .child(
            div().flex_1().min_h_0().bg(theme::surface()).child(
                Textarea::new(self.positions.state())
                    .readonly(true)
                    .bordered(false)
                    .size_full()
                    .token(|_, _, _| {
                        div()
                            .px(px(2.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::accent().opacity(0.45))
                            .bg(theme::accent().opacity(0.14))
                            .text_color(theme::accent())
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("§")
                    })
                    .aria_label("Intruder 冻结 Positions"),
            ),
        )
        .when_some(self.positions.position_error.clone(), |this, error| {
            this.child(toolbar().text_color(theme::danger_fg()).child(error))
        })
        .into_any_element()
    }

    pub(super) fn can_save_result_to_project(&self, cx: &App) -> bool {
        let Some(workbench) = self.target.workbench.upgrade() else {
            return false;
        };
        let workbench = workbench.read(cx);
        self.target.owns(workbench)
            && workbench.project_path.is_some()
            && workbench.backend_available()
            && workbench.transition.is_none()
            && !workbench.file_picker_pending()
            && workbench
                .intruder
                .results
                .views
                .get(&self.target.view_id)
                .is_some_and(|result| {
                    !result.discarded
                        && result.summary.as_ref().is_some_and(|run| {
                            run.retention == IntruderRetention::Session
                                && (run.state.is_terminal()
                                    || workbench.can_save_active_intruder_run(run))
                        })
                })
    }

    pub(super) fn render_result_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(result) = self.target.view(cx) else {
            return div().into_any_element();
        };
        let summary = result.summary.as_ref();
        let named = self
            .target
            .workbench
            .upgrade()
            .is_some_and(|view| view.read(cx).project_path.is_some());
        let saved = named && summary.is_some_and(|run| run.retention == IntruderRetention::Project);
        let enabled = self.can_save_result_to_project(cx);
        let baseline = summary.is_some_and(|run| run.baseline.is_some());
        let baseline_available = summary.is_some_and(|run| {
            self.target
                .workbench
                .upgrade()
                .is_some_and(|owner| owner.read(cx).can_request_intruder_baseline(run))
        });
        let baseline_pending = self
            .target
            .workbench
            .upgrade()
            .is_some_and(|owner| owner.read(cx).intruder_baseline_pending(self.target.run_id));
        div()
            .id("intruder-result-settings-panel")
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
                    .py(px(10.))
                    .flex_shrink_0()
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(13.5))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("设置"),
            )
            .child(
                div()
                    .id("intruder-result-settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        Checkbox::new("intruder-result-save-to-project")
                            .label("保存攻击到项目文件")
                            .tooltip("将本次攻击任务的配置快照与响应结果持久化写入当前磁盘项目文件")
                            .checked(saved)
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, value, window, cx| {
                                if *value && this.can_save_result_to_project(cx) {
                                    this.target.set_project_retention(true, window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::muted_foreground())
                            .child("保存后的攻击可在重开项目时从 Dashboard 访问。仅内存保留的攻击不会随普通保存或另存为写入项目。"),
                    )
                    .when(!named, |this| {
                        this.child(
                            div()
                                .text_size(theme::scaled_px(11.5))
                                .text_color(theme::muted_foreground())
                                .child("（仅适用于磁盘项目）"),
                        )
                    })
                    .child(
                        div()
                            .border_t_1()
                            .border_color(theme::border())
                            .pt(px(12.))
                            .text_size(theme::scaled_px(13.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Attack results"),
                    )
                    .child(
                        Checkbox::new("intruder-result-baseline")
                            .label("发送未修改的基准请求")
                            .tooltip("发送一次不替换任何 § 标记位的原始基准请求（请求 #0）以便对照响应差异")
                            .checked(baseline)
                            .disabled(!baseline_available)
                            .on_click(cx.listener(|this, value, window, cx| {
                                if *value {
                                    let view_id = this.target.view_id;
                                    this.target.update(window, cx, |owner, window, cx| {
                                        owner.request_intruder_baseline(view_id, window, cx)
                                    });
                                }
                            })),
                    )
                    .when(baseline_pending, |this| {
                        this.child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child("正在确认一次基准请求…"),
                        )
                    })
                    .when_some(result.baseline_error.clone(), |this, error| {
                        this.child(
                            div()
                                .id("intruder-baseline-error")
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
}
