use super::*;

#[gpui_kit::test]
fn native_save_defaults_to_documents_then_keeps_the_project_directory(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![], 64);
    let documents = PathBuf::from(std::env::var_os("HOME").expect("macOS test user has a home"))
        .join("Documents");
    cx.read(|cx| assert_eq!(fixture.view.read(cx).default_directory(), documents));
    fixture.view.update(cx, |view, _| {
        view.project_path = Some(PathBuf::from("/tmp/mio-ui-save-default/existing.mio"));
        assert_eq!(
            view.default_directory(),
            PathBuf::from("/tmp/mio-ui-save-default")
        );
    });
}

#[gpui_kit::test]
fn click_send_reads_current_chinese_draft_and_cancel_targets_that_tab(cx: &mut TestAppContext) {
    let first = tab("1");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let raw = "POST /中文 HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nX-Test: a\r\nX-Test: b\r\nContent-Length: 6\r\n\r\n最新";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(raw, cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. })),
        "network cannot start before durable workspace acknowledgement"
    );
    let saved = commands
        .iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace.clone()),
            _ => None,
        })
        .expect("latest draft is persisted before sending");
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let sent = commands
        .iter()
        .find_map(|command| match command {
            Command::Send { tab_id, draft, .. } => Some((*tab_id, draft)),
            _ => None,
        })
        .expect("real Send click queues the exact snapshot after its save acknowledgement");
    assert_eq!(sent.0, id);
    assert_eq!(sent.1.raw, raw.as_bytes());
    assert_eq!(sent.1.revision, 2);
    assert_eq!(saved.tabs[0].draft, *sent.1);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_none());
        assert_eq!(window.find("repeater-cancel").disabled(), None);
        window.click("repeater-cancel", cx);
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .any(|command| matches!(command, Command::CancelSend { tab_id, .. } if *tab_id == id))
    );
}

#[gpui_kit::test]
fn send_click_never_unmarks_or_sends_unconfirmed_composition(cx: &mut TestAppContext) {
    let first = tab("中文");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        editor.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
        });
        window.render_frame(cx);
        let composing = editor.read(cx).value();
        window.press("cmd-enter", cx);
        assert_eq!(editor.read(cx).value(), composing);
        window.click("repeater-send", cx);
        assert!(
            editor
                .update(cx, |state, cx| state.marked_text_range(window, cx))
                .is_some()
        );
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let commands = acknowledge_history_query(&fixture, commands, cx);
    assert!(
        commands.is_empty(),
        "neither save nor send may harvest preedit"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(None, "中", window, cx)
        });
        window.render_frame(cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    let workspace = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("confirmed composition is saved");
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(commands.iter().any(|command| matches!(command, Command::Send { draft, .. } if draft.raw.ends_with("中".as_bytes()))));
}

#[gpui_kit::test]
fn intercept_edit_forward_and_drop_start_at_real_buttons(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let first = record("http://127.0.0.1:9001/", "旧");
    let first_id = first.summary.id;
    fixture
        .events
        .try_send(Event::InterceptPending {
            project: fixture.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(first),
        })
        .unwrap();
    cx.run_until_parked();
    let editor = cx.read(|cx| fixture.view.read(cx).intercepts[0].editor.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("POST /edited HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nX-Test: a\r\nX-Test: b\r\nContent-Length: 6\r\n\r\n新稿", cx);
        window.click("intercept-forward", cx);
    }).unwrap();
    let forwarded = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::Decide {
                flow_id,
                decision: Decision::ForwardRequest(request),
                ..
            } => Some((flow_id, request)),
            _ => None,
        })
        .unwrap();
    assert_eq!(forwarded.0, first_id);
    assert_eq!(forwarded.1.body, "新稿".as_bytes());
    assert_eq!(
        forwarded
            .1
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("x-test"))
            .count(),
        2
    );
    fixture
        .events
        .try_send(Event::InterceptResolved {
            project: fixture.project,
            flow_id: first_id,
        })
        .unwrap();
    let second = record("http://127.0.0.1:9001/drop", "不发送");
    let second_id = second.summary.id;
    fixture
        .events
        .try_send(Event::InterceptPending {
            project: fixture.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(second),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("intercept-drop", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).iter().any(|command| matches!(command, Command::Decide { flow_id, decision: Decision::Drop, .. } if *flow_id == second_id)));
}

#[gpui_kit::test]
fn streaming_intercept_rejects_body_edits_without_losing_them_and_forwards_headers(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let mut response = record("http://127.0.0.1:9001/events", "");
    let id = response.summary.id;
    response.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    fixture
        .events
        .try_send(Event::InterceptPending {
            project: fixture.project,
            stage: InterceptStage::Response,
            response_head_only: true,
            record: Box::new(response),
        })
        .unwrap();
    cx.run_until_parked();
    let editor = cx.read(|cx| fixture.view.read(cx).intercepts[0].editor.raw.clone());
    let with_body = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n不能静默丢弃";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(with_body, cx);
        window.click("intercept-forward", cx);
        let view = fixture.view.read(cx);
        assert!(!view.intercepts[0].deciding);
        assert_eq!(editor.read(cx).value().as_ref(), with_body);
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("只能修改响应头"))
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("HTTP/1.1 101 Switching Protocols\r\n\r\n", cx);
        window.click("intercept-forward", cx);
        let view = fixture.view.read(cx);
        assert!(!view.intercepts[0].deciding);
        assert_eq!(
            editor.read(cx).value().as_ref(),
            "HTTP/1.1 101 Switching Protocols\r\n\r\n"
        );
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("协议升级"))
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(
            "HTTP/1.1 201 Created\r\nContent-Type: text/event-stream\r\nX-Edited: yes\r\n\r\n",
            cx,
        );
        window.click("intercept-forward", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [
        Command::Decide {
            flow_id,
            decision: Decision::ForwardResponse(response),
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one explicit head-only Forward decision is required")
    };
    assert_eq!(*flow_id, id);
    assert_eq!(response.status, 201);
    assert!(response.body.is_empty() && response.trailers.is_empty());
    assert!(
        response
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("x-edited") && header.value == b"yes")
    );
}

#[gpui_kit::test]
fn intercept_resolution_returns_hidden_editor_focus_to_workspace(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let request = record("http://127.0.0.1:9001/events", "");
    let id = request.summary.id;
    fixture
        .events
        .try_send(Event::InterceptPending {
            project: fixture.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(request),
        })
        .unwrap();
    cx.run_until_parked();
    let editor = cx.read(|cx| fixture.view.read(cx).intercepts[0].editor.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        assert!(editor.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    fixture
        .events
        .try_send(Event::InterceptResolved {
            project: fixture.project,
            flow_id: id,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(fixture.view.read(cx).intercepts.is_empty());
        assert!(!editor.focus_handle(cx).is_focused(window));
        assert!(window.is_action_available(&SaveProject, cx));
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
}
