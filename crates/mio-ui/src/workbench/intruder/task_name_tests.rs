use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use mio_core::{IntruderRetention, IntruderRunState};

use super::*;
use crate::workbench::intruder::tests::{attack, intruder_fixture};
use crate::workbench::tests::{Fixture, drain};

fn setup(
    cx: &mut TestAppContext,
) -> (
    Fixture,
    IntruderRunSummary,
    AnyWindowHandle,
    Entity<TaskNameWindow>,
) {
    let tab = attack();
    let run = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: Uuid::new_v4(),
        tab_id: tab.id,
        revision: 5,
        spec_revision: tab.spec.revision,
        total: 2,
        dispatched: 2,
        completed: 2,
        state: IntruderRunState::Completed,
        started_unix_ms: 1,
        retention: IntruderRetention::Project,
        title: "原任务名".into(),
        title_revision: 3,
    };
    let mut fixture = intruder_fixture(cx, vec![tab], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.intruder.results.runs.rows = vec![run.clone()];
            view.open_intruder_task_name(run.id, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .intruder
            .task_name
            .as_ref()
            .unwrap()
            .dialog
            .unwrap()
    });
    let content = cx
        .update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            window
                .root::<gpui_kit::base::Root>()
                .unwrap()
                .unwrap()
                .read(cx)
                .view()
                .clone()
                .downcast::<TaskNameWindow>()
                .unwrap()
        })
        .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    (fixture, run, dialog, content)
}

fn type_name(
    dialog: AnyWindowHandle,
    content: &Entity<TaskNameWindow>,
    title: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(dialog, |_, window, cx| {
        let input = content.read(cx).input.clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(title, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(dialog: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(dialog, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
}

fn confirm(
    fixture: &mut Fixture,
    run: &IntruderRunSummary,
    dialog: AnyWindowHandle,
    title: &str,
    cx: &mut TestAppContext,
) -> Uuid {
    click(dialog, "intruder-task-name-confirm", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::RenameIntruder {
            project,
            run_id,
            operation_id,
            expected_title_revision,
            title: sent,
        },
    ] = commands.as_slice()
    else {
        panic!("Confirm must emit one correlated rename command")
    };
    assert_eq!(
        (*project, *run_id, *expected_title_revision, sent.as_str()),
        (fixture.project, run.id, run.title_revision, title)
    );
    *operation_id
}

fn ack(
    fixture: &Fixture,
    run_id: Uuid,
    operation: Uuid,
    result: Result<IntruderRunSummary, String>,
    cx: &mut TestAppContext,
) {
    fixture
        .events
        .try_send(Event::IntruderRenamed {
            project: fixture.project,
            run_id,
            operation_id: operation,
            result,
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn task_name_cancel_and_confirm_use_independent_dialog_and_correlated_ack(cx: &mut TestAppContext) {
    let (mut fixture, run, dialog, content) = setup(cx);
    type_name(dialog, &content, "取消的名称", cx);
    click(dialog, "intruder-task-name-cancel", cx);
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder_task_name_pending());
        assert_eq!(view.intruder.results.runs.rows[0], run);
    });
    assert!(drain(&mut fixture.commands).is_empty());

    let (mut fixture, mut run, dialog, content) = setup(cx);
    type_name(dialog, &content, "中文任务名", cx);
    let operation = confirm(&mut fixture, &run, dialog, "中文任务名", cx);
    let mut renamed = run.clone();
    renamed.title = "中文任务名".into();
    renamed.title_revision += 1;
    ack(&fixture, run.id, Uuid::new_v4(), Ok(renamed.clone()), cx);
    cx.read(|cx| assert_eq!(fixture.view.read(cx).intruder.results.runs.rows[0], run));
    assert!(dialog.update(cx, |_, _, _| ()).is_ok());
    ack(&fixture, run.id, operation, Ok(renamed.clone()), cx);
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    run = renamed;
    cx.read(|cx| {
        assert_eq!(fixture.view.read(cx).intruder.results.runs.rows[0], run);
        assert!(!fixture.view.read(cx).intruder_task_name_pending());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn task_name_ime_and_error_preserve_input_then_allow_explicit_retry(cx: &mut TestAppContext) {
    let (mut fixture, run, dialog, content) = setup(cx);
    cx.update_window(dialog, |_, window, cx| {
        let input = content.read(cx).input.clone();
        input.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
        });
        window.click("intruder-task-name-confirm", cx);
        assert!(input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(content.read(cx).status, editing::COMPOSITION_PENDING));
    cx.update_window(dialog, |_, window, cx| {
        content.read(cx).input.clone().update(cx, |state, cx| {
            state.replace_text_in_range(None, "中文", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let operation = confirm(&mut fixture, &run, dialog, "中文", cx);
    ack(
        &fixture,
        run.id,
        operation,
        Err("磁盘不可写，名称未改变".into()),
        cx,
    );
    cx.read(|cx| {
        assert_eq!(content.read(cx).input.read(cx).value().as_ref(), "中文");
        assert!(content.read(cx).status.contains("磁盘不可写"));
        assert!(!content.read(cx).pending);
    });
    let commands = drain(&mut fixture.commands);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryIntruderRuns { .. }))
    );
    let retry = confirm(&mut fixture, &run, dialog, "中文", cx);
    assert_ne!(retry, operation);
}

#[gpui_kit::test]
fn task_name_timeout_late_ack_and_disconnect_have_bounded_exit(cx: &mut TestAppContext) {
    let (mut fixture, run, dialog, content) = setup(cx);
    type_name(dialog, &content, "迟到名称", cx);
    let operation = confirm(&mut fixture, &run, dialog, "迟到名称", cx);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_secs(31));
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(content.read(cx).status.contains("超时"));
        assert_eq!(content.read(cx).input.read(cx).value().as_ref(), "迟到名称");
    });
    let mut renamed = run.clone();
    renamed.title = "迟到名称".into();
    renamed.title_revision += 1;
    ack(&fixture, run.id, operation, Ok(renamed.clone()), cx);
    assert!(dialog.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).intruder.results.runs.rows[0], renamed));
    click(dialog, "intruder-task-name-cancel", cx);
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    assert!(drain(&mut fixture.commands).is_empty());

    let (mut fixture, run, dialog, content) = setup(cx);
    type_name(dialog, &content, "未确认名称", cx);
    confirm(&mut fixture, &run, dialog, "未确认名称", cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            view.backend_disconnected = true;
            view.intruder_task_name_disconnected(cx);
        })
    });
    cx.run_until_parked();
    cx.read(|cx| assert!(content.read(cx).status.contains("未确认")));
    click(dialog, "intruder-task-name-cancel", cx);
    cx.read(|cx| assert!(!fixture.view.read(cx).intruder_task_name_pending()));
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
}

#[gpui_kit::test]
fn task_name_ack_keeps_edits_and_composition_made_after_confirm(cx: &mut TestAppContext) {
    for composing in [false, true] {
        let (mut fixture, mut run, dialog, content) = setup(cx);
        let input = cx.read(|cx| content.read(cx).input.clone());
        type_name(dialog, &content, "已提交 A", cx);
        let first = confirm(&mut fixture, &run, dialog, "已提交 A", cx);
        if composing {
            cx.update_window(dialog, |_, window, cx| {
                window.click(("input", input.entity_id()), cx);
                window.press(crate::kbd!("a"), cx);
                input.update(cx, |state, cx| {
                    state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
                });
            })
            .unwrap();
        } else {
            type_name(dialog, &content, "继续编辑 B", cx);
        }
        let mut first_ack = run.clone();
        first_ack.title = "已提交 A".into();
        first_ack.title_revision += 1;
        ack(&fixture, run.id, first, Ok(first_ack.clone()), cx);
        assert!(
            dialog.update(cx, |_, _, _| ()).is_ok(),
            "ACK A must not close newer input, composing={composing}"
        );
        cx.update_window(dialog, |_, window, cx| {
            assert_eq!(content.read(cx).input.entity_id(), input.entity_id());
            assert!(!content.read(cx).pending);
            assert_eq!(
                fixture.view.read(cx).intruder.results.runs.rows[0],
                first_ack
            );
            assert_eq!(
                input.read(cx).value().as_ref(),
                if composing { "zhong" } else { "继续编辑 B" }
            );
            assert_eq!(
                input.update(cx, |state, cx| state
                    .marked_text_range(window, cx)
                    .is_some()),
                composing
            );
            if composing {
                input.update(cx, |state, cx| {
                    state.replace_text_in_range(None, "继续编辑 B", window, cx)
                });
            }
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "the ACK must not automatically write the newer input"
        );
        run = first_ack.clone();
        let second = confirm(&mut fixture, &run, dialog, "继续编辑 B", cx);
        assert_ne!(first, second);
        ack(&fixture, run.id, first, Ok(first_ack), cx);
        cx.read(|cx| {
            assert!(
                content.read(cx).pending,
                "a repeated old ACK cannot finish B"
            );
            assert_eq!(fixture.view.read(cx).intruder.results.runs.rows[0], run);
            assert_eq!(
                content.read(cx).input.read(cx).value().as_ref(),
                "继续编辑 B"
            );
        });
        assert!(drain(&mut fixture.commands).is_empty());
        let mut second_ack = run;
        second_ack.title = "继续编辑 B".into();
        second_ack.title_revision += 1;
        ack(&fixture, second_ack.id, second, Ok(second_ack), cx);
        assert!(dialog.update(cx, |_, _, _| ()).is_err());
    }
}

#[gpui_kit::test]
fn task_name_error_after_cancel_releases_guard_and_replaces_waiting_footer(
    cx: &mut TestAppContext,
) {
    let (mut fixture, run, dialog, content) = setup(cx);
    type_name(dialog, &content, "提交后关闭", cx);
    let operation = confirm(&mut fixture, &run, dialog, "提交后关闭", cx);
    click(dialog, "intruder-task-name-cancel", cx);
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| assert!(fixture.view.read(cx).intruder_task_name_pending()));
    ack(
        &fixture,
        run.id,
        operation,
        Err("磁盘不可写，名称未改变".into()),
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.intruder.task_name.is_none());
        assert!(!view.intruder_task_name_pending());
        assert_eq!(view.intruder.results.runs.rows[0], run);
        let footer = window.find("project-status");
        let label = footer
            .label()
            .expect("the main window exposes the rename outcome");
        assert!(
            label.contains("磁盘不可写"),
            "the closed dialog's error must remain visible: {label}"
        );
        assert!(
            !label.contains("等待回执"),
            "an acknowledged failure must retire the waiting state: {label}"
        );
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryIntruderRuns { .. })),
        "failure may refresh summaries but must not retry the write"
    );
}

#[gpui_kit::test]
fn task_name_rechecks_input_after_ack_before_deferred_window_close(cx: &mut TestAppContext) {
    let (mut fixture, mut run, dialog, content) = setup(cx);
    type_name(dialog, &content, "提交 A", cx);
    let operation = confirm(&mut fixture, &run, dialog, "提交 A", cx);
    run.title = "提交 A".into();
    run.title_revision += 1;
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::IntruderRenamed {
                    project: fixture.project,
                    run_id: run.id,
                    operation_id: operation,
                    result: Ok(run.clone()),
                },
                window,
                cx,
            );
        });
        // The ACK has queued its owner-window close; input arrives before that deferred callback.
        dialog
            .update(cx, |_, window, cx| {
                let input = content.read(cx).input.clone();
                window.click(("input", input.entity_id()), cx);
                window.press(crate::kbd!("a"), cx);
                window.input("ACK 后的新输入", cx);
            })
            .unwrap();
    })
    .unwrap();
    cx.run_until_parked();
    assert!(dialog.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        assert_eq!(
            content.read(cx).input.read(cx).value().as_ref(),
            "ACK 后的新输入"
        );
        assert!(!content.read(cx).pending);
        assert_eq!(fixture.view.read(cx).intruder.results.runs.rows[0], run);
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
