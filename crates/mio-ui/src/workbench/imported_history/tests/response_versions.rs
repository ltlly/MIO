use super::*;

#[gpui_kit::test]
fn imported_request_without_declared_version_reports_both_transfer_failures_and_keeps_raw(
    cx: &mut TestAppContext,
) {
    let raw = b"GET /unsupported HTTP/9\r\nX-Repeat: one\r\nX-Repeat: two\r\n\r\n\0\xff";
    for intruder in [false, true] {
        let mut f = fixture(cx, vec![], 64);
        let mut selected = row(0, raw.len() as u64, None);
        let HistoryRow::ExtProxy(archive_row_9) = &mut selected else {
            unreachable!()
        };
        let ExtHistoryRow {
            request: Some(request),
            ..
        } = archive_row_9.as_mut()
        else {
            unreachable!()
        };
        request.declared_version = None;
        let summary = request.clone();
        page(&mut f, vec![selected.clone()], 1, 0, cx);
        click_row(&f, 0, cx);
        let request = read(&mut f);
        fragment(&f, request, summary, raw.to_vec(), cx);
        assert!(drain(&mut f.commands).is_empty());
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                assert!(f.view.read(cx).imported_history_request_ready());
                if intruder {
                    window.press(crate::kbd!("i"), cx);
                } else {
                    window.click("history-request-message-actions", cx);
                    window.render_frame(cx);
                    window.click("history-request-to-repeater", cx);
                }
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            drain(&mut f.commands).is_empty(),
            "an unknown HTTP declaration must not append, send, or begin Intruder pool admission"
        );
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let error = window.find("workspace-error");
                assert!(error.visible());
                assert!(
                    error
                        .label()
                        .is_some_and(|error| error.contains("声明版本"))
                );
                f.view.update(cx, |view, cx| {
                    assert_eq!(view.selected_history, Some(selected.id()));
                    assert_eq!(view.history_request.bytes(window, cx).unwrap(), raw);
                    assert!(
                        view.workspace.tabs.is_empty() && view.workspace.intruder.tabs.is_empty()
                    );
                    assert!(view.pending_sends.is_empty() && view.sending.is_empty());
                });
            })
            .unwrap();
    }
}

fn response_version(f: &Fixture, label: &'static str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("imported-response-version", cx);
            window.render_frame(cx);
            let position = window.find_by_label(label).bounds().center();
            window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
}
fn four_variants() -> (HistoryRow, [ExtMessageSummary; 4]) {
    let mut item = row(0, 3, Some(3));
    let HistoryRow::ExtProxy(archive) = &mut item else {
        unreachable!()
    };
    let original_request = archive.request.clone().unwrap();
    let original_response = archive.response.clone().unwrap();
    let mut edited_request = original_request.clone();
    edited_request.id = Uuid::new_v4();
    edited_request.field_id = 17;
    edited_request.role = "edited_request".into();
    let mut edited_response = original_response.clone();
    edited_response.id = Uuid::new_v4();
    edited_response.field_id = 20;
    edited_response.role = "edited_response".into();
    archive.edited_request = Some(edited_request.clone());
    archive.edited_response = Some(edited_response.clone());
    (
        item,
        [
            original_request,
            edited_request,
            original_response,
            edited_response,
        ],
    )
}
fn assert_pair(f: &Fixture, request: &str, response: &str, cx: &mut TestAppContext) {
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.history_request.raw.read(cx).value().as_ref(), request);
        assert_eq!(
            view.history_response.raw.read(cx).value().as_ref(),
            response
        );
    });
}

#[gpui_kit::test]
fn request_and_response_versions_are_independent_and_response_switch_keeps_request_transfer(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let (item, [request, edited_request, response, edited_response]) = four_variants();
    page(&mut f, vec![item], 1, 0, cx);
    click_row(&f, 0, cx);
    let first = read(&mut f);
    assert_eq!(first.1, request.id);
    fragment(&f, first, request.clone(), b"req".to_vec(), cx);
    let first_response = read(&mut f);
    assert_eq!(first_response.1, response.id);
    fragment(&f, first_response, response.clone(), b"res".to_vec(), cx);
    assert_pair(&f, "req", "res", cx);
    response_version(&f, "已编辑响应", cx);
    let edited_read = read(&mut f);
    fragment(
        &f,
        edited_read,
        edited_response.clone(),
        b"RES".to_vec(),
        cx,
    );
    assert_pair(&f, "req", "RES", cx);
    request_version(&f, "已编辑请求", cx);
    let edited_read = read(&mut f);
    fragment(&f, edited_read, edited_request.clone(), b"REQ".to_vec(), cx);
    assert_pair(&f, "REQ", "RES", cx);
    response_version(&f, "原始响应", cx);
    let original_read = read(&mut f);
    fragment(&f, original_read, response, b"res".to_vec(), cx);
    assert_pair(&f, "REQ", "res", cx);
    request_version(&f, "原始请求", cx);
    let original_read = read(&mut f);
    fragment(&f, original_read, request.clone(), b"req".to_vec(), cx);
    assert_pair(&f, "req", "res", cx);
    assert!(drain(&mut f.commands).is_empty());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("history-request-message-actions", cx);
            window.render_frame(cx);
            window.click("history-request-to-repeater", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let transfer = transfer_command(&mut f);
    let transfer_revision = cx.read(|cx| f.view.read(cx).history_detail_revision);
    response_version(&f, "已编辑响应", cx);
    let pending_response = read(&mut f);
    assert_eq!(pending_response.1, edited_response.id);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.history_detail_revision, transfer_revision);
        assert_eq!(view.imported_history_request().unwrap().id, request.id);
        assert!(view.imported_history_request_ready());
    });
    let added = transfer_proposal(&f, &transfer, b"req", cx);
    assert!(
        matches!(drain(&mut f.commands).as_slice(), [Command::SaveWorkspace { workspace, .. }] if workspace.tabs == [added])
    );
}

#[gpui_kit::test]
fn failed_request_does_not_block_response_and_cross_side_switches_reject_late_variants(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let (item, [request, edited_request, response, edited_response]) = four_variants();
    page(&mut f, vec![item], 1, 0, cx);
    click_row(&f, 0, cx);
    let failed_request = read(&mut f);
    response_version(&f, "已编辑响应", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "switching response while request is in flight must not start a second read"
    );
    f.events
        .try_send(Event::ExtMessage {
            project: f.project,
            selection_revision: failed_request.0,
            message_id: failed_request.1,
            offset: 0,
            result: Err("request read cancelled".into()),
        })
        .unwrap();
    cx.run_until_parked();
    let old_response = read(&mut f);
    assert_eq!(old_response.1, edited_response.id);
    request_version(&f, "已编辑请求", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "request switch must retain in-flight response"
    );
    response_version(&f, "原始响应", cx);
    let current_request = read(&mut f);
    assert_eq!(current_request.1, edited_request.id);
    fragment(
        &f,
        old_response,
        edited_response.clone(),
        b"OLD".to_vec(),
        cx,
    );
    fragment(&f, failed_request, request, b"old".to_vec(), cx);
    assert!(drain(&mut f.commands).is_empty());
    fragment(&f, current_request, edited_request, b"REQ".to_vec(), cx);
    let current_response = read(&mut f);
    assert_eq!(current_response.1, response.id);
    fragment(&f, current_response, response, b"res".to_vec(), cx);
    assert_pair(&f, "REQ", "res", cx);
    response_version(&f, "已编辑响应", cx);
    let failed_response = read(&mut f);
    f.events
        .try_send(Event::ExtMessage {
            project: f.project,
            selection_revision: failed_response.0,
            message_id: failed_response.1,
            offset: 0,
            result: Err("response unavailable".into()),
        })
        .unwrap();
    cx.run_until_parked();
    request_version(&f, "原始请求", cx);
    let original_request = read(&mut f);
    cx.read(|cx| {
        assert!(
            f.view
                .read(cx)
                .imported_history
                .response_error
                .as_ref()
                .unwrap()
                .message
                .contains("response unavailable")
        )
    });
    assert_ne!(original_request.1, edited_response.id);
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn both_oversized_messages_share_one_working_archive_entry_without_reading_payloads(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let item = row(0, MAX_MESSAGE_BYTES + 1, Some(MAX_MESSAGE_BYTES + 2));
    page(&mut f, vec![item.clone()], 1, 0, cx);
    click_row(&f, 0, cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "neither oversized message may begin a payload read"
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let entries = window.find_all("imported-history-open-archive");
            assert_eq!(
                entries.len(),
                1,
                "both oversized sides must share exactly one archive action identity"
            );
            assert!(entries[0].visible());
            assert!(!entries[0].disabled().unwrap_or(false));
            assert_eq!(entries[0].label(), Some("查看 外部导入记录"));
            let view = f.view.read(cx);
            assert!(
                view.imported_history
                    .request_error
                    .as_ref()
                    .unwrap()
                    .oversized
            );
            assert!(
                view.imported_history
                    .response_error
                    .as_ref()
                    .unwrap()
                    .oversized
            );
            assert_eq!(view.imported_history.row.as_ref(), Some(&item));
            assert!(view.history_request.raw.read(cx).value().is_empty());
            assert!(view.history_response.raw.read(cx).value().is_empty());
            window.click("imported-history-open-archive", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryExtReport { project, .. }] if *project == f.project),
        "the shared archive entry must query the import report; observed {} commands",
        commands.len()
    );
}

#[gpui_kit::test]
fn notes_only_history_refresh_keeps_loaded_hex_views_focus_and_editor_revisions(
    cx: &mut TestAppContext,
) {
    let request_bytes = b"GET /notes-only HTTP/1.1\r\n\r\n\0\xff";
    let response_bytes = b"HTTP/1.1 200 OK\r\n\r\n\xff\0";
    let mut f = fixture(cx, vec![], 64);
    let mut selected = row(
        0,
        request_bytes.len() as u64,
        Some(response_bytes.len() as u64),
    );
    let HistoryRow::ExtProxy(archive) = &selected else {
        unreachable!()
    };
    let request = archive.request.clone().unwrap();
    let response = archive.response.clone().unwrap();
    page(&mut f, vec![selected.clone()], 1, 0, cx);
    click_row(&f, 0, cx);
    let read_request = read(&mut f);
    fragment(&f, read_request, request, request_bytes.to_vec(), cx);
    let read_response = read(&mut f);
    fragment(&f, read_response, response, response_bytes.to_vec(), cx);
    assert!(drain(&mut f.commands).is_empty());
    let (request_revision, response_revision) = f
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let response = f.view.read(cx).history_response.hex.clone();
            window
                .within(("hex-grid-view", response.entity_id()))
                .click(("hex-byte-cell", 0usize), cx);
            window.press(crate::kbd!("a"), cx);
            let view = f.view.read(cx);
            assert!(view.history_response.hex_grid_has_focus(window, cx));
            assert_eq!(view.history_request.view(), MessageView::Hex);
            assert_eq!(view.history_response.view(), MessageView::Hex);
            (
                view.history_request.hex.read(cx).document_revision(),
                response.read(cx).document_revision(),
            )
        })
        .unwrap();
    let HistoryRow::ExtProxy(archive) = &mut selected else {
        unreachable!()
    };
    archive.notes_revision = 1;
    archive.notes_preview = "only notes changed".into();
    archive.time_unix_ms = Some(1794126317491);
    archive.time_error = None;
    let query_revision = cx.read(|cx| f.view.read(cx).query_revision);
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision,
            offset: 0,
            total: 1,
            rows: vec![selected.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut f.commands).is_empty(),
        "notes/time metadata refresh must not re-read any archived request or response fragments"
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            f.view.update(cx, |view, cx| {
                assert_eq!(view.selected_history, Some(selected.id()));
                assert_eq!(view.history_request.view(), MessageView::Hex);
                assert_eq!(view.history_response.view(), MessageView::Hex);
                assert_eq!(
                    view.history_request.hex.read(cx).document_revision(),
                    request_revision
                );
                assert_eq!(
                    view.history_response.hex.read(cx).document_revision(),
                    response_revision
                );
                assert!(view.history_response.hex_grid_has_focus(window, cx));
                assert_eq!(
                    view.history_request.bytes(window, cx).unwrap(),
                    request_bytes
                );
                assert_eq!(
                    view.history_response.bytes(window, cx).unwrap(),
                    response_bytes
                );
                assert_eq!(view.imported_history.row.as_ref(), Some(&selected));
            });
        })
        .unwrap();
}
