use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, record};
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt, TestWindowExt};
use mio_core::{
    ExtHttpHighlight, ExtItemSummary, HttpHistoryResult, HttpHistorySortColumn, TrafficSource,
    Uuid,
};

#[path = "sort_tests.rs"]
mod sort_tests;

mod part_1;
mod part_2;
fn click(handle: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn query(f: &mut Fixture) -> HttpHistoryQuery {
    let commands = drain(&mut f.commands);
    let [Command::QueryHttpHistory { query }] = commands.as_slice() else {
        panic!(
            "one bounded HTTP history query, received {} commands",
            commands.len()
        );
    };
    assert_eq!(query.project, f.project);
    query.clone()
}

pub(super) fn reply(
    f: &Fixture,
    query: &HttpHistoryQuery,
    rows: Vec<HistoryRow>,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::HttpHistoryPage {
            query: query.clone(),
            result: Ok(HttpHistoryResult {
                total: rows.len(),
                selected_rows: Vec::new(),
                selected_ranks: query
                    .selected_ids
                    .iter()
                    .filter_map(|id| {
                        rows.iter()
                            .position(|row| row.id() == *id)
                            .map(|rank| (*id, rank))
                    })
                    .collect(),
                selected_matches: query
                    .selected_ids
                    .iter()
                    .map(|id| (*id, rows.iter().any(|row| row.id() == *id)))
                    .collect(),
                rows,
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn boot(cx: &mut TestAppContext) -> (Fixture, Vec<HistoryRow>) {
    let mut f = fixture(cx, vec![], 64);
    let first = query(&mut f);
    let mut live = record("http://127.0.0.1:9001/retained", "retained body");
    live.summary.source = TrafficSource::Proxy;
    let rows = vec![HistoryRow::Live(live.summary)];
    reply(&f, &first, rows.clone(), cx);
    click(f.window, "proxy-history", cx);
    match f.commands.try_recv() {
        Ok(Command::QueryHttpHistory { query }) => reply(&f, &query, rows.clone(), cx),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
        _ => panic!("HTTP history entry must not enqueue another operation"),
    }
    assert!(drain(&mut f.commands).is_empty());
    (f, rows)
}

pub(super) fn open(
    f: &Fixture,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<FilterWindow>) {
    click(f.window, "http-history-filter", cx);
    let handle = cx.read(|cx| f.view.read(cx).http_filter.window.unwrap());
    let dialog = handle
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
    (handle, dialog)
}

pub(super) fn type_in(
    handle: AnyWindowHandle,
    input: &Entity<InputState>,
    text: &str,
    cx: &mut TestAppContext,
) {
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(text, cx);
            assert_eq!(input.read(cx).value().as_ref(), text);
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn type_in_named(
    handle: AnyWindowHandle,
    id: &'static str,
    input: &Entity<InputState>,
    text: &str,
    cx: &mut TestAppContext,
) {
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
            window.press(crate::kbd!("a"), cx);
            window.input(text, cx);
            assert_eq!(input.read(cx).value().as_ref(), text);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn http_filter_apply_cancel_presets_and_toggle_preserve_successful_conditions(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = boot(cx);
    let initial = cx.read(|cx| f.view.read(cx).http_filter.applied.clone());
    assert!(initial.hide_connect);
    assert_eq!(
        initial.extensions,
        HttpExtensionFilter::Hide(HIDDEN_EXTENSIONS.into())
    );
    assert_eq!(
        initial.mime_types,
        [true, true, true, false, true, false, true, true]
    );
    let (handle, dialog) = open(&f, cx);
    click(handle, "http-filter-no-response", cx);
    click(handle, "http-filter-hide-connect", cx);
    click(handle, "http-filter-4xx", cx);
    click(handle, "http-filter-mime-html", cx);
    click(handle, "http-filter-apply", cx);
    let applied = query(&mut f);
    assert!(applied.filter.hide_without_response);
    assert!(!applied.filter.hide_connect);
    assert!(!applied.filter.status_classes[2]);
    assert_eq!(
        applied.filter.mime_types,
        [false, true, true, false, true, false, true, true]
    );
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).http_filter.applied, initial);
        assert_eq!(f.view.read(cx).history.read(cx).delegate().rows, rows);
    });
    reply(&f, &applied, rows.clone(), cx);
    assert!(handle.update(cx, |_, _, _| ()).is_ok());
    click(handle, "http-filter-notes", cx);
    click(handle, "http-filter-cancel", cx);
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| assert_eq!(f.view.read(cx).http_filter.applied, applied.filter));
    drop(dialog);
    cx.run_until_parked();
    click(f.window, "http-history-filter-enabled", cx);
    let bypass = query(&mut f);
    assert_eq!(bypass.filter, HttpHistoryFilter::default());
    reply(&f, &bypass, rows.clone(), cx);
    cx.read(|cx| {
        let filter = &f.view.read(cx).http_filter;
        assert!(!filter.enabled);
        assert_eq!(filter.applied, applied.filter);
    });
    click(f.window, "http-history-filter-enabled", cx);
    let restored = query(&mut f);
    assert_eq!(restored.filter, applied.filter);
    reply(&f, &restored, rows.clone(), cx);
    let (handle, dialog) = open(&f, cx);
    click(handle, "http-filter-all", cx);
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).draft, HttpHistoryFilter::default());
        assert_eq!(
            dialog.read(cx).hide.read(cx).value().as_ref(),
            HIDDEN_EXTENSIONS
        );
    });
    click(handle, "http-filter-hide-all", cx);
    cx.read(|cx| {
        let draft = &dialog.read(cx).draft;
        assert!(draft.hide_without_response);
        assert!(draft.hide_connect);
        assert!(draft.parameterized_only);
        assert_eq!(draft.status_classes, [false; 4]);
        assert_eq!(draft.extensions, HttpExtensionFilter::Any);
        assert!(!draft.only_with_notes && !draft.only_highlighted);
    });
    click(handle, "http-filter-revert", cx);
    cx.read(|cx| assert_eq!(dialog.read(cx).draft, applied.filter));
    click(handle, "http-filter-all", cx);
    click(handle, "http-filter-apply-close", cx);
    let all = query(&mut f);
    reply(&f, &all, rows, cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
}

#[gpui_kit::test]
fn http_filter_failure_and_ime_keep_raw_extension_inputs_until_successful_apply(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = boot(cx);
    let (handle, dialog) = open(&f, cx);
    let (only, hide) = cx.read(|cx| (dialog.read(cx).only.clone(), dialog.read(cx).hide.clone()));
    type_in(handle, &hide, "css, PNG", cx);
    type_in(handle, &only, "jsp,PHP", cx);
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", hide.entity_id()), cx);
            hide.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
            assert!(hide.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }));
            window.click("http-filter-apply", cx);
            window.click("http-filter-revert", cx);
            assert!(hide.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }));
            hide.update(cx, |input, cx| {
                input.unmark_text(window, cx);
                input.set_value("css, PNG", window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    click(handle, "http-filter-apply", cx);
    let failed = query(&mut f);
    assert_eq!(
        failed.filter.extensions,
        HttpExtensionFilter::Hide("css, PNG".into())
    );
    f.events
        .try_send(Event::HttpHistoryPage {
            query: failed.clone(),
            result: Err("数据库读取失败\n请求未完成，原列表仍保留".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).hide.read(cx).value().as_ref(), "css, PNG");
        assert_eq!(dialog.read(cx).only.read(cx).value().as_ref(), "jsp,PHP");
        assert!(dialog.read(cx).error.is_some());
        assert_eq!(f.view.read(cx).history.read(cx).delegate().rows, rows);
        assert_eq!(
            f.view.read(cx).http_filter.applied.extensions,
            HttpExtensionFilter::Hide(HIDDEN_EXTENSIONS.into())
        );
    });
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let bounds = window.find("http-filter-dialog").bounds();
            for id in [
                "http-filter-cancel",
                "http-filter-apply",
                "http-filter-apply-close",
            ] {
                let target = window.find(id);
                assert!(target.visible());
                assert!(target.bounds().bottom() <= bounds.bottom());
            }
            assert_eq!(
                window.find("http-filter-error").role(),
                Some(gpui_kit::accesskit::Role::Alert)
            );
        })
        .unwrap();
    click(handle, "http-filter-apply", cx);
    let retry = query(&mut f);
    reply(&f, &retry, rows, cx);
    type_in(handle, &hide, "changed-but-not-applied", cx);
    click(handle, "http-filter-revert", cx);
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).hide.read(cx).value().as_ref(), "css, PNG");
        assert_eq!(dialog.read(cx).only.read(cx).value().as_ref(), "jsp,PHP");
    });
    click(handle, "http-filter-cancel", cx);
}

#[gpui_kit::test]
fn http_filter_cancel_before_deferred_apply_sends_no_query(cx: &mut TestAppContext) {
    let (mut f, _) = boot(cx);
    let (handle, _) = open(&f, cx);
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("http-filter-no-response", cx);
            window.click("http-filter-apply", cx);
            window.click("http-filter-cancel", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| assert!(!f.view.read(cx).http_filter.applied.hide_without_response));
}

#[gpui_kit::test]
fn http_filter_apply_close_follows_latest_refresh_and_ignores_old_result(cx: &mut TestAppContext) {
    let (mut f, rows) = boot(cx);
    let (handle, _) = open(&f, cx);
    click(handle, "http-filter-no-response", cx);
    click(handle, "http-filter-apply-close", cx);
    let first = query(&mut f);
    f.events
        .try_send(Event::HistoryChanged { project: f.project })
        .unwrap();
    cx.run_until_parked();
    // Explicit refresh uses the same pending condition while the previous query is in flight.
    click(f.window, "history-filter", cx);
    let newest = query(&mut f);
    assert!(newest.query_revision > first.query_revision);
    assert_eq!(newest.filter, first.filter);
    reply(&f, &first, vec![], cx);
    assert!(cx.read(|cx| cx.windows().contains(&handle)));
    cx.read(|cx| assert!(!f.view.read(cx).http_filter.applied.hide_without_response));
    reply(&f, &newest, rows, cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    cx.read(|cx| assert!(f.view.read(cx).http_filter.applied.hide_without_response));
}

#[gpui_kit::test]
fn http_filter_toggle_accepts_latest_intent_before_the_previous_query_finishes(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = boot(cx);
    let applied = cx.read(|cx| f.view.read(cx).http_filter.applied.clone());
    click(f.window, "http-history-filter-enabled", cx);
    let disabled = query(&mut f);
    assert_eq!(disabled.filter, HttpHistoryFilter::default());
    click(f.window, "http-history-filter-enabled", cx);
    let enabled = query(&mut f);
    assert!(enabled.query_revision > disabled.query_revision);
    assert_eq!(enabled.filter, applied);
    reply(&f, &disabled, vec![], cx);
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).history.read(cx).delegate().rows, rows);
        assert!(f.view.read(cx).http_filter.enabled);
    });
    reply(&f, &enabled, rows, cx);
    cx.read(|cx| assert!(f.view.read(cx).http_filter.enabled));
}

#[gpui_kit::test]
fn http_filter_cancelled_query_keeps_rows_without_error_while_real_backend_error_is_reported(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = boot(cx);
    let applied = cx.read(|cx| f.view.read(cx).http_filter.applied.clone());
    click(f.window, "http-history-filter-enabled", cx);
    let cancelled_query = query(&mut f);
    f.events
        .try_send(Event::HttpHistoryPage {
            query: cancelled_query,
            result: Err("HTTP 历史查询已取消".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.error.is_none());
        assert!(!view.history_loading);
        assert!(!view.history.read(cx).delegate().loading);
        assert!(view.http_filter.enabled);
        assert_eq!(view.http_filter.applied, applied);
        assert_eq!(view.history.read(cx).delegate().rows, rows);
    });

    click(f.window, "http-history-filter-enabled", cx);
    let failed_query = query(&mut f);
    f.events
        .try_send(Event::HttpHistoryPage {
            query: failed_query,
            result: Err("数据库索引损坏".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.error.as_deref(),
            Some("HTTP 历史查询失败：数据库索引损坏；当前列表保留。")
        );
        assert!(!view.history_loading);
        assert_eq!(view.history.read(cx).delegate().rows, rows);
    });
}
