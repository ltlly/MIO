use super::*;

#[gpui_kit::test]
fn websocket_intercept_queue_edits_forward_and_drop_without_losing_other_messages(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let first = websocket_message(
        1,
        mio_core::WebSocketDirection::ClientToServer,
        mio_core::WebSocketKind::Text,
        b"hello-orig",
    );
    let mut second = first.clone();
    second.id = Uuid::new_v4();
    second.sequence = 2;
    second.direction = mio_core::WebSocketDirection::ServerToClient;
    second.kind = mio_core::WebSocketKind::Binary;
    second.payload = vec![0xff];
    for record in [first.clone(), second.clone()] {
        fixture
            .events
            .try_send(Event::WebSocketInterceptPending {
                project: fixture.project,
                url: "ws://127.0.0.1:9001/socket".into(),
                record: Box::new(record),
            })
            .unwrap();
    }
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| fixture.view.read(cx).intercepts.len()), 2);
    let editor = cx.read(|cx| fixture.view.read(cx).intercepts[0].editor.raw.clone());
    let revision_before = cx.read(|cx| editor.read(cx).document_revision());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("hello-edited", cx);
        window.click("intercept-forward", cx);
    })
    .unwrap();
    assert!(cx.read(|cx| editor.read(cx).document_revision()) > revision_before);
    assert!(
        drain(&mut fixture.commands)
            .into_iter()
            .any(|command| matches!(
                command,
                Command::DecideWebSocket {
                    message_id,
                    decision: mio_core::WebSocketDecision::Forward(bytes),
                    ..
                } if message_id == first.id && bytes == b"hello-edited"
            ))
    );

    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "decide_websocket",
            message: "WebSocket 拦截已结束".into(),
        })
        .unwrap();
    cx.run_until_parked();
    // A backend correlated decision failure keeps the exact edit and clears deciding.
    let edited = cx
        .update_window(fixture.window, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                let pending = &view.intercepts[0];
                (pending.deciding, pending.editor.bytes(window, cx).unwrap())
            })
        })
        .unwrap();
    assert!(!edited.0);
    assert_eq!(edited.1, b"hello-edited");
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("intercept-forward", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::DecideWebSocket {
                message_id,
                decision: mio_core::WebSocketDecision::Forward(bytes),
                ..
            } if *message_id == first.id && bytes == b"hello-edited"
        )),
        "the second Forward reuses the preserved edit"
    );
    assert_eq!(cx.read(|cx| fixture.view.read(cx).intercepts.len()), 2);
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "decide_websocket",
            message: "WebSocket 拦截已结束".into(),
        })
        .unwrap();
    cx.run_until_parked();

    // A Text frame edited into invalid UTF-8 via Hex is rejected atomically and stays queued.
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intercept-hex", cx);
        let hex = fixture.view.read(cx).intercepts[0].editor.hex.clone();
        window.click(("input", hex.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("ff", cx);
        window.click("intercept-forward", cx);
        let view = fixture.view.read(cx);
        assert!(!view.intercepts[0].deciding);
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("UTF-8"))
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());

    let second_id = cx.read(|cx| {
        let view = fixture.view.read(cx);
        let second_label = view.intercepts[1].queue_label();
        assert!(second_label.contains("WS ←"));
        view.intercepts[1].id()
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let id = SharedString::from(format!("intercept-{second_id}"));
        window.click(id, cx);
        window.render_frame(cx);
        window.click("intercept-drop", cx);
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands)
            .into_iter()
            .any(|command| matches!(
                command,
                Command::DecideWebSocket {
                    message_id,
                    decision: mio_core::WebSocketDecision::Drop,
                    ..
                } if message_id == second.id
            ))
    );
    assert_eq!(cx.read(|cx| fixture.view.read(cx).intercepts.len()), 2);
    fixture
        .events
        .try_send(Event::WebSocketInterceptResolved {
            project: fixture.project,
            message_id: second.id,
        })
        .unwrap();
    cx.run_until_parked();
    let ids = cx.read(|cx| {
        let view = fixture.view.read(cx);
        view.intercepts
            .iter()
            .map(|pending| pending.id())
            .collect::<Vec<_>>()
    });
    assert_eq!(ids, vec![first.id]);
}

#[gpui_kit::test]
fn websocket_intercept_transfer_actions_do_not_treat_messages_as_http(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let message = websocket_message(
        1,
        mio_core::WebSocketDirection::ClientToServer,
        mio_core::WebSocketKind::Text,
        b"transfer-me",
    );
    fixture
        .events
        .try_send(Event::WebSocketInterceptPending {
            project: fixture.project,
            url: "ws://127.0.0.1:9001/socket".into(),
            record: Box::new(message.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("i"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        cx.read(|cx| fixture.view.read(cx).error.clone())
            .is_some_and(|error| error.contains("Intruder")),
        "WebSocket intercept transfer to Intruder must fail explicitly"
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::SendToDecoder), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let decoder_input = cx.read(|cx| fixture.view.read(cx).extra_tools.decoder_input.clone());
    let text = cx.update_window(fixture.window, |_, window, cx| {
        editing::snapshot(&decoder_input, window, cx)
    });
    assert!(cx.read(|cx| fixture.view.read(cx).tool == Tool::Decoder));
    assert_eq!(text.unwrap(), Ok("transfer-me".into()));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-proxy", cx);
        window.render_frame(cx);
        window.click("proxy-intercept", cx);
        window.render_frame(cx);
        let raw = fixture.view.read(cx).intercepts[0].editor.raw.clone();
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("r"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(
        cx.read(|cx| fixture.view.read(cx).workspace.tabs.is_empty()),
        "WebSocket intercept transfer must not create an HTTP Repeater tab"
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. })),
        "WebSocket intercept transfer must not emit HTTP Send"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::WebSocketRepeater(mio_core::WebSocketRepeaterCommand::OpenFromHistory {
                message_id,
                use_original: false,
                payload_override: Some((mio_core::WebSocketKind::Text, payload)),
                ..
            }) if *message_id == message.id && payload.as_slice() == b"transfer-me"
        )),
        "WebSocket intercept Cmd+R must route to WebSocketRepeaterCommand::OpenFromHistory"
    );
}

#[gpui_kit::test]
fn websocket_intercept_mixes_with_http_forward_all_and_stale_resolutions(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let http = record("http://127.0.0.1:9001/http", "body");
    let http_id = http.summary.id;
    let text = websocket_message(
        1,
        mio_core::WebSocketDirection::ClientToServer,
        mio_core::WebSocketKind::Text,
        b"ws-text",
    );
    let binary = websocket_message(
        2,
        mio_core::WebSocketDirection::ServerToClient,
        mio_core::WebSocketKind::Binary,
        &[0x00, 0xff],
    );
    for pending in [
        Event::InterceptPending {
            project: fixture.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(http.clone()),
        },
        Event::WebSocketInterceptPending {
            project: fixture.project,
            url: "ws://127.0.0.1:9001/socket".into(),
            record: Box::new(text.clone()),
        },
        Event::WebSocketInterceptPending {
            project: fixture.project,
            url: "ws://127.0.0.1:9001/socket".into(),
            record: Box::new(binary.clone()),
        },
    ] {
        fixture.events.try_send(pending).unwrap();
    }
    cx.run_until_parked();
    let before_revisions: Vec<_> = cx.read(|cx| {
        let view = fixture.view.read(cx);
        view.intercepts
            .iter()
            .map(|pending| pending.editor.raw.read(cx).document_revision())
            .collect()
    });
    let text_editor = cx.read(|cx| fixture.view.read(cx).intercepts[1].editor.raw.clone());
    let text_tab = SharedString::from(format!("intercept-{}", text.id));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(text_tab, cx);
        window.render_frame(cx);
        window.click(("input", text_editor.entity_id()), cx);
        text_editor.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "中", Some(7..7), window, cx)
        });
        window.click("intercept-forward-all", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        commands.is_empty(),
        "one pending composition must stop the whole forward batch"
    );
    let revision_after: Vec<_> = cx.read(|cx| {
        let view = fixture.view.read(cx);
        view.intercepts
            .iter()
            .map(|pending| pending.editor.raw.read(cx).document_revision())
            .collect()
    });
    assert_eq!(revision_after[0], before_revisions[0]);
    assert_eq!(revision_after[2], before_revisions[2]);
    let pending_error = cx.read(|cx| fixture.view.read(cx).error.clone());
    assert!(pending_error.is_some_and(|error| error == editing::COMPOSITION_PENDING));
    cx.update_window(fixture.window, |_, window, cx| {
        text_editor.update(cx, |state, cx| {
            state.replace_text_in_range(None, "中", window, cx)
        });
        window.click("intercept-forward-all", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::Decide { flow_id, decision: Decision::ForwardRequest(_), .. } if *flow_id == http_id
        )),
        "HTTP pending must send an HTTP decision"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::DecideWebSocket {
                message_id,
                decision: mio_core::WebSocketDecision::Forward(bytes),
                ..
            } if *message_id == text.id && bytes == "ws-text中".as_bytes()
        )),
        "websocket pending must send a websocket decision"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::DecideWebSocket {
                message_id,
                decision: mio_core::WebSocketDecision::Forward(bytes),
                ..
            } if *message_id == binary.id && bytes.as_slice() == [0x00, 0xff]
        )),
        "binary pending must preserve exact bytes"
    );
    for command in commands {
        match command {
            Command::Decide { flow_id, .. } => {
                fixture
                    .events
                    .try_send(Event::InterceptResolved {
                        project: fixture.project,
                        flow_id,
                    })
                    .unwrap();
            }
            Command::DecideWebSocket { message_id, .. } => {
                fixture
                    .events
                    .try_send(Event::WebSocketInterceptResolved {
                        project: fixture.project,
                        message_id,
                    })
                    .unwrap();
            }
            _ => {}
        }
    }
    cx.run_until_parked();
    assert!(cx.read(|cx| fixture.view.read(cx).intercepts.is_empty()));
    let stale_project = ProjectKey {
        id: Uuid::new_v4(),
        generation: fixture.project.generation + 1,
    };
    fixture
        .events
        .try_send(Event::WebSocketInterceptPending {
            project: stale_project,
            url: "ws://old/socket".into(),
            record: Box::new(text.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        cx.read(|cx| fixture.view.read(cx).intercepts.is_empty()),
        "stale pending must be ignored by the current project"
    );
    fixture
        .events
        .try_send(Event::WebSocketInterceptPending {
            project: fixture.project,
            url: "ws://new/socket".into(),
            record: Box::new(text.clone()),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketInterceptResolved {
            project: stale_project,
            message_id: text.id,
        })
        .unwrap();
    cx.run_until_parked();
    let ids: Vec<_> = cx.read(|cx| {
        let view = fixture.view.read(cx);
        view.intercepts.iter().map(|pending| pending.id()).collect()
    });
    assert_eq!(ids, vec![text.id]);
}
