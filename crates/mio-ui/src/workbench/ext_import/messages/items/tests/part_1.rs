use super::*;

#[gpui_kit::test]
fn all_positions_keep_null_failed_empty_duplicate_addresses_and_readonly_original_metadata(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, revision) = open(&mut fixture, cx);
    let mut rows = vec![
        row(source, 0, "null", 0),
        row(source, 1, "failed", 0),
        row(source, 2, "empty", 0),
        row(source, 3, "present", 1),
        row(source, 4, "present", 0),
    ];
    let mut history = row(source, 0, "present", 1);
    history.collection = "root.f2.f1[3].f2".into();
    rows.push(history);
    page(
        &fixture,
        source,
        revision,
        None,
        0,
        rows.len(),
        rows.clone(),
    );
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "summary results do not read item metadata or bodies"
    );
    let metadata = b" { \"f17\": null, \"f20\": { \"state\": \"unverified\" } }\n";
    for (index, item) in rows.iter().take(3).enumerate() {
        select(&fixture, "ext-items-table", index, cx);
        let request = selected(&mut fixture, source);
        assert_eq!((request.1, request.2), (item.id, 0));
        deliver(
            &fixture,
            source,
            request,
            Ok(detail(item, metadata, 0, vec![])),
        );
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
            let items = messages.items.as_ref().unwrap();
            assert_eq!(items.table.read(cx).delegate().rows, rows);
            assert_eq!(items.detail.as_ref().unwrap().metadata_json, metadata);
            assert_eq!(items.metadata.read(cx).value().as_bytes(), metadata);
            assert!(messages.table.read(cx).delegate().rows.is_empty());
            assert!(messages.detail.is_none());
            let editor = items.metadata.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("must stay read only", cx);
            assert_eq!(editor.read(cx).value().as_bytes(), metadata);
        })
        .unwrap();
        assert!(drain(&mut fixture.commands).is_empty());
    }
    select(&fixture, "ext-items-table", 3, cx);
    let request = selected(&mut fixture, source);
    let raw = message(&rows[3], FRAGMENT_BYTES as u64 + 2);
    deliver(
        &fixture,
        source,
        request,
        Ok(detail(&rows[3], metadata, 0, vec![raw.clone()])),
    );
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "item details contain only summaries, never automatic body reads"
    );
    select(&fixture, "ext-message-table", 0, cx);
    let first = read(&mut fixture);
    assert_eq!((first.1, first.2, first.3), (raw.id, 0, FRAGMENT_BYTES));
    fragment(&fixture, first, raw.clone(), vec![0xff; FRAGMENT_BYTES]);
    cx.run_until_parked();
    click(&fixture, "ext-fragment-next", cx);
    let last = read(&mut fixture);
    assert_eq!((last.2, last.3), (FRAGMENT_BYTES as u64, 2));
    fragment(&fixture, last, raw, vec![0, 0xff]);
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.payload.view(), MessageView::Hex);
        assert_eq!(messages.payload.hex.read(cx).value().as_ref(), "00 ff");
    });
    select(&fixture, "ext-items-table", 4, cx);
    let repeated = selected(&mut fixture, source);
    assert_ne!(
        repeated.1, request.1,
        "equal object addresses do not merge source positions"
    );
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert!(messages.detail.is_none());
        assert!(messages.table.read(cx).delegate().rows.is_empty());
        assert!(
            messages
                .items
                .as_ref()
                .unwrap()
                .metadata
                .read(cx)
                .value()
                .is_empty()
        );
    });
    deliver(
        &fixture,
        source,
        repeated,
        Ok(detail(&rows[4], br#"{"current_request":null}"#, 0, vec![])),
    );
    cx.run_until_parked();
    select(&fixture, "ext-items-table", 5, cx);
    let history = selected(&mut fixture, source);
    let mut old = message(&rows[5], 0);
    old.role = "request".into();
    deliver(
        &fixture,
        source,
        history,
        Ok(detail(
            &rows[5],
            br#"{"history":true}"#,
            0,
            vec![old.clone()],
        )),
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.table.read(cx).delegate().rows, [old]);
        assert!(messages.detail.is_none());
    });
}

#[gpui_kit::test]
fn item_selection_generations_and_field_ownership_reject_late_or_corrupt_details(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (source, revision) = open(&mut fixture, cx);
    let a = row(source, 0, "present", 1);
    let b = row(source, 1, "empty", 0);
    page(
        &fixture,
        source,
        revision,
        None,
        0,
        2,
        vec![a.clone(), b.clone()],
    );
    cx.run_until_parked();
    select(&fixture, "ext-items-table", 0, cx);
    let old_a = selected(&mut fixture, source);
    select(&fixture, "ext-items-table", 1, cx);
    let old_b = selected(&mut fixture, source);
    select(&fixture, "ext-items-table", 0, cx);
    let current = selected(&mut fixture, source);
    assert!(current.0 > old_b.0 && old_b.0 > old_a.0);
    let raw = message(&a, 4);
    let correct = detail(&a, b" { \"a\": 2 }\n", 0, vec![raw]);
    deliver(&fixture, source, old_a, Ok(correct.clone()));
    deliver(&fixture, source, old_b, Err("late B error".into()));
    let mut wrong_project = fixture.project;
    wrong_project.generation += 1;
    fixture
        .events
        .try_send(Event::ExtItem {
            project: wrong_project,
            source_id: source,
            selection_revision: current.0,
            item_id: a.id,
            message_offset: 0,
            result: Ok(correct.clone()),
        })
        .unwrap();
    deliver(&fixture, Uuid::new_v4(), current, Ok(correct.clone()));
    deliver(
        &fixture,
        source,
        (current.0, current.1, 1),
        Ok(correct.clone()),
    );
    cx.run_until_parked();
    cx.read(|cx| {
        let items = fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .items
            .as_ref()
            .unwrap();
        assert!(items.detail.is_none());
        assert!(items.detail_error.is_none());
    });
    deliver(&fixture, source, current, Ok(correct.clone()));
    cx.run_until_parked();
    for corruption in [
        "id",
        "source",
        "collection",
        "index",
        "address",
        "state",
        "count",
        "message-item",
        "message-source",
        "message-collection",
        "message-index",
        "utf8",
        "oversize",
        "next",
    ] {
        click(&fixture, "ext-messages-refresh", cx);
        let request = selected(&mut fixture, source);
        let mut invalid = correct.clone();
        match corruption {
            "id" => invalid.item.id = Uuid::new_v4(),
            "source" => invalid.item.source_id = Uuid::new_v4(),
            "collection" => invalid.item.collection = b.collection.clone() + "[1].f2",
            "index" => invalid.item.index += 1,
            "address" => invalid.item.object_address = None,
            "state" => invalid.item.state = "failed".into(),
            "count" => {
                invalid.messages.total = 0;
                invalid.messages.rows.clear();
            }
            "message-item" => invalid.messages.rows[0].item_id = b.id,
            "message-source" => invalid.messages.rows[0].source_id = Uuid::new_v4(),
            "message-collection" => invalid.messages.rows[0].collection += "[0].f2",
            "message-index" => invalid.messages.rows[0].item_index += 1,
            "utf8" => invalid.item.metadata_json = vec![0xff],
            "oversize" => invalid.item.metadata_json = vec![b' '; METADATA_BYTES + 1],
            "next" => invalid.messages.next_offset = Some(0),
            _ => unreachable!(),
        }
        deliver(&fixture, source, request, Ok(invalid));
        cx.run_until_parked();
        cx.read(|cx| {
            let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
            let items = messages.items.as_ref().unwrap();
            assert!(items.detail.is_none(), "must reject {corruption}");
            assert!(items.metadata.read(cx).value().is_empty());
            assert!(items.detail_error.as_deref().unwrap().contains("不一致"));
            assert!(messages.selected_fragment().is_none());
            assert!(messages.payload.raw.read(cx).value().is_empty());
        });
        click(&fixture, "ext-item-retry", cx);
        let retry = selected(&mut fixture, source);
        assert!(retry.0 > request.0);
        deliver(&fixture, source, request, Err("obsolete failure".into()));
        deliver(&fixture, source, retry, Ok(correct.clone()));
        cx.run_until_parked();
        cx.read(|cx| {
            let items = fixture
                .view
                .read(cx)
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .items
                .as_ref()
                .unwrap();
            assert_eq!(items.detail.as_ref().unwrap(), &correct.item);
            assert!(items.detail_error.is_none());
        });
    }
}
