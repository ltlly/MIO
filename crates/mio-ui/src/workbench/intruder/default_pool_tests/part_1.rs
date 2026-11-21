use super::*;

#[gpui_kit::test]
fn new_button_waits_for_matching_default_pool_and_duplicate_click_is_inert(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "opening an old project never ensures a default pool"
    );
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    new_tab(&fixture, cx);
    new_tab(&fixture, cx);
    let (operation, cancel) = ensure(&mut fixture, cx);
    cx.read(|cx| {
        assert_eq!(fixture.view.read(cx).workspace, original);
        assert!(fixture.view.read(cx).intruder.default_pool.is_some());
    });
    ready(&fixture, Uuid::new_v4(), cx);
    assert!(!cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    ready(&fixture, operation, cx);
    let saved = saved_tab(&mut fixture);
    assert!(saved.intruder.tabs[0].spec.baseline_request);
    assert!(!saved.intruder.tabs[0].spec.save_to_project);
    assert!(saved.intruder.tabs[0].spec.draft.raw.is_empty());
    assert!(cancel.is_cancelled());
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.default_pool.is_none()));
    ready(&fixture, operation, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn failed_or_timed_out_default_pool_retains_first_transferred_draft_for_explicit_retry(
    cx: &mut TestAppContext,
) {
    for failure in ["error", "timeout", "missing default"] {
        let mut fixture = intruder_fixture(cx, vec![], 64);
        let original = cx.read(|cx| fixture.view.read(cx).workspace.tabs[0].draft.clone());
        crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
        cx.update_window(fixture.window, |_, window, cx| {
            window.press(crate::kbd!("i"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let (operation, cancel) = ensure(&mut fixture, cx);
        let changed = "GET /newer-source HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n";
        cx.update_window(fixture.window, |_, window, cx| {
            let id = fixture.view.read(cx).workspace.active.unwrap();
            let input = fixture.view.read(cx).tab_editors[&id]
                .request
                .state()
                .clone();
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(changed, cx);
            window.press(crate::kbd!("i"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let commands = drain(&mut fixture.commands);
        let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
            panic!(
                "the duplicate transfer only saves its source edit; it cannot replace the first draft or re-ensure"
            )
        };
        assert_eq!(*project, fixture.project);
        assert_eq!(workspace.tabs[0].draft.raw, changed.as_bytes());
        emit(
            &fixture,
            Event::WorkspaceChanged {
                project: *project,
                workspace: workspace.clone(),
            },
            cx,
        );
        match failure {
            "timeout" => {
                cx.dispatcher
                    .scheduler()
                    .clock()
                    .advance(DEADLINE + Duration::from_secs(1));
                cx.run_until_parked();
            }
            "missing default" => {
                let mut catalog = default_pool_catalog();
                catalog.default_pool_id = None;
                emit(
                    &fixture,
                    Event::IntruderDefaultPoolReady {
                        project: fixture.project,
                        operation_id: operation,
                        result: Ok(catalog),
                    },
                    cx,
                );
            }
            _ => emit(
                &fixture,
                Event::IntruderDefaultPoolReady {
                    project: fixture.project,
                    operation_id: operation,
                    result: Err("fixture failure".into()),
                },
                cx,
            ),
        }
        assert!(cancel.is_cancelled());
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "failure never retries automatically"
        );
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(
                view.intruder.default_pool.as_ref().unwrap().draft.raw,
                original.raw
            );
            assert!(
                view.intruder
                    .default_pool
                    .as_ref()
                    .unwrap()
                    .operation
                    .is_none()
            );
            assert!(view.workspace.intruder.tabs.is_empty());
            assert!(view.error.as_ref().unwrap().contains("重试"));
        });
        new_tab(&fixture, cx);
        let (retry, retry_cancel) = ensure(&mut fixture, cx);
        assert_ne!(retry, operation);
        ready(&fixture, operation, cx);
        assert!(drain(&mut fixture.commands).is_empty());
        assert!(!retry_cancel.is_cancelled());
        ready(&fixture, retry, cx);
        let saved = saved_tab(&mut fixture);
        assert_eq!(saved.intruder.tabs[0].spec.draft.raw, original.raw);
        assert_eq!(saved.tabs[0].draft.raw, changed.as_bytes());
    }
}

#[gpui_kit::test]
fn changed_transfer_after_default_failure_is_rejected_until_original_new_retry_completes(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.tabs[0].draft.clone());
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("i"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (first, first_cancel) = ensure(&mut fixture, cx);
    emit(
        &fixture,
        Event::IntruderDefaultPoolReady {
            project: fixture.project,
            operation_id: first,
            result: Err("fixture failure".into()),
        },
        cx,
    );
    assert!(first_cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    let changed = "POST /different-transfer HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n保留 B";
    cx.update_window(fixture.window, |_, window, cx| {
        let id = fixture.view.read(cx).workspace.active.unwrap();
        let input = fixture.view.read(cx).tab_editors[&id]
            .request
            .state()
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(changed, cx);
        window.press(crate::kbd!("i"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!(
            "a changed transfer after failure must only save source B, never ensure A under B's Send action"
        )
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(workspace.tabs[0].draft.raw, changed.as_bytes());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let pending = view.intruder.default_pool.as_ref().unwrap();
        assert_eq!(pending.draft.raw, original.raw);
        assert!(pending.operation.is_none());
        assert!(view.workspace.intruder.tabs.is_empty());
        let error = view
            .error
            .as_deref()
            .expect("the changed request needs an explicit visible refusal");
        assert!(error.contains("请求") && error.contains("已改变") && error.contains("保留"));
    });
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: *project,
            workspace: workspace.clone(),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    new_tab(&fixture, cx);
    let (retry, _) = ensure(&mut fixture, cx);
    assert_ne!(retry, first);
    ready(&fixture, retry, cx);
    let saved_original = saved_tab(&mut fixture);
    assert_eq!(saved_original.intruder.tabs[0].spec.draft.raw, original.raw);
    assert_eq!(saved_original.tabs[0].draft.raw, changed.as_bytes());
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved_original,
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("i"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (second, _) = ensure(&mut fixture, cx);
    assert_ne!(second, retry);
    ready(&fixture, second, cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!(
            "after A is resolved, transferring B creates exactly one additional tab without Start"
        )
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(workspace.intruder.tabs.len(), 2);
    assert_eq!(workspace.intruder.tabs[0].spec.draft.raw, original.raw);
    assert_eq!(
        workspace.intruder.tabs[1].spec.draft.raw,
        changed.as_bytes()
    );
    assert_ne!(workspace.intruder.tabs[0].id, workspace.intruder.tabs[1].id);
    assert_eq!(
        workspace.intruder.tabs[0].spec.resource_pool,
        workspace.intruder.tabs[1].spec.resource_pool
    );
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.default_pool.is_none()));
}

#[gpui_kit::test]
fn full_default_pool_queue_retains_draft_until_new_button_retry(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 1);
    cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .commands
            .try_send(Command::QueryIntruderResourcePools {
                project: fixture.project,
                query_revision: 999,
            })
            .unwrap()
    });
    new_tab(&fixture, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace.intruder.tabs.is_empty());
        let pending = view.intruder.default_pool.as_ref().unwrap();
        assert!(pending.cancel.is_cancelled() && pending.operation.is_none());
        assert!(view.error.as_ref().unwrap().contains("队列已满"));
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryIntruderResourcePools {
            query_revision: 999,
            ..
        }]
    ));
    new_tab(&fixture, cx);
    let (operation, _) = ensure(&mut fixture, cx);
    ready(&fixture, operation, cx);
    saved_tab(&mut fixture);
}

#[gpui_kit::test]
fn project_switch_and_disconnect_cancel_default_pool_without_late_tab(cx: &mut TestAppContext) {
    for disconnect in [false, true] {
        let mut fixture = intruder_fixture(cx, vec![], 64);
        new_tab(&fixture, cx);
        let (operation, cancel) = ensure(&mut fixture, cx);
        if disconnect {
            let (replacement, _) = mpsc::channel(1);
            drop(std::mem::replace(&mut fixture.events, replacement));
            cx.run_until_parked();
        } else {
            let next = ProjectKey {
                id: fixture.project.id,
                generation: fixture.project.generation + 1,
            };
            let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
            emit(
                &fixture,
                Event::ProjectOpened {
                    project: next,
                    path: None,
                    workspace,
                },
                cx,
            );
            assert!(
                drain(&mut fixture.commands)
                    .iter()
                    .all(|command| matches!(command, Command::QueryHttpHistory { .. }))
            );
        }
        assert!(cancel.is_cancelled());
        cx.update_window(fixture.window, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.on_intruder_default_pool_ready(
                    fixture.project,
                    operation,
                    Ok(default_pool_catalog()),
                    window,
                    cx,
                );
                assert!(view.workspace.intruder.tabs.is_empty());
                if disconnect {
                    assert!(
                        view.intruder
                            .default_pool
                            .as_ref()
                            .unwrap()
                            .operation
                            .is_none()
                    );
                } else {
                    assert!(view.intruder.default_pool.is_none());
                }
            });
        })
        .unwrap();
        assert!(drain(&mut fixture.commands).is_empty());
    }
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn cancelled_native_quit_revokes_default_pool_and_keeps_explicit_retry(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    new_tab(&fixture, cx);
    let (operation, cancel) = ensure(&mut fixture, cx);
    let probe = crate::native_terminate::tests::Probe::installed();
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    assert!(cancel.is_cancelled());
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert_eq!(probe.replies(), [false]);
    ready(&fixture, operation, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace.intruder.tabs.is_empty());
        assert!(view.intruder.default_pool.is_some());
        assert!(!view.shutdown.is_cancelled());
    });
    new_tab(&fixture, cx);
    let (retry, _) = ensure(&mut fixture, cx);
    assert_ne!(retry, operation);
    ready(&fixture, retry, cx);
    saved_tab(&mut fixture);
}

#[gpui_kit::test]
fn default_pool_ack_does_not_harvest_composition_or_change_legacy_reopened_tabs(
    cx: &mut TestAppContext,
) {
    let mut legacy = attack();
    legacy.spec.resource_pool = None;
    let mut fixture = intruder_fixture(cx, vec![legacy.clone()], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.press(crate::kbd!("w"), cx);
        window.press(crate::kbd!("shift-t"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    for command in drain(&mut fixture.commands) {
        let Command::SaveWorkspace { project, workspace } = command else {
            panic!("legacy restore must not ensure or bind a default pool")
        };
        emit(&fixture, Event::WorkspaceChanged { project, workspace }, cx);
    }
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace.intruder.tabs[0], legacy));
    new_tab(&fixture, cx);
    let (operation, cancel) = ensure(&mut fixture, cx);
    let input = cx
        .update_window(fixture.window, |_, window, cx| {
            let input = fixture.view.read(cx).intruder.editors[&legacy.id]
                .target
                .clone();
            window.click(("input", input.entity_id()), cx);
            input.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
            input
        })
        .unwrap();
    ready(&fixture, operation, cx);
    assert!(cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.tabs.len(), 1);
        assert_eq!(view.workspace.intruder.tabs[0].spec.resource_pool, None);
        assert!(
            view.intruder
                .default_pool
                .as_ref()
                .unwrap()
                .operation
                .is_none()
        );
    })
    .unwrap();
}
