use super::*;

#[gpui_kit::test]
fn repeater_convert_selection_preserves_utf8_selection_undo_and_guards_readonly_or_ime(
    cx: &mut TestAppContext,
) {
    let mut first = tab("1");
    let raw_initial =
        "GET /search?q=%E4%B8%AD%E6%96%87&name=a+b HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n";
    first.draft.raw = raw_initial.as_bytes().to_vec();
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let (request_editor, response_editor, target_input) = cx.read(|cx| {
        let editors = &fixture.view.read(cx).tab_editors[&id];
        (
            editors.request.raw.clone(),
            editors.response.raw.clone(),
            editors.target.clone(),
        )
    });

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("repeater-request-raw", cx);
        window.render_frame(cx);
        window.click(("input", request_editor.entity_id()), cx);

        let encoded_cn = "%E4%B8%AD%E6%96%87";
        let start = raw_initial.find(encoded_cn).unwrap();
        let end = start + encoded_cn.len();
        request_editor.update(cx, |state, cx| {
            state.set_selected_range_with_direction(start..end, true, cx);
        });
        let rev_before = request_editor.read(cx).document_revision();

        window.press(crate::kbd!("shift-u"), cx);

        let after_decode = "GET /search?q=中文&name=a+b HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n";
        assert_eq!(request_editor.read(cx).value().as_ref(), after_decode);
        assert_eq!(
            request_editor.read(cx).selected_range(),
            start..start + "中文".len()
        );
        assert_eq!(request_editor.read(cx).selected_value().as_ref(), "中文");
        assert!(request_editor.read(cx).is_selection_reversed());
        assert!(request_editor.read(cx).document_revision() > rev_before);
        assert!(fixture.view.read(cx).tab_editors[&id].dirty);

        window.press(crate::kbd!("u"), cx);
        assert_eq!(request_editor.read(cx).value().as_ref(), raw_initial);
        assert_eq!(request_editor.read(cx).selected_range(), start..end);
        assert_eq!(
            request_editor.read(cx).selected_value().as_ref(),
            encoded_cn
        );

        window.press(crate::kbd!("shift-u"), cx);
        assert_eq!(request_editor.read(cx).value().as_ref(), after_decode);
        assert_eq!(request_editor.read(cx).selected_value().as_ref(), "中文");

        window.press(crate::kbd!("b"), cx);
        let b64_cn = "5Lit5paH";
        let after_b64 = "GET /search?q=5Lit5paH&name=a+b HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n";
        assert_eq!(request_editor.read(cx).value().as_ref(), after_b64);
        assert_eq!(
            request_editor.read(cx).selected_range(),
            start..start + b64_cn.len()
        );
        assert_eq!(request_editor.read(cx).selected_value().as_ref(), b64_cn);

        window.press(crate::kbd!("z"), cx);
        assert_eq!(request_editor.read(cx).value().as_ref(), after_decode);
        assert_eq!(
            request_editor.read(cx).selected_range(),
            start..start + "中文".len()
        );
        assert_eq!(request_editor.read(cx).selected_value().as_ref(), "中文");

        window.press(crate::kbd!("z"), cx);
        assert_eq!(request_editor.read(cx).value().as_ref(), raw_initial);
        assert_eq!(request_editor.read(cx).selected_range(), start..end);
        assert_eq!(
            request_editor.read(cx).selected_value().as_ref(),
            encoded_cn
        );

        let param_start = raw_initial.find("a+b").unwrap();
        let param_end = param_start + 3;
        request_editor.update(cx, |state, cx| {
            state.set_selected_range(param_start..param_end, cx);
        });
        window.press(crate::kbd!("shift-u"), cx);
        let after_param_decode =
            "GET /search?q=%E4%B8%AD%E6%96%87&name=a b HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n";
        assert_eq!(request_editor.read(cx).value().as_ref(), after_param_decode);
        assert_eq!(request_editor.read(cx).selected_value().as_ref(), "a b");
        window.press(crate::kbd!("z"), cx);
        assert_eq!(request_editor.read(cx).value().as_ref(), raw_initial);

        request_editor.update(cx, |state, cx| {
            state.set_selected_range(start..end, cx);
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        });
        let during_ime = request_editor.read(cx).value();
        window.press(crate::kbd!("b"), cx);
        assert_eq!(request_editor.read(cx).value(), during_ime);
        assert!(
            request_editor
                .update(cx, |state, cx| state.marked_text_range(window, cx))
                .is_some()
        );
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
        request_editor.update(cx, |state, cx| {
            state.replace_text_in_range(None, encoded_cn, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let param_start = raw_initial.find("a+b").unwrap();
        let param_end = param_start + 3;

        response_editor.update(cx, |state, cx| {
            state.set_value("HTTP/1.1 200 OK\r\n\r\n%E4%B8%AD%E6%96%87", window, cx);
            state.set_selected_range(19..37, cx);
        });
        window.render_frame(cx);
        window.click(("input", response_editor.entity_id()), cx);
        window.press(crate::kbd!("shift-u"), cx);
        assert_eq!(
            response_editor.read(cx).value().as_ref(),
            "HTTP/1.1 200 OK\r\n\r\n%E4%B8%AD%E6%96%87"
        );
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some("当前消息为只读，不能原地转换选区。")
        );
        let direct_readonly = fixture.view.update(cx, |view, cx| {
            view.tab_editors
                .get_mut(&id)
                .unwrap()
                .request
                .apply_conversion(editing::ConvertKind::UrlDecode, true, window, cx)
        });
        assert_eq!(
            direct_readonly,
            Err("当前消息为只读，不能原地转换选区。".into())
        );

        request_editor.update(cx, |state, cx| {
            state.set_selected_range(param_start..param_end, cx);
        });
        let captured = fixture.view.update(cx, |view, cx| {
            view.error = None;
            view.capture_drafts(window, cx)
        });
        assert_eq!(captured, Ok(false));
        let focus_handle = fixture.view.read(cx).focus.clone();
        focus_handle.focus(window, cx);
        assert!(!request_editor.focus_handle(cx).is_focused(window));
        window.press(crate::kbd!("u"), cx);
        assert_eq!(request_editor.read(cx).value().as_ref(), raw_initial);
        assert_eq!(
            request_editor.read(cx).selected_range(),
            param_start..param_end
        );
        assert!(fixture.view.read(cx).error.is_none());
        assert!(!fixture.view.read(cx).tab_editors[&id].dirty);
        window.click("repeater-edit-target", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = cx.read(|cx| {
        *cx.windows()
            .iter()
            .find(|&&w| w != fixture.window)
            .expect("Repeater target dialog window")
    });
    let dialog_target = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .open_repeater_target_dialog_for_tab(id, cx)
            .expect("Repeater target dialog entity")
            .read(cx)
            .target
            .clone()
    });
    assert_ne!(dialog_target.entity_id(), target_input.entity_id());
    cx.update_window(dialog, |_, window, cx| {
        let param_start = raw_initial.find("a+b").unwrap();
        let param_end = param_start + 3;
        window.render_frame(cx);
        assert!(dialog_target.focus_handle(cx).is_focused(window));
        window.press(crate::kbd!("u"), cx);
        assert_eq!(request_editor.read(cx).value().as_ref(), raw_initial);
        assert_eq!(
            request_editor.read(cx).selected_range(),
            param_start..param_end
        );
        assert!(fixture.view.read(cx).error.is_none());
        assert!(!fixture.view.read(cx).tab_editors[&id].dirty);
        window.click("repeater-target-cancel", cx);
    })
    .unwrap();
    cx.run_until_parked();
}
