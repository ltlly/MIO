use super::*;

#[gpui_kit::test]
fn budgeted_summary_pages_use_true_next_offsets_and_previous_page_starts(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, first) = open(&mut fixture, cx);
    // The store can return a complete prefix below the 200-row ceiling to meet its byte budget.
    let rows: Vec<_> = (0..40).map(|index| summary(source, index, 4)).collect();
    page(&fixture, source, first, None, 0, 40, rows[..37].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-messages-next", cx);
    let (second, filter, offset) = query(&mut fixture);
    assert_eq!(
        (filter, offset),
        (None, 37),
        "a short page must not skip unseen messages"
    );
    page(
        &fixture,
        source,
        second,
        None,
        37,
        40,
        rows[37..39].to_vec(),
    );
    cx.run_until_parked();
    click(&fixture, "ext-messages-next", cx);
    let (third, _, offset) = query(&mut fixture);
    assert_eq!(offset, 39);
    page(&fixture, source, third, None, 39, 40, rows[39..].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-messages-next", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "last page has no next query"
    );
    click(&fixture, "ext-messages-previous", cx);
    let (back, _, offset) = query(&mut fixture);
    assert_eq!(
        offset, 37,
        "previous must return the actual prior page start"
    );
    page(&fixture, source, back, None, 37, 40, rows[37..39].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-messages-refresh", cx);
    let (refresh, _, offset) = query(&mut fixture);
    assert_eq!(offset, 37);
    page(
        &fixture,
        source,
        refresh,
        None,
        37,
        40,
        rows[37..39].to_vec(),
    );
    cx.run_until_parked();
    click(&fixture, "ext-messages-previous", cx);
    let (back, _, offset) = query(&mut fixture);
    assert_eq!(
        offset, 0,
        "refresh must not push a duplicate navigation anchor"
    );
    page(&fixture, source, back, None, 0, 40, rows[..37].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-messages-previous", cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn failed_next_page_retries_requested_offset_without_discarding_visible_page(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, first) = open(&mut fixture, cx);
    let rows: Vec<_> = (0..PAGE_SIZE)
        .map(|index| summary(source, index as u64, 4))
        .collect();
    page(
        &fixture,
        source,
        first,
        None,
        0,
        PAGE_SIZE + 1,
        rows.clone(),
    );
    cx.run_until_parked();
    click(&fixture, "ext-messages-next", cx);
    let (failed, _, offset) = query(&mut fixture);
    assert_eq!(offset, PAGE_SIZE);
    fixture
        .events
        .try_send(Event::ExtMessages {
            project: fixture.project,
            query_revision: failed,
            source_id: source,
            collection: None,
            offset,
            result: Err("synthetic query unavailable".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.table.read(cx).delegate().offset, 0);
        assert_eq!(messages.table.read(cx).delegate().rows, rows);
        assert!(messages.table.read(cx).delegate().error.is_some());
    });
    click(&fixture, "ext-messages-refresh", cx);
    let (retry, _, offset) = query(&mut fixture);
    assert!(retry > failed);
    assert_eq!(
        offset, PAGE_SIZE,
        "retry must repeat the failed request, not the previous visible page"
    );
    // A delayed failed-request reply cannot overwrite the latest request or its navigation state.
    page(
        &fixture,
        source,
        failed,
        None,
        PAGE_SIZE,
        PAGE_SIZE + 1,
        vec![summary(source, PAGE_SIZE as u64, 4)],
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.table.read(cx).delegate().offset, 0);
        assert!(messages.table.read(cx).delegate().loading);
    });
    page(
        &fixture,
        source,
        retry,
        None,
        PAGE_SIZE,
        PAGE_SIZE + 1,
        vec![summary(source, PAGE_SIZE as u64, 4)],
    );
    cx.run_until_parked();
    click(&fixture, "ext-messages-previous", cx);
    assert_eq!(query(&mut fixture).2, 0);
}

#[gpui_kit::test]
fn ext_message_fragment_search_bar_handles_cmd_f_and_options(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, revision) = open(&mut fixture, cx);
    let bytes = b"GET /archive?token=Alpha HTTP/1.1\r\nX-Token: alpha\r\n\r\nTOKEN=ALPHA".to_vec();
    let item = summary(source, 0, bytes.len() as u64);
    page(&fixture, source, revision, None, 0, 1, vec![item.clone()]);
    cx.run_until_parked();
    select(&fixture, 0, cx);
    let selected = read(&mut fixture);
    fragment(&fixture, selected, item, bytes);
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let (raw, search) = {
            let payload = &fixture
                .view
                .read(cx)
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .payload;
            (payload.raw.clone(), payload.search.clone())
        };
        window.click(("input", raw.entity_id()), cx);
        let text = raw.read(cx).value().to_string();
        let start = text.find("Alpha").unwrap();
        raw.update(cx, |state, cx| {
            state.set_selected_range(start..start + 5, cx)
        });
        window.press(crate::kbd!("f"), cx);
        assert!(search.focus_handle(cx).is_focused(window));
        assert!(!raw.read(cx).search_session().open);
        assert_eq!(search.read(cx).value().as_ref(), "Alpha");
        assert_eq!(
            fixture
                .view
                .read(cx)
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .payload
                .search_status_label(cx),
            "1/3 个匹配"
        );
        assert!(window.try_find("ext-message-search-case").is_none());
        assert!(window.try_find("ext-message-search-regex").is_none());
        assert!(window.try_find("ext-message-search-replace-toggle").is_none());
        window.click("ext-message-search-options", cx);
        window.render_frame(cx);
        assert!(window.try_find("ext-message-search-case").is_some());
        assert!(window.try_find("ext-message-search-regex").is_some());
        assert!(window.try_find("ext-message-search-replace-toggle").is_none());
        window.click("ext-message-search-case", cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .payload
                .search_status_label(cx),
            "1/1 个匹配"
        );
    })
    .unwrap();
}
