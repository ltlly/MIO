use super::*;

#[gpui_kit::test]
fn stable_selection_retains_the_current_fragment_when_a_refreshed_page_moves_its_row(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, initial) = open(&mut fixture, cx);
    let message = summary(source, 7, FRAGMENT_BYTES as u64 + 2);
    page(&fixture, source, initial, None, 0, 1, vec![message.clone()]);
    cx.run_until_parked();
    select(&fixture, 0, cx);
    let first = read(&mut fixture);
    fragment(&fixture, first, message.clone(), vec![b'a'; FRAGMENT_BYTES]);
    cx.run_until_parked();
    click(&fixture, "ext-fragment-next", cx);
    let last = read(&mut fixture);
    assert_eq!((last.2, last.3), (FRAGMENT_BYTES as u64, 2));
    fragment(&fixture, last, message.clone(), b"xy".to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-messages-refresh", cx);
    let refreshed = query(&mut fixture).0;
    page(
        &fixture,
        source,
        refreshed,
        None,
        0,
        2,
        vec![summary(source, 6, 2), message],
    );
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "stable selection must not silently reread the first fragment"
    );
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.table.read(cx).selected_row(), Some(1));
        assert_eq!(
            messages.detail.as_ref().unwrap().offset,
            FRAGMENT_BYTES as u64
        );
        assert_eq!(messages.detail.as_ref().unwrap().bytes, b"xy");
    });
}

#[gpui_kit::test]
fn short_or_failed_current_fragment_never_leaves_previous_payload_visible_and_empty_is_valid(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, initial) = open(&mut fixture, cx);
    let message = summary(source, 0, 4);
    let empty = summary(source, 1, 0);
    page(
        &fixture,
        source,
        initial,
        None,
        0,
        2,
        vec![message.clone(), empty.clone()],
    );
    cx.run_until_parked();
    select(&fixture, 0, cx);
    let selected = read(&mut fixture);
    fragment(&fixture, selected, message.clone(), b"body".to_vec());
    cx.run_until_parked();
    fragment(&fixture, selected, message.clone(), b"b".to_vec());
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert!(
            messages.detail.is_none(),
            "incomplete range must not render as a valid message"
        );
        assert!(messages.payload.raw.read(cx).value().is_empty());
        assert!(messages.detail_error.is_some());
    });
    click(&fixture, "ext-fragment-retry", cx);
    let retry = read(&mut fixture);
    fragment(&fixture, retry, message, b"body".to_vec());
    fixture
        .events
        .try_send(Event::ExtMessage {
            project: fixture.project,
            selection_revision: retry.0,
            message_id: retry.1,
            offset: retry.2,
            result: Err("current read failed".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert!(messages.detail.is_none());
        assert!(messages.payload.raw.read(cx).value().is_empty());
        assert_eq!(
            messages.detail_error.as_deref(),
            Some("current read failed")
        );
    });
    select(&fixture, 1, cx);
    let selected = read(&mut fixture);
    assert_eq!((selected.2, selected.3), (0, 0));
    fragment(&fixture, selected, empty, vec![]);
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("ext-fragment-next").disabled(), Some(true));
        assert_eq!(window.find("ext-fragment-previous").disabled(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn many_collections_use_a_bounded_searchable_selector_without_losing_paths(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let mut imported = report();
    imported.collections = (0..150)
        .map(|index| mio_core::ExtCollectionReport {
            path: format!("root.f2.f1[{index}].f2"),
            logical_items: Some(1),
            indexed_items: 1,
            message_count: 1,
            state: "present".into(),
        })
        .collect();
    let (source, initial) = open_report(&mut fixture, imported, cx);
    page(&fixture, source, initial, None, 0, 0, vec![]);
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let selector = window.find("ext-collection-select");
        assert!(selector.bounds().size.width <= px(500.));
    })
    .unwrap();
    choose_collection(&fixture, "[149]", cx);
    let (_, collection, offset) = query(&mut fixture);
    assert_eq!(
        (collection.as_deref(), offset),
        (Some("root.f2.f1[149].f2"), 0)
    );
}
