use super::*;

#[gpui_kit::test]
fn editing_after_save_ack_cannot_keep_the_visible_saved_status(cx: &mut TestAppContext) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.active.unwrap();
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /not-saved HTTP/1.1\r\n\r\n保存之后继续输入";
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "editing must not silently save or send to make the status look current"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(editor.read(cx).value().as_ref(), latest);
        assert!(view.tab_editors[&id].dirty);
        assert_eq!(
            *view.workspace, stored,
            "new native input is not the old persisted snapshot"
        );
        assert!(view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        let status = rendered_status(window);
        assert!(
            !status.contains("项目已保存") && status.contains("未保存"),
            "the visible footer must distinguish unsaved input from the last ACK: {status}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn save_rejection_and_error_dismissal_keep_the_unsaved_status_and_latest_payload(
    cx: &mut TestAppContext,
) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.active.unwrap();
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /retry HTTP/1.1\r\n\r\n失败后仍须保留";
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            workspace: attempted,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("explicit Save must wait for the current workspace ACK before saving the project")
    };
    assert_eq!(attempted.revision, stored.revision + 1);
    assert_eq!(attempted.tabs[0].draft.raw, latest.as_bytes());
    let waiting = cx
        .update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert_eq!(view.pending_workspace, Some(attempted.revision));
            assert_eq!(view.workspace.revision, stored.revision);
            rendered_status(window)
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: attempted.revision,
            stored: stored.clone(),
            message: "synthetic workspace write failure".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "write rejection must not start a project save or automatically send/retry the payload"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap()
                .contains("workspace write failure")
        );
        window.click("dismiss-error", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.error.is_none());
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(view.workspace.revision, stored.revision);
        assert_eq!(view.workspace.tabs[0].draft.raw, latest.as_bytes());
        assert_eq!(editor.read(cx).value().as_ref(), latest);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        let status = rendered_status(window);
        assert!(!waiting.contains("项目已保存") && waiting.contains("正在保存")
            && !status.contains("项目已保存") && status.contains("未保存"),
            "the footer must show pending acknowledgement, then unsaved after closing the error; waiting={waiting}; after rejection={status}");
    }).unwrap();
}

#[gpui_kit::test]
fn late_editor_input_stays_unsaved_until_its_own_save_ack(cx: &mut TestAppContext) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.active.unwrap();
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let path = cx.read(|cx| fixture.view.read(cx).project_path.clone().unwrap());
    let first = "POST /first HTTP/1.1\r\n\r\n正在保存的版本";
    let latest = "POST /latest HTTP/1.1\r\n\r\n等待确认期间继续输入";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(first, cx);
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            workspace: first_save,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("the first explicit Save must submit only the captured workspace")
    };
    assert_eq!(first_save.tabs[0].draft.raw, first.as_bytes());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: first_save.clone(),
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
        assert!(status.contains("正在保存") && status.contains("未保存"));
        assert!(!status.contains("项目已保存"));
    })
    .unwrap();
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
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, *first_save);
        assert_eq!(editor.read(cx).value().as_ref(), latest);
        assert!(view.tab_editors[&id].dirty);
        assert!(view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        let status = rendered_status(window);
        assert!(status.contains("未保存") && !status.contains("项目已保存"));
        assert!(!status.contains("正在保存"));
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            workspace: latest_save,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("late input is saved only after the second explicit Save")
    };
    assert_eq!(latest_save.revision, first_save.revision + 1);
    assert_eq!(latest_save.tabs[0].draft.raw, latest.as_bytes());
    assert_eq!(
        latest_save.tabs[0].draft.revision,
        first_save.tabs[0].draft.revision + 1
    );
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: latest_save.clone(),
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
        assert!(status.contains("正在保存") && !status.contains("未保存"));
        assert!(!status.contains("项目已保存"));
    })
    .unwrap();
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 16384,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, *latest_save);
        assert_eq!(editor.read(cx).value().as_ref(), latest);
        assert!(!view.tab_editors[&id].dirty && !view.workspace_dirty);
        assert!(view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        let status = rendered_status(window);
        assert!(status.starts_with("项目已保存（16384 字节"));
        assert!(!status.contains("未保存") && !status.contains("正在保存"));
    })
    .unwrap();
}
