use super::*;

const INCOMPLETE_ORIGINAL: &str = "原始响应正文未完整接收（仅显示已观察内容）";

fn visible_text(window: &Window, expected: &str) -> bool {
    gpui_kit::base::test_support::texts(window)
        .iter()
        .any(|text| text.visible() && !text.is_alternative() && text.text() == expected)
}

fn query(fixture: &mut Fixture) -> u64 {
    drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .expect("history query must originate from the UI")
}

fn read(fixture: &mut Fixture) -> (u64, FlowId) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            selection_revision,
            flow_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one current history detail query is required")
    };
    (*selection_revision, *flow_id)
}

fn select_version(
    fixture: &Fixture,
    menu_id: &'static str,
    label: &'static str,
    cx: &mut TestAppContext,
) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(menu_id, cx);
        window.render_frame(cx);
        let position = window.find_by_label(label).bounds().center();
        window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn refresh_history(window: &mut Window, cx: &mut App) {
    window.render_frame(cx);
    window.click("history-filter", cx);
}

fn show(fixture: &mut Fixture, record: &FlowRecord, cx: &mut TestAppContext) -> u64 {
    let revision = query(fixture);
    page(fixture, revision, &record.summary, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.render_frame(cx);
        window.click("table-cell-0:0", cx);
    })
    .unwrap();
    let (selection_revision, id) = read(fixture);
    assert_eq!(id, record.summary.id);
    selection_revision
}

fn page(fixture: &Fixture, revision: u64, summary: &FlowSummary, cx: &mut TestAppContext) {
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: revision,
            offset: 0,
            total: 1,
            rows: vec![summary.clone().into()],
        })
        .unwrap();
    cx.run_until_parked();
}

fn detail(
    fixture: &Fixture,
    revision: u64,
    record: &FlowRecord,
    complete: Option<bool>,
    cx: &mut TestAppContext,
) {
    let mut original = record.response.clone().expect("response fixture");
    original.status = 200;
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(record.clone()),
            original: Box::new(OriginalMessages {
                response: Some(original),
                response_body_complete: complete,
                ..OriginalMessages::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

fn response(status: u16, state: FlowState, body: &str) -> FlowRecord {
    let mut record = record("http://127.0.0.1:9001/events", "");
    record.summary.state = state;
    record.summary.status = Some(status);
    record.summary.response_bytes = body.len() as u64;
    record.response = Some(
        parse_response(
            format!("HTTP/1.1 {status} Test\r\nContent-Type: text/event-stream\r\n\r\n{body}")
                .as_bytes(),
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    record
}

#[gpui_kit::test]
fn history_terminal_revision_refreshes_selected_body_and_rejects_late_detail(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let first = response(201, FlowState::Pending, "");
    let initial = show(&mut fixture, &first, cx);
    detail(&fixture, initial, &first, Some(false), cx);
    select_version(&fixture, "history-response-version", "原始响应", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let editor = fixture.view.read(cx).history_response.raw.clone();
        window.click(("input", editor.entity_id()), cx);
        assert!(editor.focus_handle(cx).is_focused(window));
    })
    .unwrap();

    let mut final_record = first.clone();
    final_record.summary.revision += 1;
    final_record.summary.state = FlowState::Cancelled;
    final_record.response.as_mut().unwrap().body = b"data: observed\n\n".to_vec();
    final_record.summary.response_bytes = final_record.response.as_ref().unwrap().body.len() as u64;
    fixture
        .events
        .try_send(Event::HistoryChanged {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    let refresh = query(&mut fixture);
    page(&fixture, refresh, &final_record.summary, cx);
    let latest = read(&mut fixture);
    assert_eq!(latest.1, first.summary.id);
    assert!(latest.0 > initial);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(view.detail.is_none());
        assert!(view.history_original_view);
        assert!(view.focus.is_focused(window));
        assert!(window.is_action_available(&Quit, cx));
    })
    .unwrap();

    detail(&fixture, initial, &first, Some(true), cx);
    cx.read(|cx| assert!(fixture.view.read(cx).detail.is_none()));
    detail(&fixture, latest.0, &final_record, Some(false), cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.history_original_view);
        assert_eq!(view.detail.as_deref(), Some(&final_record));
        let editor = view.history_response.raw.clone();
        assert!(editor.read(cx).value().starts_with("HTTP/1.1 200"));
        assert!(editor.read(cx).value().ends_with("data: observed\n\n"));
        assert!(visible_text(
            window,
            &format!("已取消 · {INCOMPLETE_ORIGINAL}")
        ));
    })
    .unwrap();
    select_version(&fixture, "history-response-version", "最终响应", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let editor = fixture.view.read(cx).history_response.raw.clone();
        assert!(editor.read(cx).value().starts_with("HTTP/1.1 201"));
        assert!(!visible_text(
            window,
            &format!("已取消 · {INCOMPLETE_ORIGINAL}")
        ));
    })
    .unwrap();
    select_version(&fixture, "history-response-version", "原始响应", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let editor = fixture.view.read(cx).history_response.raw.clone();
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("不能修改历史响应", cx);
        assert!(editor.read(cx).value().ends_with("data: observed\n\n"));
    })
    .unwrap();

    page(&fixture, refresh, &final_record.summary, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: ProjectKey {
                generation: fixture.project.generation + 1,
                ..fixture.project
            },
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    detail(&fixture, latest.0, &final_record, Some(false), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.detail.is_none());
        assert!(!view.history_original_view);
        assert_eq!(view.detail_original.response_body_complete, None);
    });
}

#[gpui_kit::test]
fn history_original_stream_completeness_does_not_follow_final_status(cx: &mut TestAppContext) {
    for (method, status, state, complete, body) in [
        ("GET", 204, FlowState::Complete, Some(false), ""),
        ("GET", 304, FlowState::Complete, Some(false), ""),
        (
            "GET",
            201,
            FlowState::Cancelled,
            Some(false),
            "data: partial\n\n",
        ),
        (
            "GET",
            201,
            FlowState::Complete,
            Some(true),
            "data: complete\n\n",
        ),
        ("HEAD", 201, FlowState::Complete, None, ""),
    ] {
        let mut fixture = fixture(cx, vec![], 64);
        let mut record = response(status, state, body);
        record.request.method = method.into();
        record.summary.method = method.into();
        let revision = show(&mut fixture, &record, cx);
        detail(&fixture, revision, &record, complete, cx);
        select_version(&fixture, "history-response-version", "原始响应", cx);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let status = format!("{} · {INCOMPLETE_ORIGINAL}", record.summary.state.label());
            assert_eq!(
                visible_text(window, &status),
                complete == Some(false),
                "{method} / {complete:?}: final state cannot prove the original body is complete"
            );
            let view = fixture.view.read(cx);
            assert_eq!(view.detail_original.response.as_ref().unwrap().status, 200);
            assert_eq!(
                view.detail_original.response.as_ref().unwrap().body,
                body.as_bytes()
            );
        })
        .unwrap();
        select_version(&fixture, "history-response-version", "最终响应", cx);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let status = format!("{} · {INCOMPLETE_ORIGINAL}", record.summary.state.label());
            assert!(!visible_text(window, &status));
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn history_read_ahead_is_valid_but_stale_detail_has_a_real_retry(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let mut record = response(201, FlowState::Pending, "");
    let revision = show(&mut fixture, &record, cx);
    record.summary.revision += 1;
    detail(&fixture, revision, &record, Some(false), cx);
    cx.read(|cx| assert_eq!(fixture.view.read(cx).detail.as_deref(), Some(&record)));

    cx.update_window(fixture.window, |_, window, cx| {
        refresh_history(window, cx);
    })
    .unwrap();
    let refresh = query(&mut fixture);
    page(&fixture, refresh, &record.summary, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let mut next = record.clone();
    next.summary.revision += 1;
    page(&fixture, refresh, &next.summary, cx);
    let selected = read(&mut fixture);
    detail(&fixture, selected.0, &record, Some(true), cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.detail.is_none());
        assert!(view.detail_error.as_ref().unwrap().contains("版本"));
        window.click("history-retry", cx);
    })
    .unwrap();
    let retry = read(&mut fixture);
    assert_eq!(retry.1, record.summary.id);
    assert!(retry.0 > selected.0);
    detail(&fixture, retry.0, &next, Some(false), cx);
    cx.read(|cx| assert_eq!(fixture.view.read(cx).detail.as_deref(), Some(&next)));
}

#[gpui_kit::test]
fn history_revision_refresh_queue_failure_keeps_retry_and_focus(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 1);
    let mut record = response(201, FlowState::Pending, "");
    let selection = show(&mut fixture, &record, cx);
    detail(&fixture, selection, &record, Some(false), cx);
    select_version(&fixture, "history-response-version", "原始响应", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        refresh_history(window, cx);
        let raw = fixture.view.read(cx).history_response.raw.clone();
        window.click(("input", raw.entity_id()), cx);
        assert!(raw.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    let revision = cx.read(|cx| fixture.view.read(cx).query_revision);
    record.summary.revision += 1;
    page(&fixture, revision, &record.summary, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(view.detail.is_none());
        assert!(view.detail_error.as_ref().unwrap().contains("队列已满"));
        assert_eq!(view.selected_flow, Some(record.summary.id));
        assert!(view.history_original_view);
        assert!(view.focus.is_focused(window));
        assert!(window.is_action_available(&Quit, cx));
    })
    .unwrap();
    assert_eq!(query(&mut fixture), revision);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-retry", cx);
    })
    .unwrap();
    let retry = read(&mut fixture);
    detail(&fixture, retry.0, &record, Some(false), cx);
    cx.read(|cx| assert!(fixture.view.read(cx).history_original_view));
}

#[gpui_kit::test]
fn history_disconnect_resolves_loading_without_discarding_loaded_payload(cx: &mut TestAppContext) {
    for stage in 0..3 {
        let mut fixture = fixture(cx, vec![], 64);
        let record = response(201, FlowState::Complete, "data: keep\n\n");
        if stage == 0 {
            query(&mut fixture);
            cx.update_window(fixture.window, |_, window, cx| {
                window.click("proxy-history", cx)
            })
            .unwrap();
        } else {
            let revision = show(&mut fixture, &record, cx);
            if stage == 2 {
                detail(&fixture, revision, &record, Some(true), cx);
                cx.update_window(fixture.window, |_, window, cx| {
                    window.render_frame(cx);
                    let raw = fixture.view.read(cx).history_response.raw.clone();
                    window.click(("input", raw.entity_id()), cx);
                })
                .unwrap();
            }
        }
        fixture.commands.close();
        let (replacement, _) = mpsc::channel(1);
        drop(std::mem::replace(&mut fixture.events, replacement));
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert!(view.backend_disconnected);
            assert!(!view.history_loading);
            assert!(!view.history_dirty);
            assert!(!view.history.read(cx).delegate().loading);
            if stage == 1 {
                assert!(view.detail.is_none());
                assert!(view.detail_error.as_ref().unwrap().contains("断开"));
                assert!(window.try_find_by_label("重试读取").is_some());
            } else if stage == 2 {
                assert_eq!(view.detail.as_deref(), Some(&record));
                assert!(view.detail_error.is_none());
                assert!(
                    view.history_response
                        .raw
                        .read(cx)
                        .value()
                        .ends_with("data: keep\n\n")
                );
                assert!(
                    view.history_response
                        .raw
                        .focus_handle(cx)
                        .is_focused(window)
                );
            }
            assert!(window.is_action_available(&Quit, cx));
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn history_version_dropdown_replaces_title_and_switches_request_and_response_independently(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let mut final_rec = response(201, FlowState::Complete, "final-resp-body");
    final_rec.request.body = b"final-req-body".to_vec();
    final_rec.summary.request_bytes = final_rec.request.body.len() as u64;

    let mut orig_req = final_rec.request.clone();
    orig_req.body = b"orig-req-body".to_vec();
    let mut orig_resp = final_rec.response.clone().unwrap();
    orig_resp.status = 200;
    orig_resp.body = b"orig-resp-body".to_vec();

    let revision = show(&mut fixture, &final_rec, cx);
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(final_rec.clone()),
            original: Box::new(OriginalMessages {
                request: Some(orig_req),
                response: Some(orig_resp),
                response_body_complete: Some(true),
                ..OriginalMessages::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();

    // Non-combined default (SideBySide): version dropdown replaces static Request/Response title
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let req_btn = window.find("history-request-version");
        let resp_btn = window.find("history-response-version");
        assert!(req_btn.visible());
        assert!(resp_btn.visible());
        assert_eq!(req_btn.label(), Some("最终请求"));
        assert_eq!(resp_btn.label(), Some("最终响应"));
        assert!((f32::from(req_btn.bounds().size.height) - 23.0).abs() <= 1.0);
        let view = fixture.view.read(cx);
        assert!(view.history_request.raw.read(cx).value().ends_with("final-req-body"));
        assert!(view.history_response.raw.read(cx).value().ends_with("final-resp-body"));
    })
    .unwrap();

    // Switch ONLY request to Original -> response must stay Final
    select_version(&fixture, "history-request-version", "原始请求", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.history_request_original_view);
        assert!(!view.history_response_original_view);
        assert!(view.history_original_view);
        assert!(view.history_request.raw.read(cx).value().ends_with("orig-req-body"));
        assert!(view.history_response.raw.read(cx).value().ends_with("final-resp-body"));
        assert_eq!(window.find("history-request-version").label(), Some("原始请求"));
        assert_eq!(window.find("history-response-version").label(), Some("最终响应"));
    })
    .unwrap();

    // Switch response to Original -> request stays Original; then switch request back to Final -> response stays Original
    select_version(&fixture, "history-response-version", "原始响应", cx);
    select_version(&fixture, "history-request-version", "最终请求", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(!view.history_request_original_view);
        assert!(view.history_response_original_view);
        assert!(view.history_original_view);
        assert!(view.history_request.raw.read(cx).value().ends_with("final-req-body"));
        assert!(view.history_response.raw.read(cx).value().ends_with("orig-resp-body"));
    })
    .unwrap();

    // Verify TopBottom and Combined modes keep version dropdown visible with active pane
    for mode in [
        mio_core::RepeaterLayoutMode::TopBottom,
        mio_core::RepeaterLayoutMode::Combined,
        mio_core::RepeaterLayoutMode::SideBySide,
    ] {
        cx.update_window(fixture.window, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_message_pair_layout_for_owner("history", mode);
                cx.notify();
            });
            window.render_frame(cx);
            assert!(window.find("history-request-version").visible());
        })
        .unwrap();
    }
}
