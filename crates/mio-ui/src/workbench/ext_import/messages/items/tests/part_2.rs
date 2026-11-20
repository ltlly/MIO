use super::*;

#[gpui_kit::test]
fn item_and_item_message_short_pages_use_actual_offsets_and_retry_without_global_queries(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, first) = open(&mut fixture, cx);
    let rows: Vec<_> = (0..6)
        .map(|index| row(source, index, "present", 5))
        .collect();
    page(&fixture, source, first, None, 0, 6, rows[..2].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-items-next", cx);
    let (failed, _, offset) = query(&mut fixture, source);
    assert_eq!(offset, 2);
    fixture
        .events
        .try_send(Event::ExtItems {
            project: fixture.project,
            source_id: source,
            query_revision: failed,
            collection: None,
            offset,
            result: Err("synthetic page failure".into()),
        })
        .unwrap();
    cx.run_until_parked();
    click(&fixture, "ext-items-refresh", cx);
    let (retry, _, offset) = query(&mut fixture, source);
    assert!(retry > failed);
    assert_eq!(offset, 2);
    page(&fixture, source, failed, None, 2, 6, rows[2..5].to_vec());
    cx.run_until_parked();
    cx.read(|cx| {
        let table = &fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .items
            .as_ref()
            .unwrap()
            .table;
        assert_eq!(table.read(cx).delegate().offset, 0);
        assert!(table.read(cx).delegate().loading);
    });
    page(&fixture, source, retry, None, 2, 6, rows[2..5].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-items-next", cx);
    let (last, _, offset) = query(&mut fixture, source);
    assert_eq!(offset, 5);
    page(&fixture, source, last, None, 5, 6, rows[5..].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-items-previous", cx);
    let (back, _, offset) = query(&mut fixture, source);
    assert_eq!(offset, 2);
    page(&fixture, source, back, None, 2, 6, rows[2..5].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-items-refresh", cx);
    let (refresh, _, offset) = query(&mut fixture, source);
    assert_eq!(offset, 2);
    page(&fixture, source, refresh, None, 2, 6, rows[2..5].to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-items-previous", cx);
    let (back, _, offset) = query(&mut fixture, source);
    assert_eq!(offset, 0);
    page(&fixture, source, back, None, 0, 6, rows[..2].to_vec());
    cx.run_until_parked();
    select(&fixture, "ext-items-table", 0, cx);
    let read_item = selected(&mut fixture, source);
    let messages: Vec<_> = (0..5)
        .map(|field| {
            let mut message = message(&rows[0], 0);
            message.field_id = field;
            message
        })
        .collect();
    deliver(
        &fixture,
        source,
        read_item,
        Ok(detail(&rows[0], b"{}", 0, messages[..2].to_vec())),
    );
    cx.run_until_parked();
    for (expected, range) in [(2, 2..4), (4, 4..5)] {
        click(&fixture, "ext-messages-next", cx);
        let request = selected(&mut fixture, source);
        assert_eq!((request.1, request.2), (rows[0].id, expected));
        deliver(
            &fixture,
            source,
            request,
            Ok(detail(&rows[0], b"{}", expected, messages[range].to_vec())),
        );
        cx.run_until_parked();
    }
    click(&fixture, "ext-messages-previous", cx);
    let request = selected(&mut fixture, source);
    assert_eq!(request.2, 2);
    deliver(
        &fixture,
        source,
        request,
        Ok(detail(&rows[0], b"{}", 2, messages[2..4].to_vec())),
    );
    cx.run_until_parked();
    click(&fixture, "ext-messages-refresh", cx);
    let request = selected(&mut fixture, source);
    assert_eq!(request.2, 2);
    deliver(
        &fixture,
        source,
        request,
        Ok(detail(&rows[0], b"{}", 2, messages[2..4].to_vec())),
    );
    cx.run_until_parked();
    click(&fixture, "ext-messages-previous", cx);
    assert_eq!(selected(&mut fixture, source).2, 0);
    choose(&fixture, "root.f2.f1", cx);
    let (filtered, filter, offset) = query(&mut fixture, source);
    assert_eq!((filter.as_deref(), offset), (Some("root.f2.f1"), 0));
    page(&fixture, source, filtered, filter, 0, 0, vec![]);
    cx.run_until_parked();
    click(&fixture, "ext-items-all", cx);
    let (all, filter, offset) = query(&mut fixture, source);
    assert!(all > filtered && filter.is_none() && offset == 0);
    page(&fixture, source, back, None, 0, 6, rows[..2].to_vec());
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .items
                .as_ref()
                .unwrap()
                .table
                .read(cx)
                .delegate()
                .loading
        )
    });
    page(&fixture, source, all, None, 0, 6, rows[..2].to_vec());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn item_message_retry_keeps_the_nonzero_offset_and_rejects_expired_pages(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, revision) = open(&mut fixture, cx);
    let item = row(source, 0, "present", 3);
    let messages: Vec<_> = (0..3).map(|_| message(&item, 0)).collect();
    page(&fixture, source, revision, None, 0, 1, vec![item.clone()]);
    cx.run_until_parked();
    select(&fixture, "ext-items-table", 0, cx);
    let first = selected(&mut fixture, source);
    deliver(
        &fixture,
        source,
        first,
        Ok(detail(&item, b"{}", 0, messages[..2].to_vec())),
    );
    cx.run_until_parked();

    click(&fixture, "ext-messages-next", cx);
    let failed = selected(&mut fixture, source);
    assert_eq!((failed.1, failed.2), (item.id, 2));
    deliver(
        &fixture,
        source,
        failed,
        Err("synthetic nonzero message page failure".into()),
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let browser = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        let items = browser.items.as_ref().unwrap();
        assert_eq!(items.selected, Some(item.id));
        assert_eq!(items.requested_message_offset, 2);
        assert!(items.detail.is_none() && items.metadata.read(cx).value().is_empty());
        assert!(
            items
                .detail_error
                .as_deref()
                .unwrap()
                .contains("nonzero message page failure")
        );
        assert_eq!(browser.table.read(cx).delegate().offset, 0);
        assert!(!browser.table.read(cx).delegate().loading);
    });

    click(&fixture, "ext-item-retry", cx);
    let retry = selected(&mut fixture, source);
    assert_eq!((retry.1, retry.2), (item.id, 2));
    assert!(retry.0 > failed.0);
    deliver(
        &fixture,
        source,
        failed,
        Ok(detail(
            &item,
            br#"{"obsolete":true}"#,
            2,
            messages[2..].to_vec(),
        )),
    );
    deliver(&fixture, source, failed, Err("obsolete retry error".into()));
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let browser = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        let items = browser.items.as_ref().unwrap();
        assert_eq!(items.selection_revision, retry.0);
        assert!(items.detail.is_none() && items.detail_error.is_none());
        assert!(items.metadata.read(cx).value().is_empty());
        let page = browser.table.read(cx).delegate();
        assert!(page.loading && page.error.is_none());
        assert_eq!(page.offset, 0);
        assert_eq!(page.rows, messages[..2]);
    });

    let metadata = br#"{"accepted_offset":2}"#;
    deliver(
        &fixture,
        source,
        retry,
        Ok(detail(&item, metadata, 2, messages[2..].to_vec())),
    );
    cx.run_until_parked();
    deliver(
        &fixture,
        source,
        first,
        Ok(detail(&item, b"{}", 0, messages[..2].to_vec())),
    );
    deliver(
        &fixture,
        source,
        failed,
        Err("late failure after success".into()),
    );
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let browser = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        let items = browser.items.as_ref().unwrap();
        assert_eq!(items.selected, Some(item.id));
        assert_eq!(items.detail.as_ref().unwrap().metadata_json, metadata);
        assert_eq!(items.metadata.read(cx).value().as_bytes(), metadata);
        assert!(items.detail_error.is_none());
        let page = browser.table.read(cx).delegate();
        assert_eq!((page.offset, page.total, page.next_offset), (2, 3, None));
        assert_eq!(page.rows, messages[2..]);
        assert!(!page.loading && page.error.is_none());
        assert_eq!(browser.previous_offsets, [0]);
    });
    click(&fixture, "ext-messages-previous", cx);
    let previous = selected(&mut fixture, source);
    assert_eq!((previous.1, previous.2), (item.id, 0));
}
