use gpui_kit::{MouseButton, TestAppContext, size};
use gpui_kit::test::{TestEventExt, TestWindowExt};

use super::*;
use crate::workbench::tests::{drain, fixture, record, tab};
use crate::workbench::wb_render_chrome::sidebar_rail_rotated_label_svg;
use mio_core::{Header, HttpResponse, HttpVersion, InspectorSection, inspect_http_messages};

fn multipart_fixture(boundary: &str) -> Vec<u8> {
    let binary = b"\x89PNG\r\n\x1a\n\x00\xff\xfeBIN";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"username\"\r\n\r\nalice\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n",
    );
    body.extend_from_slice(binary);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut raw = format!(
        "POST /upload HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(&body);
    raw
}

#[gpui_kit::test]
fn inspector_ui_edits_json_leaves_multipart_binary_and_selection_writeback(
    cx: &mut TestAppContext,
) {
    let mut initial = tab("Inspector结构化");
    initial.draft.raw = b"POST /api HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Type: application/json\r\nContent-Length: 39\r\n\r\n{\"user\":{\"name\":\"alice\"},\"items\":[\"a\"]}".to_vec();
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);
    let tab_id = cx.read(|cx| f.view.read(cx).workspace.tabs[0].id);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("inspector-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert_eq!(
            window.find("inspector-section-body").bounds().size,
            size(px(343.), px(38.))
        );
        assert!(window.find("inspector-section-body").visible());
        assert!(window.try_find("inspector-target-body").is_none());
        assert!(window.try_find("inspector-item-body-0").is_none());
        window.click("inspector-section-body", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-target-body").visible());
        window.click("inspector-item-body-0", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            assert_eq!(
                view.inspector.name_input.read(cx).value().as_ref(),
                "user.name"
            );
            assert_eq!(
                view.inspector.value_input.read(cx).value().as_ref(),
                "alice"
            );
            view.inspector.value_input.update(cx, |input, cx| {
                input.set_value("bob".to_owned(), window, cx);
            });
        });
        window.click("inspector-apply-button", cx);
        window.click("inspector-target-body", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.inspector.name_input.update(cx, |input, cx| {
                input.set_value("items[1]".to_owned(), window, cx);
            });
            view.inspector.value_input.update(cx, |input, cx| {
                input.set_value("42".to_owned(), window, cx);
            });
        });
        window.click("inspector-add-button", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let raw = f.view.read(cx).workspace.tabs[0].draft.raw.clone();
        let data = inspect_http_messages(&raw, None);
        assert!(
            data.body_params
                .iter()
                .any(|item| item.name == "user.name" && item.value == "bob")
        );
        assert!(
            data.body_params
                .iter()
                .any(|item| item.name == "items[1]" && item.value == "42")
        );
    });

    let boundary = "----MioUiBoundary123";
    let multipart = multipart_fixture(boundary);
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            let tab = view
                .workspace
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .unwrap();
            tab.draft.raw = multipart.clone();
            tab.draft.revision += 1;
            if let Some(editors) = view.tab_editors.get_mut(&tab_id) {
                editors.request.load(&multipart, window, cx);
                editors.loaded_revision = tab.draft.revision;
                editors.dirty = false;
            }
        });
        window.render_frame(cx);
        window.click("inspector-item-body-0", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.inspector.value_input.update(cx, |input, cx| {
                input.set_value("charlie".to_owned(), window, cx);
            });
        });
        window.click("inspector-apply-button", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.inspector.editing_section = InspectorSection::BodyParams;
            view.inspector.name_input.update(cx, |input, cx| {
                input.set_value("role".to_owned(), window, cx);
            });
            view.inspector.value_input.update(cx, |input, cx| {
                input.set_value("admin".to_owned(), window, cx);
            });
        });
        window.click("inspector-add-button", cx);
        window.render_frame(cx);
        window.click("inspector-remove-body-0", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let raw = f.view.read(cx).workspace.tabs[0].draft.raw.clone();
        let binary = b"\x89PNG\r\n\x1a\n\x00\xff\xfeBIN";
        assert!(raw.windows(binary.len()).any(|w| w == binary));
        assert!(
            raw.windows(format!("--{boundary}--").len())
                .any(|w| w == format!("--{boundary}--").as_bytes())
        );
        let data = inspect_http_messages(&raw, None);
        assert!(data.body_params.iter().any(|item| item.name == "avatar"));
        assert!(
            data.body_params
                .iter()
                .any(|item| item.name == "role" && item.value == "admin")
        );
    });

    cx.update_window(f.window, |_, window, cx| {
        let selection_raw =
            b"GET /s?tok=eyJ1c2VyIjoiYWxpY2UifQ%3D%3D HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n";
        f.view.update(cx, |view, cx| {
            let tab = view
                .workspace
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .unwrap();
            tab.draft.raw = selection_raw.to_vec();
            tab.draft.revision += 1;
            if let Some(editors) = view.tab_editors.get_mut(&tab_id) {
                editors.request.load(selection_raw, window, cx);
                editors.loaded_revision = tab.draft.revision;
                editors.dirty = false;
            }
        });
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get(&tab_id).unwrap();
            editors
                .request
                .raw
                .update(cx, |state, cx| state.set_selected_range(11..39, cx));
        });
        window.render_frame(cx);
        assert!(window.find("inspector-section-selection").visible());
        assert!(window.try_find("inspector-selection-item").is_none());
        window.click("inspector-section-selection", cx);
        window.render_frame(cx);
        window.click("inspector-selection-item", cx);
        window.render_frame(cx);
        let decoded = f
            .view
            .read(cx)
            .inspector
            .selection_editor
            .read(cx)
            .value()
            .to_string();
        assert_eq!(decoded, r#"{"user":"alice"}"#);
        f.view.update(cx, |view, cx| {
            view.inspector.selection_editor.update(cx, |state, cx| {
                state.set_value(r#"{"user":"bob"}"#, window, cx)
            });
        });
        window.click("inspector-selection-apply", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let raw = f.view.read(cx).workspace.tabs[0].draft.raw.clone();
        assert!(
            String::from_utf8(raw.clone())
                .unwrap()
                .contains("tok=eyJ1c2VyIjoiYm9iIn0%3D")
        );
    });
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get(&tab_id).unwrap();
            editors
                .request
                .raw
                .update(cx, |state, cx| state.focus(window, cx));
        });
        window.render_frame(cx);
        window.press(crate::kbd!("z"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.read(|cx| {
        let raw_editor = f
            .view
            .read(cx)
            .tab_editors
            .get(&tab_id)
            .unwrap()
            .request
            .raw
            .clone();
        let text = raw_editor.read(cx).value().to_string();
        assert!(text.contains("tok=eyJ1c2VyIjoiYWxpY2UifQ%3D%3D"));
    });
}

#[gpui_kit::test]
fn inspector_edits_response_headers_during_proxy_response_intercept(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("1")], 64);
    drain(&mut f.commands);
    let mut rec = record("http://127.0.0.1:9001/resp-intercept", "");
    let flow_id = rec.summary.id;
    rec.summary.status = Some(200);
    rec.response = Some(HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Content-Type".into(),
                value: b"text/plain".to_vec(),
            },
            Header {
                name: "X-Remove".into(),
                value: b"legacy".to_vec(),
            },
        ],
        body: b"intercepted-body".to_vec(),
        trailers: Vec::new(),
    });
    f.events
        .try_send(Event::InterceptPending {
            project: f.project,
            stage: InterceptStage::Response,
            response_head_only: false,
            record: Box::new(rec),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.click("proxy-intercept", cx);
        window.click("inspector-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert!(window.try_find("inspector-protocol-row").is_none());
        assert!(window.try_find("inspector-target-response-headers").is_none());
        assert!(window.try_find("inspector-target-query").is_none());
        assert!(window.try_find("inspector-item-response-headers-0").is_none());
        window.click("inspector-section-response-headers", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-target-response-headers").visible());
        assert!(window.try_find("inspector-target-query").is_none());

        window.click("inspector-item-response-headers-0", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            assert_eq!(
                view.inspector.name_input.read(cx).value().as_ref(),
                "Content-Type"
            );
            view.inspector.value_input.update(cx, |input, cx| {
                input.set_value("application/json".to_owned(), window, cx)
            });
        });
        window.click("inspector-apply-button", cx);
        window.render_frame(cx);

        window.click("inspector-remove-response-headers-1", cx);
        window.render_frame(cx);

        window.click("inspector-target-response-headers", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.inspector.name_input.update(cx, |input, cx| {
                input.set_value("X-Proxy-Edited".to_owned(), window, cx)
            });
            view.inspector.value_input.update(cx, |input, cx| {
                input.set_value("yes".to_owned(), window, cx)
            });
        });
        window.click("inspector-add-button", cx);
        window.render_frame(cx);
        window.click("intercept-forward", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let forwarded = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Decide {
                flow_id: id,
                decision: Decision::ForwardResponse(resp),
                ..
            } if id == flow_id => Some(resp),
            _ => None,
        })
        .expect("forwarding held response after Inspector edits must emit ForwardResponse");
    assert_eq!(forwarded.body, b"intercepted-body");
    assert_eq!(
        forwarded.headers,
        vec![
            Header {
                name: "Content-Type".into(),
                value: b"application/json".to_vec(),
            },
            Header {
                name: "X-Proxy-Edited".into(),
                value: b"yes".to_vec(),
            },
        ]
    );
}

#[gpui_kit::test]
fn inspector_readonly_response_drawer_remove_index_shift_owner_reset_and_ime_guards(
    cx: &mut TestAppContext,
) {
    let mut first = tab("Tab1");
    first.draft.raw = b"GET /items?a=1&b=2&c=3 HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let mut second = tab("Tab2");
    second.draft.raw = b"GET /other?x=9&y=8 HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let mut f = fixture(cx, vec![first, second], 64);
    drain(&mut f.commands);
    let (tab1, tab2) = cx.read(|cx| {
        let tabs = &f.view.read(cx).workspace.tabs;
        (tabs[0].id, tabs[1].id)
    });

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get_mut(&tab1).unwrap();
            editors.response.load(
                b"HTTP/1.1 200 OK\r\nServer: mio-test\r\nX-Trace: abc\r\n\r\nok",
                window,
                cx,
            );
        });
        window.click("inspector-toggle", cx);
        window.render_frame(cx);

        // 1. Clicking read-only ResponseHeaders row in editable Repeater opens read-only detail drawer,
        // NOT an editable QueryParams add form; clicking again closes it.
        assert!(window.try_find("inspector-item-response-headers-0").is_none());
        window.click("inspector-section-response-headers", cx);
        window.render_frame(cx);
        window.click("inspector-item-response-headers-0", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-copy-value").visible());
        assert!(window.try_find("inspector-add-button").is_none());
        assert!(window.try_find("inspector-apply-button").is_none());
        assert_eq!(
            f.view
                .read(cx)
                .inspector
                .name_input
                .read(cx)
                .value()
                .as_ref(),
            "Server"
        );
        window.click("inspector-item-response-headers-0", cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-copy-value").is_none());

        // 2. Selecting QueryParams item #1 ("b=2") and removing earlier item #0 ("a=1")
        // decrements editing_index to Some(0) so saving updates "b" rather than shifting to "c".
        assert!(window.try_find("inspector-item-query-1").is_none());
        window.click("inspector-section-query", cx);
        window.render_frame(cx);
        window.click("inspector-item-query-1", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).inspector.editing_index, Some(1));
        window.click("inspector-remove-query-0", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).inspector.editing_index, Some(0));
        f.view.update(cx, |view, cx| {
            view.inspector
                .value_input
                .update(cx, |s, cx| s.set_value("22", window, cx));
        });
        window.click("inspector-apply-button", cx);
        window.render_frame(cx);
        let updated_q =
            String::from_utf8(f.view.read(cx).workspace.tabs[0].draft.raw.clone()).unwrap();
        assert!(
            updated_q.contains("b=22") && updated_q.contains("c=3") && !updated_q.contains("a=1")
        );

        // 3. IME composition inside Inspector input blocks closing Inspector; once committed,
        // closing transfers focus back to Workbench.focus.
        f.view.update(cx, |view, cx| {
            view.inspector.value_input.update(cx, |s, cx| {
                s.focus(window, cx);
                s.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            });
        });
        window.render_frame(cx);
        window.click("inspector-close", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert_eq!(
            f.view.read(cx).error.as_deref(),
            Some(crate::editing::COMPOSITION_PENDING)
        );
        f.view.update(cx, |view, cx| {
            view.error = None;
            view.inspector.value_input.update(cx, |s, cx| {
                s.unmark_text(window, cx);
                s.focus(window, cx);
            });
        });
        window.click("inspector-close", cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-sidebar").is_none());
        assert!(f.view.read(cx).focus.is_focused(window));

        // 4. Switching active Repeater tab resets stale detail drawer state.
        window.click("inspector-toggle", cx);
        window.render_frame(cx);
        window.click("inspector-item-query-0", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).inspector.editor_open);
        f.view
            .update(cx, |view, cx| view.select_tab(tab2, window, cx));
        window.render_frame(cx);
        assert!(!f.view.read(cx).inspector.editor_open);
        assert_eq!(f.view.read(cx).inspector.editing_index, None);

        // 5. Combined layout on Response sub-tab ignores hidden Request selection.
        window.click("repeater-layout-combined", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get_mut(&tab2).unwrap();
            editors
                .request
                .raw
                .update(cx, |s, cx| s.set_selected_range(11..14, cx));
        });
        window.render_frame(cx);
        assert!(f.view.read(cx).inspector.selection_preview.is_some());
        window.click("repeater-combined-resp", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).inspector.selection_preview.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn inspector_supports_pretty_edits_protocol_switch_reorder_repeater_notes_and_ws_headers(
    cx: &mut TestAppContext,
) {
    let mut first = tab("PrettyTab");
    first.draft.raw =
        b"POST /api?first=1&second=2 HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Type: application/json\r\nContent-Length: 14\r\n\r\n{\"ok\":\"hello\"}".to_vec();
    let mut second = tab("NotesTab");
    second.draft.raw = b"GET /second HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let mut f = fixture(cx, vec![first, second], 64);
    drain(&mut f.commands);
    let (tab1, tab2) = cx.read(|cx| {
        let tabs = &f.view.read(cx).workspace.tabs;
        (tabs[0].id, tabs[1].id)
    });

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get_mut(&tab1).unwrap();
            editors.request.switch_pretty(window, cx).unwrap();
            assert!(editors.request.pretty_active());
        });
        window.click("inspector-toggle", cx);
        window.render_frame(cx);

        // 1. Pretty 视图下可直接交换查询参数顺序并保持 Pretty 视图状态
        assert!(window.try_find("inspector-move-down-query-0").is_none());
        window.click("inspector-section-query", cx);
        window.render_frame(cx);
        window.click("inspector-move-down-query-0", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).tab_editors[&tab1].request.pretty_active());
        let raw_after_move =
            String::from_utf8(f.view.read(cx).workspace.tabs[0].draft.raw.clone()).unwrap();
        assert!(raw_after_move.starts_with("POST /api?second=2&first=1 HTTP/1.1\r\n"));

        assert_eq!(
            window.find("inspector-section-attributes").label(),
            Some("Request attributes (2)")
        );
        assert!(window.try_find("inspector-protocol-row").is_none());
        window.click("inspector-section-attributes", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-protocol-row").visible());
        assert_eq!(
            window.find("inspector-protocol-HTTP/1").checked(),
            Some(true)
        );
        assert_eq!(
            window.find("inspector-protocol-HTTP/2").checked(),
            Some(false)
        );
        assert_eq!(
            window.find("inspector-item-attributes-0").label(),
            Some("Method: POST")
        );
        assert_eq!(
            window.find("inspector-item-attributes-1").label(),
            Some("Path: /api")
        );
        assert!(window.try_find("inspector-item-attributes-2").is_none());
        window.click("inspector-protocol-HTTP/2", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("inspector-protocol-HTTP/1").checked(),
            Some(false)
        );
        assert_eq!(
            window.find("inspector-protocol-HTTP/2").checked(),
            Some(true)
        );
        assert_eq!(
            f.view.read(cx).workspace.tabs[0].draft.version,
            HttpVersion::Http2
        );
        let raw_after_proto =
            String::from_utf8(f.view.read(cx).workspace.tabs[0].draft.raw.clone()).unwrap();
        assert!(raw_after_proto.starts_with("POST /api?second=2&first=1 HTTP/2\r\n"));

        // 3. 切回 Raw 视图选中明文，通过手动编码按钮转换为 Base64 并直接写回报文
        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get_mut(&tab1).unwrap();
            editors.request.switch(MessageView::Raw, window, cx).unwrap();
            let raw_text = editors.request.raw.read(cx).value().to_string();
            let pos = raw_text.find("hello").unwrap();
            editors
                .request
                .raw
                .update(cx, |s, cx| s.set_selected_range(pos..pos + 5, cx));
        });
        window.render_frame(cx);
        assert!(window.try_find("inspector-selection-convert-menu").is_none());
        window.click("inspector-section-selection", cx);
        window.render_frame(cx);
        window.click("inspector-selection-convert-menu", cx);
        window.render_frame(cx);
        let b64_pos = window
            .within("popup-menu")
            .find_by_label("Base64 编码")
            .bounds()
            .center();
        window.pointer_down(b64_pos, MouseButton::Left, cx);
        window.pointer_up(b64_pos, MouseButton::Left, cx);
        window.render_frame(cx);
        assert_eq!(
            f.view
                .read(cx)
                .inspector
                .selection_editor
                .read(cx)
                .value()
                .as_ref(),
            "aGVsbG8="
        );
        window.click("inspector-selection-apply", cx);
        window.render_frame(cx);
        let raw_after_wb =
            String::from_utf8(f.view.read(cx).workspace.tabs[0].draft.raw.clone()).unwrap();
        assert!(raw_after_wb.contains("{\"ok\":\"aGVsbG8=\"}"));

        // 4. Repeater Notes 侧边栏与 Inspector 互斥展开，并在标签切换时自动同步 tab_notes
        window.click("repeater-notes-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("repeater-notes-sidebar").visible());
        assert!(window.try_find("inspector-sidebar").is_none());
        f.view.update(cx, |view, cx| {
            view.inspector.tab_notes_editor.update(cx, |ed, cx| {
                ed.set_value("重点参数备注", window, cx);
                ed.set_selected_range(0.."重点参数备注".len(), cx);
            });
        });
        window.click("repeater-notes-bold", cx);
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).workspace.tab_notes.get(&tab1).map(String::as_str),
            Some("**重点参数备注**")
        );
        f.view
            .update(cx, |view, cx| view.select_tab(tab2, window, cx));
        window.render_frame(cx);
        assert_eq!(
            f.view
                .read(cx)
                .inspector
                .tab_notes_editor
                .read(cx)
                .value()
                .as_ref(),
            ""
        );
        f.view
            .update(cx, |view, cx| view.select_tab(tab1, window, cx));
        window.render_frame(cx);
        assert_eq!(
            f.view
                .read(cx)
                .inspector
                .tab_notes_editor
                .read(cx)
                .value()
                .as_ref(),
            "**重点参数备注**"
        );

        // 5. WebSocket Repeater 握手标头在 Inspector 中新增与上下移动
        let ws_id = mio_core::Uuid::new_v4();
        f.view.update(cx, |view, cx| {
            view.workspace.active = None;
            view.workspace.websocket_tabs.push(mio_core::WebSocketRepeaterTab {
                id: ws_id,
                title: "WS-1".into(),
                group: None,
                url: "ws://127.0.0.1:9001/ws".into(),
                handshake_headers: vec![mio_core::Header {
                    name: "Origin".into(),
                    value: b"http://127.0.0.1:9001".to_vec(),
                }],
                source_flow_id: None,
                source_message_id: None,
                direction: mio_core::WebSocketDirection::ClientToServer,
                draft_revision: 1,
                draft_payload: b"ping".to_vec(),
                draft_kind: mio_core::WebSocketKind::Text,
                history: Vec::new(),
                closed: false,
            });
            view.workspace.active_websocket_tab = Some(ws_id);
            view.sync_websocket_repeater_editors(window, cx);
        });
        window.click("inspector-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert!(window.try_find("inspector-target-headers").is_none());
        window.click("inspector-section-request-headers", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-target-headers").visible());
        window.click("inspector-target-headers", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.inspector
                .name_input
                .update(cx, |s, cx| s.set_value("Authorization", window, cx));
            view.inspector
                .value_input
                .update(cx, |s, cx| s.set_value("Bearer ws-tok", window, cx));
        });
        window.click("inspector-add-button", cx);
        window.render_frame(cx);
        window.click("inspector-move-up-request-headers-1", cx);
        window.render_frame(cx);
        let headers = f.view.read(cx).workspace.websocket_tabs[0]
            .handshake_headers
            .clone();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].name, "Authorization");
        assert_eq!(headers[1].name, "Origin");

        // 6. 顶栏左右停靠切换与全部收起/展开按钮几何位置与行为验证
        window.click("inspector-collapse-all", cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-item-request-headers-0").is_none());
        window.click("inspector-expand-all", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-item-request-headers-0").visible());
        let right_dock_bounds = window.find("inspector-sidebar").bounds();
        let dock_left_btn = window.find("inspector-dock-left").bounds();
        let dock_right_btn = window.find("inspector-dock-right").bounds();
        assert_eq!(dock_left_btn.size.width, px(25.));
        assert_eq!(dock_right_btn.size.width, px(25.));
        assert_eq!(dock_left_btn.size.height, px(19.));
        window.click("inspector-dock-left", cx);
        window.render_frame(cx);
        let left_dock_bounds = window.find("inspector-sidebar").bounds();
        assert!(left_dock_bounds.origin.x < right_dock_bounds.origin.x);
        assert_eq!(left_dock_bounds.size.width, px(343.));
        window.click("inspector-dock-right", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inspector-sidebar").bounds().origin.x, right_dock_bounds.origin.x);
    })
    .unwrap();
}

#[gpui_kit::test]
fn inspector_default_pretty_form_13_params_protocol_buttons_and_readonly_protection(
    cx: &mut TestAppContext,
) {
    let form_body = "domain=api-gateway.example.test static-cdn.example.test&appName=com.example.demoapp&netType=UNKNOWN&t=1700000000000&abStrategy=&secData=sampleEncodedPayload%2F001%3D&stackType=4&signType=noSec&st=1700000000000&platform=HARMONY&rm=l&sv=1&sign=1";
    let form_raw = format!(
        "POST /api/dispatchService?appkey=10000001&v=6.5&appVersion=1.0.0&cv=1 HTTP/1.1\r\nHost: api.example.test\r\nCookie: sid=1\r\nAccept: */*\r\nContent-Length: {}\r\nContent-Type: application/x-www-form-urlencoded\r\nAccept-Encoding: gzip, deflate, br\r\nConnection: keep-alive\r\n\r\n{form_body}",
        form_body.len()
    )
    .into_bytes();

    let mut initial = tab("FormPrettyTab");
    initial.draft.raw = form_raw.clone();
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);
    let tab_id = cx.read(|cx| f.view.read(cx).workspace.tabs[0].id);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("inspector-toggle", cx);
        window.render_frame(cx);

        let req_editor = &f.view.read(cx).tab_editors[&tab_id].request;
        assert!(req_editor.pretty_active());
        let pretty_display = req_editor.raw.read(cx).value().to_string();
        assert!(pretty_display.ends_with(form_body));
        assert_eq!(req_editor.bytes_lossless(cx), form_raw);

        assert_eq!(
            window.find("inspector-section-attributes").label(),
            Some("Request attributes (2)")
        );
        assert_eq!(
            window.find("inspector-section-query").label(),
            Some("Request query parameters (4)")
        );
        assert_eq!(
            window.find("inspector-section-body").label(),
            Some("Request body parameters (13)")
        );

        assert!(window.try_find("inspector-protocol-row").is_none());
        window.click("inspector-section-body", cx);
        window.render_frame(cx);
        for idx in 0..13 {
            assert!(
                window
                    .find(SharedString::from(format!("inspector-item-body-{idx}")))
                    .visible()
            );
        }
        assert!(window.try_find("inspector-item-body-13").is_none());
        assert_eq!(
            window.find("inspector-item-body-9").label(),
            Some("platform: HARMONY")
        );
        assert_eq!(
            window.find("inspector-item-body-12").label(),
            Some("sign: 1")
        );

        window.click("inspector-item-body-9", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.inspector
                .value_input
                .update(cx, |s, cx| s.set_value("IOS", window, cx));
        });
        window.click("inspector-apply-button", cx);
        window.render_frame(cx);

        let req_editor = &f.view.read(cx).tab_editors[&tab_id].request;
        assert!(req_editor.pretty_active());
        let updated_wire = f.view.read(cx).workspace.tabs[0].draft.raw.clone();
        assert_eq!(req_editor.bytes_lossless(cx), updated_wire);
        let updated_wire_str = String::from_utf8(updated_wire.clone()).unwrap();
        let (_, wire_body) = updated_wire_str.split_once("\r\n\r\n").unwrap();
        assert!(!wire_body.contains('\n'));
        assert!(wire_body.contains("&platform=IOS&rm=l&sv=1&sign=1"));
        assert_eq!(inspect_http_messages(&updated_wire, None).body_params.len(), 13);

        window.click("inspector-section-attributes", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-protocol-row").visible());
        let attr_header = window
            .find("inspector-section-attributes-table-header")
            .bounds();
        assert_eq!(attr_header.size.height, px(27.));
        assert_eq!(
            window.find("inspector-item-attributes-chevron-0").bounds().size,
            size(px(14.), px(14.))
        );
        assert_eq!(
            window.find("inspector-item-attributes-chevron-1").bounds().size,
            size(px(14.), px(14.))
        );
        assert_eq!(
            window.find("inspector-section-body-table-header").bounds().size.height,
            px(27.)
        );
        assert_eq!(
            window.find("inspector-item-body-chevron-0").bounds().size,
            size(px(14.), px(14.))
        );
        let right_svg = String::from_utf8(sidebar_rail_rotated_label_svg("Inspector", 64.0, false)).unwrap();
        let left_svg = String::from_utf8(sidebar_rail_rotated_label_svg("Inspector", 64.0, true)).unwrap();
        assert!(right_svg.contains("transform=\"translate(4, 0) rotate(90)\""));
        assert!(left_svg.contains("transform=\"translate(15, 64) rotate(-90)\""));
        let attr_header_scaled = attr_header.scale(window.scale_factor());
        assert!(
            window.painted_quads().iter().any(|quad| {
                quad.bounds.contains(&attr_header_scaled.center())
                    && quad.background == rgb(0xeeeeee).into()
            }),
            "Inspector section table header must paint #eeeeee background"
        );
        let http1_btn = window.find("inspector-protocol-HTTP/1");
        let http2_btn = window.find("inspector-protocol-HTTP/2");
        assert_eq!(http1_btn.bounds().origin.y, http2_btn.bounds().origin.y);
        assert_eq!(http1_btn.bounds().size, size(px(56.), px(21.)));
        assert_eq!(http2_btn.bounds().size, size(px(56.), px(21.)));
        assert_eq!(http1_btn.checked(), Some(true));
        assert_eq!(http2_btn.checked(), Some(false));
        assert_eq!(http1_btn.disabled(), None);
        assert_eq!(http2_btn.disabled(), None);
        assert_eq!(
            window.find("inspector-item-attributes-0").label(),
            Some("Method: POST")
        );
        assert_eq!(
            window.find("inspector-item-attributes-1").label(),
            Some("Path: /api/dispatchService")
        );
        assert!(window.try_find("inspector-item-attributes-2").is_none());

        window.click("inspector-item-attributes-1", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inspector-prev-item").disabled(), None);
        assert_eq!(window.find("inspector-next-item").disabled(), Some(true));
        f.view.update(cx, |view, cx| {
            view.inspector
                .value_input
                .update(cx, |s, cx| s.set_value("/api/v2", window, cx));
        });
        window.click("inspector-apply-button", cx);
        window.render_frame(cx);
        window.click("inspector-prev-item", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).inspector.editing_index, Some(0));
        f.view.update(cx, |view, cx| {
            view.inspector
                .value_input
                .update(cx, |s, cx| s.set_value("PUT", window, cx));
        });
        window.click("inspector-apply-button", cx);
        window.render_frame(cx);

        window.click("inspector-protocol-HTTP/2", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inspector-protocol-HTTP/2").checked(), Some(true));
        assert_eq!(window.find("inspector-protocol-HTTP/1").checked(), Some(false));
        assert_eq!(
            f.view.read(cx).workspace.tabs[0].draft.version,
            HttpVersion::Http2
        );
        let wire_h2 =
            String::from_utf8(f.view.read(cx).workspace.tabs[0].draft.raw.clone()).unwrap();
        assert!(wire_h2.starts_with(
            "PUT /api/v2?appkey=10000001&v=6.5&appVersion=1.0.0&cv=1 HTTP/2\r\n"
        ));
        assert!(wire_h2.ends_with("&platform=IOS&rm=l&sv=1&sign=1"));

        window.click("inspector-protocol-HTTP/1", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inspector-protocol-HTTP/1").checked(), Some(true));
        assert_eq!(window.find("inspector-protocol-HTTP/2").checked(), Some(false));
        assert_eq!(
            f.view.read(cx).workspace.tabs[0].draft.version,
            HttpVersion::Http11
        );
        let wire_h1 =
            String::from_utf8(f.view.read(cx).workspace.tabs[0].draft.raw.clone()).unwrap();
        assert!(wire_h1.starts_with(
            "PUT /api/v2?appkey=10000001&v=6.5&appVersion=1.0.0&cv=1 HTTP/1.1\r\n"
        ));

        let readonly_h2_raw = wire_h2.into_bytes();
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Proxy;
            view.proxy_page = ProxyPage::History;
            view.detail = Some(Box::new(record(
                "http://api.example.test/api/dispatchService",
                "",
            )));
            view.history_request.load(&readonly_h2_raw, window, cx);
            assert!(view.history_request.pretty_active());
            view.history_request
                .raw
                .update(cx, |s, cx| s.set_selected_range(0..3, cx));
        });
        window.render_frame(cx);

        assert!(window.find("inspector-section-selection").visible());
        window.click("inspector-section-selection", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("inspector-selection-item").label(),
            Some("Selected text")
        );
        let sel_hdr = window.find("inspector-selection-table-header").bounds();
        let sel_wrap = window.find("inspector-selection-wrap-toggle").bounds();
        let sel_ed = window.find("inspector-selection-editor").bounds();
        assert_eq!(sel_hdr.size, size(px(313.), px(27.)));
        assert_eq!(sel_wrap.size, size(px(28.), px(27.)));
        assert_eq!(sel_wrap.right(), sel_hdr.right());
        assert_eq!(sel_ed.size, size(px(313.), px(41.)));
        assert_eq!(
            window.find("inspector-selection-wrap-toggle").checked(),
            Some(false)
        );
        assert!(window.try_find("inspector-selection-convert-menu").is_none());

        assert_eq!(
            window.find("inspector-section-attributes").label(),
            Some("Request attributes (2)")
        );
        assert_eq!(
            window.find("inspector-section-body").label(),
            Some("Request body parameters (13)")
        );
        assert_eq!(window.find("inspector-protocol-HTTP/1").checked(), Some(false));
        assert_eq!(window.find("inspector-protocol-HTTP/2").checked(), Some(true));
        assert_eq!(window.find("inspector-protocol-HTTP/1").disabled(), Some(true));
        assert_eq!(window.find("inspector-protocol-HTTP/2").disabled(), Some(true));
        assert!(window.try_find("inspector-item-attributes-2").is_none());

        window.click("inspector-item-attributes-0", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-readonly-name").visible());
        assert!(window.find("inspector-readonly-value").visible());
        assert!(window.try_find("inspector-apply-button").is_none());
        window.click("inspector-editor-close", cx);
        window.render_frame(cx);

        f.view.update(cx, |view, cx| {
            view.history_response.load(
                b"HTTP/1.1 200 OK\r\nServer: MIO-Test\r\nContent-Type: application/json; charset=utf-8; profile=extended-inspector-wrap-test\r\n\r\nok",
                window,
                cx,
            );
        });
        window.click("inspector-collapse-all", cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-sections-scrollbar").is_none());
        assert_eq!(
            window.find("inspector-sections").bounds().size.width,
            px(343.)
        );
        assert_eq!(
            window.find("inspector-section-attributes").bounds().size,
            size(px(343.), px(38.))
        );

        window.click("inspector-expand-all", cx);
        window.render_frame(cx);
        let sidebar_right = window.find("inspector-sidebar").bounds().right();
        let sections_bounds = window.find("inspector-sections").bounds();
        let scrollbar_bounds = window.find("inspector-sections-scrollbar").bounds();
        assert_eq!(sections_bounds.size.width, px(333.));
        assert_eq!(scrollbar_bounds.size.width, px(10.));
        assert_eq!(scrollbar_bounds.left(), sections_bounds.right());
        assert_eq!(scrollbar_bounds.right(), sidebar_right);
        for (header_id, chevron_id) in [
            ("inspector-section-attributes-table-header", "inspector-item-attributes-chevron-0"),
            ("inspector-section-query-table-header", "inspector-item-query-chevron-0"),
            ("inspector-section-body-table-header", "inspector-item-body-chevron-0"),
            ("inspector-section-cookies-table-header", "inspector-item-cookies-chevron-0"),
            ("inspector-section-request-headers-table-header", "inspector-item-request-headers-chevron-0"),
            ("inspector-section-response-headers-table-header", "inspector-item-response-headers-chevron-0"),
        ] {
            let hdr = window.find(header_id).bounds();
            assert_eq!(hdr.size.height, px(27.), "{header_id} must be 27px high");
            assert_eq!(hdr.size.width, px(313.), "{header_id} must be 313px wide");
            assert_eq!(hdr.left() - sections_bounds.left(), px(10.));
            assert_eq!(sections_bounds.right() - hdr.right(), px(10.));
            let chev = window.find(chevron_id).bounds();
            assert_eq!(chev.size, size(px(14.), px(14.)), "{chevron_id} must be 14x14px");
            assert!(
                hdr.right() - chev.right() <= px(10.),
                "{chevron_id} must sit at the rightmost side of its row"
            );
        }
        let scroll_handle = f.view.read(cx).inspector.sections_scroll_handle.clone();
        assert_eq!(scroll_handle.offset().y, px(0.));
        assert!(
            scroll_handle.max_offset().y > px(0.),
            "expanded Inspector sections must produce a positive real scroll range"
        );
        let proto_top_before = window.find("inspector-protocol-row").bounds().origin.y;
        let scale = window.scale_factor();
        let bar_scaled = scrollbar_bounds.scale(scale);
        let thumb_width = gpui_kit::ScaledPixels(6. * scale);
        let thumb_quad = window
            .painted_quads()
            .iter()
            .find(|quad| {
                quad.bounds.origin.x >= bar_scaled.origin.x
                    && quad.bounds.right() <= bar_scaled.right()
                    && (quad.bounds.size.width.0 - thumb_width.0).abs() <= 0.5
                    && quad.background == rgb(0xc8c8c8).into()
            })
            .expect("Inspector scrollbar must paint a 6pt #c8c8c8 thumb quad")
            .bounds;
        let thumb_center = gpui_kit::point(
            px(thumb_quad.center().x.0 / scale),
            px(thumb_quad.center().y.0 / scale),
        );
        window.drag(
            thumb_center,
            gpui_kit::point(scrollbar_bounds.center().x, scrollbar_bounds.bottom() - px(8.)),
            cx,
        );
        window.render_frame(cx);
        assert!(
            scroll_handle.offset().y < px(0.),
            "dragging the Inspector scrollbar thumb must scroll sections downward"
        );
        let proto_top_after = window.find("inspector-protocol-row").bounds().origin.y;
        assert!(proto_top_after < proto_top_before);
        scroll_handle.set_offset(gpui_kit::point(px(0.), px(0.)));
        window.render_frame(cx);

        f.view.update(cx, |view, cx| {
            view.switch_inspector_request_protocol("HTTP/1.1", window, cx);
            let mutated = view.handle_inspector_mutation(
                InspectorMutation::Update {
                    section: InspectorSection::BodyParams,
                    index: 0,
                    name: "domain".into(),
                    value: "tampered".into(),
                },
                window,
                cx,
            );
            assert!(!mutated);
            assert_eq!(view.history_request.bytes_lossless(cx), readonly_h2_raw);
        });

        let wrap_test_req = format!(
            "PUT /api/v2/very/long/inspector/attribute/path/segment?appkey=10000001&v=6.5&appVersion=1.0.0.very.long.query.parameter.value&cv=1 HTTP/2\r\nHost: api.example.test\r\nCookie: sid=1; long_token=eyJ1c2VyIjoiYWxpY2UiLCJyb2xlIjoiYWRtaW4ifQ==\r\nAccept: */*\r\nContent-Length: {}\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\n{form_body}",
            form_body.len()
        )
        .into_bytes();
        f.view.update(cx, |view, cx| {
            view.history_request.load(&wrap_test_req, window, cx);
            view.history_request
                .raw
                .update(cx, |s, cx| s.set_selected_range(0..96, cx));
        });
        window.click("inspector-collapse-all", cx);
        window.render_frame(cx);

        window.click("inspector-section-selection", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("inspector-selection-wrap-toggle").checked(),
            Some(false)
        );
        assert_eq!(
            window.find("inspector-selection-editor").bounds().size.height,
            px(41.)
        );
        window.click("inspector-selection-wrap-toggle", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("inspector-selection-wrap-toggle").checked(),
            Some(true)
        );
        assert!(
            f.view
                .read(cx)
                .inspector
                .selection_editor
                .read(cx)
                .display_map()
                .wrap_row_count()
                > 1
        );
        assert!(
            window.find("inspector-selection-editor").bounds().size.height > px(41.),
            "enabling Selection wrap must expand multi-line wrapped selection box beyond 41px"
        );
        window.click("inspector-selection-wrap-toggle", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("inspector-selection-wrap-toggle").checked(),
            Some(false)
        );
        assert_eq!(
            window.find("inspector-selection-editor").bounds().size.height,
            px(41.)
        );
        window.click("inspector-section-selection", cx);
        window.render_frame(cx);

        for (sec_id, wrap_id, short_row_id, long_row_id) in [
            (
                "inspector-section-attributes",
                "inspector-section-attributes-wrap-toggle",
                "inspector-item-attributes-0",
                "inspector-item-attributes-1",
            ),
            (
                "inspector-section-query",
                "inspector-section-query-wrap-toggle",
                "inspector-item-query-1",
                "inspector-item-query-2",
            ),
            (
                "inspector-section-body",
                "inspector-section-body-wrap-toggle",
                "inspector-item-body-3",
                "inspector-item-body-0",
            ),
            (
                "inspector-section-cookies",
                "inspector-section-cookies-wrap-toggle",
                "inspector-item-cookies-0",
                "inspector-item-cookies-1",
            ),
            (
                "inspector-section-request-headers",
                "inspector-section-request-headers-wrap-toggle",
                "inspector-item-request-headers-0",
                "inspector-item-request-headers-1",
            ),
            (
                "inspector-section-response-headers",
                "inspector-section-response-headers-wrap-toggle",
                "inspector-item-response-headers-0",
                "inspector-item-response-headers-1",
            ),
        ] {
            window.click(sec_id, cx);
            window.render_frame(cx);
            assert_eq!(window.find(wrap_id).checked(), Some(false), "{wrap_id}");
            assert_eq!(
                window.find(short_row_id).bounds().size.height,
                px(25.),
                "{short_row_id}"
            );
            assert_eq!(
                window.find(long_row_id).bounds().size.height,
                px(25.),
                "{long_row_id}"
            );
            window.click(wrap_id, cx);
            window.render_frame(cx);
            assert_eq!(window.find(wrap_id).checked(), Some(true), "{wrap_id}");
            assert_eq!(
                window.find(short_row_id).bounds().size.height,
                px(25.),
                "{short_row_id} must stay 25pt tall when wrap is enabled"
            );
            assert!(
                window.find(long_row_id).bounds().size.height > px(25.),
                "{long_row_id} must grow taller than 25pt when wrap is enabled"
            );
            window.click(wrap_id, cx);
            window.render_frame(cx);
            assert_eq!(window.find(wrap_id).checked(), Some(false), "{wrap_id}");
            assert_eq!(
                window.find(long_row_id).bounds().size.height,
                px(25.),
                "{long_row_id} must restore to 25pt when wrap is disabled"
            );
            window.click(sec_id, cx);
            window.render_frame(cx);
        }

        // Verify read-only GET /generate_204 without query/body/cookies hides empty sections while showing attributes, request headers (4), and response headers (3).
        let get_204_req = b"GET /generate_204 HTTP/1.1\r\nHost: 180.163.151.34\r\nUser-Agent: Mozilla/5.0\r\nAccept-Encoding: gzip\r\nConnection: keep-alive\r\n\r\n";
        let get_204_resp = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nDate: Wed, 18 Nov 2026 11:34:44 GMT\r\nConnection: keep-alive\r\n\r\n";
        f.view.update(cx, |view, cx| {
            view.history_request.load(get_204_req, window, cx);
            view.history_response.load(get_204_resp, window, cx);
            view.inspector.expanded = [true, true, true, true, true, false];
            view.inspector.selection_expanded = false;
        });
        window.render_frame(cx);
        assert_eq!(
            window.find("inspector-section-attributes").label(),
            Some("Request attributes (2)")
        );
        assert_eq!(
            window.find("inspector-section-request-headers").label(),
            Some("Request headers (4)")
        );
        assert_eq!(
            window.find("inspector-section-response-headers").label(),
            Some("Response headers (3)")
        );
        assert!(window.try_find("inspector-section-query").is_none());
        assert!(window.try_find("inspector-section-body").is_none());
        assert!(window.try_find("inspector-section-cookies").is_none());

        // Collapsing the visible sections when hidden empty sections still have expanded[1..=3] == true must hide the scrollbar and give sections full 343px width.
        f.view.update(cx, |view, _| {
            view.inspector.expanded = [false, true, true, true, false, false];
        });
        window.render_frame(cx);
        assert!(window.try_find("inspector-sections-scrollbar").is_none());
        assert_eq!(
            window.find("inspector-sections").bounds().size.width,
            px(343.)
        );

        // Switching to editable Repeater with an empty GET request must keep empty query/body/cookies sections visible so new items can be added.
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Repeater;
            view.workspace.tabs[0].draft.raw = get_204_req.to_vec();
            if let Some(editors) = view.tab_editors.get_mut(&tab_id) {
                editors.request.load(get_204_req, window, cx);
            }
            view.inspector.expanded = [true, false, false, false, true, false];
        });
        window.render_frame(cx);
        assert_eq!(
            window.find("inspector-section-query").label(),
            Some("Request query parameters (0)")
        );
        assert_eq!(
            window.find("inspector-section-body").label(),
            Some("Request body parameters (0)")
        );
        assert_eq!(
            window.find("inspector-section-cookies").label(),
            Some("Request cookies (0)")
        );
        window.click("inspector-section-query", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-target-query").visible());
    })
    .unwrap();
}
