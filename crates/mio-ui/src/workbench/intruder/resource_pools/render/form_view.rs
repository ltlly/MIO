use gpui_kit::base::CheckboxState;

use super::super::form::DelayKind;
use super::*;

type FormTarget = (gpui_kit::WeakEntity<Workbench>, Option<ProjectKey>, TabId);

#[derive(Clone, Copy)]
pub(in crate::workbench) enum PoolFormAction {
    Flag(PoolFlag, bool),
    Delay(DelayKind),
}

pub(in crate::workbench) type PoolFormChange =
    std::rc::Rc<dyn Fn(PoolFormAction, &mut Window, &mut App)>;

pub(super) fn create_pool_form(
    form: &CreatePoolForm,
    editable: bool,
    target: Option<FormTarget>,
) -> AnyElement {
    let change: PoolFormChange = std::rc::Rc::new(move |action, window, cx| {
        if let Some((owner, project, id)) = &target {
            let _ = owner.update(cx, |view, cx| {
                if view.project != *project {
                    return;
                }
                match action {
                    PoolFormAction::Flag(flag, checked) => {
                        view.change_intruder_pool_flag(*id, flag, checked, window, cx)
                    }
                    PoolFormAction::Delay(kind) => {
                        view.change_intruder_pool_delay(*id, kind, window, cx)
                    }
                }
            });
        }
    });
    render_pool_form(form, editable, false, change)
}

pub(in crate::workbench) fn render_pool_form(
    form: &CreatePoolForm,
    editable: bool,
    management: bool,
    change: PoolFormChange,
) -> AnyElement {
    let compact = management && theme::ui_scale() <= 1.0;
    let checkbox = |key: &'static str, label: &'static str, value: bool, available: bool, flag| {
        let change = change.clone();
        pool_checkbox(key, label, value, available)
            .when(compact, |control| control.h(px(22.)).py_0())
            .on_change(move |state, _, window, cx| {
                change(
                    PoolFormAction::Flag(flag, state == CheckboxState::Checked),
                    window,
                    cx,
                );
            })
    };
    let mut choices = div()
        .flex()
        .flex_col()
        .pl(px(24.))
        .gap(px(if management { 2. } else { 7. }));
    for (key, label, kind) in [
        ("intruder-pool-fixed", "固定", DelayKind::Fixed),
        ("intruder-pool-random", "随机变化", DelayKind::Random),
        (
            "intruder-pool-increment",
            "每次增加间隔",
            DelayKind::Increment,
        ),
    ] {
        let change = change.clone();
        let option = pool_radio(
            key,
            label,
            form.delay_kind == kind,
            editable && form.delay_enabled,
        )
        .when(compact, |control| control.h(px(22.)))
        .when(management && kind == DelayKind::Increment, |control| {
            control
                .min_w(theme::scaled_px(204.).max(px(204.)))
                .flex_shrink_0()
        })
        .on_change(move |checked, _, window, cx| {
            if checked {
                change(PoolFormAction::Delay(kind), window, cx);
            }
        });
        choices = choices.child(div().flex().items_center().gap(px(6.)).child(option).when(
            kind == DelayKind::Increment,
            |row| {
                row.child(field(
                    &form.increment,
                    "资源池间隔增量（毫秒）",
                    editable && form.delay_enabled && form.delay_kind == DelayKind::Increment,
                    if management { 58. } else { 70. },
                ))
                .child("毫秒")
            },
        ));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(if management { 3. } else { 10. }))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .when(management, |row| row.mb(px(8.)))
                .child("名称：")
                .child(field(&form.name, "资源池名称", editable, 260.)),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(
                    checkbox(
                        "intruder-pool-cap-enabled",
                        "最大并发请求数：",
                        form.cap_enabled,
                        editable,
                        PoolFlag::Cap,
                    )
                    .when(management, |control| {
                        control
                            .min_w(theme::scaled_px(226.).max(px(226.)))
                            .flex_shrink_0()
                    }),
                )
                .child(field(
                    &form.cap,
                    "资源池最大并发请求数",
                    editable && form.cap_enabled,
                    if management { 58. } else { 72. },
                )),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(
                    checkbox(
                        "intruder-pool-delay-enabled",
                        "请求间隔：",
                        form.delay_enabled,
                        editable,
                        PoolFlag::Delay,
                    )
                    .when(management, |control| {
                        control
                            .min_w(theme::scaled_px(226.).max(px(226.)))
                            .flex_shrink_0()
                    }),
                )
                .child(field(
                    &form.delay,
                    "资源池请求间隔（毫秒）",
                    editable && form.delay_enabled,
                    if management { 58. } else { 72. },
                ))
                .child("毫秒"),
        )
        .child(choices)
        .child(checkbox(
            "intruder-pool-auto",
            "自动节流",
            form.automatic,
            editable,
            PoolFlag::Auto,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .pl(px(24.))
                .gap(px(if management { 2. } else { 7. }))
                .child(checkbox(
                    "intruder-pool-429",
                    "429",
                    form.status_429,
                    editable && form.automatic,
                    PoolFlag::Status429,
                ))
                .child(checkbox(
                    "intruder-pool-503",
                    "503",
                    form.status_503,
                    editable && form.automatic,
                    PoolFlag::Status503,
                ))
                .child(checkbox(
                    "intruder-pool-other-enabled",
                    "其它",
                    form.other_enabled,
                    editable && form.automatic,
                    PoolFlag::Other,
                )),
        )
        .child(
            div()
                .pl(px(24.))
                .flex()
                .flex_col()
                .gap(px(if management { 2. } else { 5. }))
                .child("CSV 格式（例如 504,505）")
                .child(field(
                    &form.other,
                    "自动节流其它状态码",
                    editable && form.automatic && form.other_enabled,
                    270.,
                )),
        )
        .when_some(form.error.clone().filter(|_| editable), |this, error| {
            this.child(
                div()
                    .id("intruder-pool-form-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .text_color(theme::danger_fg())
                    .child(error),
            )
        })
        .into_any_element()
}
