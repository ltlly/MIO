use super::*;

#[gpui_kit::test]
fn item_queue_failure_clears_old_editors_preserves_focus_and_invalidates_repeater_load(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("保留的工作区")], 1);
    let (source, revision) = open(&mut fixture, cx);
    let a = row(source, 0, "present", 1);
    let b = row(source, 1, "empty", 0);
    let rows = vec![a.clone(), b.clone()];
    page(&fixture, source, revision, None, 0, 2, rows.clone());
    pump_native(cx);
    select(&fixture, "ext-items-table", 0, cx);
    let request = selected(&mut fixture, source);
    let raw = message(&a, 4);
    deliver(
        &fixture,
        source,
        request,
        Ok(detail(&a, b"{\"owner\":\"A\"}", 0, vec![raw.clone()])),
    );
    pump_native(cx);
    select(&fixture, "ext-message-table", 0, cx);
    let body = read(&mut fixture);
    fragment(&fixture, body, raw, b"body".to_vec());
    pump_native(cx);
    click(&fixture, "ext-open-repeater", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::OpenExtRepeater {
            project,
            operation_id,
            source_id,
            item_id,
            message_id,
            workspace_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("real load click must start only the selected archive proposal")
    };
    let mut proposal = tab("迟到不应载入");
    proposal.draft.target.clear();
    proposal.draft.raw = b"body".to_vec();
    let late = Event::ExtRepeaterOpened {
        project: *project,
        operation_id: *operation_id,
        source_id: *source_id,
        item_id: *item_id,
        message_id: *message_id,
        workspace_revision: *workspace_revision,
        result: Ok(proposal),
    };
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let editor = fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .items
            .as_ref()
            .unwrap()
            .metadata
            .clone();
        window.click(("input", editor.entity_id()), cx);
        window.render_frame(cx);
        assert!(editor.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .commands
            .try_send(Command::QueryExtReport {
                project: fixture.project,
                query_revision: 99,
            })
            .unwrap()
    });
    select(&fixture, "ext-items-table", 1, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        let messages = view.ext_import.messages.as_ref().unwrap();
        let items = messages.items.as_ref().unwrap();
        assert_eq!(items.selected, Some(b.id));
        assert!(items.metadata.read(cx).value().is_empty());
        assert!(items.detail_error.as_deref().unwrap().contains("队列已满"));
        assert!(messages.table.read(cx).delegate().rows.is_empty());
        assert!(messages.payload.raw.read(cx).value().is_empty());
        // A row click focuses the visible table, not its workspace ancestor.
        assert!(items.table.focus_handle(cx).is_focused(window));
        assert!(view.focus.contains_focused(window, cx));
        assert!(!items.metadata.focus_handle(cx).is_focused(window));
        assert!(!messages.payload.raw.focus_handle(cx).is_focused(window));
        assert!(!messages.payload.hex.focus_handle(cx).is_focused(window));
        assert!(window.is_action_available(&Quit, cx));
    })
    .unwrap();
    for (key, expected) in [("up", a.id), ("down", b.id)] {
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        })
        .unwrap();
        pump_native(cx);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            let items = view
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .items
                .as_ref()
                .unwrap();
            assert_eq!(items.selected, Some(expected));
            assert!(items.table.focus_handle(cx).is_focused(window));
            assert!(view.focus.contains_focused(window, cx));
            assert!(items.detail_error.as_deref().unwrap().contains("队列已满"));
        })
        .unwrap();
    }
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("q"), cx)
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "the focused table must route Cmd-Q to the workspace"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(
        !cx.has_pending_prompt(),
        "one key press must open one confirmation"
    );
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryExtReport {
            query_revision: 99,
            ..
        }]
    ));
    fixture.events.try_send(late).unwrap();
    pump_native(cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "scope change revokes the pending Repeater proposal even when the item read could not enqueue"
    );
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace.tabs.len(), 1));
    click(&fixture, "ext-item-retry", cx);
    let retry = selected(&mut fixture, source);
    assert_eq!(retry.1, b.id);
    deliver(
        &fixture,
        source,
        retry,
        Ok(detail(&b, b"{\"owner\":\"B\"}", 0, vec![])),
    );
    pump_native(cx);
    // A failed collection enqueue must also hide the previous item's metadata immediately.
    cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .commands
            .try_send(Command::QueryExtReport {
                project: fixture.project,
                query_revision: 100,
            })
            .unwrap()
    });
    choose(&fixture, "root.f2.f1", cx);
    drain(&mut fixture.commands);
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        let items = messages.items.as_ref().unwrap();
        assert!(items.selected.is_none());
        assert!(items.detail.is_none());
        assert!(items.metadata.read(cx).value().is_empty());
        assert!(
            items
                .table
                .read(cx)
                .delegate()
                .error
                .as_deref()
                .unwrap()
                .contains("队列已满")
        );
    });
    click(&fixture, "ext-items-refresh", cx);
    let (retry, filter, offset) = query(&mut fixture, source);
    assert_eq!((filter.as_deref(), offset), (Some("root.f2.f1"), 0));
    page(&fixture, source, retry, filter, 0, 2, rows);
    pump_native(cx);
    select(&fixture, "ext-items-table", 1, cx);
    let request = selected(&mut fixture, source);
    deliver(&fixture, source, request, Ok(detail(&b, b"{}", 0, vec![])));
    pump_native(cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let editor = fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .items
            .as_ref()
            .unwrap()
            .metadata
            .clone();
        window.click(("input", editor.entity_id()), cx);
        window.render_frame(cx);
        assert!(editor.focus_handle(cx).is_focused(window));
        window.click("ext-browser-mode", cx);
        window.render_frame(cx);
        assert!(fixture.view.read(cx).focus.is_focused(window));
        window.press(crate::kbd!("q"), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "hiding the metadata editor must preserve the actual Quit action route"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryExtMessages { .. }]
    ));
}
