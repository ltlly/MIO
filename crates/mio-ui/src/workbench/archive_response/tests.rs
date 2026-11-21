use super::*;
use crate::workbench::tests::{Fixture, acknowledge_history_query, drain, fixture, record, tab};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use mio_core::{ExtMessageFragment, Uuid};

pub(super) fn archived() -> WorkTab {
    let mut tab = tab("archive");
    tab.archive_response = Some(ExtResponseRef {
        source_id: Uuid::new_v4(),
        item_id: Uuid::new_v4(),
        message_id: Uuid::new_v4(),
        draft_revision: 1,
    });
    tab
}
pub(super) fn click(f: &Fixture, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}
pub(super) fn read(f: &mut Fixture, cx: &mut TestAppContext) -> (u64, ExtResponseRef, u64, usize) {
    let commands = drain(&mut f.commands);
    let commands = acknowledge_history_query(f, commands, cx);
    let mut reads = commands.iter().filter_map(|c| match c {
        Command::ReadExtResponse {
            selection_revision,
            reference,
            offset,
            limit,
            ..
        } => Some((*selection_revision, reference.clone(), *offset, *limit)),
        _ => None,
    });
    let read = reads
        .next()
        .expect("actual activation must read archived response");
    assert!(reads.next().is_none());
    assert!(commands.iter().all(|c| matches!(
        c,
        Command::ReadExtResponse { .. }
            | Command::SaveWorkspace { .. }
            | Command::QueryHttpHistory { .. }
    )));
    read
}
pub(super) fn summary(reference: &ExtResponseRef, total: u64) -> ExtMessageSummary {
    ExtMessageSummary {
        id: reference.message_id,
        source_id: reference.source_id,
        item_id: reference.item_id,
        collection: "root.f2.f1[0].f2".into(),
        item_index: 0,
        role: "response".into(),
        field_id: 3,
        raw_offset: 100,
        raw_bytes: total,
        body_offset: Some(19),
        declared_version: Some(HttpVersion::Http11),
        method: None,
        request_target: None,
        status: Some(200),
        diagnostic: None,
    }
}
pub(super) fn reply(
    f: &Fixture,
    read: &(u64, ExtResponseRef, u64, usize),
    summary: ExtMessageSummary,
    bytes: Vec<u8>,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::ExtResponse {
            project: f.project,
            selection_revision: read.0,
            reference: read.1.clone(),
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

mod imported;

#[gpui_kit::test]
fn active_archive_response_reads_sequential_fragments_and_publishes_only_complete_binary(
    cx: &mut TestAppContext,
) {
    let tab = archived();
    let id = tab.id;
    let reference = tab.archive_response.clone().unwrap();
    let mut f = fixture(cx, vec![tab], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let head = read(&mut f, cx);
    assert_eq!((head.2, head.3), (0, 0));
    assert_eq!(head.1, reference);
    let mut raw = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
    raw.extend(vec![255; FRAGMENT_BYTES]);
    let meta = summary(&reference, raw.len() as u64);
    reply(&f, &head, meta.clone(), vec![], cx);
    let first = read(&mut f, cx);
    assert_eq!((first.2, first.3), (0, FRAGMENT_BYTES));
    reply(&f, &first, meta.clone(), raw[..FRAGMENT_BYTES].to_vec(), cx);
    let last = read(&mut f, cx);
    assert_eq!(
        (last.2, last.3),
        (FRAGMENT_BYTES as u64, raw.len() - FRAGMENT_BYTES)
    );
    cx.read(|cx| {
        assert!(
            f.view.read(cx).tab_editors[&id]
                .response
                .raw
                .read(cx)
                .value()
                .is_empty()
        )
    });
    reply(&f, &last, meta, raw[FRAGMENT_BYTES..].to_vec(), cx);
    assert!(drain(&mut f.commands).is_empty());
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                let editors = &view.tab_editors[&id];
                assert_eq!(editors.response.view(), MessageView::Hex);
                assert_eq!(editors.response.bytes(window, cx).unwrap(), raw);
                assert_eq!(editors.response_revision, Some(1));
                assert!(editors.result.as_ref().unwrap().contains("归档响应"));
            });
        })
        .unwrap();
    click(&f, "repeater-response-raw", cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.tab_editors[&id].response.view(), MessageView::Hex);
        assert!(view.error.as_ref().unwrap().contains("非 UTF-8"));
    });
}

#[gpui_kit::test]
fn switching_archive_tabs_releases_old_body_and_late_fragments_cannot_replace_current_response(
    cx: &mut TestAppContext,
) {
    let first = archived();
    let firstid = first.id;
    let second = archived();
    let secondid = second.id;
    let mut f = fixture(cx, vec![first, second], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let old = read(&mut f, cx);
    click(&f, format!("work-tab-{secondid}"), cx);
    let next = read(&mut f, cx);
    assert!(next.0 > old.0);
    reply(&f, &old, summary(&old.1, 3), vec![], cx);
    assert!(drain(&mut f.commands).is_empty());
    reply(&f, &next, summary(&next.1, 3), vec![], cx);
    let body = read(&mut f, cx);
    reply(&f, &body, summary(&body.1, 3), b"two".to_vec(), cx);
    click(&f, format!("work-tab-{firstid}"), cx);
    read(&mut f, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(
            view.tab_editors[&secondid]
                .response
                .raw
                .read(cx)
                .value()
                .is_empty()
        );
        assert!(
            view.tab_editors[&firstid]
                .response
                .raw
                .read(cx)
                .value()
                .is_empty()
        );
    });
}

#[gpui_kit::test]
fn archive_response_error_retries_and_actual_send_result_owns_the_response(
    cx: &mut TestAppContext,
) {
    let tab = archived();
    let id = tab.id;
    let mut f = fixture(cx, vec![tab], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let failed = read(&mut f, cx);
    f.events
        .try_send(Event::ExtResponse {
            project: f.project,
            selection_revision: failed.0,
            reference: failed.1.clone(),
            offset: 0,
            result: Err("source unavailable".into()),
        })
        .unwrap();
    cx.run_until_parked();
    click(&f, "archive-response-retry", cx);
    let retry = read(&mut f, cx);
    assert!(retry.0 > failed.0);
    click(&f, "repeater-send", cx);
    let commands = drain(&mut f.commands);
    let (run, draft) = commands
        .iter()
        .find_map(|c| match c {
            Command::Send { run_id, draft, .. } => Some((*run_id, draft.revision)),
            _ => None,
        })
        .expect("only manual Send starts network");
    reply(&f, &retry, summary(&retry.1, 3), vec![], cx);
    assert!(drain(&mut f.commands).is_empty());
    let mut response = record("http://127.0.0.1:9001/", "live");
    response.summary.id = run;
    response.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nlive",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id: id,
            draft_revision: draft,
            record: Box::new(response.clone()),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.tabs[0].last_flow, Some(run));
        assert!(view.workspace.tabs[0].archive_response.is_none());
        assert_eq!(
            view.tab_editors[&id].response.bytes_lossless(cx),
            response_to_raw(response.response.as_ref().unwrap())
        );
    });
}

#[gpui_kit::test]
fn oversized_archive_response_preserves_reference_without_reading_payload(cx: &mut TestAppContext) {
    let tab = archived();
    let reference = tab.archive_response.clone();
    let mut f = fixture(cx, vec![tab], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let head = read(&mut f, cx);
    reply(
        &f,
        &head,
        summary(&head.1, MAX_RESPONSE_BYTES + 1),
        vec![],
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.tabs[0].archive_response, reference);
        assert!(
            view.archive_responses
                .error
                .as_ref()
                .unwrap()
                .0
                .contains("完整分片查看")
        );
    });
}

#[gpui_kit::test]
fn cancelling_send_before_workspace_ack_keeps_archive_response_reading(cx: &mut TestAppContext) {
    let tab = archived();
    let id = tab.id;
    let mut f = fixture(cx, vec![tab], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let head = read(&mut f, cx);
    let raw = vec![b'A'; FRAGMENT_BYTES + 7];
    let meta = summary(&head.1, raw.len() as u64);
    reply(&f, &head, meta.clone(), vec![], cx);
    let first = read(&mut f, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let request = f.view.read(cx).tab_editors[&id].request.raw.clone();
            window.click(("input", request.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("GET /edited HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n", cx);
        })
        .unwrap();
    click(&f, "repeater-send", cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("Send must wait for the edited workspace to be acknowledged")
    };
    let workspace = workspace.clone();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.pending_sends.contains_key(&id));
        assert!(!view.sending.contains_key(&id));
    });
    reply(&f, &first, meta.clone(), raw[..FRAGMENT_BYTES].to_vec(), cx);
    let last = read(&mut f, cx);
    assert_eq!((last.2, last.3), (FRAGMENT_BYTES as u64, 7));
    click(&f, "repeater-cancel", cx);
    assert!(drain(&mut f.commands).is_empty());
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut f.commands).is_empty(),
        "a cancelled pending send must not become network work after ACK"
    );
    reply(&f, &last, meta, raw[FRAGMENT_BYTES..].to_vec(), cx);
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                assert_eq!(
                    view.tab_editors[&id].response.bytes(window, cx).unwrap(),
                    raw
                );
                assert!(view.workspace.tabs[0].archive_response.is_some());
                assert!(view.workspace.tabs[0].last_flow.is_none());
                assert!(view.sending.is_empty() && view.pending_sends.is_empty());
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn restoring_closed_archive_tab_from_proxy_starts_response_reading(cx: &mut TestAppContext) {
    let mut tab = archived();
    tab.title = format!("{}🙂{}", "A".repeat(31), "B".repeat(320));
    let id = tab.id;
    let reference = tab.archive_response.clone().unwrap();
    let mut f = fixture(cx, vec![tab.clone()], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    read(&mut f, cx);
    click(&f, format!("close-tab-{id}"), cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("closing the archive tab must persist the closed-tab reference")
    };
    assert!(workspace.tabs.is_empty());
    assert_eq!(workspace.closed.as_slice(), std::slice::from_ref(&tab));
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    click(&f, "tool-proxy", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("shift-t"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let restored = read(&mut f, cx);
    assert_eq!(restored.1, reference);
    assert_eq!((restored.2, restored.3), (0, 0));
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.tool == Tool::Repeater);
        assert_eq!(view.workspace.active, Some(id));
        assert_eq!(view.workspace.tabs, [tab]);
        assert!(view.workspace.closed.is_empty());
        assert!(view.sending.is_empty() && view.pending_sends.is_empty());
    });
    reply(&f, &restored, summary(&reference, 3), vec![], cx);
    let body = read(&mut f, cx);
    reply(&f, &body, summary(&reference, 3), b"old".to_vec(), cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).tab_editors[&id]
                .response
                .raw
                .read(cx)
                .value()
                .as_ref(),
            "old"
        )
    });
    assert!(drain(&mut f.commands).is_empty());
}
