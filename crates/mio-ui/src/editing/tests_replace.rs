use gpui_kit::component::input::{Editor, InlineToken, InputContent, Textarea, TextareaState};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Bounds, EntityInputHandler, ParentElement, Point, Render, Styled, TestAppContext,
    Window, WindowBounds, WindowOptions, div, px, size,
};

use super::replace::replace_inline_token_matches;
use super::*;

struct ReplaceHarness {
    editor: MessageEditor,
    textarea: Entity<TextareaState>,
}

impl Render for ReplaceHarness {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(self.editor.state())
                        .readonly(false)
                        .size_full()
                        .aria_label("replace-test-editor"),
                ),
            )
            .child(self.editor.render_search_bar_editable(
                "test",
                false,
                cx,
                |this, action, window, cx| {
                    let _ = this
                        .editor
                        .handle_search_bar_action_editable(action, false, window, cx);
                    cx.notify();
                },
            ))
            .child(
                div()
                    .h(px(80.))
                    .child(Textarea::new(&self.textarea).size_full()),
            )
    }
}

fn open_replace_window(
    cx: &mut TestAppContext,
) -> (gpui_kit::AnyWindowHandle, Entity<ReplaceHarness>) {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(800.), px(600.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let editor = MessageEditor::new(window, cx);
                    let textarea = cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .soft_wrap(false)
                            .autocorrect(false)
                    });
                    ReplaceHarness { editor, textarea }
                })
            },
        )
        .unwrap()
    })
}

fn set_search_and_replace(
    editor: &mut MessageEditor,
    query: &str,
    replacement: &str,
    window: &mut Window,
    cx: &mut App,
) {
    editor.search.update(cx, |input, cx| {
        input.set_value(query.to_owned(), window, cx);
    });
    editor.replace.update(cx, |input, cx| {
        input.set_value(replacement.to_owned(), window, cx);
    });
}

#[gpui_kit::test]
fn message_editor_replace_bar_supports_confirm_regex_case_undo_ime_and_payload_markers(
    cx: &mut TestAppContext,
) {
    let (window, harness) = open_replace_window(cx);
    cx.update_window(window, |_, window, cx| {
        let raw = "GET /?q=alpha&x=alpha HTTP/1.1\r\nHost: h\r\n\r\nid=§alpha§".as_bytes();
        harness.update(cx, |h, cx| {
            h.editor.load(raw, window, cx);
            h.editor.raw.update(cx, |state, cx| state.focus(window, cx));
        });
        window.render_frame(cx);
        window.click("test-search-options", cx);
        window.render_frame(cx);
        window.click("test-search-replace-toggle", cx);
        window.render_frame(cx);
    })
    .unwrap();

    cx.update_window(window, |_, window, cx| {
        let before_third = harness.update(cx, |h, cx| {
            assert!(h.editor.replace_open);
            set_search_and_replace(&mut h.editor, "alpha", "beta", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            assert_eq!(h.editor.search_match_count(cx), 3);

            h.editor.replace_current(false, window, cx).unwrap();
            let text = h.editor.raw.read(cx).value().to_string();
            assert!(text.contains("q=beta&x=alpha"));
            assert!(text.contains("§alpha§"));

            h.editor.replace_current(false, window, cx).unwrap();
            let text = h.editor.raw.read(cx).value().to_string();
            assert!(text.contains("q=beta&x=beta"));
            assert!(text.contains("§alpha§"));

            let before_third = text.clone();
            h.editor.replace_current(false, window, cx).unwrap();
            assert!(h.editor.raw.read(cx).value().contains("§beta§"));
            h.editor.raw.update(cx, |state, cx| state.focus(window, cx));
            before_third
        });
        window.render_frame(cx);
        window.press(crate::kbd!("z"), cx);
        harness.update(cx, |h, cx| {
            assert_eq!(h.editor.raw.read(cx).value().as_ref(), before_third);
        });
        window.press(crate::kbd!("shift-z"), cx);
        harness.update(cx, |h, cx| {
            assert!(h.editor.raw.read(cx).value().contains("§beta§"));

            set_search_and_replace(&mut h.editor, r"id=\S+", "id=x", window, cx);
            h.editor.toggle_regex(window, cx).unwrap();
            h.editor.update_search(None, window, cx).unwrap();
            let err = h.editor.replace_current(false, window, cx).unwrap_err();
            assert!(err.contains("§ payload 标记"), "unexpected error: {err}");
            assert!(h.editor.raw.read(cx).value().contains("§beta§"));

            h.editor.toggle_regex(window, cx).unwrap();
            set_search_and_replace(&mut h.editor, "beta", "§one", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            let err = h.editor.replace_current(false, window, cx).unwrap_err();
            assert!(err.contains("未配对的 §"), "unexpected error: {err}");
            assert!(h.editor.raw.read(cx).value().contains("§beta§"));

            h.editor.replace.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zh", Some(2..2), window, cx);
            });
            assert_eq!(
                h.editor.replace_current(false, window, cx),
                Err(COMPOSITION_PENDING.into())
            );
            assert_eq!(
                h.editor.request_replace_all(false, window, cx),
                Err(COMPOSITION_PENDING.into())
            );
            h.editor.replace.update(cx, |input, cx| {
                input.replace_text_in_range(None, "", window, cx);
            });
        });
    })
    .unwrap();

    cx.update_window(window, |_, window, cx| {
        let initial = b"GET /?token=Alpha&token=beta HTTP/1.1\r\nHost: h\r\n\r\ntoken=GAMMA";
        harness.update(cx, |h, cx| {
            h.editor.load(initial, window, cx);
            if h.editor.pretty_active {
                h.editor.switch(MessageView::Raw, window, cx).unwrap();
            }
            h.editor.toggle_regex(window, cx).unwrap();
            set_search_and_replace(&mut h.editor, r"token=(\w+)", "k=$1", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            assert_eq!(h.editor.search_match_count(cx), 3);

            h.editor.request_replace_all(false, window, cx).unwrap();
            assert_eq!(h.editor.active_pending_replace_all(cx), Some(3));
            assert_eq!(h.editor.bytes(window, cx).unwrap(), initial);

            h.editor.cancel_replace_all();
            assert!(h.editor.pending_replace_all.is_none());

            h.editor.request_replace_all(false, window, cx).unwrap();
            h.editor.search.update(cx, |input, cx| {
                input.set_value("changed".to_owned(), window, cx);
            });
            let err = h.editor.confirm_replace_all(false, window, cx).unwrap_err();
            assert!(err.contains("已变更"), "unexpected error: {err}");

            set_search_and_replace(&mut h.editor, r"token=(\w+)", "k=$1", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            h.editor.request_replace_all(false, window, cx).unwrap();
        });
        window.render_frame(cx);
        window.click("test-replace-confirm", cx);
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            assert_eq!(
                h.editor.raw.read(cx).value().as_ref(),
                "GET /?k=Alpha&k=beta HTTP/1.1\r\nHost: h\r\n\r\nk=GAMMA"
            );
            h.editor.raw.update(cx, |state, cx| state.focus(window, cx));
        });
        window.render_frame(cx);
        window.press(crate::kbd!("z"), cx);
        harness.update(cx, |h, cx| {
            assert_eq!(h.editor.bytes(window, cx).unwrap(), initial);
        });
        window.press(crate::kbd!("shift-z"), cx);
        harness.update(cx, |h, cx| {
            assert!(h.editor.raw.read(cx).value().contains("k=Alpha&k=beta"));

            h.editor.load(b"\x00\xff\x41\x42", window, cx);
            assert_eq!(h.editor.view, MessageView::Hex);
            h.editor.toggle_regex(window, cx).unwrap();
            set_search_and_replace(&mut h.editor, "41 42", "zz 42", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            assert!(h.editor.replace_current(false, window, cx).is_err());
            assert_eq!(h.editor.bytes(window, cx).unwrap(), b"\x00\xff\x41\x42");

            set_search_and_replace(&mut h.editor, "41 42", "43 44", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            h.editor.replace_current(false, window, cx).unwrap();
            assert_eq!(h.editor.bytes(window, cx).unwrap(), b"\x00\xff\x43\x44");
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn inline_token_marker_replacement_preserves_tokens_and_rejects_overlap(cx: &mut TestAppContext) {
    let (window, harness) = open_replace_window(cx);
    cx.update_window(window, |_, window, cx| {
        let textarea = harness.read(cx).textarea.clone();
        let pair = |text: &str| {
            let value = format!("§{text}§");
            let end = value.len();
            InputContent::new(value)
                .with_token(0..2, InlineToken::new("intruder-position:0:start", "§"))
                .unwrap()
                .with_token(
                    end - 2..end,
                    InlineToken::new("intruder-position:0:end", "§"),
                )
                .unwrap()
        };
        textarea.update(cx, |state, cx| {
            state.set_value("plain alpha tail", window, cx);
            state
                .replace_range_with_content(6..11, pair("alpha"), window, cx)
                .unwrap();
        });
        let count =
            replace_inline_token_matches(&textarea, "alpha", "beta", true, false, window, cx)
                .unwrap();
        assert_eq!(count, 1);
        let next = textarea.read(cx).content();
        assert_eq!(next.text().as_ref(), "plain §beta§ tail");
        assert_eq!(next.tokens().len(), 2);
        assert_eq!(
            next.tokens()[0].token().id().as_ref(),
            "intruder-position:0:start"
        );

        let err =
            replace_inline_token_matches(&textarea, "§beta", "gamma", true, false, window, cx)
                .unwrap_err();
        assert!(err.contains("§ payload 标记"), "unexpected error: {err}");
        assert_eq!(textarea.read(cx).value().as_ref(), "plain §beta§ tail");

        textarea.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
        window.press(crate::kbd!("z"), cx);
        assert_eq!(textarea.read(cx).value().as_ref(), "plain §alpha§ tail");
        assert_eq!(textarea.read(cx).tokens().len(), 2);
    })
    .unwrap();
}

#[gpui_kit::test]
fn message_editor_pretty_formats_xml_html_and_preserves_raw_on_parse_errors(
    cx: &mut TestAppContext,
) {
    let (window, harness) = open_replace_window(cx);
    cx.update_window(window, |_, window, cx| {
        let xml =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\n\r\n<user id=\"1\"><name>alice</name></user>";
        let broken_xml = b"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\n\r\n<root><a></root>";
        let html = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<!DOCTYPE html><html><body><div class=\"c\"><p>Hi</p><br></div></body></html>";
        let broken_html =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><div><p>x</div></body></html>";

        harness.update(cx, |h, cx| {
            h.editor.load(xml, window, cx);
            h.editor.switch_pretty(window, cx).unwrap();
            assert!(h.editor.pretty_active);
            let pretty = h.editor.state().read(cx).value().to_string();
            assert!(pretty.contains("\n  <name>alice</name>"));
            assert_eq!(h.editor.bytes(window, cx).unwrap(), xml);
            h.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert_eq!(h.editor.bytes(window, cx).unwrap(), xml);

            h.editor.load(broken_xml, window, cx);
            let err = h.editor.switch_pretty(window, cx).unwrap_err();
            assert!(err.contains("XML 解析失败"), "unexpected error: {err}");
            assert!(!h.editor.pretty_active);
            assert_eq!(h.editor.bytes(window, cx).unwrap(), broken_xml);

            h.editor.load(html, window, cx);
            h.editor.switch_pretty(window, cx).unwrap();
            assert!(h.editor.pretty_active);
            let pretty = h.editor.state().read(cx).value().to_string();
            assert!(pretty.contains("<!DOCTYPE html>"));
            assert!(pretty.contains("\n      <p>Hi</p>"));
            assert_eq!(h.editor.bytes(window, cx).unwrap(), html);

            set_search_and_replace(&mut h.editor, "Hi", "Hello", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            h.editor.replace_current(false, window, cx).unwrap();
            assert!(h.editor.state().read(cx).value().contains("\n      <p>Hello</p>"));
            let edited_html = String::from_utf8(h.editor.bytes(window, cx).unwrap()).unwrap();
            assert!(edited_html.starts_with("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n"));
            assert!(edited_html.contains("<p>Hello</p>"));
            h.editor.state().update(cx, |state, cx| state.focus(window, cx));
        });
        window.render_frame(cx);
        window.press(crate::kbd!("z"), cx);
        harness.update(cx, |h, cx| {
            assert_eq!(h.editor.bytes(window, cx).unwrap(), html);

            let chunked_json = b"POST /api HTTP/1.1\r\nHost: example.com\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n9\r\n{\"k\":\"v\"}\r\n0\r\nX-Trailer: t1\r\n\r\n";
            h.editor.load(chunked_json, window, cx);
            h.editor.switch_pretty(window, cx).unwrap();
            assert_eq!(h.editor.bytes(window, cx).unwrap(), chunked_json);
            set_search_and_replace(&mut h.editor, "example.com", "edited.example", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            h.editor.replace_current(false, window, cx).unwrap();
            assert_eq!(
                h.editor.bytes(window, cx).unwrap(),
                b"POST /api HTTP/1.1\r\nHost: edited.example\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n9\r\n{\"k\":\"v\"}\r\n0\r\nX-Trailer: t1\r\n\r\n"
            );
            set_search_and_replace(&mut h.editor, "\"v\"", "\"v2\"", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            h.editor.replace_current(false, window, cx).unwrap();
            let rechunked = h.editor.bytes(window, cx).unwrap();
            let parsed_req = mio_core::parse_request(
                &rechunked,
                "http://edited.example",
                mio_core::HttpVersion::Http11,
            )
            .unwrap();
            assert_eq!(String::from_utf8_lossy(&parsed_req.body), "{\n  \"k\": \"v2\"\n}");
            assert_eq!(parsed_req.trailers.len(), 1);
            assert_eq!(parsed_req.trailers[0].name, "X-Trailer");

            let form_req = b"POST /login HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\na=1&b=2";
            h.editor.load(form_req, window, cx);
            h.editor.switch_pretty(window, cx).unwrap();
            assert_eq!(h.editor.bytes(window, cx).unwrap(), form_req);
            let pos = h.editor.state().read(cx).value().find("b=2").unwrap();
            h.editor.state().update(cx, |s, cx| s.set_selected_range(pos + 2..pos + 3, cx));
            h.editor
                .apply_conversion(ConvertKind::Base64Encode, false, window, cx)
                .unwrap();
            assert_eq!(
                h.editor.bytes(window, cx).unwrap(),
                b"POST /login HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\na=1&b=Mg=="
            );

            h.editor.load(broken_html, window, cx);
            let err = h.editor.switch_pretty(window, cx).unwrap_err();
            assert!(err.contains("HTML 解析失败"), "unexpected error: {err}");
            assert!(!h.editor.pretty_active);
            assert_eq!(h.editor.bytes(window, cx).unwrap(), broken_html);

            let req = b"GET /items?token=alpha HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\ntoken=alpha";
            h.editor.load(req, window, cx);
            set_search_and_replace(&mut h.editor, "alpha", "beta", window, cx);
            h.editor.update_search(None, window, cx).unwrap();
            h.editor.toggle_replace(false, window, cx).unwrap();
            assert!(h.editor.replace_open);

            h.editor.toggle_hide_headers(window, cx).unwrap();
            assert!(h.editor.hide_headers);
            assert!(!h.editor.replace_open);
            assert!(h.editor.toggle_replace(false, window, cx).is_err());
            assert!(h.editor.replace_current(false, window, cx).is_err());
            assert!(h.editor.request_replace_all(false, window, cx).is_err());
            assert!(h.editor.confirm_replace_all(false, window, cx).is_err());
            assert!(
                h.editor
                    .apply_conversion(ConvertKind::UrlEncodeAll, false, window, cx)
                    .is_err()
            );
            h.editor
                .header_preview
                .as_ref()
                .unwrap()
                .update(cx, |state, cx| {
                    state.set_selected_range(0..3, cx);
                });
            let cand = h
                .editor
                .selection_candidate(true, Some(window), cx)
                .unwrap();
            assert!(!cand.identity.writable);
            assert!(cand.writeback_editor.is_none());
            assert_eq!(h.editor.bytes(window, cx).unwrap(), req);

            h.editor.toggle_hide_headers(window, cx).unwrap();
            assert!(!h.editor.hide_headers);
            h.editor.toggle_non_printable(window, cx).unwrap();
            assert!(h.editor.toggle_replace(false, window, cx).is_err());
            assert!(h.editor.replace_current(false, window, cx).is_err());
            assert!(h.editor.request_replace_all(false, window, cx).is_err());
            assert_eq!(h.editor.bytes(window, cx).unwrap(), req);
        });
    })
    .unwrap();
}
