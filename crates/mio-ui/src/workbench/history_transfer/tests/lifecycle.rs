use super::*;

#[gpui_kit::test]
fn history_transfer_waits_for_saved_revision_before_reading_and_rejects_partial_proposals(
    cx: &mut TestAppContext,
) {
    let (mut f, _) = setup(cx);
    edit_active(
        &f,
        "GET /pending HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n",
        cx,
    );
    f.window
        .update(cx, |_, window, cx| {
            let active = f.view.read(cx).workspace.active.unwrap();
            // The center of this short tab can hit its nested close button.
            window.click_at(
                format!("work-tab-{active}"),
                gpui_kit::point(px(8.), px(12.)),
                cx,
            );
            let view = f.view.read(cx);
            assert_eq!(
                view.workspace.tabs.len(),
                1,
                "selecting the label must not close the tab"
            );
            assert!(
                view.pending_workspace.is_some(),
                "the edited tab selection must start a real save"
            );
            window.click("tool-proxy", cx);
            window.click("proxy-history", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = relevant(&mut f);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!(
            "selecting the edited tab saves current workspace; commands={:?}, state={:?}",
            command_summary(&commands),
            state_summary(&f, cx)
        )
    };
    let saved = workspace.clone();
    menu(&f, false, cx);
    assert!(relevant(&mut f).is_empty());
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: saved.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let mut commands = relevant(&mut f);
    let mut confirmed_revision = saved.revision;
    if let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() {
        confirmed_revision = workspace.revision;
        f.events
            .try_send(Event::WorkspaceChanged {
                project: f.project,
                workspace: workspace.clone(),
            })
            .unwrap();
        cx.run_until_parked();
        commands = relevant(&mut f);
    }
    let [
        Command::ReadHistoryTransfer {
            operation_id: op,
            workspace_revision: revision,
            entries,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("read begins only after current workspace acknowledgements")
    };
    assert_eq!(*revision, confirmed_revision);
    let mut partial = proposals(entries);
    partial.pop();
    reply(&f, *op, *revision, Ok(partial), cx);
    assert!(relevant(&mut f).is_empty());
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.tabs, saved.tabs);
        assert!(view.history_transfer.is_none());
        assert!(view.error.as_ref().unwrap().contains("不匹配"));
    });
}

#[gpui_kit::test]
fn history_transfer_timeout_keeps_ownership_until_actual_reply_and_project_reset_rejects_late_result(
    cx: &mut TestAppContext,
) {
    let (mut f, _) = setup(cx);
    menu(&f, false, cx);
    let (op, revision, entries, cancel) = read(&mut f);
    cx.executor()
        .advance_clock(DEADLINE + Duration::from_secs(1));
    cx.run_until_parked();
    assert!(cancel.is_cancelled());
    cx.read(|cx| assert!(f.view.read(cx).history_transfer.is_some()));
    menu(&f, false, cx);
    assert!(relevant(&mut f).is_empty());
    reply(&f, op, revision, Ok(proposals(&entries)), cx);
    cx.read(|cx| {
        assert!(f.view.read(cx).history_transfer.is_none());
        assert_eq!(f.view.read(cx).workspace.tabs.len(), 1);
    });
    assert!(relevant(&mut f).is_empty());
    menu(&f, false, cx);
    let (op, revision, entries, cancel) = read(&mut f);
    let new_project = ProjectKey {
        id: Uuid::new_v4(),
        generation: f.project.generation + 1,
    };
    f.events
        .try_send(Event::ProjectOpened {
            project: new_project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cancel.is_cancelled());
    reply(&f, op, revision, Ok(proposals(&entries)), cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.project, Some(new_project));
        assert!(view.workspace.tabs.is_empty() && view.workspace.intruder.tabs.is_empty());
        assert!(view.history_transfer.is_none());
    });
    assert!(relevant(&mut f).is_empty());
}
