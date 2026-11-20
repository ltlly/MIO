use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, record};
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{TestAppContext, point};
use mio_core::{ExtHistoryRow, ExtItemSummary, ExtMessageFragment, Uuid};

mod request_versions;
mod response_versions;
mod transfer;

pub(in crate::workbench) use request_versions::request_version;
pub(in crate::workbench) use transfer::{transfer_command, transfer_proposal};

pub(super) fn row(index: u64, request_len: u64, response_len: Option<u64>) -> HistoryRow {
    let source_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();
    let message = |response: bool, len| ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id,
        item_id,
        collection: "root.f1.f0".into(),
        item_index: index,
        role: if response { "response" } else { "request" }.into(),
        field_id: if response { 18 } else { 15 },
        raw_offset: 100,
        raw_bytes: len,
        body_offset: None,
        declared_version: Some(HttpVersion::Http11),
        method: (!response).then(|| "GET".into()),
        request_target: (!response).then(|| "/proxy229-probe-a".into()),
        status: response.then_some(201),
        diagnostic: None,
    };
    HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
        notes_revision: 0,
        notes_preview: String::new(),
        notes_error: None,
        highlight: Some(mio_core::ExtHttpHighlight {
            revision: 0,
            color: 0,
        }),
        highlight_error: None,
        time_unix_ms: None,
        time_error: None,
        item: ExtItemSummary {
            id: item_id,
            source_id,
            collection: "root.f1.f0".into(),
            index,
            object_address: None,
            state: "mapped".into(),
            message_count: 1 + usize::from(response_len.is_some()),
        },
        request: Some(message(false, request_len)),
        edited_request: None,
        response: response_len.map(|len| message(true, len)),
        edited_response: None,
        origin: Some("http://127.0.0.1:62701".into()),
    }))
}
pub(super) fn page(
    f: &mut Fixture,
    rows: Vec<HistoryRow>,
    total: usize,
    offset: usize,
    cx: &mut TestAppContext,
) {
    let commands = drain(&mut f.commands);
    let revision = commands
        .iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(*query_revision),
            _ => None,
        })
        .expect("history query");
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: revision,
            offset,
            total,
            rows,
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn click_row(f: &Fixture, index: usize, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-history", cx);
            window.render_frame(cx);
            window.click_at(
                "table",
                point(px(90.), px(theme::ROW_HEIGHT * (index as f32 + 1.5))),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn read(f: &mut Fixture) -> (u64, Uuid, u64, usize) {
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtMessage {
            selection_revision,
            message_id,
            offset,
            limit,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("select only reads one archive fragment, never network or another tool")
    };
    (*selection_revision, *message_id, *offset, *limit)
}
pub(super) fn fragment(
    f: &Fixture,
    read: (u64, Uuid, u64, usize),
    summary: ExtMessageSummary,
    bytes: Vec<u8>,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::ExtMessage {
            project: f.project,
            selection_revision: read.0,
            message_id: read.1,
            offset: read.2,
            result: Ok(ExtMessageFragment {
                summary,
                offset: read.2,
                bytes,
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn imported_proxy_history_shows_source_number_raw_response_length_and_binary_pair(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let request = b"GET /proxy229-probe-a HTTP/1.1\r\nX-A: one\r\nX-A: two\r\n\r\n\0\xff";
    let response = b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\n\r\n\0\xffx";
    let item = row(2, request.len() as u64, Some(response.len() as u64));
    let HistoryRow::ExtProxy(archive_row_1) = &item else {
        unreachable!()
    };
    let ExtHistoryRow {
        request: Some(req),
        response: Some(resp),
        ..
    } = archive_row_1.as_ref()
    else {
        unreachable!()
    };
    page(
        &mut f,
        vec![
            item.clone(),
            record("http://127.0.0.1:1/live", "live").summary.into(),
        ],
        2,
        0,
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
    click_row(&f, 0, cx);
    let first = read(&mut f);
    assert_eq!((first.1, first.2, first.3), (req.id, 0, request.len()));
    fragment(&f, first, req.clone(), request.to_vec(), cx);
    let second = read(&mut f);
    assert_eq!((second.1, second.2, second.3), (resp.id, 0, response.len()));
    fragment(&f, second, resp.clone(), response.to_vec(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("table-cell-0:0").label(), Some("3"));
            assert_eq!(
                window.find("table-cell-0:7").label(),
                Some(response.len().to_string().as_str())
            );
            f.view.update(cx, |view, cx| {
                assert_eq!(view.selected_history, Some(item.id()));
                assert!(view.selected_flow.is_none());
                assert_eq!(view.history_request.bytes(window, cx).unwrap(), request);
                assert_eq!(view.history_response.bytes(window, cx).unwrap(), response);
                assert_eq!(view.history_request.view, MessageView::Hex);
                assert_eq!(view.history_response.view, MessageView::Hex);
                assert!(view.detail.is_none());
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn imported_proxy_late_response_is_ignored_and_failed_response_retry_preserves_request(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let first = row(1, 3, Some(3));
    let second = row(0, 3, Some(3));
    let HistoryRow::ExtProxy(archive_row_2) = &first else {
        unreachable!()
    };
    let ExtHistoryRow {
        request: Some(req1),
        response: Some(resp1),
        ..
    } = archive_row_2.as_ref()
    else {
        unreachable!()
    };
    let HistoryRow::ExtProxy(archive_row_3) = &second else {
        unreachable!()
    };
    let ExtHistoryRow {
        request: Some(req2),
        response: Some(resp2),
        ..
    } = archive_row_3.as_ref()
    else {
        unreachable!()
    };
    page(&mut f, vec![first.clone(), second.clone()], 2, 0, cx);
    click_row(&f, 0, cx);
    let request = read(&mut f);
    fragment(&f, request, req1.clone(), b"one".to_vec(), cx);
    let late = read(&mut f);
    click_row(&f, 1, cx);
    let request = read(&mut f);
    fragment(&f, late, resp1.clone(), b"old".to_vec(), cx);
    assert!(drain(&mut f.commands).is_empty());
    fragment(&f, request, req2.clone(), b"two".to_vec(), cx);
    let failed = read(&mut f);
    f.events
        .try_send(Event::ExtMessage {
            project: f.project,
            selection_revision: failed.0,
            message_id: failed.1,
            offset: 0,
            result: Err("archive unavailable".into()),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                f.view
                    .read(cx)
                    .history_request
                    .raw
                    .read(cx)
                    .value()
                    .as_ref(),
                "two"
            );
            window.click("imported-history-response-retry", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let retry = read(&mut f);
    assert!(retry.0 > failed.0);
    assert_eq!(retry.1, resp2.id);
    fragment(&f, retry, resp2.clone(), b"new".to_vec(), cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.selected_history, Some(second.id()));
        assert_eq!(view.history_request.raw.read(cx).value().as_ref(), "two");
        assert_eq!(view.history_response.raw.read(cx).value().as_ref(), "new");
    });
}

#[gpui_kit::test]
fn imported_history_continuous_scroll_omits_pagination_buttons_and_auto_loads_more(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let total = HTTP_HISTORY_BATCH_SIZE + 3;
    let all_rows: Vec<_> = (0..total).rev().map(|idx| row(idx as u64, 3, None)).collect();
    page(
        &mut f,
        all_rows[..HTTP_HISTORY_BATCH_SIZE].to_vec(),
        total,
        0,
        cx,
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-history", cx);
            window.render_frame(cx);
            assert!(window.try_find("history-previous").is_none());
            assert!(window.try_find("history-next").is_none());
            f.view.update(cx, |view, cx| {
                view.history.update(cx, |table, cx| {
                    table.scroll_to_row(HTTP_HISTORY_BATCH_SIZE - 1, cx)
                });
            });
            window.render_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let expected_limit = HTTP_HISTORY_BATCH_SIZE * 2;
    let [
        Command::QueryHttpHistory {
            query:
                mio_core::HttpHistoryQuery {
                    query_revision,
                    offset: 0,
                    limit,
                    ..
                },
        },
    ] = commands.as_slice()
    else {
        panic!("scrolling near the end of partial history must request a larger continuous batch")
    };
    assert_eq!(*limit, expected_limit);
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total,
            rows: all_rows,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let delegate = f.view.read(cx).history.read(cx).delegate();
        assert_eq!(delegate.rows.len(), total);
        assert_eq!(delegate.total, total);
        assert!(!delegate.loading);
    });
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn imported_history_assembles_bounded_fragments_and_preserves_oversized_response_reference(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let raw = vec![b'x'; FRAGMENT_BYTES + 3];
    let item = row(0, raw.len() as u64, Some(MAX_MESSAGE_BYTES + 1));
    let HistoryRow::ExtProxy(archive_row_4) = &item else {
        unreachable!()
    };
    let ExtHistoryRow {
        request: Some(request),
        ..
    } = archive_row_4.as_ref()
    else {
        unreachable!()
    };
    page(&mut f, vec![item.clone()], 1, 0, cx);
    click_row(&f, 0, cx);
    let first = read(&mut f);
    assert_eq!((first.2, first.3), (0, FRAGMENT_BYTES));
    fragment(
        &f,
        first,
        request.clone(),
        raw[..FRAGMENT_BYTES].to_vec(),
        cx,
    );
    cx.read(|cx| {
        assert!(
            f.view
                .read(cx)
                .history_request
                .raw
                .read(cx)
                .value()
                .is_empty()
        )
    });
    let last = read(&mut f);
    assert_eq!((last.2, last.3), (FRAGMENT_BYTES as u64, 3));
    fragment(
        &f,
        last,
        request.clone(),
        raw[FRAGMENT_BYTES..].to_vec(),
        cx,
    );
    assert!(
        drain(&mut f.commands).is_empty(),
        "oversized response must not begin an unbounded read"
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("imported-history-open-archive").visible());
            f.view.update(cx, |view, cx| {
                assert_eq!(view.history_request.bytes(window, cx).unwrap(), raw);
                assert_eq!(view.imported_history.row.as_ref(), Some(&item));
                assert!(view.history_response.raw.read(cx).value().is_empty());
            });
        })
        .unwrap();
}
