use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use mio_core::{
    AuditCommand, AuditTaskMode, Command, Event, Header, HttpResponse, HttpVersion,
    InterceptStage, ProxyInterceptRelation, ProxyInterceptType, Uuid, WebSocketDirection,
    WebSocketKind, WebSocketMessage,
};

use crate::workbench::tests::{drain, fixture, record};
use super::*;

#[gpui_kit::test]
fn history_menu_extracts_links_starts_passive_scan_and_saves_items_to_disk(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    let initial = drain(&mut f.commands);
    let query_revision = initial
        .iter()
        .find_map(|cmd| match cmd {
            Command::QueryHttpHistory { query } => Some(query.query_revision),
            _ => None,
        })
        .expect("initial history query");

    let mut flow = record("https://app.example.com/index.html", "");
    flow.request.url = "https://app.example.com/index.html".into();
    flow.summary.status = Some(200);
    flow.response = Some(HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"text/html; charset=utf-8".to_vec(),
        }],
        body: b"<html><body><a href=\"/login\">Login</a><a href=\"https://cdn.example.com/app.js\">JS</a></body></html>".to_vec(),
        trailers: Vec::new(),
    });
    let flow_id = flow.summary.id;

    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision,
            offset: 0,
            total: 1,
            rows: vec![HistoryRow::Live(flow.summary.clone())],
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.proxy_page = ProxyPage::History;
            view.select_flow(flow_id, window, cx);
        });
    })
    .unwrap();
    let sel_rev = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::ReadFlow {
                selection_revision, ..
            } => Some(selection_revision),
            _ => None,
        })
        .expect("read flow command");
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: sel_rev,
            record: Box::new(flow.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.copy_selected_history_links(window, cx);
        });
    })
    .unwrap();
    let copied_links = cx
        .read(|cx| cx.read_from_clipboard().and_then(|i| i.text()))
        .expect("extracted links in clipboard");
    assert!(copied_links.contains("https://app.example.com/login"));
    assert!(copied_links.contains("https://cdn.example.com/app.js"));

    let out_dir = std::env::current_dir()
        .unwrap()
        .join(".local")
        .join(format!("test-history-export-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let xml_path = out_dir.join("saved.xml");
    let har_path = out_dir.join("saved.har");

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.save_selected_history_item_to_file(true, window, cx);
        });
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(xml_path.clone()));
    cx.run_until_parked();
    let xml_content = std::fs::read_to_string(&xml_path).unwrap();
    assert!(xml_content.contains("<items "));
    assert!(xml_content.contains("<item>"));
    assert!(xml_content.contains("<url>https://app.example.com/index.html</url>"));

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.save_selected_history_item_to_file(false, window, cx);
        });
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(har_path.clone()));
    cx.run_until_parked();
    let har_content = std::fs::read_to_string(&har_path).unwrap();
    assert!(har_content.contains("\"log\""));
    assert!(har_content.contains("https://app.example.com/index.html"));

    let _ = std::fs::remove_dir_all(&out_dir);

    drain(&mut f.commands);
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.send_to_passive_scanner(window, cx);
        });
    })
    .unwrap();
    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(AuditCommand::StartScan {
            mode: AuditTaskMode::PassiveOnly,
            ..
        })
    )));
}

#[gpui_kit::test]
fn intercept_actions_support_response_interception_exclusion_rules_and_websocket_menu(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);

    let req_flow = record("https://static.example.com/assets/logo.png", "");
    let flow_id = req_flow.summary.id;
    f.events
        .try_send(Event::InterceptPending {
            project: f.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(req_flow),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.toggle_intercept_response_for_selected(cx);
        });
    })
    .unwrap();
    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::SetLiveFlowAnnotated {
            flow_id: id,
            annotated: true,
            ..
        } if *id == flow_id
    )));
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::SetIntercept {
            responses: true,
            ..
        }
    )));
    cx.read(|cx| {
        let ws = &f.view.read(cx).workspace;
        assert!(ws.proxy_interception.response.enabled);
        assert!(ws.proxy_interception.response.rules.iter().any(|r| {
            r.enabled
                && r.match_type == ProxyInterceptType::Request
                && r.relationship == ProxyInterceptRelation::WasAnnotated
        }));
    });

    cx.update_window(f.window, |_, _, cx| {
        f.view.update(cx, |view, cx| {
            view.add_do_not_intercept_host_rule(cx);
            view.add_do_not_intercept_extension_rule(cx);
        });
    })
    .unwrap();
    cx.read(|cx| {
        let rules = &f.view.read(cx).workspace.proxy_interception.request.rules;
        assert!(rules.iter().any(|r| {
            r.enabled
                && r.match_type == ProxyInterceptType::DomainName
                && r.relationship == ProxyInterceptRelation::DoesNotMatch
                && r.condition == "^static\\.example\\.com$"
        }));
        assert!(rules.iter().any(|r| {
            r.enabled
                && r.match_type == ProxyInterceptType::FileExtension
                && r.relationship == ProxyInterceptRelation::DoesNotMatch
                && r.condition == "^png$"
        }));
    });

    let ws_msg = WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id,
        sequence: 1,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Text,
        payload: b"{\"ping\":1}".to_vec(),
        observed_unix_ms: 100,
    };
    let ws_id = ws_msg.id;
    f.events
        .try_send(Event::WebSocketInterceptPending {
            project: f.project,
            url: "wss://ws.example.com/live".into(),
            record: Box::new(ws_msg),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.selected_intercept = Some(ws_id);
            cx.notify();
        });
        window.render_frame(cx);
        let action_btn = window.find("intercept-action-menu");
        assert_ne!(action_btn.disabled(), Some(true));
        f.view.update(cx, |view, cx| {
            view.copy_intercept_websocket_url(cx);
            view.modify_intercept_websocket_scope(true, cx);
            view.send_intercept_websocket_to_comparer(window, cx);
        });
    })
    .unwrap();
    assert_eq!(
        cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
        Some("wss://ws.example.com/live".into())
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(
            view.workspace
                .scope
                .include
                .iter()
                .any(|r| r.prefix == "https://ws.example.com")
        );
        assert_eq!(view.extra_tools.comparer_items.len(), 1);
    });
}

#[gpui_kit::test]
fn toggle_intercept_response_does_not_mutate_ui_state_when_enqueue_fails(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    let rec = record("https://example.invalid/login", "");
    let flow_id = rec.summary.id;
    f.events
        .try_send(Event::InterceptPending {
            project: f.project,
            stage: InterceptStage::Request,
            response_head_only: false,
            record: Box::new(rec),
        })
        .unwrap();
    cx.run_until_parked();

    drop(f.commands);
    cx.update_window(f.window, |_, _, cx| {
        f.view.update(cx, |view, cx| {
            view.selected_intercept = Some(flow_id);
            view.toggle_intercept_response_for_selected(cx);
        });
    })
    .unwrap();

    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.annotated_live_flows.contains(&flow_id));
        assert!(!view.intercept_responses);
        assert!(!view.workspace.proxy_interception.response.enabled);
        assert!(view.error.is_some());
    });
}
