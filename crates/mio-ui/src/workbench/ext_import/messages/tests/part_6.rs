use super::*;

#[gpui_kit::test]
fn collection_filters_and_pages_preserve_source_positions_and_ignore_obsolete_results(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, initial) = open(&mut fixture, cx);
    let selected = summary(source, 7, 4);
    page(
        &fixture,
        source,
        initial,
        None,
        0,
        1,
        vec![selected.clone()],
    );
    cx.run_until_parked();
    select(&fixture, 0, cx);
    let selected_read = read(&mut fixture);
    assert_eq!(selected_read.3, 4);
    fragment(&fixture, selected_read, selected.clone(), b"body".to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-messages-refresh", cx);
    let refresh = query(&mut fixture).0;
    let inserted = summary(source, 6, 2);
    page(
        &fixture,
        source,
        refresh,
        None,
        0,
        2,
        vec![inserted, selected.clone()],
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.table.read(cx).selected_row(), Some(1));
        assert_eq!(messages.selected, Some(selected.id));
        assert_eq!(messages.detail.as_ref().unwrap().bytes, b"body");
    });
    assert!(drain(&mut fixture.commands).is_empty());
    choose_collection(&fixture, "root.f2.f1", cx);
    let (filtered, collection, offset) = query(&mut fixture);
    assert_eq!(collection.as_deref(), Some("root.f2.f1"));
    assert_eq!(offset, 0);
    fixture
        .events
        .try_send(Event::ExtMessages {
            project: fixture.project,
            query_revision: refresh,
            source_id: source,
            collection: None,
            offset: 0,
            result: Err("old all-collections failure".into()),
        })
        .unwrap();
    let rows: Vec<_> = (0..PAGE_SIZE)
        .map(|index| summary(source, index as u64, 4))
        .collect();
    page(
        &fixture,
        source,
        filtered,
        collection.clone(),
        0,
        PAGE_SIZE + 1,
        rows,
    );
    cx.run_until_parked();
    click(&fixture, "ext-messages-next", cx);
    click(&fixture, "ext-messages-next", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryExtMessages {
            query_revision: next,
            collection: actual,
            offset,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("loading disables duplicate page navigation")
    };
    assert_eq!(*offset, PAGE_SIZE);
    assert_eq!(*actual, collection);
    let last = summary(source, 999, 2);
    page(
        &fixture,
        source,
        *next,
        collection,
        PAGE_SIZE,
        PAGE_SIZE + 1,
        vec![last.clone()],
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.table.read(cx).delegate().rows[0].item_index, 999);
        assert!(messages.table.read(cx).delegate().error.is_none());
        assert!(messages.detail.is_none());
    });
    click(&fixture, "ext-messages-all", cx);
    let (all, collection, offset) = query(&mut fixture);
    assert!(all > *next);
    assert_eq!((collection, offset), (None, 0));
}

#[gpui_kit::test]
fn body_queue_failure_and_correlated_query_errors_have_working_retry_controls(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 1);
    let (source, initial) = open(&mut fixture, cx);
    let message = summary(source, 0, 4);
    page(&fixture, source, initial, None, 0, 1, vec![message.clone()]);
    cx.run_until_parked();
    click(&fixture, "ext-messages-refresh", cx);
    select(&fixture, 0, cx);
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert!(messages.detail.is_none());
        assert!(messages.detail_error.as_ref().unwrap().contains("队列已满"));
    });
    let pending_query = query(&mut fixture).0;
    click(&fixture, "ext-fragment-retry", cx);
    let retry = read(&mut fixture);
    assert_eq!(retry.3, 4);
    fixture
        .events
        .try_send(Event::ExtMessage {
            project: fixture.project,
            selection_revision: retry.0 - 1,
            message_id: message.id,
            offset: 0,
            result: Err("old body failure".into()),
        })
        .unwrap();
    fragment(&fixture, retry, message.clone(), b"body".to_vec());
    fixture
        .events
        .try_send(Event::ExtMessages {
            project: fixture.project,
            query_revision: pending_query,
            source_id: source,
            collection: None,
            offset: 0,
            result: Err("archive query unavailable".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("重试查询").is_some());
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.detail.as_ref().unwrap().bytes, b"body");
        assert!(messages.detail_error.is_none());
        assert_eq!(messages.table.read(cx).delegate().rows, [message]);
    })
    .unwrap();
    click(&fixture, "ext-messages-refresh", cx);
    assert!(query(&mut fixture).0 > pending_query);
}
