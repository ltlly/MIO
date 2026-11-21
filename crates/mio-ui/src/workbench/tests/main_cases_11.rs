use super::*;

#[gpui_kit::test]
fn workbench_message_search_bars_handle_cmd_f_options_navigation_row_switch_raw_hex_and_ime(
    cx: &mut TestAppContext,
) {
    let mut first = tab("1");
    let repeater_req = "GET /items?token=Alpha HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nX-Token: alpha\r\n\r\nTOKEN=ALPHA";
    first.draft.raw = repeater_req.as_bytes().to_vec();
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let win = fixture.window;
    let type_search = |search: &Entity<InputState>,
                       raw: &Entity<gpui_kit::component::input::EditorState>,
                       text: &str,
                       cx: &mut TestAppContext| {
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
    let initial_history_rev = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .unwrap();

    // 1. Intercept panel: Cmd+F focuses bottom search bar, never opens built-in top overlay.
    let intercept_flow = record("http://127.0.0.1:9001/intercept", "Beta beta BETA");
    fixture
        .events
        .try_send(Event::InterceptPending {
            project: fixture.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(intercept_flow),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let (raw, search) = {
            let editor = &fixture.view.read(cx).intercepts[0].editor;
            (editor.raw.clone(), editor.search.clone())
        };
        window.click(("input", raw.entity_id()), cx);
        let text = raw.read(cx).value().to_string();
        let start = text.find("Beta").unwrap();
        raw.update(cx, |state, cx| {
            state.set_selected_range(start..start + 4, cx)
        });
        window.press(crate::kbd!("f"), cx);
        assert!(search.focus_handle(cx).is_focused(window));
        assert!(!raw.read(cx).search_session().open);
        assert_eq!(search.read(cx).value().as_ref(), "Beta");
        let editor = &fixture.view.read(cx).intercepts[0].editor;
        assert_eq!(editor.search_match_count(cx), 3);
        assert_eq!(editor.search_status_label(cx), "1/3 个匹配");
        window.render_frame(cx);
        assert_eq!(
            window.find("intercept-search-matches").label(),
            Some("1/3 个匹配")
        );
    })
    .unwrap();

    // 2. Proxy HTTP History panel: Cmd+F routes to focused request/response and recomputes on row switch.
    let mut flow_one = record("http://127.0.0.1:9001/one", "Gamma gamma GAMMA");
    flow_one.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\nX-Reply: Delta\r\n\r\ndelta DELTA",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    let mut flow_two = record("http://127.0.0.1:9001/two", "Gamma only one");
    flow_two.response =
        Some(parse_response(b"HTTP/1.1 200 OK\r\n\r\nno match", HttpVersion::Http11).unwrap());
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: initial_history_rev,
            offset: 0,
            total: 2,
            rows: vec![
                flow_one.summary.clone().into(),
                flow_two.summary.clone().into(),
            ],
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).history.read(cx).delegate().columns.len(),
            18
        );
        window.click("table-cell-0:0", cx);
    })
    .unwrap();
    let read_one = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } if flow_id == flow_one.summary.id => Some(selection_revision),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: read_one,
            record: Box::new(flow_one.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    let (req_raw, req_search, resp_raw, resp_search) = cx
        .update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let (req_raw, req_search, resp_raw, resp_search) = {
                let view = fixture.view.read(cx);
                (
                    view.history_request.raw.clone(),
                    view.history_request.search.clone(),
                    view.history_response.raw.clone(),
                    view.history_response.search.clone(),
                )
            };
            window.click(("input", req_raw.entity_id()), cx);
            window.press(crate::kbd!("f"), cx);
            assert!(req_search.focus_handle(cx).is_focused(window));
            assert!(!req_raw.read(cx).search_session().open);
            (req_raw, req_search, resp_raw, resp_search)
        })
        .unwrap();
    type_search(&req_search, &req_raw, "gamma", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .history_request
                .search_status_label(cx),
            "1/3 个匹配"
        );
        assert_eq!(
            window.find("history-request-search-matches").label(),
            Some("1/3 个匹配")
        );

        window.click(("input", resp_raw.entity_id()), cx);
        window.press(crate::kbd!("f"), cx);
        assert!(resp_search.focus_handle(cx).is_focused(window));
        assert!(!resp_raw.read(cx).search_session().open);
    })
    .unwrap();
    type_search(&resp_search, &resp_raw, "delta", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .history_response
                .search_status_label(cx),
            "1/3 个匹配"
        );
        assert_eq!(
            window.find("history-response-search-matches").label(),
            Some("1/3 个匹配")
        );

        // Switch row and verify matches recompute for the new flow detail.
        window.click("table-cell-1:0", cx);
    })
    .unwrap();
    let read_two = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } if flow_id == flow_two.summary.id => Some(selection_revision),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: read_two,
            record: Box::new(flow_two),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.history_request.search_status_label(cx), "1/1 个匹配");
        assert_eq!(view.history_response.search_status_label(cx), "0 个匹配");
        assert_eq!(
            window.find("history-request-search-matches").label(),
            Some("1/1 个匹配")
        );
        assert_eq!(
            window.find("history-response-search-matches").label(),
            Some("0 个匹配")
        );
    })
    .unwrap();

    // 3. Repeater panel: Cmd+F, Aa, .*, invalid regex error, ‹/› navigation, Raw/Hex switch, IME guard.
    let (req_raw, req_search, resp_raw, resp_search) = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("tool-repeater", cx);
            window.render_frame(cx);
            let (req_raw, req_search, resp_raw, resp_search) = {
                let editors = &fixture.view.read(cx).tab_editors[&id];
                (
                    editors.request.raw.clone(),
                    editors.request.search.clone(),
                    editors.response.raw.clone(),
                    editors.response.search.clone(),
                )
            };
            resp_raw.update(cx, |state, cx| {
                state.set_value("HTTP/1.1 200 OK\r\n\r\nresp-body", window, cx);
            });
            window.click(("input", req_raw.entity_id()), cx);
            let start = repeater_req.find("Alpha").unwrap();
            req_raw.update(cx, |state, cx| {
                state.set_selected_range(start..start + 5, cx);
            });
            window.press(crate::kbd!("f"), cx);
            assert!(req_search.focus_handle(cx).is_focused(window));
            assert!(!req_raw.read(cx).search_session().open);
            assert_eq!(req_search.read(cx).value().as_ref(), "Alpha");
            assert_eq!(
                fixture.view.read(cx).tab_editors[&id]
                    .request
                    .search_status_label(cx),
                "1/3 个匹配"
            );

            window.click("repeater-request-search-next", cx);
            assert_eq!(
                fixture.view.read(cx).tab_editors[&id]
                    .request
                    .search_status_label(cx),
                "2/3 个匹配"
            );
            window.click("repeater-request-search-prev", cx);
            assert_eq!(
                fixture.view.read(cx).tab_editors[&id]
                    .request
                    .search_status_label(cx),
                "1/3 个匹配"
            );

            assert!(window.try_find("repeater-request-search-case").is_none());
            assert!(window.try_find("repeater-request-search-regex").is_none());
            assert!(window.try_find("repeater-request-search-replace-toggle").is_none());
            window.click("repeater-request-search-options", cx);
            window.render_frame(cx);
            assert!(window.try_find("repeater-request-search-case").is_some());
            assert!(window.try_find("repeater-request-search-regex").is_some());
            assert!(window.try_find("repeater-request-search-replace-toggle").is_some());
            window.click("repeater-request-search-case", cx);
            assert!(
                fixture.view.read(cx).tab_editors[&id]
                    .request
                    .case_sensitive()
            );
            assert_eq!(
                fixture.view.read(cx).tab_editors[&id]
                    .request
                    .search_status_label(cx),
                "1/1 个匹配"
            );
            window.click("repeater-request-search-case", cx);
            assert!(
                !fixture.view.read(cx).tab_editors[&id]
                    .request
                    .case_sensitive()
            );
            (req_raw, req_search, resp_raw, resp_search)
        })
        .unwrap();

    type_search(&req_search, &req_raw, r"token=\w+", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).tab_editors[&id]
                .request
                .search_status_label(cx),
            "0 个匹配"
        );
        assert_eq!(
            window.find("repeater-request-search-matches").label(),
            Some("0 个匹配")
        );
        if window.try_find("repeater-request-search-regex").is_none() {
            window.click("repeater-request-search-options", cx);
            window.render_frame(cx);
        }
        window.click("repeater-request-search-regex", cx);
        assert!(fixture.view.read(cx).tab_editors[&id].request.regex());
        assert_eq!(
            fixture.view.read(cx).tab_editors[&id]
                .request
                .search_status_label(cx),
            "1/2 个匹配"
        );
        assert_eq!(
            window.find("repeater-request-search-matches").label(),
            Some("1/2 个匹配")
        );
    })
    .unwrap();

    type_search(&req_search, &req_raw, "[unclosed", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).tab_editors[&id]
                .request
                .search_status_label(cx),
            "正则无效"
        );
        assert_eq!(
            window.find("repeater-request-search-matches").label(),
            Some("正则无效")
        );
        window.click("repeater-request-search-next", cx);
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .is_some_and(|err| err.contains("搜索正则表达式无效"))
        );
        if window.try_find("repeater-request-search-regex").is_none() {
            window.click("repeater-request-search-options", cx);
            window.render_frame(cx);
        }
        window.click("repeater-request-search-regex", cx);
        assert!(!fixture.view.read(cx).tab_editors[&id].request.regex());
    })
    .unwrap();

    // Switch Raw/Hex recomputes matches on the active view.
    type_search(&req_search, &req_raw, "41 6c 70 68 61", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).tab_editors[&id]
                .request
                .search_match_count(cx),
            0
        );
        assert_eq!(
            window.find("repeater-request-search-matches").label(),
            Some("0 个匹配")
        );
        window.click("repeater-request-hex", cx);
        assert_eq!(
            fixture.view.read(cx).tab_editors[&id]
                .request
                .search_status_label(cx),
            "1/1 个匹配"
        );
        assert_eq!(
            window.find("repeater-request-search-matches").label(),
            Some("1/1 个匹配")
        );
        window.click("repeater-request-raw", cx);
        assert_eq!(
            fixture.view.read(cx).tab_editors[&id]
                .request
                .search_match_count(cx),
            0
        );

        // Response editor Cmd+F focuses response search bar.
        window.click(("input", resp_raw.entity_id()), cx);
        window.press(crate::kbd!("f"), cx);
        assert!(resp_search.focus_handle(cx).is_focused(window));

        // IME composition guard on search input blocks button actions and Cmd+F.
        fixture.view.update(cx, |view, _| view.error = None);
        if window.try_find("repeater-request-search-case").is_none() {
            window.click("repeater-request-search-options", cx);
            window.render_frame(cx);
        }
        req_search.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        });
        window.click("repeater-request-search-case", cx);
        assert!(
            !fixture.view.read(cx).tab_editors[&id]
                .request
                .case_sensitive()
        );
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
        fixture.view.update(cx, |view, _| view.error = None);
        window.click(("input", req_raw.entity_id()), cx);
        window.press(crate::kbd!("f"), cx);
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
    })
    .unwrap();
}
