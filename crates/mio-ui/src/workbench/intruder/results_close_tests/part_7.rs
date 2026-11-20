use super::*;

#[gpui_kit::test]
fn session_settings_save_clears_waiting_footer_after_project_ack(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let flows = [flow(0), flow(1)];
    bootstrap(&mut fixture, &mut started, &flows, cx);
    let path = PathBuf::from("/tmp/mio-intruder-save-status.mio");
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(path.clone());
        });
    });
    let query_revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Stopped;
    started.summary.retention = mio_core::IntruderRetention::Session;
    let rows = vec![row(0, &flows[0]), row(1, &flows[1])];
    let display = cx.read(|cx| {
        fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .query
            .pending
            .as_ref()
            .unwrap()
            .query
            .clone()
    });
    assert_eq!(display.query_revision, query_revision);
    emit(
        &fixture,
        Event::IntruderResultView {
            query: display,
            result: Ok(IntruderResultViewPage {
                run: started.summary.clone(),
                offset: 0,
                total: rows.len(),
                rows: rows
                    .iter()
                    .map(|row| {
                        let response_length = match &row.outcome {
                            mio_core::IntruderResultOutcome::Flow(summary)
                                if summary.status.is_some() =>
                            {
                                Some(summary.response_bytes)
                            }
                            _ => None,
                        };
                        IntruderResultViewRow {
                            ordinal: row.ordinal,
                            result: Some(row.clone()),
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
    click(started.window, "intruder-results-settings", cx);
    click(started.window, "intruder-result-save-to-project", cx);
    let commands = drain(&mut fixture.commands);
    let [
        command @ Command::SetIntruderRetention {
            retention: mio_core::IntruderRetention::Project,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("the Settings checkbox must promote the Session attack to the project")
    };
    emit(&fixture, retain_reply(command, &started.summary), cx);
    let retention_commands = drain(&mut fixture.commands);
    assert_eq!(
        retention_commands
            .iter()
            .filter(|command| matches!(command, Command::SaveProject { path: None, .. }))
            .count(),
        1
    );
    assert!(retention_commands.iter().all(|command| match command {
        Command::SaveProject {
            websocket_notes: _,
            ext_http_notes: _,
            ext_http_highlights: _,
            project,
            path: None,
            workspace: None,
        }
        | Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { project, .. },
        }
        | Command::QueryIntruderResults { project, .. } => *project == fixture.project,
        _ => false,
    }));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let footer = window.find("project-status");
        assert!(footer.visible());
        assert!(footer.label().unwrap().contains("正在确认项目文件保存"));
    })
    .unwrap();
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(!view.intruder.session_runs.contains(&started.summary.id));
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(
            result
                .query
                .page
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .filter_map(|row| row.result.clone())
                .collect::<Vec<_>>(),
            rows
        );
        assert_eq!(result.persisted, Some(true));
        assert_eq!(
            result.summary.as_ref().unwrap().retention,
            mio_core::IntruderRetention::Project
        );
    });
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-save-to-project").checked(),
            Some(true)
        );
    })
    .unwrap();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let footer = window.find("project-status");
        assert!(footer.visible());
        let status = footer.label().unwrap();
        assert!(status.contains("项目已保存"));
        assert!(
            !status.contains("正在确认项目文件保存"),
            "the acknowledged project save must retire its waiting footer: {status}"
        );
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryIntruderResults { .. })),
        "project acknowledgement must not issue another write, start, send, or unrelated read"
    );
    // The owner window can refresh at the retention ACK rather than deferring until ProjectSaved.
    let refreshes = retention_commands
        .iter()
        .chain(&commands)
        .filter(|command| matches!(command, Command::QueryIntruderResults { .. }))
        .collect::<Vec<_>>();
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision: refresh_revision,
            after,
            limit,
        },
    ] = refreshes.as_slice()
    else {
        panic!(
            "the retention and project acknowledgements must together refresh exactly one result page"
        )
    };
    assert_eq!(
        (*project, *run_id, *refresh_revision, *after, *limit),
        (
            fixture.project,
            started.summary.id,
            query_revision + 1,
            None,
            1
        )
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id].pending,
            Some(*refresh_revision)
        );
    });
    started.summary.revision += 1;
    started.summary.retention = mio_core::IntruderRetention::Project;
    page(
        &fixture,
        &started,
        *refresh_revision,
        vec![rows[0].clone()],
        Some(0),
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let footer = window.find("project-status");
        let status = footer.label().unwrap();
        assert!(status.contains("项目已保存"));
        assert!(!status.contains("正在确认项目文件保存"));
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .query
                .page
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .filter_map(|row| row.result.clone())
                .collect::<Vec<_>>(),
            rows
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn project_backed_explicit_save_waits_for_named_project_save_without_retention_replay(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let path = PathBuf::from("/tmp/mio-close-ui-fixture.mio");
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, _| view.project_path = Some(path.clone()))
    });
    type_target(&mut fixture, true, "http://127.0.0.1:19", cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    save_started_from_dashboard(&mut fixture, &started, cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("a named Project dataset must save current drafts before the project file")
    };
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { path: None, .. }]
    ));
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        },
        cx,
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_keystrokes(started.window, crate::kbd!("w"));
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!has_close_dialog(cx));
}

#[gpui_kit::test]
fn overdue_workspace_ack_cannot_start_project_save_before_timer_callback(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-close-delayed-workspace.mio"));
        });
    });
    type_target(&mut fixture, true, "http://127.0.0.1:19", cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    save_started_from_dashboard(&mut fixture, &started, cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("a named Project dataset must save current drafts before the project file")
    };
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::WorkspaceChanged {
                    project: fixture.project,
                    workspace: workspace.clone(),
                },
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "an overdue workspace acknowledgement must not start the expired project file save"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_project_save.is_none());
        assert!(view.pending_workspace.is_none());
        assert_eq!(view.workspace.revision, workspace.revision);
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft.target,
            "http://127.0.0.1:19"
        );
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
}
