use super::*;

#[gpui_kit::test]
fn project_save_status_waits_for_all_acks_and_preserves_network_status(cx: &mut TestAppContext) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let path = cx.read(|cx| fixture.view.read(cx).project_path.clone().unwrap());
    fixture
        .events
        .try_send(Event::ProxyState {
            project: fixture.project,
            address: Some("127.0.0.1:9002".parse().unwrap()),
            cancel: Some(CancellationToken::new()),
            intercept_requests: false,
            intercept_responses: false,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("s"), cx);
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 1);
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::SaveProject {
            path: None,
            workspace: None,
            ..
        }
    )));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project_saves_in_flight, 1);
        assert_eq!(view.pending_project_save, Some(None));
    });
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: path.clone(),
            total_bytes: 8192,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject {
            path: None,
            workspace: None,
            ..
        }]
    ));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let status = rendered_status(window);
        assert!(status.contains("正在保存") && !status.contains("项目已保存"));
        assert!(status.contains("Proxy 正在监听 127.0.0.1:9002"));
    })
    .unwrap();
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "save_project",
            message: "synthetic project checkpoint failure".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap()
                .contains("checkpoint failure")
        );
        window.click("dismiss-error", cx);
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.error.is_none());
        assert_eq!(view.workspace, stored);
        assert_eq!(view.status, "Proxy 正在监听 127.0.0.1:9002");
        let status = rendered_status(window);
        assert!(status.contains("项目保存失败") && !status.contains("项目已保存"));
        assert!(!status.contains("正在保存"));
        assert!(status.contains(&view.status));
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject {
            path: None,
            workspace: None,
            ..
        }]
    ));
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 12288,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, stored);
        assert_eq!(view.status, "Proxy 正在监听 127.0.0.1:9002");
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        let status = rendered_status(window);
        assert!(status.starts_with("项目已保存（12288 字节"));
        assert!(!status.contains("未保存") && !status.contains("正在保存"));
        assert!(status.contains(&view.status));
    })
    .unwrap();
}

fn saved_composition_keeps_unsaved_status(cx: &mut TestAppContext, target_input: bool) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.active.unwrap();
    let (request, base_target, hidden) = cx.read(|cx| {
        let editors = &fixture.view.read(cx).tab_editors[&id];
        (
            editors.request.raw.clone(),
            editors.target.clone(),
            editors.request.hex.clone(),
        )
    });
    let (edit_window, target) = if target_input {
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("repeater-edit-target", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let dialog_window = cx.read(|cx| {
            *cx.windows()
                .iter()
                .find(|&&w| w != fixture.window)
                .expect("Repeater target dialog window")
        });
        let dialog_target = cx.read(|cx| {
            let dialog = fixture
                .view
                .read(cx)
                .open_repeater_target_dialog_for_tab(id, cx)
                .expect("Repeater target dialog entity");
            dialog.read(cx).target.clone()
        });
        assert_ne!(dialog_target.entity_id(), base_target.entity_id());
        (dialog_window, dialog_target)
    } else {
        (fixture.window, base_target.clone())
    };
    let focus = cx.read(|cx| {
        if target_input {
            target.focus_handle(cx)
        } else {
            request.focus_handle(cx)
        }
    });
    let input_id: gpui_kit::ElementId = if target_input {
        "repeater-target-input".into()
    } else {
        ("input", request.entity_id()).into()
    };
    let (mut handler, marked, composed_text) = cx
        .update_window(edit_window, |_, window, cx| {
            window.render_frame(cx);
            window.click(input_id.clone(), cx);
            if target_input {
                #[cfg(target_os = "macos")]
                window.press("cmd-right", cx);
                #[cfg(not(target_os = "macos"))]
                window.press("end", cx);
            } else {
                window.press(crate::kbd!("end"), cx);
            }
            assert!(focus.is_focused(window));
            let bounds = window.find(input_id.clone()).bounds();
            // Exercise the input protocol on the clicked control, not a native IME session.
            let mut handler: Box<dyn InputHandler> = if target_input {
                Box::new(ElementInputHandler::new(bounds, target.clone()))
            } else {
                Box::new(ElementInputHandler::new(bounds, request.clone()))
            };
            handler.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            let marked = handler.marked_text_range(window, cx).unwrap();
            let text = if target_input {
                target.read(cx).value().to_string()
            } else {
                request.read(cx).value().to_string()
            };
            assert!(text.ends_with("zhong"));
            (handler, marked, text)
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(edit_window, |_, window, cx| {
        assert_eq!(handler.marked_text_range(window, cx), Some(marked.clone()));
        assert_eq!(window.find(input_id.clone()).value(), Some(composed_text.as_str()));
        assert!(focus.is_focused(window));
        if target_input {
            window.click("repeater-target-ok", cx);
            window.render_frame(cx);
            assert_eq!(
                window.find("repeater-target-status").label(),
                Some(editing::COMPOSITION_PENDING)
            );
            window.click("repeater-target-cancel", cx);
            window.render_frame(cx);
            window.press("escape", cx);
            window.render_frame(cx);
            assert_eq!(handler.marked_text_range(window, cx), Some(marked.clone()));
            assert_eq!(window.find(input_id.clone()).value(), Some(composed_text.as_str()));
        }
    })
    .unwrap();
    if target_input {
        cx.read(|cx| {
            assert!(cx.windows().contains(&edit_window));
            assert_eq!(
                base_target.read(cx).value().as_ref(),
                stored.tabs[0].draft.target
            );
        });
    }
    let (composing_status, rejected_status) = cx
        .update_window(fixture.window, |_, window, cx| {
            // A child composition must invalidate the footer without a forced window refresh.
            window.draw(cx).clear(cx);
            let composing_status = rendered_status(window);
            window.press(crate::kbd!("s"), cx);
            window.render_frame(cx);
            assert_eq!(
                fixture.view.read(cx).error.as_deref(),
                Some(editing::COMPOSITION_PENDING)
            );
            assert!(!hidden.focus_handle(cx).is_focused(window));
            (composing_status, rendered_status(window))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(edit_window, |_, window, cx| {
        assert_eq!(handler.marked_text_range(window, cx), Some(marked.clone()));
        assert_eq!(window.find(input_id.clone()).value(), Some(composed_text.as_str()));
        assert!(focus.is_focused(window));
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "unconfirmed input must neither enqueue a save nor Send"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("dismiss-error", cx);
        window.render_frame(cx);
        assert!(!hidden.focus_handle(cx).is_focused(window));
        let view = fixture.view.read(cx);
        assert!(view.error.is_none());
        assert_eq!(view.workspace, stored);
        assert!(view.pending_workspace.is_none() && view.pending_project_save.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        for (stage, status) in [
            ("composing", composing_status),
            ("save rejected", rejected_status),
            ("error dismissed", rendered_status(window)),
        ] {
            assert!(
                status.contains("未保存") && !status.contains("项目已保存"),
                "unconfirmed input cannot look persisted (target={target_input}, {stage}): {status}"
            );
        }
    })
    .unwrap();
    cx.update_window(edit_window, |_, window, cx| {
        assert_eq!(handler.marked_text_range(window, cx), Some(marked));
        assert_eq!(window.find(input_id).value(), Some(composed_text.as_str()));
        assert!(focus.is_focused(window));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn request_composition_after_save_is_not_reported_as_saved(cx: &mut TestAppContext) {
    saved_composition_keeps_unsaved_status(cx, false);
}

#[gpui_kit::test]
fn target_composition_after_save_is_not_reported_as_saved(cx: &mut TestAppContext) {
    saved_composition_keeps_unsaved_status(cx, true);
}

#[gpui_kit::test]
fn backend_disconnect_keeps_pending_save_results_unconfirmed_after_dismissal(
    cx: &mut TestAppContext,
) {
    let mut statuses = Vec::new();
    for workspace_pending in [true, false] {
        let (mut fixture, stored) = saved_repeater_fixture(cx);
        let id = stored.active.unwrap();
        let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
        let latest = "POST /disconnect HTTP/1.1\r\n\r\n断开后也保留";
        cx.update_window(fixture.window, |_, window, cx| {
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(latest, cx);
            window.press(crate::kbd!("s"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let queued = drain(&mut fixture.commands);
        let [
            Command::SaveWorkspace {
                workspace: attempted,
                ..
            },
        ] = queued.as_slice()
        else {
            panic!("explicit Save first queues the latest workspace")
        };
        assert_eq!(attempted.tabs[0].draft.raw, latest.as_bytes());
        if !workspace_pending {
            fixture
                .events
                .try_send(Event::WorkspaceChanged {
                    project: fixture.project,
                    workspace: attempted.clone(),
                })
                .unwrap();
            cx.run_until_parked();
            assert!(matches!(
                drain(&mut fixture.commands).as_slice(),
                [Command::SaveProject {
                    path: None,
                    workspace: None,
                    ..
                }]
            ));
        }
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert_eq!(view.pending_workspace.is_some(), workspace_pending);
            assert_eq!(
                view.project_saves_in_flight,
                usize::from(!workspace_pending)
            );
            let status = rendered_status(window);
            assert!(status.contains("正在保存") && !status.contains("项目已保存"));
        })
        .unwrap();
        let Fixture {
            window,
            view,
            mut commands,
            events,
            ..
        } = fixture;
        drop(events);
        cx.run_until_parked();
        assert!(
            drain(&mut commands).is_empty(),
            "a lost ACK cannot trigger a retry or Send"
        );
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                view.read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("后台服务已断开")
            );
            assert!(window.find("dismiss-error").visible());
            statuses.push((workspace_pending, "error visible", rendered_status(window)));
            window.click("dismiss-error", cx);
            window.render_frame(cx);
            let view = view.read(cx);
            assert!(view.error.is_none());
            assert_eq!(view.workspace.tabs[0].draft.raw, latest.as_bytes());
            assert_eq!(editor.read(cx).value().as_ref(), latest);
            assert_eq!(
                view.workspace.revision,
                if workspace_pending {
                    stored.revision
                } else {
                    attempted.revision
                }
            );
            assert_eq!(view.pending_workspace.is_some(), workspace_pending);
            assert_eq!(view.pending_project_save.is_some(), workspace_pending);
            assert_eq!(
                view.project_saves_in_flight,
                usize::from(!workspace_pending)
            );
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
            statuses.push((
                workspace_pending,
                "error dismissed",
                rendered_status(window),
            ));
        })
        .unwrap();
        assert!(drain(&mut commands).is_empty());
    }
    for (workspace_pending, stage, status) in statuses {
        assert!(
            status.contains("保存结果未确认")
                && status.contains("后台")
                && !status.contains("正在保存")
                && !status.contains("项目已保存"),
            "a disconnected save must remain unconfirmed (workspace_pending={workspace_pending}, {stage}): {status}"
        );
    }
}

#[gpui_kit::test]
fn repeater_hex_half_byte_edit_does_not_panic_and_completes_draft_capture(
    cx: &mut TestAppContext,
) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.active.unwrap();
    let path = cx.read(|cx| fixture.view.read(cx).project_path.clone().unwrap());
    let hex_editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.hex.clone());
    let initial_hex = editing::hex_encode(&stored.tabs[0].draft.raw);
    assert!(initial_hex.ends_with("0a"));

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-request-hex", cx);
        window.render_frame(cx);
        assert_eq!(hex_editor.read(cx).value().as_ref(), initial_hex.as_str());
        assert!(!fixture.view.read(cx).tab_editors[&id].dirty);

        window.click(("input", hex_editor.entity_id()), cx);
        #[cfg(target_os = "macos")]
        window.press("cmd-down", cx);
        #[cfg(not(target_os = "macos"))]
        window.press("ctrl-end", cx);
        window.press("end", cx);
        window.press("backspace", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);

        let half_deleted = &initial_hex[..initial_hex.len() - 1];
        assert_eq!(hex_editor.read(cx).value().as_ref(), half_deleted);
        assert!(fixture.view.read(cx).tab_editors[&id].dirty);
        let status = rendered_status(window);
        assert!(status.contains("未保存") && !status.contains("项目已保存"));

        window.click("repeater-send", cx);
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some("Hex 必须为完整字节：每个字节需要两位十六进制数字。")
        );
        assert_eq!(hex_editor.read(cx).value().as_ref(), half_deleted);
        assert!(fixture.view.read(cx).tab_editors[&id].dirty);

        window.click("dismiss-error", cx);
        window.press(crate::kbd!("s"), cx);
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some("Hex 必须为完整字节：每个字节需要两位十六进制数字。")
        );
        assert_eq!(hex_editor.read(cx).value().as_ref(), half_deleted);
        assert!(fixture.view.read(cx).tab_editors[&id].dirty);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "incomplete hex byte must block both Send and Save without enqueuing commands"
    );

    let mut expected_bytes = stored.tabs[0].draft.raw.clone();
    *expected_bytes.last_mut().unwrap() = b'\r';
    let completed_hex = format!("{}d", &initial_hex[..initial_hex.len() - 1]);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("dismiss-error", cx);
        window.click(("input", hex_editor.entity_id()), cx);
        #[cfg(target_os = "macos")]
        window.press("cmd-down", cx);
        #[cfg(not(target_os = "macos"))]
        window.press("ctrl-end", cx);
        window.press("end", cx);
        window.input("d", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(hex_editor.read(cx).value().as_ref(), completed_hex.as_str());
        assert!(fixture.view.read(cx).tab_editors[&id].dirty);

        window.press(crate::kbd!("s"), cx);
        window.render_frame(cx);
        assert!(fixture.view.read(cx).error.is_none());
        assert!(!fixture.view.read(cx).tab_editors[&id].dirty);
    })
    .unwrap();
    cx.run_until_parked();

    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace: saved_ws, .. }] = commands.as_slice() else {
        panic!("completing the hex byte and saving must enqueue SaveWorkspace");
    };
    assert_eq!(saved_ws.tabs[0].draft.raw, expected_bytes);
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved_ws.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject {
            path: None,
            workspace: None,
            ..
        }]
    ));
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(!view.tab_editors[&id].dirty);
        assert_eq!(view.workspace.tabs[0].draft.raw, expected_bytes);
        let status = rendered_status(window);
        assert!(status.starts_with("项目已保存") && !status.contains("未保存"));
    })
    .unwrap();
}
