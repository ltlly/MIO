use super::*;

#[gpui_kit::test]
fn imported_repeater_workspace_restores_order_titles_and_selected_pairs_without_send_or_plugins(
    cx: &mut TestAppContext,
) {
    let old = tab("旧项目标签");
    let old_id = old.id;
    let mut f = fixture(cx, vec![old], 64);
    drain(&mut f.commands);
    let mut first = archived();
    first.title = format!("{}🙂{}", "A".repeat(31), "B".repeat(320));
    first.draft.target = "http://127.0.0.1:19081".into();
    first.draft.raw = b"GET /selected-history HTTP/1.1\r\nHost: original.example\r\n\r\n".to_vec();
    let first_ref = first.archive_response.clone().unwrap();
    let mut second = archived();
    second.title = "第二个历史标签".into();
    second.draft.target = "https://127.0.0.1:19443".into();
    second.draft.raw =
        b"POST /saved-request HTTP/1.1\r\nX-Order: one\r\nX-Order: two\r\n\r\n\0\xff".to_vec();
    let second_ref = second.archive_response.clone().unwrap();
    let imported = Workspace {
        revision: 4,
        active: Some(first.id),
        tabs: vec![first.clone(), second.clone()],
        ..Workspace::default()
    };
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: f.project.generation + 1,
    };
    f.events
        .try_send(Event::ProjectOpened {
            project,
            path: Some(PathBuf::from("/tmp/mio-imported-repeater-workspace.mio")),
            workspace: imported.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project: actual, .. } }] if *actual == project),
        "opening an imported project must not send, load plugins or eagerly read inactive response payloads"
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.tool == Tool::Proxy);
        assert_eq!(*view.workspace, imported);
        assert!(!view.tab_editors.contains_key(&old_id));
        assert_eq!(
            view.tab_editors[&first.id].target.read(cx).value().as_ref(),
            first.draft.target
        );
        assert_eq!(
            view.tab_editors[&first.id]
                .request
                .bytes_lossless(cx),
            first.draft.raw
        );
        assert!(!view.tab_editors.contains_key(&second.id));
        assert!(view.sending.is_empty() && view.pending_sends.is_empty());
    });
    f.project = project;
    click(&f, "tool-repeater", cx);
    let first_read = read(&mut f, cx);
    assert_eq!(first_read.1, first_ref);
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\narchived";
    let meta = summary(&first_ref, raw.len() as u64);
    reply(&f, &first_read, meta.clone(), vec![], cx);
    let body_read = read(&mut f, cx);
    reply(&f, &body_read, meta, raw.to_vec(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find(format!("work-tab-{}", first.id)).visible());
            assert_eq!(
                window.find(format!("work-tab-{}", first.id)).label(),
                Some(first.title.as_str()),
                "a visually truncated tab must expose its complete imported title"
            );
            assert_eq!(
                window.find(format!("work-tab-{}", first.id)).role(),
                Some(gpui_kit::accesskit::Role::Tab)
            );
            assert_eq!(
                window.find(format!("work-tab-{}", first.id)).selected(),
                Some(true)
            );
            assert!(window.find(format!("work-tab-{}", second.id)).visible());
            assert_eq!(
                window.find(format!("work-tab-{}", second.id)).role(),
                Some(gpui_kit::accesskit::Role::Tab)
            );
            assert_eq!(
                window.find(format!("work-tab-{}", second.id)).selected(),
                Some(false)
            );
            f.view.update(cx, |view, cx| {
                assert_eq!(view.workspace.tabs, [first.clone(), second.clone()]);
                assert_eq!(
                    view.tab_editors[&first.id]
                        .response
                        .bytes(window, cx)
                        .unwrap(),
                    raw
                );
                assert!(!view.tab_editors.contains_key(&second.id));
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    click(&f, format!("work-tab-{}", second.id), cx);
    let selected = read(&mut f, cx);
    assert_eq!(selected.1, second_ref);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.active, Some(second.id));
        assert_eq!(view.tab_editors[&second.id].request.view, MessageView::Hex);
        assert_eq!(view.workspace.tabs, [first, second]);
        assert!(view.sending.is_empty() && view.pending_sends.is_empty());
    });
}

#[gpui_kit::test]
fn imported_active_second_tab_preserves_space_title_and_reads_the_selected_pair(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    drain(&mut f.commands);
    let mut first = archived();
    first.title = " ".into();
    let first_ref = first.archive_response.clone().unwrap();
    let mut second = archived();
    second.title = "源项目选中的标签".into();
    let second_ref = second.archive_response.clone().unwrap();
    let workspace = Workspace {
        revision: 1,
        active: Some(second.id),
        tabs: vec![first.clone(), second.clone()],
        ..Workspace::default()
    };
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: f.project.generation + 1,
    };
    f.events
        .try_send(Event::ProjectOpened {
            project,
            path: Some(PathBuf::from("/tmp/mio-imported-active-second.mio")),
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project: actual, .. } }] if *actual == project),
        "import must not send or execute plugins"
    );
    f.project = project;
    click(&f, "tool-repeater", cx);
    let selected = read(&mut f, cx);
    assert_eq!(selected.1, second_ref);
    reply(&f, &selected, summary(&second_ref, 6), vec![], cx);
    let body = read(&mut f, cx);
    reply(&f, &body, summary(&second_ref, 6), b"second".to_vec(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find(format!("work-tab-{}", first.id)).label(),
                Some(" ")
            );
            assert_eq!(
                window.find(format!("work-tab-{}", first.id)).selected(),
                Some(false)
            );
            assert_eq!(
                window.find(format!("work-tab-{}", second.id)).selected(),
                Some(true)
            );
            let view = f.view.read(cx);
            assert_eq!(*view.workspace, workspace);
            assert_eq!(
                view.tab_editors[&second.id]
                    .response
                    .raw
                    .read(cx)
                    .value()
                    .as_ref(),
                "second"
            );
            assert!(view.sending.is_empty() && view.pending_sends.is_empty());
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    click(&f, format!("work-tab-{}", first.id), cx);
    let selected = read(&mut f, cx);
    assert_eq!(selected.1, first_ref);
    reply(&f, &selected, summary(&first_ref, 5), vec![], cx);
    let body = read(&mut f, cx);
    reply(&f, &body, summary(&first_ref, 5), b"first".to_vec(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find(format!("work-tab-{}", first.id)).selected(),
                Some(true)
            );
            assert_eq!(
                window.find(format!("work-tab-{}", second.id)).selected(),
                Some(false)
            );
            let view = f.view.read(cx);
            assert_eq!(view.workspace.tabs[0].title, " ");
            assert_eq!(
                view.tab_editors[&first.id]
                    .response
                    .raw
                    .read(cx)
                    .value()
                    .as_ref(),
                "first"
            );
            assert!(
                view.tab_editors[&second.id]
                    .response
                    .raw
                    .read(cx)
                    .value()
                    .is_empty()
            );
            assert!(view.sending.is_empty() && view.pending_sends.is_empty());
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
}
