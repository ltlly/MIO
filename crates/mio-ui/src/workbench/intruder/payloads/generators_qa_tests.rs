use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Bounds, ElementInputHandler, InputHandler, TestAppContext};

use super::super::results_view_tests::{bootstrap, click, detail, select, start};
use super::super::tests::{attack, intruder_fixture};
use super::ensure_payload_encode_input;
use crate::workbench::tests::{drain, record};
use mio_core::intruder::{AttackType, Position};
use mio_core::{
    Command, FlowState, HttpVersion, IntruderPayloadPreview, TrafficSource, parse_response,
};

#[gpui_kit::test]
fn intruder_null_payloads_copy_other_set_selected_char_encoding_echo_and_tsv_export_work(
    cx: &mut TestAppContext,
) {
    let mut tab = attack();
    tab.spec.draft.raw = b"POST /qa?p1=a&p2=b HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n".to_vec();
    tab.spec.attack_type = AttackType::ClusterBomb;
    tab.spec.positions = vec![
        Position { start: 12, end: 13 },
        Position { start: 17, end: 18 },
    ];
    tab.spec.payload_sets = vec![vec![b"k=1&x=2".to_vec()], Vec::new()];
    let id = tab.id;
    let mut fixture = intruder_fixture(cx, vec![tab], 64);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.scroll(
            "intruder-payloads-panel",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-620.))),
            cx,
        );
        window.render_frame(cx);
        assert!(window.try_find("intruder-copy-first-set").is_none());

        fixture.view.update(cx, |view, cx| {
            view.intruder.editors.get_mut(&id).unwrap().payload_position = Some(1);
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.find("intruder-copy-first-set").visible());
        window.click("intruder-copy-first-set", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[1],
            vec![b"k=1&x=2".to_vec()]
        );

        let encode_input = ensure_payload_encode_input(id, window, cx);
        let mut handler = ElementInputHandler::new(Bounds::default(), encode_input.clone());
        handler.replace_and_mark_text_in_range(None, "zhong", None, window, cx);
        window.click("intruder-payload-encode-selected", cx);
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(crate::editing::COMPOSITION_PENDING)
        );
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[1],
            vec![b"k=1&x=2".to_vec()]
        );
        encode_input.update(cx, |state, cx| state.set_value("=&", window, cx));
        window.click("intruder-payload-encode-selected", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[1],
            vec![b"k%3D1%26x%3D2".to_vec()]
        );

        window.click("intruder-payload-clear", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .payload_input
            .clone();
        input.update(cx, |state, cx| state.set_value("3", window, cx));
        window.scroll(
            "intruder-payloads-panel",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-240.))),
            cx,
        );
        window.render_frame(cx);
        window.click("intruder-null-payloads", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[1],
            vec![Vec::<u8>::new(); 3]
        );
    })
    .unwrap();
    cx.run_until_parked();
    while let Some(Command::SaveWorkspace { project, workspace }) =
        drain(&mut fixture.commands).into_iter().last()
    {
        fixture
            .events
            .try_send(mio_core::Event::WorkspaceChanged { project, workspace })
            .unwrap();
        cx.run_until_parked();
    }
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.intruder.editors.get_mut(&id).unwrap().dirty = true;
        });
    });

    let mut started = start(&mut fixture, cx);
    let mut echoed_flow = record("http://127.0.0.1:1/qa/0", "req-0");
    echoed_flow.summary.source = TrafficSource::Intruder;
    echoed_flow.summary.status = Some(200);
    echoed_flow.summary.state = FlowState::Complete;
    echoed_flow.summary.response_bytes = 38;
    echoed_flow.summary.elapsed_ms = 17;
    echoed_flow.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\n\r\nmirror=k=1&x=2 done",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    started.summary.total = 1;
    bootstrap(&mut fixture, &mut started, &[echoed_flow.clone()], cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            let page = view
                .intruder
                .results
                .views
                .get_mut(&started.summary.id)
                .unwrap()
                .query
                .page
                .as_mut()
                .unwrap();
            page.rows[0].payloads = vec![IntruderPayloadPreview {
                set_index: 0,
                payload_index: 0,
                raw_bytes: 7,
                prefix: b"k=1&x=2".to_vec(),
            }];
            page.rows[0].highlight = Some(2);
            page.rows[0].notes_preview = "line1\tline2".into();
            cx.notify();
        });
    });

    let sel0 = select(&mut fixture, &started, 0, &echoed_flow, cx);
    detail(&fixture, sel0, &echoed_flow, cx);
    click(started.window, "intruder-copy-results-tsv", cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-grep-summary").label(),
            Some("载荷回显 (Grep - Payloads): k=1&x=2")
        );
        let tsv = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap();
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(
            lines[0],
            "Request #\tPayload\tStatus\tResponse received\tLength\tHighlight\tNotes"
        );
        assert_eq!(lines[1], "1\tk=1&x=2\t200\t17\t38\t2\tline1 line2");
    })
    .unwrap();
}

#[gpui_kit::test]
fn intruder_settings_request_engine_redirections_grep_payloads_and_regex_extract_work(
    cx: &mut TestAppContext,
) {
    let tab = attack();
    let id = tab.id;
    let mut fixture = intruder_fixture(cx, vec![tab], 64);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.render_frame(cx);
        window.click("intruder-sidebar-settings", cx);
        window.render_frame(cx);

        window.click("intruder-grep-extract-start", cx);
        window.input("re:token=([a-z0-9]+)", cx);
        window.click("intruder-grep-extract-add", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.grep_configs[&id].extract_rules,
            vec![mio_core::IntruderGrepExtractRule::regex(
                "re:token=([a-z0-9]+)"
            )]
        );

        window.scroll(
            "intruder-config-settings-body",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-420.))),
            cx,
        );
        window.render_frame(cx);
        window.click("intruder-grep-payloads-enabled", cx);
        window.click("intruder-grep-payloads-case", cx);
        let grep_cfg = &fixture.view.read(cx).workspace.intruder.grep_configs[&id];
        assert!(grep_cfg.grep_payloads);
        assert!(grep_cfg.grep_payloads_case_sensitive);

        window.click("intruder-settings-connection-close", cx);
        window.click("intruder-redirect-on-site", cx);
        window.click("intruder-settings-process-cookies", cx);
        let attack_settings = &fixture.view.read(cx).workspace.intruder.attack_settings[&id];
        assert!(attack_settings.set_connection_close);
        assert_eq!(
            attack_settings.follow_redirects,
            mio_core::RepeaterRedirectMode::OnSiteOnly
        );
        assert!(attack_settings.process_cookies_in_redirections);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|cmd| matches!(cmd, Command::SaveWorkspace { .. }))
    );
}
