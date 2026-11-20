use super::*;

#[gpui_kit::test]
fn appended_archive_tab_survives_queue_or_save_rejection_and_real_save_retries_the_same_id(
    cx: &mut TestAppContext,
) {
    let original = tab("保存失败时仍保留的旧稿");
    let mut fixture = fixture(cx, vec![original.clone()], 1);
    let path = PathBuf::from("/tmp/mio-ui-ext-retry-synthetic.mio");
    fixture
        .view
        .update(cx, |view, _| view.project_path = Some(path.clone()));
    for rejected in [false, true] {
        browse(&mut fixture, cx);
        let before = cx.read(|cx| fixture.view.read(cx).workspace.clone());
        let command = request(&mut fixture, cx);
        let added = archived();
        let mut expected_tabs = before.tabs.clone();
        expected_tabs.push(added.clone());
        if !rejected {
            fixture.view.update(cx, |view, cx| {
                assert!(view.enqueue(
                    Command::QueryExtReport {
                        project: fixture.project,
                        query_revision: 90,
                    },
                    cx,
                ));
            });
        }
        fixture
            .events
            .try_send(proposal(&command, added.clone()))
            .unwrap();
        cx.run_until_parked();
        if rejected {
            let attempted = saved(&mut fixture, cx);
            assert_eq!(attempted.tabs, expected_tabs);
            fixture
                .events
                .try_send(Event::WorkspaceRejected {
                    project: fixture.project,
                    revision: attempted.revision,
                    stored: before.clone(),
                    message: "synthetic appended-workspace write rejection".into(),
                })
                .unwrap();
            cx.run_until_parked();
            assert!(drain(&mut fixture.commands).is_empty());
        } else {
            assert!(matches!(
                drain(&mut fixture.commands).as_slice(),
                [Command::QueryExtReport {
                    query_revision: 90,
                    ..
                }]
            ));
        }
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.workspace.tabs, expected_tabs);
            assert_eq!(view.workspace.tabs[0], original);
            assert_eq!(view.workspace.active, Some(added.id));
            assert!(!view.ext_import.visible);
            assert!(view.ext_import.repeater.pending.is_none());
            assert!(view.workspace_dirty && view.pending_workspace.is_none());
            assert!(view.error.is_some());
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        });

        project_menu(&fixture, "保存项目", cx);
        let retry = saved(&mut fixture, cx);
        assert_eq!(retry.tabs, expected_tabs);
        assert_eq!(retry.active, Some(added.id));
        assert_eq!(retry.revision, before.revision + 1);
        fixture
            .events
            .try_send(Event::WorkspaceChanged {
                project: fixture.project,
                workspace: retry.clone(),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(matches!(
            drain(&mut fixture.commands).as_slice(),
            [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
                if *project == fixture.project
        ));
        fixture
            .events
            .try_send(Event::ProjectSaved {
                websocket_notes: vec![],
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project: fixture.project,
                path: path.clone(),
                total_bytes: 4096,
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "manual saving must not reload another tab or send an archived request"
        );
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.workspace.tabs, expected_tabs);
            assert_eq!(view.workspace.tabs[0], original);
            assert_eq!(view.workspace.revision, retry.revision);
            assert!(!view.workspace_dirty);
            assert!(view.pending_workspace.is_none());
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        });
    }
}

#[gpui_kit::test]
fn archived_origin_target_loads_unsent_and_survives_workspace_save_and_reopen(
    cx: &mut TestAppContext,
) {
    for target in [
        "http://127.0.0.1:19081",
        "https://localhost:19443",
        "http://[::1]:19082",
    ] {
        let original = tab("原有草稿");
        let mut fixture = fixture(cx, vec![original.clone()], 64);
        browse(&mut fixture, cx);
        let command = request(&mut fixture, cx);
        let mut added = archived();
        added.draft.target = target.into();
        fixture
            .events
            .try_send(proposal(&command, added.clone()))
            .unwrap();
        cx.run_until_parked();
        let workspace = saved(&mut fixture, cx);
        assert_eq!(workspace.tabs, [original, added.clone()]);
        assert_eq!(workspace.active, Some(added.id));
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.workspace.tabs.last(), Some(&added));
            assert_eq!(
                view.tab_editors[&added.id].target.read(cx).value().as_ref(),
                target
            );
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
            assert!(view.ext_import.repeater.error.is_none());
        });
        fixture
            .events
            .try_send(Event::WorkspaceChanged {
                project: fixture.project,
                workspace: workspace.clone(),
            })
            .unwrap();
        cx.run_until_parked();
        let commands = drain(&mut fixture.commands);
        assert!(
            crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx).is_empty()
        );
        let mut reopened_project = fixture.project;
        reopened_project.generation += 1;
        fixture
            .events
            .try_send(Event::ProjectOpened {
                project: reopened_project,
                path: Some(PathBuf::from("/tmp/mio-ext-origin-reopened.mio")),
                workspace: workspace.clone(),
            })
            .unwrap();
        cx.run_until_parked();
        fixture.project = reopened_project;
        let commands = drain(&mut fixture.commands);
        let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
        assert!(
            matches!(commands.as_slice(), [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == reopened_project),
            "reopening must query history without sending or loading another archive"
        );
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.workspace, workspace);
            assert_eq!(
                view.tab_editors[&added.id].target.read(cx).value().as_ref(),
                target
            );
            assert_eq!(view.workspace.tabs.last().unwrap().draft.raw, RAW);
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        });
    }
}

#[gpui_kit::test]
fn archived_target_must_be_a_valid_http_origin_and_old_selection_cannot_install_it(
    cx: &mut TestAppContext,
) {
    let original = tab("保留原有草稿");
    let mut fixture = fixture(cx, vec![original.clone()], 64);
    let rows = browse(&mut fixture, cx);
    for target in [
        "http://127.0.0.1:19081/path",
        "http://127.0.0.1:19081/",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://127.0.0.1:19081?query",
        "http://127.0.0.1:19081#fragment",
        "http://user@127.0.0.1:19081",
        "http://127.0.0.1:65536",
        "http://",
        "ws://127.0.0.1:19081",
        " http://127.0.0.1:19081",
        "http://127.0.0.1:19081\\other",
        "http://127.0.0.1:19\n081",
    ] {
        let command = request(&mut fixture, cx);
        let mut added = archived();
        added.draft.target = target.into();
        fixture.events.try_send(proposal(&command, added)).unwrap();
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(
                view.workspace.tabs.as_slice(),
                std::slice::from_ref(&original)
            );
            assert!(view.ext_import.repeater.error.is_some());
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        });
    }
    let command = request(&mut fixture, cx);
    select(&mut fixture, &rows[1], 1, cx);
    let mut added = archived();
    added.draft.target = "http://127.0.0.1:19081".into();
    fixture.events.try_send(proposal(&command, added)).unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.tabs.as_slice(),
            std::slice::from_ref(&original)
        );
        assert_eq!(view.ext_repeater_selection().unwrap().message, rows[1].id);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
    });
}
