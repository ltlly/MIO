use super::*;

pub(in crate::workbench) fn request_version(
    f: &Fixture,
    label: &'static str,
    cx: &mut TestAppContext,
) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("imported-request-version", cx);
            window.render_frame(cx);
            let position = window.find_by_label(label).bounds().center();
            window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn imported_request_version_menu_defaults_original_preserves_response_and_transfers_selected_bytes(
    cx: &mut TestAppContext,
) {
    let original = b"GET /original HTTP/1.1\r\nX-Repeat: one\r\nX-Repeat: two\r\n\r\n";
    let edited = b"GET /edited HTTP/1.1\r\nX-Edited: yes\r\n\r\n\0\xff";
    let response = b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\n\r\n\0\xffR";
    let mut item = row(3, original.len() as u64, Some(response.len() as u64));
    let HistoryRow::ExtProxy(archive_row_8) = &mut item else {
        unreachable!()
    };
    let ExtHistoryRow {
        request: Some(request),
        edited_request,
        response: Some(reply),
        ..
    } = archive_row_8.as_mut()
    else {
        unreachable!()
    };
    let original_summary = request.clone();
    let response_summary = reply.clone();
    let mut edited_summary = request.clone();
    edited_summary.id = Uuid::new_v4();
    edited_summary.field_id = 17;
    edited_summary.role = "edited_request".into();
    edited_summary.request_target = Some("/edited".into());
    edited_summary.raw_bytes = edited.len() as u64;
    *edited_request = Some(edited_summary.clone());
    let mut f = fixture(cx, vec![], 64);
    page(&mut f, vec![item.clone()], 1, 0, cx);
    click_row(&f, 0, cx);
    let original_read = read(&mut f);
    assert_eq!(original_read.1, original_summary.id);
    fragment(
        &f,
        original_read,
        original_summary.clone(),
        original.to_vec(),
        cx,
    );
    let response_read = read(&mut f);
    assert_eq!(response_read.1, response_summary.id);
    fragment(&f, response_read, response_summary, response.to_vec(), cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "loading a row must not prefetch its unselected request version"
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.imported_history_request().unwrap().id,
            original_summary.id
        );
        assert!(view.imported_history_request_ready());
        assert!(!view.imported_history.request_edited);
    });
    request_version(&f, "已编辑请求", cx);
    let superseded_edited = read(&mut f);
    assert_eq!(superseded_edited.1, edited_summary.id);
    request_version(&f, "原始请求", cx);
    let superseded_original = read(&mut f);
    assert_eq!(superseded_original.1, original_summary.id);
    request_version(&f, "已编辑请求", cx);
    let current = read(&mut f);
    assert!(current.0 > superseded_original.0 && superseded_original.0 > superseded_edited.0);
    fragment(
        &f,
        superseded_original,
        original_summary,
        original.to_vec(),
        cx,
    );
    fragment(
        &f,
        superseded_edited,
        edited_summary.clone(),
        edited.to_vec(),
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                assert!(!view.imported_history_request_ready());
                assert!(view.history_request.raw.read(cx).value().is_empty());
                assert_eq!(view.history_response.bytes(window, cx).unwrap(), response);
            });
        })
        .unwrap();
    fragment(&f, current, edited_summary.clone(), edited.to_vec(), cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "switching the request version must not re-read the response"
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            f.view.update(cx, |view, cx| {
                assert_eq!(view.selected_history, Some(item.id()));
                assert_eq!(
                    view.imported_history_request().unwrap().id,
                    edited_summary.id
                );
                assert!(view.imported_history_request_ready());
                assert_eq!(view.history_request.bytes(window, cx).unwrap(), edited);
                assert_eq!(view.history_response.bytes(window, cx).unwrap(), response);
                assert_eq!(view.history_request.view, MessageView::Hex);
            });
            window.click("history-request-message-actions", cx);
            window.render_frame(cx);
            window.click("history-request-to-repeater", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let command = transfer_command(&mut f);
    assert!(
        matches!(&command, Command::OpenExtRepeater { message_id, .. } if *message_id == edited_summary.id)
    );
    let tab = transfer_proposal(&f, &command, edited, cx);
    let commands = drain(&mut f.commands);
    assert!(
        matches!(commands.as_slice(), [Command::SaveWorkspace { workspace, .. }] if workspace.tabs == [tab])
    );
}
