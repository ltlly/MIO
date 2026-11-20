use super::*;

#[gpui_kit::test]
fn incomplete_specs_stay_editable_and_saved_but_cannot_start(cx: &mut TestAppContext) {
    let mut unfinished = attack();
    unfinished.spec.positions = vec![Position { start: 900, end: 1 }];
    unfinished.spec.payload_sets.clear();
    unfinished.spec.draft.target.clear();
    let mut fixture = intruder_fixture(cx, vec![unfinished.clone()], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            assert!(view.flush_workspace(window, cx));
            assert!(view.save_workspace(cx));
        });
    })
    .unwrap();
    let draft = saved(&mut fixture);
    assert_eq!(draft.intruder.tabs, vec![unfinished.clone()]);
    acknowledge(&fixture, draft, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    start(&fixture, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.tabs, vec![unfinished]);
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("未开始"))
        );
        assert!(view.intruder.starts.is_empty());
    });
}

#[gpui_kit::test]
fn exhausted_intruder_revision_prevents_partial_repeater_capture(cx: &mut TestAppContext) {
    let mut full = attack();
    full.spec.revision = i64::MAX as u64;
    let id = full.id;
    let mut fixture = intruder_fixture(cx, vec![full], 64);
    let stored = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.tab_editors[&stored.active.unwrap()]
                .request
                .raw
                .update(cx, |state, cx| {
                    state.set_value("GET /repeater HTTP/1.1\r\n\r\n", window, cx);
                });
            view.intruder.editors[&id].target.update(cx, |state, cx| {
                state.set_value("http://127.0.0.1:2", window, cx);
            });
            assert!(
                view.capture_drafts(window, cx)
                    .unwrap_err()
                    .contains("revision 已耗尽")
            );
            assert_eq!(view.workspace, stored);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn changing_any_attack_configuration_during_save_requires_a_new_explicit_start(
    cx: &mut TestAppContext,
) {
    for field in 0..6 {
        let original = attack();
        let id = original.id;
        let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
        start(&fixture, cx);
        let first = saved(&mut fixture);
        cx.update_window(fixture.window, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.change_intruder_spec(
                    id,
                    |spec| match field {
                        0 => spec.attack_type = AttackType::BatteringRam,
                        1 => spec.positions.clear(),
                        2 => spec.payload_sets[0].push(b"new value".to_vec()),
                        3 => spec.max_concurrent_requests = 2,
                        4 => spec.request_timeout_ms = 501,
                        _ => spec.update_host_header = false,
                    },
                    window,
                    cx,
                );
                assert!(view.intruder.starts.is_empty());
                assert_eq!(
                    view.workspace.intruder.tabs[0].spec.draft,
                    original.spec.draft
                );
                assert_eq!(
                    view.workspace.intruder.tabs[0].spec.revision,
                    original.spec.revision + 1
                );
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        acknowledge(&fixture, first, cx);
        let latest = saved(&mut fixture);
        acknowledge(&fixture, latest, cx);
        assert!(drain(&mut fixture.commands).is_empty());
    }
}

#[gpui_kit::test]
fn a_matching_revision_does_not_replace_full_spec_equality(cx: &mut TestAppContext) {
    for field in 0..4 {
        let mut fixture = intruder_fixture(cx, vec![attack()], 64);
        start(&fixture, cx);
        let mut different = saved(&mut fixture);
        let spec = &mut different.intruder.tabs[0].spec;
        match field {
            0 => spec.payload_sets[0][0].push(255),
            1 => spec.update_host_header = !spec.update_host_header,
            2 => spec.draft.version = HttpVersion::Http2,
            _ => {
                let closed = different.intruder.tabs.remove(0);
                different.intruder.closed.push(closed);
                different.intruder.active = None;
            }
        }
        acknowledge(&fixture, different, cx);
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            assert!(fixture.view.read(cx).intruder.starts.is_empty());
            assert!(
                fixture
                    .view
                    .read(cx)
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("未开始"))
            );
        });
    }
}

#[gpui_kit::test]
fn started_ack_updates_only_last_run_after_a_new_local_configuration(cx: &mut TestAppContext) {
    let original = attack();
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    start(&fixture, cx);
    let persisted = saved(&mut fixture);
    acknowledge(&fixture, persisted, cx);
    let commands = drain(&mut fixture.commands);
    let [Command::StartIntruder { run_id, cancel, .. }] = commands.as_slice() else {
        panic!("one Start after its save acknowledgement");
    };
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.change_intruder_spec(id, |spec| spec.update_host_header = false, window, cx);
        });
    })
    .unwrap();
    let latest = saved(&mut fixture);
    assert!(!latest.intruder.tabs[0].spec.update_host_header);
    assert!(
        !cancel.is_cancelled(),
        "an admitted run keeps its frozen configuration"
    );
    fixture
        .events
        .try_send(Event::IntruderStarted {
            project: fixture.project,
            run_id: *run_id,
            tab_id: id,
            spec_revision: original.spec.revision,
            result: Ok(IntruderRunSummary {
                resource_pool: None,
                resource_pool_revision: 0,
                baseline: None,
                id: *run_id,
                tab_id: id,
                title: String::new(),
                title_revision: 0,
                revision: 1,
                spec_revision: original.spec.revision,
                total: 2,
                dispatched: 0,
                completed: 0,
                state: IntruderRunState::Running,
                started_unix_ms: 1,
                retention: mio_core::IntruderRetention::Project,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id: queried_run,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("the pending workspace CAS owns the next save; only a status query is allowed");
    };
    assert_eq!((*project, *queried_run), (fixture.project, *run_id));
    assert!(*query_revision > 0);
    acknowledge(&fixture, latest.clone(), cx);
    let metadata = saved(&mut fixture);
    assert_eq!(metadata.intruder.tabs[0].spec, latest.intruder.tabs[0].spec);
    assert_eq!(metadata.intruder.tabs[0].last_run, Some(*run_id));
    acknowledge(&fixture, metadata, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn full_queue_before_save_retains_input_and_does_not_leave_a_future_start(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 1);
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
    start(&fixture, cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryIntruderRuns { .. }]
    ));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.intruder.starts.is_empty());
    });
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn project_save_as_captures_binary_intruder_and_closed_drafts_from_another_tool(
    cx: &mut TestAppContext,
) {
    let mut original = attack();
    original.spec.draft.raw.extend_from_slice(&[0, 255]);
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    let mut closed = attack();
    closed.spec.draft.target.clear();
    closed.spec.positions = vec![Position { start: 900, end: 1 }];
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, _| {
            assert_eq!(view.intruder.editors[&id].request.view, MessageView::Hex);
            view.workspace.intruder.closed.push(closed.clone());
            view.workspace_dirty = true;
        });
    })
    .unwrap();
    project_menu(&fixture, "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(PathBuf::from("/tmp/mio-intruder-ui-fixture.mio")));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveProject {
            workspace: Some(workspace),
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must carry one complete workspace snapshot without starting attacks");
    };
    assert_eq!(workspace.intruder.tabs, vec![original]);
    assert_eq!(workspace.intruder.closed, vec![closed]);
    assert_eq!(workspace.tabs.len(), 1);
    assert_eq!(workspace.tabs[0].title, "Repeater");
}

#[gpui_kit::test]
fn stale_editor_events_cannot_dirty_a_successor_project_reusing_the_same_tab_id(
    cx: &mut TestAppContext,
) {
    let original = attack();
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    let old = cx.read(|cx| {
        fixture.view.read(cx).intruder.editors[&id]
            .request
            .raw
            .clone()
    });
    let successor = ProjectKey {
        id: Uuid::new_v4(),
        generation: fixture.project.generation + 1,
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: successor,
            path: None,
            workspace: Workspace {
                intruder: IntruderWorkspace {
                    tabs: vec![original],
                    active: Some(id),
                    ..Default::default()
                },
                ..Default::default()
            },
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    let next = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        old.update(cx, |state, cx| {
            state.set_value("GET /obsolete HTTP/1.1\r\n\r\n", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(successor));
        assert_eq!(view.workspace, next);
        assert!(!view.intruder.editors[&id].dirty);
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn rejection_rebase_advances_spec_when_only_the_retained_draft_revision_is_newer(
    cx: &mut TestAppContext,
) {
    let mut original = attack();
    original.spec.revision = 5;
    original.spec.draft.revision = 2;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    start(&fixture, cx);
    let attempted = saved(&mut fixture);
    let mut stored = attempted.clone();
    stored.revision += 1;
    stored.intruder.tabs[0].spec.revision = 10;
    stored.intruder.tabs[0].spec.draft.revision = 1;
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: attempted.revision,
            stored,
            message: "synthetic concurrent configuration update".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let latest = &view.workspace.intruder.tabs[0].spec;
        assert_eq!(latest.draft.revision, 2, "retain the local draft revision");
        assert_eq!(
            latest.revision, 11,
            "a newer nested draft revision changes the complete spec"
        );
        assert_eq!(latest.draft.raw, original.spec.draft.raw);
        assert_eq!(latest.payload_sets, original.spec.payload_sets);
        assert!(view.intruder.starts.is_empty());
    });
}

#[gpui_kit::test]
fn rejected_save_then_repeater_open_preserves_intruder_raw_preedit(cx: &mut TestAppContext) {
    rejected_save_then_repeater_open_keeps_intruder_composition(cx, 0);
}

#[gpui_kit::test]
fn rejected_save_then_repeater_open_preserves_intruder_hex_preedit(cx: &mut TestAppContext) {
    rejected_save_then_repeater_open_keeps_intruder_composition(cx, 1);
}

#[gpui_kit::test]
fn rejected_save_then_repeater_open_preserves_intruder_target_preedit(cx: &mut TestAppContext) {
    rejected_save_then_repeater_open_keeps_intruder_composition(cx, 2);
}
