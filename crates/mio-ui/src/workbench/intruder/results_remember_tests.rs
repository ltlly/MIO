use std::time::Duration;

use mio_core::{
    FinishedAttackClose as Finished, RunningAttackClose as Running, UserPreferences,
    UserPreferencesState,
};

use super::*;

#[path = "results_remember_tests/part_1.rs"]
mod part_1;
#[path = "results_remember_tests/part_2.rs"]
mod part_2;

pub(super) fn query_preferences(fixture: &mut Fixture, cx: &mut TestAppContext) -> Uuid {
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    let commands = drain(&mut fixture.commands);
    let [Command::QueryUserPreferences { operation_id }] = commands.as_slice() else {
        panic!("one correlated preference query expected");
    };
    *operation_id
}

pub(super) fn setup(
    terminal: bool,
    cx: &mut TestAppContext,
) -> (Fixture, Started, UserPreferences) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    if terminal {
        let revision = poll(&mut fixture, &started, cx);
        let query = cx.read(|cx| {
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .query
                .pending
                .as_ref()
                .expect("terminal setup must await its display query")
                .query
                .clone()
        });
        assert_eq!(
            (
                query.project,
                query.run_id,
                query.view_id,
                query.query_revision
            ),
            (
                fixture.project,
                started.summary.id,
                started.summary.id,
                revision
            )
        );
        assert_eq!(
            (query.offset, query.limit, query.selected_ordinal),
            (0, PAGE_SIZE, None)
        );
        started.summary.revision += 1;
        started.summary.state = IntruderRunState::Stopped;
        started.summary.retention = mio_core::IntruderRetention::Session;
        emit(
            &fixture,
            Event::IntruderResultView {
                query,
                result: Ok(mio_core::IntruderResultViewPage {
                    run: started.summary.clone(),
                    offset: 0,
                    total: 0,
                    rows: vec![],
                    next_offset: None,
                    selected_rank: None,
                    selected_row: None,
                    persisted: false,
                }),
            },
            cx,
        );
    }
    let confirmed = UserPreferences {
        version: 1,
        revision: 4,
        running: if terminal {
            Running::Continue
        } else {
            Running::Ask
        },
        finished: if terminal {
            Finished::Ask
        } else {
            Finished::KeepMemory
        },
        ..UserPreferences::default()
    };
    let operation = query_preferences(&mut fixture, cx);
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: operation,
            state: UserPreferencesState {
                preferences: confirmed.clone(),
                load_error: None,
            },
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    if terminal {
        // UI-only named-project fixture; no file is created or read by this test.
        cx.update(|cx| {
            fixture.view.update(cx, |view, _| {
                view.project_path = Some(PathBuf::from("/tmp/mio-remember-mapping.mio"));
            });
        });
    }
    (fixture, started, confirmed)
}

pub(super) fn remember(cx: &mut TestAppContext) -> AnyWindowHandle {
    let dialog = close_dialog(cx).expect("close dialog must be visible");
    cx.update_window(dialog, |_, window, cx| {
        assert_eq!(
            window.find("intruder-close-remember").checked(),
            Some(false)
        );
        window.click("intruder-close-remember", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-close-remember").checked(), Some(true));
    })
    .unwrap();
    dialog
}

pub(super) fn take_remember(
    fixture: &mut Fixture,
    before: &UserPreferences,
    running: Running,
    finished: Finished,
) -> (Uuid, UserPreferences, Vec<Command>) {
    let mut commands = drain(&mut fixture.commands);
    assert!(
        !commands.is_empty(),
        "Remember must submit a preference write"
    );
    let Command::SetUserPreferences {
        operation_id,
        expected_revision,
        running: actual_running,
        finished: actual_finished,
    } = commands.remove(0)
    else {
        panic!("the remembered choice must be admitted before its close action");
    };
    assert!(!operation_id.is_nil());
    assert_eq!(expected_revision, before.revision);
    assert_eq!((actual_running, actual_finished), (running, finished));
    (
        operation_id,
        UserPreferences {
            revision: before.revision + 1,
            running,
            finished,
            ..before.clone()
        },
        commands,
    )
}

pub(super) fn mapped_choice(
    terminal: bool,
    answer: &str,
    running: Running,
    finished: Finished,
    cx: &mut TestAppContext,
) {
    let (mut fixture, started, confirmed) = setup(terminal, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    ask_to_close(&started, true, cx);
    remember(cx);
    answer_close_dialog(answer, cx);
    let (_, _, actions) = take_remember(&mut fixture, &confirmed, running, finished);
    assert!(!has_close_dialog(cx));
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&confirmed));
        assert_eq!(view.workspace, workspace);
        assert!(!view.shutdown.is_cancelled());
    });
    match (terminal, answer) {
        (false, "在后台继续运行") => {
            assert!(started.window.update(cx, |_, _, _| ()).is_err());
            assert!(!started.cancel.is_cancelled());
            assert!(
                matches!(actions.as_slice(), [Command::QueryIntruderResults { project, run_id, after: None, limit: 1, .. }] if *project == fixture.project && *run_id == started.summary.id)
            );
        }
        (false, "丢弃") => {
            assert!(started.cancel.is_cancelled());
            assert!(started.window.update(cx, |_, _, _| ()).is_ok());
            assert!(
                matches!(actions.as_slice(), [Command::StopIntruder { project, run_id }] if *project == fixture.project && *run_id == started.summary.id)
            );
        }
        (true, "保存到项目文件") => {
            assert!(started.window.update(cx, |_, _, _| ()).is_ok());
            let [
                command @ Command::SetIntruderRetention {
                    project,
                    run_id,
                    retention: mio_core::IntruderRetention::Project,
                    expected_revision,
                    ..
                },
            ] = actions.as_slice()
            else {
                panic!("Remember Save must first promote its Session attack");
            };
            assert_eq!(
                (*project, *run_id, *expected_revision),
                (
                    fixture.project,
                    started.summary.id,
                    started.summary.revision
                )
            );
            assert!(!cx.did_prompt_for_new_path());
            emit(&fixture, retain_reply(command, &started.summary), cx);
            assert!(started.window.update(cx, |_, _, _| ()).is_ok());
            let commands = drain(&mut fixture.commands);
            assert_eq!(
                commands
                    .iter()
                    .filter(|command| matches!(command,
                    Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }
                        if *project == fixture.project))
                    .count(),
                1,
                "the promotion ACK must submit exactly one project-file save"
            );
            assert!(commands.iter().all(|command| match command {
                Command::SaveProject {
                    websocket_notes: _,
                    ext_http_notes: _,
                    ext_http_highlights: _,
                    project,
                    path: None,
                    workspace: None,
                } => *project == fixture.project,
                Command::QueryHttpHistory { .. } | Command::QueryIntruderRuns { .. } => true,
                Command::QueryIntruderResults {
                    project,
                    run_id,
                    after: None,
                    limit: 1,
                    ..
                } => {
                    *project == fixture.project && *run_id == started.summary.id
                }
                _ => false,
            }));
            emit(
                &fixture,
                Event::ProjectSaved {
                    websocket_notes: vec![],
                    ext_http_notes: Vec::new(),
                    ext_http_highlights: Vec::new(),
                    project: fixture.project,
                    path: PathBuf::from("/tmp/mio-remember-mapping.mio"),
                    total_bytes: 128,
                },
                cx,
            );
            assert!(started.window.update(cx, |_, _, _| ()).is_err());
            let mut promoted_workspace = workspace.clone();
            promoted_workspace.revision += 1;
            cx.read(|cx| {
                let view = fixture.view.read(cx);
                assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&confirmed));
                assert_eq!(view.workspace, promoted_workspace);
                assert!(!view.intruder.session_runs.contains(&started.summary.id));
                assert!(view.intruder_result_close.is_none());
            });
            assert!(
                drain(&mut fixture.commands).is_empty(),
                "project confirmation must not retry the still-unconfirmed Remember write"
            );
        }
        (true, "保留在内存") => {
            assert!(started.window.update(cx, |_, _, _| ()).is_err());
            assert!(
                actions.is_empty(),
                "already-Session Keep only detaches, without replaying retention"
            );
            cx.read(|cx| {
                assert!(
                    fixture
                        .view
                        .read(cx)
                        .intruder
                        .session_runs
                        .contains(&started.summary.id)
                )
            });
        }
        (true, "丢弃") => {
            assert!(started.window.update(cx, |_, _, _| ()).is_ok());
            assert!(
                matches!(actions.as_slice(), [Command::DiscardIntruder { project, run_id, expected_revision, .. }] if *project == fixture.project && *run_id == started.summary.id && *expected_revision == started.summary.revision)
            );
        }
        _ => unreachable!(),
    }
    if terminal {
        assert!(!started.cancel.is_cancelled());
    }
}
