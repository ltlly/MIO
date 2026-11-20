use super::*;

#[test]
fn draft_changes_advance_both_revisions_and_configuration_changes_only_the_spec() {
    let original = attack().spec;
    assert_eq!(revised_spec(&original, original.clone()).unwrap(), original);
    for field in 0..3 {
        let mut next = original.clone();
        match field {
            0 => next.draft.raw.push(255),
            1 => next.draft.target = "unfinished target".into(),
            _ => next.draft.version = HttpVersion::Http2,
        }
        let changed = revised_spec(&original, next).unwrap();
        assert_eq!(changed.revision, 2);
        assert_eq!(changed.draft.revision, 2);
    }
    for field in 0..6 {
        let mut next = original.clone();
        match field {
            0 => next.attack_type = AttackType::Pitchfork,
            1 => next.positions.push(Position { start: 500, end: 1 }),
            2 => next.payload_sets.push(vec![vec![0, 255]]),
            3 => next.max_concurrent_requests = 0,
            4 => next.request_timeout_ms = 0,
            _ => next.update_host_header = false,
        }
        let changed = revised_spec(&original, next).unwrap();
        assert_eq!(changed.revision, 2);
        assert_eq!(changed.draft.revision, 1);
    }
}

#[test]
fn rejection_rebase_checks_both_revision_ceilings_without_rewriting_input() {
    let local = attack().spec;
    let mut stored = local.clone();
    stored.revision = i64::MAX as u64;
    let mut changed = local.clone();
    changed.draft.revision += 1;
    assert!(
        rebased_spec(&changed, &stored)
            .unwrap_err()
            .contains("revision 已耗尽")
    );
    assert_eq!(changed.draft.raw, local.draft.raw);
    assert_eq!(changed.draft.revision, 2);
    stored.revision = 5;
    stored.draft.revision = i64::MAX as u64;
    changed.draft.raw.push(255);
    assert!(
        rebased_spec(&changed, &stored)
            .unwrap_err()
            .contains("revision 已耗尽")
    );
    assert_eq!(changed.draft.raw.last(), Some(&255));
    assert_eq!(changed.revision, 1);
    assert_eq!(rebased_spec(&stored, &stored).unwrap(), stored);
}

#[gpui_kit::test]
fn start_requires_its_own_exact_save_ack_and_queues_only_one_attack(cx: &mut TestAppContext) {
    let original = attack();
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    start(&fixture, cx);
    let persisted = saved(&mut fixture);
    assert_eq!(persisted.intruder.tabs, vec![original.clone()]);
    start(&fixture, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, persisted.clone(), cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::StartIntruder {
            project,
            run_id,
            tab_id,
            spec,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("the acknowledged, unchanged input must start exactly once");
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(*tab_id, id);
    assert_eq!(**spec, original.spec);
    assert!(!cancel.is_cancelled());
    acknowledge(&fixture, persisted, cx);
    start(&fixture, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let summary = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: *run_id,
        tab_id: id,
        title: String::new(),
        title_revision: 0,
        revision: 1,
        spec_revision: spec.revision,
        total: 2,
        dispatched: 0,
        completed: 0,
        state: IntruderRunState::Running,
        started_unix_ms: 1,
        retention: mio_core::IntruderRetention::Project,
    };
    fixture
        .events
        .try_send(Event::IntruderStarted {
            project: fixture.project,
            run_id: Uuid::new_v4(),
            tab_id: id,
            spec_revision: spec.revision,
            result: Ok(summary.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    fixture
        .events
        .try_send(Event::IntruderStarted {
            project: fixture.project,
            run_id: *run_id,
            tab_id: id,
            spec_revision: spec.revision,
            result: Ok(summary),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project: saved_project,
            workspace: latest,
        },
        Command::QueryIntruderResults {
            project: queried_project,
            run_id: queried_run,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("Started must save its metadata and query only that run's status");
    };
    assert_eq!(
        (*saved_project, *queried_project),
        (fixture.project, fixture.project)
    );
    assert_eq!(*queried_run, *run_id);
    assert!(*query_revision > 0);
    assert_eq!(latest.intruder.tabs[0].last_run, Some(*run_id));
    assert_eq!(latest.intruder.tabs[0].spec, original.spec);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.starts.is_empty());
        assert_eq!(
            view.intruder.runs[run_id].summary.spec_revision,
            original.spec.revision
        );
    });
}

#[gpui_kit::test]
fn an_ack_already_in_flight_before_start_cannot_admit_the_attack(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    cx.update_window(fixture.window, |_, _, cx| {
        fixture
            .view
            .update(cx, |view, cx| assert!(view.save_workspace(cx)));
    })
    .unwrap();
    let earlier = saved(&mut fixture);
    start(&fixture, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, earlier.clone(), cx);
    let own_save = saved(&mut fixture);
    assert_eq!(own_save.revision, earlier.revision + 1);
    acknowledge(&fixture, own_save, cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::StartIntruder { .. }]
    ));
}

#[gpui_kit::test]
fn ack_rechecks_real_input_before_its_delayed_change_event(cx: &mut TestAppContext) {
    let original = attack();
    let id = original.id;
    for target in [false, true] {
        let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
        start(&fixture, cx);
        let persisted = saved(&mut fixture);
        cx.update_window(fixture.window, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                let editors = &view.intruder.editors[&id];
                if target {
                    editors.target.update(cx, |state, cx| {
                        state.set_value("http://127.0.0.1:2", window, cx);
                    });
                } else {
                    editors.request.raw.update(cx, |state, cx| {
                        state.set_value("POST /latest HTTP/1.1\r\n\r\nlatest", window, cx);
                    });
                }
                assert!(
                    !view.intruder.editors[&id].dirty,
                    "Change has not been delivered yet"
                );
                view.on_event(
                    Event::WorkspaceChanged {
                        project: fixture.project,
                        workspace: persisted,
                    },
                    window,
                    cx,
                );
                assert!(view.intruder.starts.is_empty());
                assert!(view.intruder.editors[&id].dirty);
                assert!(
                    view.error
                        .as_ref()
                        .is_some_and(|error| error.contains("未开始"))
                );
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "no stale Start and no automatic retry"
        );
    }
}

#[gpui_kit::test]
fn native_commit_during_ack_wait_revokes_start_without_discarding_input(cx: &mut TestAppContext) {
    let original = attack();
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original], 64);
    start(&fixture, cx);
    let persisted = saved(&mut fixture);
    let latest = "POST / HTTP/1.1\r\n\r\n已确认的中文🙂";
    cx.update_window(fixture.window, |_, window, cx| {
        let raw = fixture.view.read(cx).intruder.editors[&id]
            .request
            .raw
            .clone();
        raw.update(cx, |state, cx| {
            state.replace_text_in_range(
                Some(0..state.value().encode_utf16().count()),
                latest,
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.starts.is_empty()));
    acknowledge(&fixture, persisted, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.editors[&id]
                .request
                .raw
                .read(cx)
                .value()
                .as_ref(),
            latest
        );
    });
}

#[gpui_kit::test]
fn unfinished_composition_prevents_partial_capture_across_tools(cx: &mut TestAppContext) {
    let first = attack();
    let second = attack();
    let id = second.id;
    let mut fixture = intruder_fixture(cx, vec![first, second], 64);
    let stored = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            let repeater = &view.tab_editors[&stored.active.unwrap()].request.raw;
            repeater.update(cx, |state, cx| {
                state.set_value("POST /changed HTTP/1.1\r\n\r\n", window, cx)
            });
            view.intruder.editors[&id].target.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            });
            assert_eq!(
                view.capture_drafts(window, cx).unwrap_err(),
                editing::COMPOSITION_PENDING
            );
            assert_eq!(view.workspace, stored);
            view.start_intruder(window, cx);
            assert!(view.intruder.starts.is_empty());
            assert_eq!(view.workspace, stored);
            assert!(view.intruder.editors[&id].target.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }));
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn save_rejection_retains_draft_and_payloads_without_start_or_retry(cx: &mut TestAppContext) {
    let original = attack();
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    start(&fixture, cx);
    let rejected = saved(&mut fixture);
    let mut stored = rejected.clone();
    stored.revision += 1;
    stored.intruder.tabs[0].spec.revision = 5;
    stored.intruder.tabs[0].spec.draft.revision = 3;
    stored.intruder.tabs[0].spec.draft.raw.push(0);
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: rejected.revision,
            stored,
            message: "synthetic disk rejection".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.starts.is_empty());
        let latest = &view.workspace.intruder.tabs[0].spec;
        assert_eq!(latest.draft.raw, original.spec.draft.raw);
        assert_eq!(latest.payload_sets, original.spec.payload_sets);
        assert_eq!((latest.revision, latest.draft.revision), (6, 4));
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(
            view.intruder.editors[&id]
                .request
                .raw
                .read(cx)
                .value()
                .as_ref(),
            format!(
                "{}§x§",
                std::str::from_utf8(&original.spec.draft.raw[..original.spec.draft.raw.len() - 1])
                    .unwrap()
            )
        );
    });
    acknowledge(&fixture, rejected, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn full_command_queue_never_retries_a_failed_start_automatically(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 1);
    start(&fixture, cx);
    let persisted = saved(&mut fixture);
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, cx| {
            assert!(view.enqueue(
                Command::QueryIntruderRuns {
                    project: fixture.project,
                    query_revision: 1,
                    after: None,
                    limit: 1,
                },
                cx
            ));
        });
    })
    .unwrap();
    acknowledge(&fixture, persisted.clone(), cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryIntruderRuns { .. }]
    ));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.starts.is_empty());
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("队列已满"))
        );
    });
    acknowledge(&fixture, persisted, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn closed_tab_and_a_successor_project_cannot_receive_late_start_acks(cx: &mut TestAppContext) {
    for switch_project in [false, true] {
        let original = attack();
        let id = original.id;
        let mut fixture = intruder_fixture(cx, vec![original], 64);
        start(&fixture, cx);
        let persisted = saved(&mut fixture);
        cx.update_window(fixture.window, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                if switch_project {
                    let project = ProjectKey {
                        id: Uuid::new_v4(),
                        generation: fixture.project.generation + 1,
                    };
                    view.on_event(
                        Event::ProjectOpened {
                            project,
                            path: None,
                            workspace: Workspace::default(),
                        },
                        window,
                        cx,
                    );
                } else {
                    view.close_tab(id, window, cx);
                }
                view.on_event(
                    Event::WorkspaceChanged {
                        project: fixture.project,
                        workspace: persisted,
                    },
                    window,
                    cx,
                );
                assert!(!view.intruder.starts.contains_key(&id));
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            !drain(&mut fixture.commands)
                .iter()
                .any(|command| matches!(command, Command::StartIntruder { .. }))
        );
    }
}

#[gpui_kit::test]
fn disconnect_cancels_save_wait_and_unconfirmed_start_out_of_band(cx: &mut TestAppContext) {
    for admitted in [false, true] {
        let original = attack();
        let id = original.id;
        let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
        start(&fixture, cx);
        let persisted = saved(&mut fixture);
        let cancel = cx.read(|cx| fixture.view.read(cx).intruder.starts[&id].cancel.clone());
        if admitted {
            acknowledge(&fixture, persisted, cx);
            assert!(matches!(
                drain(&mut fixture.commands).as_slice(),
                [Command::StartIntruder { .. }]
            ));
        }
        drop(fixture.events);
        cx.run_until_parked();
        assert!(cancel.is_cancelled());
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.backend_disconnected && view.intruder.starts.is_empty());
            assert_eq!(view.workspace.intruder.tabs[0].spec, original.spec);
            assert!(view.intruder.editors.contains_key(&id));
        });
    }
}
