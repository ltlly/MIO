use super::super::tests::{click_row, message};
use super::*;
use crate::workbench::tests::{Fixture, drain, fixture};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

mod part_1;

pub(super) fn query(f: &mut Fixture) -> (u64, WebSocketHistoryFilter, Option<Uuid>) {
    let commands = drain(&mut f.commands);
    let [
        Command::QueryWebSocketHistory {
            query_revision,
            filter,
            selected_message_id,
            offset: 0,
            limit: WS_HISTORY_BATCH_SIZE,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("expected one first-page filter query")
    };
    (*query_revision, filter.clone(), *selected_message_id)
}
pub(super) fn page(
    f: &Fixture,
    query: &(u64, WebSocketHistoryFilter, Option<Uuid>),
    rows: Vec<WebSocketSummary>,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::WebSocketHistoryPage {
            project: f.project,
            query_revision: query.0,
            flow_id: None,
            filter: query.1.clone(),
            selected_match: query.2.map(|id| (id, rows.iter().any(|row| row.id == id))),
            offset: 0,
            total: rows.len(),
            rows,
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn click(handle: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) {
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn open(
    f: &mut Fixture,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<FilterWindow>) {
    click(f.window, "websocket-filter", cx);
    let handle = cx.read(|cx| f.view.read(cx).websocket.filter_window.unwrap());
    let entity = handle
        .update(cx, |_, window, cx| {
            window
                .root::<gpui_kit::base::Root>()
                .unwrap()
                .unwrap()
                .read(cx)
                .view()
                .clone()
                .downcast::<FilterWindow>()
                .unwrap()
        })
        .unwrap();
    (handle, entity)
}
pub(super) fn boot(f: &mut Fixture, cx: &mut TestAppContext) {
    drain(&mut f.commands);
    click(f.window, "proxy-websocket-history", cx);
    let initial = query(f);
    page(f, &initial, vec![], cx);
}
pub(super) fn boot_populated(f: &mut Fixture, cx: &mut TestAppContext) {
    drain(&mut f.commands);
    click(f.window, "proxy-websocket-history", cx);
    let initial = query(f);
    let (seed, _) = message(Uuid::new_v4(), 1, b"seed", WebSocketKind::Text);
    page(f, &initial, vec![seed], cx);
}

#[gpui_kit::test]
fn metadata_filter_apply_cancel_and_toggle_keep_last_successful_conditions(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    boot(&mut f, cx);
    let (summary, record) = message(Uuid::new_v4(), 1, b"retained", WebSocketKind::Text);
    f.events
        .try_send(Event::WebSocketHistoryChanged { project: f.project })
        .unwrap();
    cx.run_until_parked();
    let initial = query(&mut f);
    page(&f, &initial, vec![summary.clone()], cx);
    click_row(&f, 0, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadWebSocket {
            selection_revision, ..
        },
    ] = commands.as_slice()
    else {
        unreachable!()
    };
    f.events
        .try_send(Event::WebSocketDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(record.clone()),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    let (window, dialog) = open(&mut f, cx);
    click(window, "websocket-hide-incoming", cx);
    click(window, "websocket-filter-apply", cx);
    let applied = query(&mut f);
    assert!(applied.1.hide_incoming);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).websocket.applied_filter,
            WebSocketHistoryFilter::default()
        );
        assert_eq!(f.view.read(cx).websocket.detail.as_deref(), Some(&record));
    });
    page(&f, &applied, vec![summary.clone()], cx);
    assert!(window.update(cx, |_, _, _| ()).is_ok());
    click(window, "websocket-hide-outgoing", cx);
    click(window, "websocket-filter-cancel", cx);
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| assert_eq!(f.view.read(cx).websocket.applied_filter, applied.1));
    drop(dialog);
    click(f.window, "websocket-filter-enabled", cx);
    let bypass = query(&mut f);
    assert_eq!(bypass.1, WebSocketHistoryFilter::default());
    page(&f, &bypass, vec![summary.clone()], cx);
    cx.read(|cx| {
        let state = &f.view.read(cx).websocket;
        assert!(!state.filter_enabled);
        assert_eq!(state.applied_filter, applied.1);
    });
    click(f.window, "websocket-filter-enabled", cx);
    let restored = query(&mut f);
    assert_eq!(restored.1, applied.1);
    page(&f, &restored, vec![summary], cx);
    let (window, dialog) = open(&mut f, cx);
    click(window, "websocket-filter-all", cx);
    cx.read(|cx| assert_eq!(dialog.read(cx).draft, WebSocketHistoryFilter::default()));
    click(window, "websocket-filter-revert", cx);
    cx.read(|cx| assert_eq!(dialog.read(cx).draft, applied.1));
    click(window, "websocket-hide-outgoing", cx);
    click(window, "websocket-filter-apply-close", cx);
    let hidden = query(&mut f);
    f.events
        .try_send(Event::WebSocketHistoryChanged { project: f.project })
        .unwrap();
    page(&f, &hidden, vec![], cx);
    assert!(
        window.update(cx, |_, _, _| ()).is_err(),
        "Apply and close must succeed despite a following automatic refresh"
    );
    cx.read(|cx| {
        let state = &f.view.read(cx).websocket;
        assert_eq!(state.applied_filter, hidden.1);
        assert!(state.detail.is_none());
    });
}

#[gpui_kit::test]
fn metadata_filter_port_ime_errors_and_failed_query_preserve_draft(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![], 64);
    boot_populated(&mut f, cx);
    let (window, dialog) = open(&mut f, cx);
    window
        .update(cx, |_, window, cx| {
            let port = dialog.read(cx).port.clone();
            window.click(("input", port.entity_id()), cx);
            window.input("0", cx);
            window.click("websocket-filter-apply", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).port.read(cx).value().as_ref(), "0");
        assert!(dialog.read(cx).error.is_some());
    });
    window
        .update(cx, |_, window, cx| {
            let port = dialog.read(cx).port.clone();
            window.click(("input", port.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("9001", cx);
            port.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
            window.click("websocket-filter-apply", cx);
            assert!(port.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }));
            port.update(cx, |input, cx| {
                input.unmark_text(window, cx);
                input.set_value("9001", window, cx);
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    click(window, "websocket-filter-apply", cx);
    let failed = query(&mut f);
    assert_eq!(failed.1.listener_port, Some(9001));
    f.events
        .try_send(Event::WebSocketHistoryError {
            project: f.project,
            query_revision: failed.0,
            message: "disk busy".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).websocket.applied_filter,
            WebSocketHistoryFilter::default()
        );
        assert_eq!(dialog.read(cx).port.read(cx).value().as_ref(), "9001");
    });
    click(f.window, "websocket-refresh", cx);
    let retry = query(&mut f);
    assert_eq!(retry.1, failed.1);
    page(&f, &retry, vec![], cx);
    cx.read(|cx| assert_eq!(f.view.read(cx).websocket.applied_filter, failed.1));
    click(window, "websocket-filter-cancel", cx);
}

#[gpui_kit::test]
fn metadata_filter_cancel_before_deferred_apply_sends_nothing(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![], 64);
    boot_populated(&mut f, cx);
    let (window, _dialog) = open(&mut f, cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("websocket-hide-incoming", cx);
            window.click("websocket-filter-apply", cx);
            window.click("websocket-filter-cancel", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).websocket.applied_filter,
            WebSocketHistoryFilter::default()
        )
    });
}

#[gpui_kit::test]
fn ws_filter_convert_to_script_validation_and_script_mode_apply(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![], 64);
    boot_populated(&mut f, cx);
    let (window, dialog) = open(&mut f, cx);

    click(window, "websocket-hide-outgoing", cx);
    click(window, "websocket-in-scope-only", cx);
    click(window, "ws-filter-convert-to-script", cx);

    cx.read(|cx| {
        let view = dialog.read(cx);
        assert!(view.script_mode);
        let script = view.filter_script_editor.read(cx).value();
        assert!(script.contains("in_scope"));
        assert!(script.contains("message.direction != \"c2s\""));
    });

    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("ws-filter-script-editor").visible());
            dialog.update(cx, |this, cx| {
                this.filter_script_editor
                    .update(cx, |editor, cx| editor.set_value("if (", window, cx));
            });
        })
        .unwrap();
    cx.run_until_parked();

    click(window, "websocket-filter-apply", cx);
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        assert!(dialog.read(cx).error.is_some());
        assert_eq!(dialog.read(cx).filter_script_editor.read(cx).value().as_ref(), "if (");
    });

    click(window, "ws-filter-script-template", cx);
    cx.read(|cx| {
        assert_eq!(
            dialog.read(cx).filter_script_editor.read(cx).value().as_ref(),
            mio_core::DEFAULT_WS_FILTER_SCRIPT_TEMPLATE
        );
    });

    click(window, "websocket-filter-apply-close", cx);
    let applied = query(&mut f);
    assert_eq!(
        applied.1.filter_script,
        mio_core::DEFAULT_WS_FILTER_SCRIPT_TEMPLATE
    );
    page(&f, &applied, vec![], cx);
    assert!(window.update(cx, |_, _, _| ()).is_err());

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("websocket-filter").label(),
                Some("筛选设置：Script mode (MioScript)")
            );
        })
        .unwrap();
}
