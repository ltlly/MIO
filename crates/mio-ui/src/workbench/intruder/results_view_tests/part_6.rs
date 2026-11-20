use super::*;

#[gpui_kit::test]
fn detached_intruder_results_window_search_bar_handles_cmd_f_options_row_switch_raw_hex_and_ime(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let mut first = record(
        "http://127.0.0.1:1/result/0",
        "token=Alpha token=alpha TOKEN=ALPHA",
    );
    first.summary.source = TrafficSource::Intruder;
    first.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\nX-Result: Omega\r\n\r\nomega OMEGA",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    let mut second = record("http://127.0.0.1:1/result/1", "token=Alpha only once");
    second.summary.source = TrafficSource::Intruder;
    second.response =
        Some(parse_response(b"HTTP/1.1 200 OK\r\n\r\nno omega", HttpVersion::Http11).unwrap());
    bootstrap(
        &mut fixture,
        &mut started,
        &[first.clone(), second.clone()],
        cx,
    );
    let sel0 = select(&mut fixture, &started, 0, &first, cx);
    detail(&fixture, sel0, &first, cx);

    let (req_raw, req_search, resp_raw, resp_search) = cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        (
            result.request.raw.clone(),
            result.request.search.clone(),
            result.response.raw.clone(),
            result.response.search.clone(),
        )
    });
    let type_search = |search: &Entity<InputState>,
                       raw: &Entity<gpui_kit::component::input::EditorState>,
                       text: &str,
                       cx: &mut TestAppContext| {
        cx.update_window(started.window, |_, window, cx| {
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

    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("input", req_raw.entity_id()), cx);
        let text = req_raw.read(cx).value().to_string();
        let start = text.find("Alpha").unwrap();
        req_raw.update(cx, |state, cx| {
            state.set_selected_range(start..start + 5, cx);
        });
        window.press(crate::kbd!("f"), cx);
        assert!(req_search.focus_handle(cx).is_focused(window));
        assert!(!req_raw.read(cx).search_session().open);
        assert_eq!(req_search.read(cx).value().as_ref(), "Alpha");
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "1/3 个匹配"
        );

        window.click("intruder-result-request-search-next", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "2/3 个匹配"
        );
        window.click("intruder-result-request-search-prev", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "1/3 个匹配"
        );
        window.click("intruder-result-request-search-case", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "1/1 个匹配"
        );
        window.click("intruder-result-request-search-case", cx);
    })
    .unwrap();
    type_search(&req_search, &req_raw, r"token=\w+", cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_match_count(cx),
            0
        );
        assert_eq!(
            window
                .find("intruder-result-request-search-matches")
                .label(),
            Some("0 个匹配")
        );
        window.click("intruder-result-request-search-regex", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "1/3 个匹配"
        );
        assert_eq!(
            window
                .find("intruder-result-request-search-matches")
                .label(),
            Some("1/3 个匹配")
        );
    })
    .unwrap();
    type_search(&req_search, &req_raw, "[unclosed", cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "正则无效"
        );
        assert_eq!(
            window
                .find("intruder-result-request-search-matches")
                .label(),
            Some("正则无效")
        );
        window.click("intruder-result-request-search-next", cx);
        assert!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail_error
                .as_deref()
                .is_some_and(|err| err.contains("搜索正则表达式无效"))
        );
        window.click("intruder-result-request-search-regex", cx);
    })
    .unwrap();

    // Raw/Hex switch recomputes matches on the active view.
    type_search(&req_search, &req_raw, "41 6c 70 68 61", cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_match_count(cx),
            0
        );
        assert_eq!(
            window
                .find("intruder-result-request-search-matches")
                .label(),
            Some("0 个匹配")
        );
        window.click("intruder-result-hex", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "1/1 个匹配"
        );
        assert_eq!(
            window
                .find("intruder-result-request-search-matches")
                .label(),
            Some("1/1 个匹配")
        );
        window.click("intruder-result-raw", cx);
    })
    .unwrap();
    type_search(&req_search, &req_raw, "Alpha", cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .search_status_label(cx),
            "1/3 个匹配"
        );
        assert_eq!(
            window
                .find("intruder-result-request-search-matches")
                .label(),
            Some("1/3 个匹配")
        );

        // Switch to Response tab and press Cmd+F.
        window.click("intruder-result-response", cx);
        window.click(("input", resp_raw.entity_id()), cx);
        window.press(crate::kbd!("f"), cx);
        assert!(resp_search.focus_handle(cx).is_focused(window));
        assert!(!resp_raw.read(cx).search_session().open);
    })
    .unwrap();
    type_search(&resp_search, &resp_raw, "omega", cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .response
                .search_status_label(cx),
            "1/3 个匹配"
        );
        assert_eq!(
            window
                .find("intruder-result-response-search-matches")
                .label(),
            Some("1/3 个匹配")
        );
    })
    .unwrap();

    // Switch result row and verify both request and response searches recompute.
    let sel1 = select(&mut fixture, &started, 1, &second, cx);
    detail(&fixture, sel1, &second, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.request.search_status_label(cx), "1/1 个匹配");
        assert_eq!(result.response.search_status_label(cx), "1/1 个匹配");
        assert_eq!(
            window
                .find("intruder-result-response-search-matches")
                .label(),
            Some("1/1 个匹配")
        );

        let resp_search = result.response.search.clone();
        resp_search.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        });
        window.click("intruder-result-response-search-next", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail_error
                .as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn intruder_results_window_renders_grep_summary_for_text_and_binary_and_hides_without_response(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let tab_id = cx.read(|cx| fixture.view.read(cx).workspace.intruder.active.unwrap());
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.workspace.intruder.grep_configs.insert(
                tab_id,
                mio_core::IntruderGrepConfig {
                    match_expressions: vec![" flag{ ".into()],
                    match_case_sensitive: false,
                    extract_rules: vec![mio_core::IntruderGrepExtractRule {
                        start_delimiter: "tok=".into(),
                        end_delimiter: ";".into(),
                    }],
                    grep_payloads: false,
                    grep_payloads_case_sensitive: false,
                },
            );
        });
    });
    let mut started = start(&mut fixture, cx);

    let mut first = record("http://127.0.0.1:1/result/0", "text-body");
    first.summary.source = TrafficSource::Intruder;
    first.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\n\r\nprefix FLAG{ hit tok=sec99; end",
            HttpVersion::Http11,
        )
        .unwrap(),
    );

    let mut second = record("http://127.0.0.1:1/result/1", "bin-body");
    second.summary.source = TrafficSource::Intruder;
    second.response = Some(HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: Vec::new(),
        body: b"\xff\xfetok=bin77;".to_vec(),
        trailers: Vec::new(),
    });

    let mut third = record("http://127.0.0.1:1/result/2", "no-resp");
    third.summary.source = TrafficSource::Intruder;
    third.summary.status = None;
    third.summary.response_bytes = 0;
    third.response = None;

    started.summary.total = 3;
    bootstrap(
        &mut fixture,
        &mut started,
        &[first.clone(), second.clone(), third.clone()],
        cx,
    );

    let sel0 = select(&mut fixture, &started, 0, &first, cx);
    detail(&fixture, sel0, &first, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let bar = window.find("intruder-result-grep-summary");
        assert!(bar.visible());
        assert_eq!(
            bar.label(),
            Some("Grep 命中:  flag{  | 提取: tok=..;: sec99")
        );
    })
    .unwrap();

    click(fixture.window, "intruder-sidebar-settings", cx);
    click(fixture.window, "intruder-grep-match-case", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("changing Grep configuration must only save the workspace");
    };
    assert_eq!(*project, fixture.project);
    assert!(workspace.intruder.grep_configs[&tab_id].match_case_sensitive);
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let result_view = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result_view.summary.as_ref().unwrap().tab_id, tab_id);
        assert_eq!(result_view.detail.as_deref(), Some(&first));
        assert_eq!(
            window.find("intruder-result-grep-summary").label(),
            Some("Grep 命中: 无 | 提取: tok=..;: sec99")
        );
    })
    .unwrap();

    let sel1 = select(&mut fixture, &started, 1, &second, cx);
    detail(&fixture, sel1, &second, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let result_view = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result_view.response.view, MessageView::Hex);
        let bar = window.find("intruder-result-grep-summary");
        assert!(bar.visible());
        assert_eq!(bar.label(), Some("Grep 命中: 无 | 提取: tok=..;: bin77"));
    })
    .unwrap();

    let sel2 = select(&mut fixture, &started, 2, &third, cx);
    detail(&fixture, sel2, &third, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("intruder-result-grep-summary").is_none());
    })
    .unwrap();

    let revision = select(&mut fixture, &started, 0, &first, cx);
    cx.update_window(started.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            assert!(view.on_intruder_result_event(
                &Event::FlowDetail {
                    project: fixture.project,
                    selection_revision: revision,
                    record: Box::new(first.clone()),
                    original: Box::default(),
                },
                window,
                cx,
            ));
            assert!(
                view.intruder.results.views[&started.summary.id]
                    .detail_grep_summary
                    .is_none(),
                "Grep must not compute synchronously while handling a UI event"
            );
            view.select_intruder_result(started.summary.id, 1, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::ReadFlow { flow_id, .. }] if *flow_id == second.summary.id)
    );
    cx.read(|cx| {
        let result_view = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result_view.selected, Some(1));
        assert!(
            result_view.detail_grep_summary.is_none(),
            "late background Grep must not restore the old selection's summary"
        );
    });
}
