use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator, Radio};
use gpui_kit::component::Icon;
use gpui_kit::uniform_list;

use super::form::DelayKind;
use super::*;

mod form_view;
use form_view::create_pool_form;
pub(in crate::workbench) use form_view::{PoolFormAction, PoolFormChange, render_pool_form};

const COLUMNS: [(&str, f32); 7] = [
    ("选中", 44.),
    ("资源池", 150.),
    ("并发请求", 96.),
    ("请求间隔", 96.),
    ("随机间隔", 96.),
    ("间隔增量", 96.),
    ("自动节流", 96.),
];
const TABLE_WIDTH: f32 = 674.;

#[derive(Clone, Copy)]
pub(in crate::workbench) enum PoolFlag {
    Cap,
    Delay,
    Auto,
    Status429,
    Status503,
    Other,
}

pub(in crate::workbench) fn pool_radio(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
) -> Radio {
    let label = label.into();
    Radio::new(id)
        .accessibility_label(label.clone())
        .checked(checked)
        .disabled(!enabled)
        .flex()
        .items_center()
        .min_h(theme::scaled_px(24.))
        .gap(px(8.))
        .text_size(theme::ui_font_size())
        .text_color(theme::foreground())
        .rounded(px(4.))
        .focus_visible(|style| style.bg(theme::selected()))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .child(
            div()
                .size(theme::scaled_px(14.).max(px(14.)))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(if checked {
                    rgb(0x1558b0).into()
                } else {
                    theme::control_border()
                })
                .bg(theme::surface())
                .when(checked, |this| {
                    this.child(
                        div()
                            .size(theme::scaled_px(6.).max(px(6.)))
                            .rounded_full()
                            .bg(theme::accent()),
                    )
                }),
        )
        .child(label)
}

pub(in crate::workbench) fn pool_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
) -> Checkbox {
    let label = label.into();
    Checkbox::new(id)
        .accessibility_label(label.clone())
        .checked(checked)
        .disabled(!enabled)
        .flex()
        .items_center()
        .min_h(theme::scaled_px(24.))
        .gap(px(9.))
        .px(px(2.))
        .py(px(3.))
        .text_size(theme::ui_font_size())
        .text_color(theme::foreground())
        .rounded(px(4.))
        .focus_visible(|style| style.bg(theme::selected()))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .child(
            CheckboxIndicator::new()
                .checked(checked)
                .size(theme::scaled_px(14.).max(px(14.)))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(if checked {
                    rgb(0x26649d)
                } else {
                    rgb(if theme::is_dark() { 0x5a5d62 } else { 0xb0afaf })
                })
                .bg(if checked {
                    rgb(0x26649d).into()
                } else {
                    theme::surface()
                })
                .rounded(px(2.))
                .when(checked, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(theme::scaled_px(11.).max(px(11.)))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        )
        .child(label)
}

pub(super) fn field(
    input: &Entity<InputState>,
    label: &'static str,
    enabled: bool,
    width: f32,
) -> Input {
    let w = if theme::ui_scale() > 1.0 {
        theme::scaled_px(width)
    } else {
        px(width)
    };
    Input::new(input)
        .aria_label(label)
        .small()
        .disabled(!enabled)
        .w(w)
}

impl Workbench {
    fn change_intruder_pool_flag(
        &mut self,
        id: TabId,
        flag: PoolFlag,
        checked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available()
            || self.transition.is_some()
            || !self
                .intruder
                .editors
                .get(&id)
                .is_some_and(|editors| editors.pool_form.creating)
        {
            return;
        }
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let form = &mut editors.pool_form;
        match flag {
            PoolFlag::Cap => form.cap_enabled = checked,
            PoolFlag::Delay => form.delay_enabled = checked,
            PoolFlag::Auto => form.automatic = checked,
            PoolFlag::Status429 => form.status_429 = checked,
            PoolFlag::Status503 => form.status_503 = checked,
            PoolFlag::Other => form.other_enabled = checked,
        }
        self.intruder_pool_form_changed(id, window, cx);
    }

    fn change_intruder_pool_delay(
        &mut self,
        id: TabId,
        kind: DelayKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available()
            || self.transition.is_some()
            || !self
                .intruder
                .editors
                .get(&id)
                .is_some_and(|editors| editors.pool_form.creating)
        {
            return;
        }
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if !editors.pool_form.delay_enabled {
            return;
        }
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        editors.pool_form.delay_kind = kind;
        self.intruder_pool_form_changed(id, window, cx);
    }

    pub(in crate::workbench::intruder) fn render_intruder_resource_pools(
        &self,
        id: TabId,
        editors: &IntruderEditors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.project;
        let create = editors.pool_form.creating;
        let selected = (!create).then_some(editors.pool_form.existing).flatten();
        let enabled = self.backend_available() && self.transition.is_none();
        let selection_enabled = enabled
            && self.intruder.resource_pools.pending.is_none()
            && self.intruder.resource_pools.error.is_none();
        let form = &editors.pool_form;
        let editable = enabled && create;
        let weak = cx.weak_entity();
        let count = self
            .intruder
            .resource_pools
            .catalog
            .as_ref()
            .map_or(0, |catalog| catalog.pools.len());
        let col_scale = theme::ui_scale();
        let table_w = px((TABLE_WIDTH * col_scale).round());
        let list = uniform_list(
            SharedString::from(format!("intruder-pool-list-{id}")),
            count,
            move |range, _, cx| {
                let Some(owner) = weak.upgrade() else {
                    return Vec::new();
                };
                let view = owner.read(cx);
                if view.project != project {
                    return Vec::new();
                }
                let Some(catalog) = &view.intruder.resource_pools.catalog else {
                    return Vec::new();
                };
                let selected = view
                    .intruder
                    .editors
                    .get(&id)
                    .filter(|editors| !editors.pool_form.creating)
                    .and_then(|editors| editors.pool_form.existing);
                let enabled = view.backend_available()
                    && view.transition.is_none()
                    && view.intruder.resource_pools.pending.is_none()
                    && view.intruder.resource_pools.error.is_none();
                let row_scale = theme::ui_scale();
                let row_w = px((TABLE_WIDTH * row_scale).round());
                range
                    .filter_map(|index| catalog.pools.get(index))
                    .map(|pool| {
                        let pool_id = pool.id;
                        let callback = weak.clone();
                        let mut row = div()
                            .id(SharedString::from(format!("intruder-pool-row-{pool_id}")))
                            .test_support()
                            .role(Role::Row)
                            .w(row_w)
                            .h(theme::scaled_px(24.).max(px(20.)))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .w(px((COLUMNS[0].1 * row_scale).round()))
                                    .flex_shrink_0()
                                    .px(px(9.))
                                    .child(
                                        pool_radio(
                                            SharedString::from(format!(
                                                "intruder-pool-select-{pool_id}"
                                            )),
                                            "",
                                            selected == Some(pool_id),
                                            enabled,
                                        )
                                        .accessibility_label(format!("选择资源池 {}", pool.name))
                                        .on_change(
                                            move |checked, _, window, cx| {
                                                if !checked {
                                                    return;
                                                }
                                                let _ = callback.update(cx, |view, cx| {
                                                    if view.project == project {
                                                        view.select_intruder_resource_pool(
                                                            id, pool_id, window, cx,
                                                        );
                                                    }
                                                });
                                            },
                                        ),
                                    ),
                            );
                        for (column, (value, (_, width))) in pool_values(pool)
                            .into_iter()
                            .zip(COLUMNS.iter().skip(1))
                            .enumerate()
                        {
                            row = row.child(
                                div()
                                    .id(SharedString::from(format!(
                                        "intruder-pool-cell-{pool_id}-{column}"
                                    )))
                                    .test_support()
                                    .role(Role::Cell)
                                    .aria_label(value.clone())
                                    .w(px((*width * row_scale).round()))
                                    .flex_shrink_0()
                                    .px(px(5.))
                                    .truncate()
                                    .child(value),
                            );
                        }
                        row
                    })
                    .collect::<Vec<_>>()
            },
        )
        .w(table_w)
        .h(theme::scaled_px(160.).max(px(128.)));
        let header = div()
            .w(table_w)
            .min_h(theme::scaled_px(26.).max(px(20.)))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::toolbar_surface())
            .text_size(theme::scaled_px(12.))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .children(COLUMNS.map(|(label, width)| {
                div()
                    .w(px((width * col_scale).round()))
                    .flex_shrink_0()
                    .px(px(5.))
                    .child(label)
            }));
        let missing = selected.is_some_and(|selected| {
            self.intruder
                .resource_pools
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.find(selected).is_none())
        });
        let side_panel_left = self.intruder_user_misc().intruder_side_panel_left;
        div()
            .id("intruder-resource-pools-panel")
            .test_support()
            .w(px(420.))
            .min_w(px(280.))
            .max_w(px(480.))
            .flex_shrink(1.0)
            .h_full()
            .bg(theme::surface())
            .when(side_panel_left, |d| d.border_r_1())
            .when(!side_panel_left, |d| d.border_l_1())
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
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.5))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("Resource pool"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(
                                button("intruder-pool-refresh", "刷新")
                                    .icon(IconName::RefreshCw)
                                    .tooltip("重新从后台读取当前项目的资源池目录与并发配置")
                                    .disabled(
                                        !enabled || self.intruder.resource_pools.pending.is_some(),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.query_intruder_resource_pools(cx);
                                    })),
                            )
                            .child(
                                button("intruder-pool-manage", "管理…")
                                    .icon(IconName::SlidersHorizontal)
                                    .tooltip("打开项目设置中的 Resource pools 页面新建、编辑或删除资源池")
                                    .disabled(!enabled)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_project_tasks_settings(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("intruder-resource-pools-body")
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
                            .child("指定攻击使用的资源池。资源池用于共同管理多个任务的资源用量。"),
                    )
                    .child(
                        div()
                            .id("intruder-resource-pools-table")
                            .test_support()
                            .role(Role::Table)
                            .w_full()
                            .h(theme::scaled_px(196.).max(px(196.)))
                            .flex_shrink_0()
                            .overflow_x_scroll()
                            .border_1()
                            .border_color(theme::control_border())
                            .rounded(px(4.))
                            .bg(theme::surface())
                            .child(header)
                            .child(list),
                    )
                    .when(self.intruder.resource_pools.pending.is_some(), |this| {
                        this.child(
                            div()
                                .id("intruder-pool-catalog-loading")
                                .test_support()
                                .role(Role::Status)
                                .aria_label("正在读取资源池…")
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child("正在读取资源池…"),
                        )
                    })
                    .when_some(self.intruder.resource_pools.error.clone(), |this, error| {
                        this.child(
                            div()
                                .id("intruder-pool-catalog-error")
                                .test_support()
                                .role(Role::Alert)
                                .aria_label(error.clone())
                                .text_color(theme::danger_fg())
                                .child(error),
                        )
                    })
                    .when(!create && selected.is_none(), |this| {
                        this.child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child("此旧配置仍使用独立并发，尚未绑定资源池；明确选择后才更改。"),
                        )
                    })
                    .when(missing, |this| {
                        this.child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::danger_fg())
                                .child("原选资源池不在已读取目录中；未自动替换，请重新选择。"),
                        )
                    })
                    .child(
                        pool_radio(
                            "intruder-pool-existing",
                            "使用已有资源池",
                            selected.is_some(),
                            selection_enabled,
                        )
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "绑定上方目录中已存在的共享资源池以统一控制并发数与请求间隔",
                            )
                            .build(window, cx)
                        })
                        .on_change({
                            let owner = cx.weak_entity();
                            move |checked, _, window, cx| {
                                if checked {
                                    let _ = owner.update(cx, |view, cx| {
                                        if view.project == project {
                                            view.use_intruder_existing_pool(id, window, cx);
                                        }
                                    });
                                }
                            }
                        }),
                    )
                    .child(
                        pool_radio("intruder-pool-create", "创建新资源池", create, enabled)
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "在启动本次攻击时同步创建并绑定一个新的项目级资源池",
                                )
                                .build(window, cx)
                            })
                            .on_change({
                                let owner = cx.weak_entity();
                                move |checked, _, window, cx| {
                                    if checked {
                                        let _ = owner.update(cx, |view, cx| {
                                            if view.project == project {
                                                view.create_intruder_pool(id, window, cx);
                                            }
                                        });
                                    }
                                }
                            }),
                    )
                    .child(create_pool_form(
                        form,
                        editable,
                        Some((cx.weak_entity(), project, id)),
                    )),
            )
            .into_any_element()
    }
}
