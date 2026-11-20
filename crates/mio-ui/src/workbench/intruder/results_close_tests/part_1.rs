use super::*;

pub(in crate::workbench::intruder) fn dashboard_tasks(
    cx: &mut TestAppContext,
) -> (Fixture, IntruderRunSummary, IntruderRunSummary) {
    let attack = attack();
    let tab_id = attack.id;
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let first = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: Uuid::new_v4(),
        tab_id,
        revision: 3,
        spec_revision: 1,
        total: 2,
        dispatched: 2,
        completed: 2,
        state: IntruderRunState::Completed,
        started_unix_ms: 1,
        retention: mio_core::IntruderRetention::Session,
        title: "未选中的任务".into(),
        title_revision: 1,
    };
    let second = IntruderRunSummary {
        id: Uuid::new_v4(),
        title: "当前任务".into(),
        started_unix_ms: 2,
        ..first.clone()
    };
    click(fixture.window, "tool-dashboard", cx);
    let revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryIntruderRuns { query_revision, .. } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: revision,
            result: Ok(IntruderRunPage {
                rows: vec![first.clone(), second.clone()],
                next_after: None,
            }),
        },
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(
            SharedString::from(format!("dashboard-intruder-{}", second.id)),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderResultView { query, cancel }] = commands.as_slice() else {
        panic!("the selected Dashboard task must issue exactly one bounded display query");
    };
    assert_eq!(
        (query.project, query.run_id, query.offset, query.limit),
        (fixture.project, second.id, 0, PAGE_SIZE)
    );
    assert!(!cancel.is_cancelled());
    emit(
        &fixture,
        Event::IntruderResultView {
            query: query.clone(),
            result: Ok(IntruderResultViewPage {
                run: second.clone(),
                offset: 0,
                total: 2,
                rows: (0..2)
                    .map(|ordinal| {
                        let result = row(ordinal, &flow(ordinal as usize));
                        let response_length = match &result.outcome {
                            mio_core::IntruderResultOutcome::Flow(summary)
                                if summary.status.is_some() =>
                            {
                                Some(summary.response_bytes)
                            }
                            _ => None,
                        };
                        IntruderResultViewRow {
                            ordinal,
                            result: Some(result),
                            payloads: vec![],
                            response_length,
                            notes_revision: 0,
                            notes_preview: String::new(),
                            highlight: None,
                            highlight_revision: 0,
                            grep_matches: Vec::new(),
                            grep_extracts: Vec::new(),
                            grep_payload_hits: Vec::new(),
                        }
                    })
                    .collect(),
                next_offset: None,
                selected_rank: None,
                selected_row: None,
                persisted: false,
            }),
        },
        cx,
    );
    drain(&mut fixture.commands);
    (fixture, first, second)
}

pub(in crate::workbench::intruder) fn dashboard_task_menu(
    fixture: &Fixture,
    run_id: Uuid,
    label: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(
            SharedString::from(format!("dashboard-intruder-options-{run_id}")),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let choice = window.find_by_label(label).bounds().center();
        window.pointer_down(choice, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(choice, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(in crate::workbench::intruder) fn save_started_from_dashboard(
    fixture: &mut Fixture,
    started: &Started,
    cx: &mut TestAppContext,
) {
    click(fixture.window, "tool-dashboard", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderRuns {
            project,
            query_revision,
            after: None,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Dashboard must query the current task list before explicit Save")
    };
    assert_eq!(*project, fixture.project);
    emit(
        fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(IntruderRunPage {
                rows: vec![started.summary.clone()],
                next_after: None,
            }),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    dashboard_task_menu(fixture, started.summary.id, "保存到项目文件", cx);
}

pub(in crate::workbench::intruder) struct Discard {
    pub(in crate::workbench::intruder) project: ProjectKey,
    pub(in crate::workbench::intruder) run_id: Uuid,
    pub(in crate::workbench::intruder) operation_id: Uuid,
    pub(in crate::workbench::intruder) expected_revision: u64,
    pub(in crate::workbench::intruder) workspace: Box<Workspace>,
}

pub(in crate::workbench::intruder) fn ask_to_close(
    started: &Started,
    native: bool,
    cx: &mut TestAppContext,
) {
    if native {
        let mut native = gpui_kit::VisualTestContext::from_window(started.window, cx);
        assert!(!native.simulate_close());
    } else {
        cx.update_window(started.window, |_, window, cx| {
            assert!(window.is_action_available(&CloseTab, cx));
            window.dispatch_action(Box::new(CloseTab), cx);
        })
        .unwrap();
    }
    cx.run_until_parked();
    assert!(has_close_dialog(cx));
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

pub(in crate::workbench::intruder) fn choose_discard(
    fixture: &mut Fixture,
    started: &Started,
    cx: &mut TestAppContext,
) {
    ask_to_close(started, false, cx);
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    answer_close_dialog("丢弃", cx);
    cx.run_until_parked();
    assert!(!has_close_dialog(cx));
    assert!(started.cancel.is_cancelled());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::StopIntruder { project, run_id }]
            if *project == fixture.project && *run_id == started.summary.id
    ));
}

pub(in crate::workbench::intruder) fn poll(
    fixture: &mut Fixture,
    started: &Started,
    cx: &mut TestAppContext,
) -> u64 {
    emit(
        fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, after) = query(fixture, started.summary.id);
    assert_eq!(after, None);
    revision
}

pub(in crate::workbench::intruder) fn stopped(
    fixture: &Fixture,
    started: &mut Started,
    revision: u64,
    cx: &mut TestAppContext,
) {
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Stopped;
    page(fixture, started, revision, vec![], None, cx);
}

pub(in crate::workbench::intruder) fn take_discard(
    fixture: &mut Fixture,
    started: &Started,
    cx: &mut TestAppContext,
) -> Discard {
    let commands = drain(&mut fixture.commands);
    // The main Repeater tab is an unsent draft; its deferred read does not mutate the attack.
    let commands = crate::workbench::tests::acknowledge_history_query(fixture, commands, cx);
    let [
        Command::DiscardIntruder {
            project,
            run_id,
            operation_id,
            expected_revision,
            workspace,
        },
    ] = commands.as_slice()
    else {
        let actual = commands
            .iter()
            .map(|command| match command {
                Command::DiscardIntruder { run_id, .. } => format!("DiscardIntruder({run_id})"),
                Command::QueryIntruderResults {
                    run_id,
                    query_revision,
                    limit,
                    ..
                } => format!("QueryIntruderResults({run_id}, rev={query_revision}, limit={limit})"),
                Command::QueryIntruderResultView { query, .. } => format!(
                    "QueryIntruderResultView({}, rev={}, offset={})",
                    query.run_id, query.query_revision, query.offset
                ),
                _ => format!("other {:?}", std::mem::discriminant(command)),
            })
            .collect::<Vec<_>>();
        let state = cx.read(|cx| {
            let workbench = fixture.view.read(cx);
            let view = &workbench.intruder.results.views[&started.summary.id];
            (
                view.summary.as_ref().map(|summary| summary.state.clone()),
                view.stop_requested,
                view.pending,
                view.query
                    .pending
                    .as_ref()
                    .map(|pending| pending.query.query_revision),
                workbench.intruder_result_close_pending(started.summary.id),
            )
        });
        panic!(
            "one terminal-only discard command, not a save or a resend, was expected; actual={actual:?}; state={state:?}"
        );
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    assert_eq!(*expected_revision, started.summary.revision);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(workspace.revision, view.workspace.revision + 1);
        assert_eq!(view.pending_workspace, Some(workspace.revision));
        assert!(view.intruder_close_owns_workspace());
        assert!(view.intruder.results.views.contains_key(run_id));
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    Discard {
        project: *project,
        run_id: *run_id,
        operation_id: *operation_id,
        expected_revision: *expected_revision,
        workspace: workspace.clone(),
    }
}

pub(in crate::workbench::intruder) fn receipt(discard: &Discard) -> IntruderDiscardReceipt {
    let mut workspace = *discard.workspace.clone();
    for tab in workspace
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut workspace.intruder.closed)
    {
        if tab.last_run == Some(discard.run_id) {
            tab.last_run = None;
        }
    }
    IntruderDiscardReceipt {
        revision: discard.expected_revision + 1,
        workspace,
        deleted_flows: 0,
        retained_flows: 0,
    }
}

pub(in crate::workbench::intruder) fn reply(
    discard: &Discard,
    result: Result<IntruderDiscardReceipt, String>,
) -> Event {
    Event::IntruderDiscarded {
        project: discard.project,
        run_id: discard.run_id,
        operation_id: discard.operation_id,
        result,
    }
}

pub(in crate::workbench::intruder) fn fill_commands(
    fixture: &Fixture,
    cx: &mut TestAppContext,
) -> usize {
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, _| {
            let capacity = view.commands.capacity();
            for revision in 0..capacity {
                view.commands
                    .try_send(Command::QueryIntruderRuns {
                        project: fixture.project,
                        query_revision: revision as u64,
                        after: None,
                        limit: 1,
                    })
                    .unwrap();
            }
            capacity
        })
    })
    .unwrap()
}

pub(in crate::workbench::intruder) fn type_target(
    fixture: &mut Fixture,
    intruder: bool,
    value: &str,
    cx: &mut TestAppContext,
) -> Entity<InputState> {
    if intruder {
        click(fixture.window, "tool-intruder", cx);
    } else {
        click(fixture.window, "tool-repeater", cx);
        let commands = drain(&mut fixture.commands);
        let _ = crate::workbench::tests::acknowledge_history_query(fixture, commands, cx);
    }
    let target = cx.read(|cx| {
        let view = fixture.view.read(cx);
        if intruder {
            view.intruder.editors[&view.workspace.intruder.active.unwrap()]
                .target
                .clone()
        } else {
            view.tab_editors[&view.workspace.active.unwrap()]
                .target
                .clone()
        }
    });
    if intruder {
        cx.update_window(fixture.window, |_, window, cx| {
            window.click(("input", target.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(value, cx);
        })
        .unwrap();
        cx.run_until_parked();
    } else {
        let tab_id = cx.read(|cx| fixture.view.read(cx).workspace.active.unwrap());
        cx.update_window(fixture.window, |_, window, cx| {
            target.update(cx, |state, cx| {
                state.set_value(value.to_owned(), window, cx);
            });
            fixture.view.update(cx, |view, cx| {
                if let Some(editors) = view.tab_editors.get_mut(&tab_id) {
                    editors.dirty = true;
                }
                cx.notify();
            });
        })
        .unwrap();
        cx.run_until_parked();
    }
    cx.read(|cx| assert_eq!(target.read(cx).value().as_str(), value));
    target
}

pub(in crate::workbench::intruder) fn retain_reply(
    command: &Command,
    before: &IntruderRunSummary,
) -> Event {
    let Command::SetIntruderRetention {
        project,
        run_id,
        operation_id,
        expected_revision,
        retention,
        workspace,
    } = command
    else {
        panic!("expected a retention command")
    };
    assert_eq!(*expected_revision, before.revision);
    let mut run = before.clone();
    run.revision += 1;
    run.retention = *retention;
    let mut workspace = *workspace.clone();
    if *retention == mio_core::IntruderRetention::Session {
        for tab in workspace
            .intruder
            .tabs
            .iter_mut()
            .chain(&mut workspace.intruder.closed)
        {
            if tab.last_run == Some(*run_id) {
                tab.last_run = None;
            }
        }
    }
    Event::IntruderRetentionChanged {
        project: *project,
        run_id: *run_id,
        operation_id: *operation_id,
        result: Ok(mio_core::IntruderRetentionReceipt {
            run,
            retention: *retention,
            workspace,
            session_logical_bytes: 128,
        }),
    }
}

pub(in crate::workbench::intruder) fn choose_terminal(
    fixture: &mut Fixture,
    started: &mut Started,
    answer: &str,
    cx: &mut TestAppContext,
) {
    let revision = poll(fixture, started, cx);
    stopped(fixture, started, revision, cx);
    ask_to_close(started, false, cx);
    answer_close_dialog(answer, cx);
    cx.run_until_parked();
}
