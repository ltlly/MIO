use super::*;

#[gpui_kit::test]
fn wrong_page_offset_and_changed_summary_cannot_relabel_a_cached_message(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, initial) = open(&mut fixture, cx);
    let original = summary(source, 0, 4);
    page(
        &fixture,
        source,
        initial,
        None,
        0,
        1,
        vec![original.clone()],
    );
    cx.run_until_parked();
    select(&fixture, 0, cx);
    let selected = read(&mut fixture);
    fragment(&fixture, selected, original.clone(), b"body".to_vec());
    cx.run_until_parked();
    click(&fixture, "ext-messages-refresh", cx);
    let refreshed = query(&mut fixture).0;
    page(
        &fixture,
        source,
        refreshed,
        None,
        PAGE_SIZE,
        PAGE_SIZE + 1,
        vec![summary(source, 1000, 2)],
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.table.read(cx).delegate().offset, 0);
        assert_eq!(messages.detail.as_ref().unwrap().summary, original);
        assert!(messages.table.read(cx).delegate().loading);
    });
    let mut changed = original;
    changed.raw_bytes = 3;
    page(
        &fixture,
        source,
        refreshed,
        None,
        0,
        1,
        vec![changed.clone()],
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert!(
            messages.detail.is_none(),
            "cached bytes must not be relabelled with a different summary"
        );
        assert!(messages.payload.raw.read(cx).value().is_empty());
        assert!(messages.detail_error.is_some());
    });
    click(&fixture, "ext-fragment-retry", cx);
    let retry = read(&mut fixture);
    assert_eq!(retry.3, 3);
    fragment(&fixture, retry, changed, b"new".to_vec());
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .detail
                .as_ref()
                .unwrap()
                .bytes,
            b"new"
        )
    });
}

#[gpui_kit::test]
fn a_full_queue_does_not_change_the_visible_collection_selection(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 1);
    let (source, initial) = open(&mut fixture, cx);
    page(&fixture, source, initial, None, 0, 0, vec![]);
    cx.run_until_parked();
    cx.update(|cx| {
        fixture
            .view
            .read(cx)
            .commands
            .try_send(Command::QueryExtReport {
                project: fixture.project,
                query_revision: 100,
            })
            .unwrap();
    });
    choose_collection(&fixture, "root.f2.f1", cx);
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.filter, None);
        assert!(messages.collections.read(cx).selected_value().is_none());
        assert!(
            messages
                .table
                .read(cx)
                .delegate()
                .error
                .as_ref()
                .unwrap()
                .contains("队列已满")
        );
    });
    drain(&mut fixture.commands);
    choose_collection(&fixture, "root.f2.f1", cx);
    assert_eq!(query(&mut fixture).1.as_deref(), Some("root.f2.f1"));
}
