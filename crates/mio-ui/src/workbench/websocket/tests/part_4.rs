use super::*;

#[gpui_kit::test]
fn websocket_payload_search_bar_handles_cmd_f_options_row_switch_raw_hex_and_ime(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let revision = open(&mut fixture, cx);
    let flow = Uuid::new_v4();
    let (s0, m0) = message(
        flow,
        0,
        b"Token=Alpha token=alpha TOKEN=ALPHA",
        WebSocketKind::Text,
    );
    let (s1, m1) = message(flow, 1, b"Token=Alpha only once", WebSocketKind::Text);
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            project: fixture.project,
            query_revision: revision,
            flow_id: None,
            filter: WebSocketHistoryFilter::default(),
            selected_match: None,
            offset: 0,
            total: 2,
            rows: vec![s0, s1],
        })
        .unwrap();
    cx.run_until_parked();

    click_row(&fixture, 0, cx);
    let (sel0, id0) = read(&mut fixture);
    assert_eq!(id0, m0.id);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: sel0,
            record: Box::new(m0),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();

    let (raw, search) = cx.read(|cx| {
        let payload = &fixture.view.read(cx).websocket.payload;
        (payload.raw.clone(), payload.search.clone())
    });
    let win = fixture.window;
    let type_search = |text: &str, cx: &mut TestAppContext| {
        cx.update_window(win, |_, window, cx| {
            let raw_rev = raw.read(cx).document_revision();
            window.click(("input", search.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            assert!(search.focus_handle(cx).is_focused(window));
            assert_eq!(
                search.read(cx).selected_range(),
                0..search.read(cx).value().len()
            );
            window.input(text, cx);
            assert_eq!(search.read(cx).value().as_ref(), text);
            assert_eq!(raw.read(cx).document_revision(), raw_rev);
        })
        .unwrap();
        cx.run_until_parked();
    };
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("input", raw.entity_id()), cx);
        raw.update(cx, |state, cx| state.set_selected_range(6..11, cx));
        window.press(crate::kbd!("f"), cx);
        assert!(search.focus_handle(cx).is_focused(window));
        assert!(!raw.read(cx).search_session().open);
        assert_eq!(search.read(cx).value().as_ref(), "Alpha");
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_status_label(cx),
            "1/3 个匹配"
        );

        window.click("websocket-payload-search-next", cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_status_label(cx),
            "2/3 个匹配"
        );
        assert!(window.try_find("websocket-payload-search-case").is_none());
        assert!(window.try_find("websocket-payload-search-regex").is_none());
        assert!(window.try_find("websocket-payload-search-replace-toggle").is_none());
        window.click("websocket-payload-search-options", cx);
        window.render_frame(cx);
        assert!(window.try_find("websocket-payload-search-case").is_some());
        assert!(window.try_find("websocket-payload-search-regex").is_some());
        assert!(window.try_find("websocket-payload-search-replace-toggle").is_none());
        window.click("websocket-payload-search-case", cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_status_label(cx),
            "1/1 个匹配"
        );
        window.click("websocket-payload-search-case", cx);
    })
    .unwrap();
    type_search(r"token=\w+", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_match_count(cx),
            0
        );
        assert_eq!(
            window.find("websocket-payload-search-matches").label(),
            Some("0 个匹配")
        );
        window.click("websocket-payload-search-options", cx);
        window.render_frame(cx);
        window.click("websocket-payload-search-regex", cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_status_label(cx),
            "1/3 个匹配"
        );
        window.click("websocket-payload-search-regex", cx);
    })
    .unwrap();
    type_search("Alpha", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_status_label(cx),
            "1/3 个匹配"
        );
        assert_eq!(
            window.find("websocket-payload-search-matches").label(),
            Some("1/3 个匹配")
        );
    })
    .unwrap();

    // Switch row recomputes matches for the newly loaded payload.
    click_row(&fixture, 1, cx);
    let (sel1, id1) = read(&mut fixture);
    assert_eq!(id1, m1.id);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: sel1,
            record: Box::new(m1),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_status_label(cx),
            "1/1 个匹配"
        );
        assert_eq!(
            window.find("websocket-payload-search-matches").label(),
            Some("1/1 个匹配")
        );
    })
    .unwrap();
    type_search("41 6c 70 68 61", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_match_count(cx),
            0
        );
        assert_eq!(
            window.find("websocket-payload-search-matches").label(),
            Some("0 个匹配")
        );
        window.click("websocket-payload-hex", cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .payload
                .search_status_label(cx),
            "1/1 个匹配"
        );
        assert_eq!(
            window.find("websocket-payload-search-matches").label(),
            Some("1/1 个匹配")
        );
        window.click("websocket-payload-raw", cx);

        fixture.view.update(cx, |view, _| view.error = None);
        search.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        });
        window.click("websocket-payload-search-next", cx);
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn websocket_edited_marker_and_original_final_toggle_without_losing_bytes(cx: &mut TestAppContext) {
    for (kind, original_payload, final_payload) in [
        (
            WebSocketKind::Binary,
            b"\x00\xfforiginal\x80".to_vec(),
            b"\x00\xfefinal\x81".to_vec(),
        ),
        (
            WebSocketKind::Text,
            "原始\r\n中文🙂\0".as_bytes().to_vec(),
            "最终\r\n中文🙂\0".as_bytes().to_vec(),
        ),
    ] {
        let mut fixture = fixture(cx, vec![], 64);
        let revision = open(&mut fixture, cx);
        let (mut edited, edited_record) = message(Uuid::new_v4(), 1, &final_payload, kind);
        edited.edited = true;
        let (plain, plain_record) = message(Uuid::new_v4(), 2, b"plain", WebSocketKind::Text);
        fixture
            .events
            .try_send(Event::WebSocketHistoryPage {
                selected_match: None,
                filter: WebSocketHistoryFilter::default(),
                project: fixture.project,
                query_revision: revision,
                flow_id: None,
                offset: 0,
                total: 2,
                rows: vec![edited.clone(), plain.clone()],
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("table-cell-0:3").label(), Some("✓"));
            assert_eq!(window.find("table-cell-1:3").label(), Some(""));
            let delegate = fixture.view.read(cx).websocket.table.read(cx).delegate();
            assert!(
                delegate
                    .rows
                    .iter()
                    .any(|row| row.id == edited.id && row.edited)
            );
            assert!(
                delegate
                    .rows
                    .iter()
                    .any(|row| row.id == plain.id && !row.edited)
            );
        })
        .unwrap();
        click_row(&fixture, 0, cx);
        let selected = read(&mut fixture);
        assert_eq!(selected.1, edited.id);
        fixture
            .events
            .try_send(Event::WebSocketDetail {
                project: fixture.project,
                selection_revision: selected.0,
                record: Box::new(edited_record.clone()),
                original_payload: Some(original_payload.clone()),
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            let assert_version =
                |original: bool, expected: &[u8], window: &mut Window, cx: &mut App| {
                    window.render_frame(cx);
                    fixture.view.update(cx, |view, cx| {
                        let state = &view.websocket;
                        assert_eq!(state.original_view, original);
                        assert_eq!(state.payload.bytes(window, cx).unwrap(), expected);
                        assert_eq!(
                            state.payload.view(),
                            if kind == WebSocketKind::Binary {
                                MessageView::Hex
                            } else {
                                MessageView::Raw
                            }
                        );
                        assert_eq!(state.detail.as_deref(), Some(&edited_record));
                        assert_eq!(
                            state.original_payload.as_deref(),
                            Some(original_payload.as_slice())
                        );
                    });
                };
            assert_version(false, &final_payload, window, cx);
            window.click("websocket-original", cx);
            assert_version(true, &original_payload, window, cx);
            window.click("websocket-final", cx);
            assert_version(false, &final_payload, window, cx);
            window.click("websocket-original", cx);
            assert_version(true, &original_payload, window, cx);
        })
        .unwrap();
        assert!(drain(&mut fixture.commands).is_empty());

        click_row(&fixture, 1, cx);
        let plain_selected = read(&mut fixture);
        assert_eq!(plain_selected.1, plain.id);
        fixture
            .events
            .try_send(Event::WebSocketDetail {
                project: fixture.project,
                selection_revision: plain_selected.0,
                record: Box::new(plain_record.clone()),
                original_payload: None,
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            fixture.view.update(cx, |view, cx| {
                let state = &view.websocket;
                assert!(!state.original_view);
                assert!(state.original_payload.is_none());
                assert_eq!(state.detail.as_deref(), Some(&plain_record));
                assert_eq!(state.payload.view(), MessageView::Raw);
                assert_eq!(
                    state.payload.bytes(window, cx).unwrap(),
                    plain_record.payload
                );
            });
            assert!(window.try_find("websocket-original").is_none());
            assert!(window.try_find("websocket-final").is_none());
        })
        .unwrap();
    }
}
