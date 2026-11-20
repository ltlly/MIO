use gpui_kit::accesskit::Role;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::{
    IndexPath,
    select::Select,
    table::{Column, TableDelegate},
};
use mio_core::{IntruderPoolUsage, ScheduledTask, ScheduledTaskAction, Uuid};

use super::*;
use crate::workbench::intruder::resource_pools::pool_values;

pub(super) struct PoolTable {
    pub(super) owner: WeakEntity<Settings>,
}
const HEADERS: [(&str, f32); 7] = [
    ("资源池", 176.),
    ("运行任务", 96.),
    ("并发请求", 110.),
    ("请求间隔", 108.),
    ("随机间隔", 108.),
    ("间隔增量", 108.),
    ("自动节流", 110.),
];

const DEFAULT_POOL_FALLBACK: [&str; 7] = ["Default resource pool", "1", "10", "", "", "", "Yes"];

impl PoolTable {
    fn uses_fallback(&self, cx: &App) -> bool {
        self.owner.upgrade().is_none_or(|owner| {
            owner
                .read(cx)
                .state
                .pools
                .catalog
                .as_ref()
                .is_none_or(|catalog| catalog.pools.is_empty())
        })
    }

    fn cell(&self, row: usize, column: usize, cx: &App) -> String {
        if self.uses_fallback(cx) {
            return if row == 0 {
                DEFAULT_POOL_FALLBACK[column].to_owned()
            } else {
                String::new()
            };
        }
        let Some(owner) = self.owner.upgrade() else {
            return String::new();
        };
        let state = &owner.read(cx).state.pools;
        let Some(pool) = state
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.pools.get(row))
        else {
            return String::new();
        };
        if column == 1 {
            return state
                .usage
                .as_ref()
                .filter(|usage| {
                    state
                        .catalog
                        .as_ref()
                        .is_some_and(|catalog| catalog.revision == usage.catalog_revision)
                })
                .and_then(|usage| usage.task_counts.get(&pool.id))
                .filter(|count| **count > 0)
                .map(u64::to_string)
                .unwrap_or_default();
        }
        pool_values(pool)[if column == 0 { 0 } else { column - 1 }].clone()
    }
}

impl TableDelegate for PoolTable {
    fn columns_count(&self, _: &App) -> usize {
        HEADERS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), HEADERS[index].0)
            .width((HEADERS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }
    fn rows_count(&self, cx: &App) -> usize {
        if self.uses_fallback(cx) {
            return 1;
        }
        self.owner
            .upgrade()
            .and_then(|owner| {
                owner
                    .read(cx)
                    .state
                    .pools
                    .catalog
                    .as_ref()
                    .map(|catalog| catalog.pools.len())
            })
            .unwrap_or(1)
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(HEADERS[col_ix].0, HEADERS[col_ix].1)
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let id = self.owner.upgrade().and_then(|owner| {
            owner
                .read(cx)
                .state
                .pools
                .catalog
                .as_ref()
                .and_then(|catalog| catalog.pools.get(row))
                .map(|pool| pool.id)
        });
        div()
            .id(SharedString::from(format!(
                "settings-pool-row-{}",
                id.unwrap_or_default()
            )))
            .border_b_0()
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().px(px(6.)).truncate().child(self.cell(row, column, cx))
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        Some(self.cell(row, column, cx).into())
    }
}

pub(in crate::workbench::settings) struct ScheduledTasksTable {
    pub(in crate::workbench::settings) owner: WeakEntity<Settings>,
}

const SCHEDULE_COLUMNS: [(&str, f32); 4] = [
    ("Enabled", 64.),
    ("Time", 120.),
    ("Repeat", 110.),
    ("Task", 234.),
];

fn scheduled_action_label(action: ScheduledTaskAction) -> &'static str {
    match action {
        ScheduledTaskAction::PauseTasks => "暂停任务",
        ScheduledTaskAction::ResumeTasks => "恢复任务",
        ScheduledTaskAction::RunScan => "运行扫描",
    }
}

fn scheduled_action_index(action: ScheduledTaskAction) -> usize {
    match action {
        ScheduledTaskAction::PauseTasks => 0,
        ScheduledTaskAction::ResumeTasks => 1,
        ScheduledTaskAction::RunScan => 2,
    }
}

fn scheduled_action_from_label(label: &str) -> ScheduledTaskAction {
    match label {
        "恢复任务" => ScheduledTaskAction::ResumeTasks,
        "运行扫描" => ScheduledTaskAction::RunScan,
        _ => ScheduledTaskAction::PauseTasks,
    }
}

fn split_trigger_spec(spec: &str) -> (&str, &str) {
    spec.split_once(" | ").unwrap_or((spec, ""))
}

fn scheduled_task_summary(task: &ScheduledTask) -> String {
    let action = scheduled_action_label(task.action);
    let desc = task.description.trim();
    if desc.is_empty() {
        action.to_owned()
    } else {
        format!("{action} — {desc}")
    }
}

impl TableDelegate for ScheduledTasksTable {
    fn columns_count(&self, _: &App) -> usize {
        SCHEDULE_COLUMNS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), SCHEDULE_COLUMNS[index].0)
            .width((SCHEDULE_COLUMNS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }
    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).state.scheduled_tasks.len())
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(SCHEDULE_COLUMNS[col_ix].0, SCHEDULE_COLUMNS[col_ix].1)
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("settings-schedule-row", row)).border_b_0()
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (task, enabled, project, wb) = {
            let settings = owner.read(cx);
            let Some(task) = settings.state.scheduled_tasks.get(row).cloned() else {
                return div().into_any_element();
            };
            (
                task,
                settings.state.enabled,
                settings.state.project,
                settings.owner.clone(),
            )
        };
        if column == 0 {
            let id = task.id;
            return settings_table_checkbox_cell(
                listeners_tls::indexed_checkbox(
                    ("settings-schedule-enabled", row),
                    "",
                    task.enabled,
                    enabled,
                )
                .p_0()
                .gap_0()
                .accessibility_label(format!("启用定时任务 {}", scheduled_task_summary(&task)))
                .on_change(move |st, _, _, cx| {
                    let checked = st == CheckboxState::Checked;
                    let _ = wb.update(cx, |view, cx| {
                        let mut next = view.workspace.scheduled_tasks.clone();
                        if let Some(item) = next.iter_mut().find(|t| t.id == id) {
                            item.enabled = checked;
                        }
                        view.update_tasks_settings(project, None, Some(next), cx);
                    });
                }),
            )
            .into_any_element();
        }
        div()
            .px(px(6.))
            .truncate()
            .child(self.cell_label(row, column, cx).unwrap_or_default())
            .into_any_element()
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let settings = owner.read(cx);
        let task = settings.state.scheduled_tasks.get(row)?;
        let (time, repeat) = split_trigger_spec(&task.trigger_spec);
        Some(match column {
            0 => if task.enabled { "Yes" } else { "No" }.into(),
            1 => time.to_owned().into(),
            2 => repeat.to_owned().into(),
            _ => scheduled_task_summary(task).into(),
        })
    }
}

impl Workbench {
    pub(in crate::workbench) fn settings_pool_usage_changed(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.intruder.resource_pools.management;
        if state.query_revision == 0 {
            return;
        }
        state.usage_dirty = true;
        if state.query.is_none() {
            self.query_settings_pool_usage(cx);
        }
    }

    pub(in crate::workbench) fn query_settings_pool_usage(&mut self, cx: &mut Context<Self>) {
        if !self.pool_management_available()
            || self.intruder.resource_pools.management.query.is_some()
        {
            return;
        }
        let project = self.project.unwrap();
        let management = &mut self.intruder.resource_pools.management;
        let Some(revision) = management.query_revision.checked_add(1) else {
            self.fail("资源池任务计数查询 revision 已耗尽。", cx);
            return;
        };
        management.query_revision = revision;
        if !self.enqueue(
            Command::QueryIntruderPoolUsage {
                project,
                query_revision: revision,
            },
            cx,
        ) {
            self.intruder.resource_pools.management.error = self.error.clone();
            return;
        }
        let management = &mut self.intruder.resource_pools.management;
        management.query = Some((project, revision, cx.background_executor().now() + DEADLINE));
        management.usage_dirty = false;
        let timer = cx.background_executor().timer(DEADLINE);
        let owner = self.owner_window;
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    let state = &mut view.intruder.resource_pools.management;
                    if view.project == Some(project)
                        && state.query.is_some_and(|(_, current, deadline)| {
                            current == revision && cx.background_executor().now() >= deadline
                        })
                    {
                        state.query = None;
                        state.error =
                            Some("资源池任务计数读取超时；请重新选择 Tasks 读取。".into());
                        if state.usage_dirty {
                            view.query_settings_pool_usage(cx);
                        }
                        cx.notify();
                    }
                });
            });
        });
        self.intruder.resource_pools.management.query_timer = Some(task);
        cx.notify();
    }

    pub(in crate::workbench) fn on_settings_pool_usage(
        &mut self,
        project: ProjectKey,
        query_revision: u64,
        result: Result<IntruderPoolUsage, String>,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project)
            || self
                .intruder
                .resource_pools
                .management
                .query
                .is_none_or(|(owner, revision, _)| owner != project || revision != query_revision)
        {
            return;
        }
        let state = &mut self.intruder.resource_pools.management;
        let (_, _, deadline) = state.query.take().unwrap();
        state.query_timer = None;
        if state.usage_dirty {
            self.query_settings_pool_usage(cx);
            cx.notify();
            return;
        }
        if cx.background_executor().now() >= deadline {
            state.error = Some("资源池任务计数回执超时，请重新读取。".into());
        } else {
            match result {
                Ok(usage)
                    if self
                        .intruder
                        .resource_pools
                        .catalog
                        .as_ref()
                        .is_some_and(|catalog| {
                            catalog.revision == usage.catalog_revision
                                && usage.task_counts.len() == catalog.pools.len()
                                && catalog
                                    .pools
                                    .iter()
                                    .all(|pool| usage.task_counts.contains_key(&pool.id))
                                && usage
                                    .requires_confirmation
                                    .iter()
                                    .all(|id| catalog.find(*id).is_some())
                        }) =>
                {
                    state.usage = Some(usage);
                    state.error = None;
                }
                Ok(_) => {
                    state.error = Some("资源池计数与目录版本不一致；请重新选择 Tasks 读取。".into())
                }
                Err(error) => state.error = Some(format!("资源池计数读取失败：{error}")),
            }
        }
        cx.notify();
    }
}

impl Settings {
    pub(in crate::workbench::settings) fn refresh_pool_page(&mut self, cx: &mut Context<Self>) {
        let _ = self.owner.update(cx, |view, cx| {
            if !view.intruder.resource_pools.management.mutation_pending() {
                view.intruder.resource_pools.management.error = None;
            }
            view.query_intruder_resource_pools(cx);
            view.query_settings_pool_usage(cx);
        });
    }
    pub(in crate::workbench::settings) fn advance_pool_page(&mut self, cx: &mut Context<Self>) {
        if self.state.pools.preparing {
            let owner = self.owner.clone();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| view.advance_settings_pool_delete(cx));
            });
        }
    }

    fn delete_pool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.pools.ready() || self.pools.confirming || self.pools.dialog.is_some() {
            return;
        }
        let (Some(project), Some(pool_id), Some(catalog), Some(usage)) = (
            self.state.project,
            self.pools.selected,
            self.state.pools.catalog.as_ref(),
            self.state.pools.usage.as_ref(),
        ) else {
            return;
        };
        if catalog.default_pool_id == Some(pool_id) {
            return;
        }
        let revision = catalog.revision;
        if usage.requires_confirmation.contains(&pool_id) {
            self.pools.confirming = true;
            let response = window.prompt(
                PromptLevel::Warning,
                "删除资源池",
                Some("确定要删除吗？相关任务将分配到默认资源池。"),
                &["否", "是"],
                cx,
            );
            cx.spawn(async move |this, cx| {
                let yes = response.await.ok() == Some(1);
                let _ = this.update(cx, |settings, cx| {
                    settings.pools.confirming = false;
                    if yes && settings.state.project == Some(project) {
                        settings.send_pool_delete(project, pool_id, revision, true, cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            self.send_pool_delete(project, pool_id, revision, false, cx);
        }
    }

    fn send_pool_delete(
        &self,
        project: ProjectKey,
        pool_id: Uuid,
        revision: u64,
        allow_reassign: bool,
        cx: &mut Context<Self>,
    ) {
        let owner = self.owner.clone();
        let window = self.owner_window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    if let Err(error) = view.prepare_pool_delete(
                        project,
                        pool_id,
                        revision,
                        allow_reassign,
                        window,
                        cx,
                    ) {
                        view.intruder.resource_pools.management.error = Some(error);
                        cx.notify();
                    }
                });
            });
        });
    }

    pub(in crate::workbench::settings) fn render_pool_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.state.pools;
        let enabled = state.ready()
            && self.pools.dialog.is_none()
            && !self.pools.confirming
            && !self.pools.opening;
        let selected = self.pools.selected.filter(|id| {
            state
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.find(*id).is_some())
        });
        let deletable = selected.is_some_and(|id| {
            state
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.default_pool_id != Some(id))
        });
        let project = self.state.project;
        let o_start = self.owner.clone();
        let o_pause = self.owner.clone();
        let has_sched_sel = self
            .scheduled_tasks_selected
            .is_some_and(|idx| idx < self.state.scheduled_tasks.len());

        let content = div()
            .px(px(22.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(9.))
                    .child(settings_section_heading_plain(
                        "Resource pools",
                        project_scope_badge(),
                    ))
                    .child(settings_desc_row(
                        "资源池将共享网络资源配额的任务分组，限制并发请求数和请求发送速率。",
                    ))
                    .child(
                        div()
                            .pl(px(31.))
                            .flex()
                            .gap(px(9.))
                            .h(theme::scaled_px(176.).max(px(176.)))
                            .child(
                                div()
                                    .w(theme::scaled_px(84.).max(px(84.)))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(5.))
                                    .child(
                                        settings_action_button(
                                            "settings-pool-new",
                                            "新建…",
                                            !enabled,
                                        )
                                        .w_full()
                                        .tooltip("新建共享并发与速率配额的资源池")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_pool_editor(true, window, cx)
                                        })),
                                    )
                                    .child(
                                        super::super::navigation::settings_secondary_action_button(
                                            "settings-pool-edit",
                                            "编辑",
                                            !enabled || selected.is_none(),
                                        )
                                        .w_full()
                                        .tooltip("编辑当前选中的资源池并发与延迟配置")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_pool_editor(false, window, cx)
                                        })),
                                    )
                                    .child(
                                        super::super::navigation::settings_secondary_action_button(
                                            "settings-pool-delete",
                                            "删除",
                                            !enabled || !deletable,
                                        )
                                        .w_full()
                                        .tooltip("删除当前选中的非默认资源池")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.delete_pool(window, cx)
                                        })),
                                    ),
                            )
                            .child(settings_table_frame(
                                DataTable::new(&self.pools.table)
                                    .with_size(px(22.))
                                    .header_row_height(px(24.))
                                    .bordered(false)
                                    .stripe(false)
                                    .scrollbar_visible(true, false)
                                    .scroll_bounce(false)
                                    .accessibility_label("资源池任务计数"),
                                true,
                            )),
                    )
                    .when(state.reading, |this| {
                        this.child(
                            div()
                                .pl(px(31.))
                                .text_color(theme::muted_foreground())
                                .child("正在读取资源池与任务计数…"),
                        )
                    })
                    .when(state.busy, |this| {
                        this.child(
                            div()
                                .pl(px(31.))
                                .text_color(theme::muted_foreground())
                                .child("正在确认资源池修改…"),
                        )
                    })
                    .when_some(state.error.clone(), |this, error| {
                        this.child(
                            div()
                                .id("settings-pool-error")
                                .test_support()
                                .role(Role::Alert)
                                .aria_label(error.clone())
                                .ml(px(31.))
                                .p(px(8.))
                                .rounded(px(4.))
                                .bg(theme::danger_bg())
                                .text_color(theme::danger_fg())
                                .child(error),
                        )
                    }),
            )
            .child(
                div()
                    .pt(px(16.))
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap(px(9.))
                    .child(settings_section_heading(
                        "settings-tasks-autostart-heading",
                        "新任务自动启动",
                        project_scope_badge(),
                    ))
                    .child(settings_desc_row_plain(
                        "控制新创建的任务是否自动开始运行。",
                    ))
                    .child(
                        div()
                            .id("settings-tasks-autostart-group")
                            .test_support()
                            .role(Role::RadioGroup)
                            .aria_label("新任务自动启动")
                            .pl(px(31.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(2.))
                            .child(
                                settings_radio(
                                    "settings-tasks-autostart-start",
                                    "启动任务",
                                    self.state.tasks_auto_start,
                                    self.state.enabled,
                                    window,
                                    cx,
                                )
                                .tab_stop(false)
                                .on_change(move |_, _, _, cx| {
                                    let _ = o_start.update(cx, |view, cx| {
                                        view.update_tasks_settings(project, Some(true), None, cx)
                                    });
                                }),
                            )
                            .child(
                                settings_radio(
                                    "settings-tasks-autostart-pause",
                                    "暂停任务",
                                    !self.state.tasks_auto_start,
                                    self.state.enabled,
                                    window,
                                    cx,
                                )
                                .tab_stop(false)
                                .on_change(move |_, _, _, cx| {
                                    let _ = o_pause.update(cx, |view, cx| {
                                        view.update_tasks_settings(project, Some(false), None, cx)
                                    });
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .pt(px(16.))
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap(px(9.))
                    .child(settings_section_heading(
                        "settings-tasks-schedule-heading",
                        "定时任务",
                        project_scope_badge(),
                    ))
                    .child(settings_desc_row(
                        "配置在指定时间自动执行任务的计划表。",
                    ))
                    .child(
                        div()
                            .pl(px(31.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(9.))
                                    .h(theme::scaled_px(148.).max(px(148.)))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(84.).max(px(84.)))
                                            .flex_shrink_0()
                                            .flex()
                                            .flex_col()
                                            .gap(px(5.))
                                            .child(
                                                settings_action_button(
                                                    "settings-schedule-add",
                                                    "添加",
                                                    !self.state.enabled,
                                                )
                                                .w_full()
                                                .tab_stop(false)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.scheduled_task_editing = None;
                                                    this.scheduled_task_editor_open = true;
                                                    this.scheduled_task_action_select.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_selected_index(
                                                                Some(IndexPath::new(0)),
                                                                window,
                                                                cx,
                                                            )
                                                        },
                                                    );
                                                    for inp in [
                                                        &this.scheduled_task_time,
                                                        &this.scheduled_task_repeat,
                                                        &this.scheduled_task_name,
                                                    ] {
                                                        inp.update(cx, |i, cx| {
                                                            i.set_value("", window, cx)
                                                        });
                                                    }
                                                    this.scheduled_task_time
                                                        .update(cx, |i, cx| i.focus(window, cx));
                                                    cx.notify();
                                                })),
                                            )
                                            .child(
                                                settings_action_button(
                                                    "settings-schedule-edit",
                                                    "编辑",
                                                    !self.state.enabled || !has_sched_sel,
                                                )
                                                .w_full()
                                                .tab_stop(false)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    if let Some(idx) = this.scheduled_tasks_selected
                                                        && let Some(item) = this
                                                            .state
                                                            .scheduled_tasks
                                                            .get(idx)
                                                            .cloned()
                                                    {
                                                        let (time, repeat) =
                                                            split_trigger_spec(&item.trigger_spec);
                                                        this.scheduled_task_editing = Some(item.id);
                                                        this.scheduled_task_editor_open = true;
                                                        this.scheduled_task_action_select.update(
                                                            cx,
                                                            |s, cx| {
                                                                s.set_selected_index(
                                                                    Some(IndexPath::new(
                                                                        scheduled_action_index(
                                                                            item.action,
                                                                        ),
                                                                    )),
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        );
                                                        this.scheduled_task_time.update(
                                                            cx,
                                                            |i, cx| {
                                                                i.set_value(
                                                                    time.to_owned(),
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        );
                                                        this.scheduled_task_repeat.update(
                                                            cx,
                                                            |i, cx| {
                                                                i.set_value(
                                                                    repeat.to_owned(),
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        );
                                                        this.scheduled_task_name.update(
                                                            cx,
                                                            |i, cx| {
                                                                i.set_value(
                                                                    item.description,
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        );
                                                        this.scheduled_task_time
                                                            .update(cx, |i, cx| i.focus(window, cx));
                                                        cx.notify();
                                                    }
                                                })),
                                            )
                                            .child(
                                                settings_action_button(
                                                    "settings-schedule-remove",
                                                    "移除",
                                                    !self.state.enabled || !has_sched_sel,
                                                )
                                                .w_full()
                                                .tab_stop(false)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(idx) = this.scheduled_tasks_selected
                                                        && let Some(task_id) = this
                                                            .state
                                                            .scheduled_tasks
                                                            .get(idx)
                                                            .map(|t| t.id)
                                                    {
                                                        if this.scheduled_task_editing
                                                            == Some(task_id)
                                                        {
                                                            this.scheduled_task_editing = None;
                                                            this.scheduled_task_editor_open = false;
                                                        }
                                                        let mut next =
                                                            this.state.scheduled_tasks.clone();
                                                        next.retain(|t| t.id != task_id);
                                                        this.scheduled_tasks_selected = None;
                                                        let _ = this.owner.update(cx, |v, cx| {
                                                            v.update_tasks_settings(
                                                                project,
                                                                None,
                                                                Some(next),
                                                                cx,
                                                            )
                                                        });
                                                        cx.notify();
                                                    }
                                                })),
                                            ),
                                    )
                                    .child(
                                        settings_table_frame(
                                            DataTable::new(&self.scheduled_tasks_table)
                                                .with_size(px(22.))
                                                .header_row_height(px(24.))
                                                .bordered(false)
                                                .stripe(false)
                                                .scrollbar_visible(true, false)
                                                .scroll_bounce(false)
                                                .accessibility_label("定时任务"),
                                            true,
                                        )
                                        .max_w(theme::scaled_px(540.).max(px(540.))),
                                    ),
                            )
                            .when(self.scheduled_task_editor_open, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(8.))
                                        .p(px(8.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .bg(theme::background())
                                        .child(
                                            Select::new(&self.scheduled_task_action_select)
                                                .id("settings-schedule-action-select")
                                                .small()
                                                .w(px(120.))
                                                .disabled(!self.state.enabled),
                                        )
                                        .child(
                                            div().w(px(100.)).child(
                                                Input::new(&self.scheduled_task_time)
                                                    .id("settings-schedule-time-input")
                                                    .small(),
                                            ),
                                        )
                                        .child(
                                            div().w(px(100.)).child(
                                                Input::new(&self.scheduled_task_repeat)
                                                    .id("settings-schedule-repeat-input")
                                                    .small(),
                                            ),
                                        )
                                        .child(
                                            div().w(px(180.)).child(
                                                Input::new(&self.scheduled_task_name)
                                                    .id("settings-schedule-name-input")
                                                    .small(),
                                            ),
                                        )
                                        .child(
                                            settings_action_button(
                                                "settings-schedule-ok",
                                                "确定",
                                                !self.state.enabled,
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                let snapshots = [
                                                    &this.scheduled_task_time,
                                                    &this.scheduled_task_repeat,
                                                    &this.scheduled_task_name,
                                                ]
                                                .map(|inp| editing::input_snapshot(inp, window, cx));
                                                let [Ok(t_raw), Ok(r_raw), Ok(desc_raw)] = snapshots
                                                else {
                                                    this.settings_error(
                                                        editing::COMPOSITION_PENDING,
                                                        cx,
                                                    );
                                                    return;
                                                };
                                                let t = t_raw.trim();
                                                let r = r_raw.trim();
                                                let description = desc_raw.trim().to_owned();
                                                let trigger_spec = if r.is_empty() {
                                                    t.to_owned()
                                                } else {
                                                    format!("{t} | {r}")
                                                };
                                                if trigger_spec.is_empty()
                                                    || trigger_spec.len() > 512
                                                    || trigger_spec.contains('\0')
                                                    || description.len() > 1024
                                                    || description.contains('\0')
                                                {
                                                    this.settings_error(
                                                        "请填写有效的定时触发时间（触发规则不超过 512 字节，描述不超过 1024 字节）。",
                                                        cx,
                                                    );
                                                    return;
                                                }
                                                let action = this
                                                    .scheduled_task_action_select
                                                    .read(cx)
                                                    .selected_value()
                                                    .copied()
                                                    .map(scheduled_action_from_label)
                                                    .unwrap_or(ScheduledTaskAction::PauseTasks);
                                                let mut next = this.state.scheduled_tasks.clone();
                                                if let Some(edit_id) = this.scheduled_task_editing
                                                    && let Some(existing) =
                                                        next.iter_mut().find(|t| t.id == edit_id)
                                                {
                                                    existing.action = action;
                                                    existing.trigger_spec = trigger_spec;
                                                    existing.description = description;
                                                } else if next.len() < 128 {
                                                    next.push(ScheduledTask {
                                                        id: Uuid::new_v4(),
                                                        enabled: true,
                                                        action,
                                                        trigger_spec,
                                                        description,
                                                    });
                                                }
                                                let _ = this.owner.update(cx, |v, cx| {
                                                    v.update_tasks_settings(
                                                        project,
                                                        None,
                                                        Some(next),
                                                        cx,
                                                    )
                                                });
                                                this.scheduled_task_editor_open = false;
                                                this.scheduled_task_editing = None;
                                                for inp in [
                                                    &this.scheduled_task_time,
                                                    &this.scheduled_task_repeat,
                                                    &this.scheduled_task_name,
                                                ] {
                                                    inp.update(cx, |i, cx| {
                                                        i.set_value("", window, cx)
                                                    });
                                                }
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            settings_action_button(
                                                "settings-schedule-cancel",
                                                "取消",
                                                false,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.scheduled_task_editor_open = false;
                                                this.scheduled_task_editing = None;
                                                for inp in [
                                                    &this.scheduled_task_time,
                                                    &this.scheduled_task_repeat,
                                                    &this.scheduled_task_name,
                                                ] {
                                                    inp.update(cx, |i, cx| {
                                                        i.set_value("", window, cx)
                                                    });
                                                }
                                                cx.notify();
                                            })),
                                        ),
                                )
                            }),
                    ),
            );
        settings_scrollable_pane("settings-resource-pools", &self.content_scroll, content)
            .into_any_element()
    }
}
