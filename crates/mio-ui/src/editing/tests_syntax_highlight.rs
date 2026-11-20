use std::ops::Range;

use gpui_kit::HighlightStyle;
use gpui_kit::base::input::HighlightStyleResolver;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Bounds, Point, Render, Styled, TestAppContext, TestSupportExt, Window,
    WindowBounds, WindowOptions, div, px, size,
};

use super::syntax_highlight::{
    HttpSyntaxHighlighter, MAX_HIGHLIGHT_BYTES, TokenKind, tokenize_http,
};
use super::*;

struct NoResolver;
impl HighlightStyleResolver for NoResolver {
    fn style(&self, _: &str) -> Option<HighlightStyle> {
        None
    }
}

fn assert_valid_non_overlapping_runs(
    text: &str,
    query: Range<usize>,
    runs: &[(Range<usize>, HighlightStyle)],
) {
    let mut cursor = query.start;
    for (range, _) in runs {
        assert!(
            range.start < range.end,
            "empty range {range:?} in {query:?}"
        );
        assert_eq!(range.start, cursor, "gap or overlap at {range:?}");
        assert!(text.is_char_boundary(range.start), "bad start {range:?}");
        assert!(text.is_char_boundary(range.end), "bad end {range:?}");
        cursor = range.end;
    }
    assert_eq!(cursor, query.end, "runs did not cover full query {query:?}");
}

#[test]
fn http_request_json_body_and_payload_markers_produce_valid_utf8_runs() {
    let msg = concat!(
        "POST /api/v1/中文?uid=§42§&role=admin#frag HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Authorization: Bearer secret-§tok§\r\n",
        "Content-Type: application/json; charset=utf-8\r\n",
        "\r\n",
        "{\"姓名\":\"张三\",\"age\":28,\"active\":true,\"extra\":null}\n",
    );
    let tokens = tokenize_http(msg);
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::Method && &msg[r.clone()] == "POST")
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::HeaderName && &msg[r.clone()] == "uid")
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::PayloadMarker && &msg[r.clone()] == "§42§")
    );
    assert!(
        tokens.iter().any(
            |(r, k)| *k == TokenKind::SensitiveHeaderValue && msg[r.clone()].contains("Bearer")
        )
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonKey && &msg[r.clone()] == "\"姓名\"")
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonString && &msg[r.clone()] == "\"张三\"")
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonNumber && &msg[r.clone()] == "28")
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonKeyword && &msg[r.clone()] == "true")
    );
    assert_eq!(TokenKind::Method.style(false).color, Some(rgb(0x141414).into()));
    assert_eq!(TokenKind::HeaderName.style(false).color, Some(rgb(0x030075).into()));
    assert_eq!(TokenKind::QueryKey.style(false).color, Some(rgb(0x141414).into()));
    assert_eq!(TokenKind::QueryValue.style(false).color, Some(rgb(0xa31616).into()));
    assert_eq!(TokenKind::JsonKey.style(false).color, Some(rgb(0x030075).into()));
    assert_eq!(TokenKind::JsonString.style(false).color, Some(rgb(0x008000).into()));
    assert_eq!(TokenKind::JsonNumber.style(false).color, Some(rgb(0x0000ff).into()));
    assert_eq!(
        TokenKind::NonPrintableBadge.style(false).color,
        Some(rgb(0x2b2b2b).into())
    );
    assert_eq!(
        TokenKind::NonPrintableBadge.style(false).background_color,
        Some(rgb(0xe1e0e0).into())
    );
    assert_eq!(
        TokenKind::NonPrintableBadge.style(true).color,
        Some(rgb(0xd4d4d4).into())
    );
    assert_eq!(
        TokenKind::NonPrintableBadge.style(true).background_color,
        Some(rgb(0x3c3f41).into())
    );

    let (np_preview, np_badges) = super::codec::format_non_printable_preview_with_badges(
        b"GET / HTTP/1.1\r\nHost: example.com\r\nX-Lit:  \\r   \\n \r\n\r\n",
    );
    let np_tokens = super::syntax_highlight::tokenize_http_with_badges(&np_preview, &np_badges);
    let badge_texts: Vec<&str> = np_tokens
        .iter()
        .filter(|(_, k)| *k == TokenKind::NonPrintableBadge)
        .map(|(r, _)| &np_preview[r.clone()])
        .collect();
    assert_eq!(
        badge_texts,
        vec![
            " \\r ", " \\n ", " \\r ", " \\n ", " \\r ", " \\n ", " \\r ", " \\n "
        ]
    );
    let raw_with_literal_badges =
        "GET / HTTP/1.1\r\nHost: example.com\r\nX-Lit:  \\r   \\n \r\n\r\na=1 \\r   \\n ";
    let raw_tokens = tokenize_http(raw_with_literal_badges);
    assert!(
        raw_tokens
            .iter()
            .all(|(_, k)| *k != TokenKind::NonPrintableBadge),
        "plain Raw mode must never highlight literal \\r or \\n strings as NonPrintableBadge"
    );

    let mut hl = HttpSyntaxHighlighter::default();
    hl.parse_str(msg);
    let light = hl.styles_for_mode(&(0..msg.len()), false);
    let dark = hl.styles_for_mode(&(0..msg.len()), true);
    assert_valid_non_overlapping_runs(msg, 0..msg.len(), &light);
    assert_valid_non_overlapping_runs(msg, 0..msg.len(), &dark);
    assert_ne!(light[0].1.color, dark[0].1.color);
}

#[test]
fn real_ext_pretty_request_and_response_multiline_headers_and_json_highlight_accurately() {
    let raw_req = concat!(
        "POST /api/dispatchService?appkey=10000001&v=6.5&appVersion=1.0.27.801&cv=1 HTTP/1.1\r\n",
        "Host: api.example.test\r\n",
        "Accept: */*\r\n",
        "Content-Length: 333\r\n",
        "Content-Type: application/x-www-form-urlencoded\r\n",
        "Accept-Encoding: gzip, deflate, br\r\n",
        "Connection: keep-alive\r\n\r\n",
        "domain=api.example.test cdn.example.test&appName=com.example.demoapp&netType=UNKNOWN&sign=a%2Bb%2Fc%3D",
    );
    let pretty_req = mio_core::format_pretty_message(raw_req.as_bytes()).unwrap();
    assert!(pretty_req.ends_with("\n\ndomain=api.example.test cdn.example.test&appName=com.example.demoapp&netType=UNKNOWN&sign=a%2Bb%2Fc%3D"));
    let req_tokens = tokenize_http(&pretty_req);
    assert!(
        req_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::Method && &pretty_req[r.clone()] == "POST")
    );
    assert!(
        req_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::HeaderName && &pretty_req[r.clone()] == "appkey")
    );
    assert!(
        req_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::QueryValue && &pretty_req[r.clone()] == "10000001")
    );
    assert!(
        req_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::HeaderName && &pretty_req[r.clone()] == "Content-Type")
    );
    assert!(
        req_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::HeaderName && &pretty_req[r.clone()] == "domain")
    );
    assert!(
        req_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::QueryValue && &pretty_req[r.clone()] == "api.example.test")
    );
    let body_offset = pretty_req.find("\n\ndomain=").unwrap() + 2;
    let space_offset = body_offset + "domain=api.example.test".len();
    assert!(
        req_tokens
            .iter()
            .all(|(r, _)| r.end <= space_offset),
        "unencoded whitespace in form body must stop form-urlencoded highlighting: {:?}",
        req_tokens
            .iter()
            .filter(|(r, _)| r.end > space_offset)
            .map(|(r, k)| (&pretty_req[r.clone()], k))
            .collect::<Vec<_>>()
    );

    let encoded_json_req = concat!(
        "POST /gw/demo.intention.gateway/1.0/ HTTP/1.1\r\n",
        "Host: gateway.example.test\r\n",
        "Content-Type: application/x-www-form-urlencoded; charset=utf-8\r\n\r\n",
        "foo=bar%2B1&data=%7B%22data%22%3A%22%7B%5C%22contextCompress%5C%22%3A%5C%22iOS%7Cno%7Cno%5C%22%7D%22%2C%22intention%22%3A%22%22%7D",
    );
    let ej_tokens = tokenize_http(encoded_json_req);
    let ej_body_offset = encoded_json_req.find("\r\n\r\n").unwrap() + 4;
    let body_tokens: Vec<(&str, TokenKind)> = ej_tokens
        .iter()
        .filter(|(r, _)| r.start >= ej_body_offset)
        .map(|(r, k)| (&encoded_json_req[r.clone()], *k))
        .collect();
    assert_eq!(
        body_tokens,
        vec![
            ("foo", TokenKind::HeaderName),
            ("=", TokenKind::QueryPunct),
            ("bar%2B1", TokenKind::QueryValue),
            ("&", TokenKind::QueryPunct),
            ("data", TokenKind::HeaderName),
            ("=", TokenKind::QueryPunct),
            (
                "%7B%22data%22%3A%22%7B%5C%22contextCompress%5C%22%3A%5C%22iOS%7Cno%7Cno%5C%22%7D%22%2C%22intention%22%3A%22%22%7D",
                TokenKind::QueryValue
            ),
        ]
    );

    let raw_resp = concat!(
        "HTTP/2 200 OK\r\n",
        "Date: Tue, 17 Nov 2026 12:17:54 GMT\r\n",
        "Content-Type: application/json\r\n",
        "Content-Length: 49\r\n",
        "Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS, PATCH\r\n",
        "Strict-Transport-Security: max-age=0\r\n\r\n",
        "{\"api\":\"demo.search.upload\",\"data\":{\"count\":2,\"ok\":true},\"ret\":[\"SUCCESS::ok\"],\"v\":\"1.0\"}",
    );
    let pretty_resp = mio_core::format_pretty_message(raw_resp.as_bytes()).unwrap();
    assert!(pretty_resp.contains("\n  \"api\": \"demo.search.upload\",\n"));
    let resp_tokens = tokenize_http(&pretty_resp);
    assert!(
        resp_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::Status2xx && &pretty_resp[r.clone()] == "200 OK")
    );
    assert!(resp_tokens.iter().any(|(r, k)| {
        *k == TokenKind::HeaderName
            && &pretty_resp[r.clone()] == "Access-Control-Allow-Methods"
    }));
    assert!(
        resp_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonKey && &pretty_resp[r.clone()] == "\"api\"")
    );
    assert!(
        resp_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonKey && &pretty_resp[r.clone()] == "\"ret\"")
    );
    assert!(resp_tokens.iter().any(|(r, k)| {
        *k == TokenKind::JsonString && &pretty_resp[r.clone()] == "\"SUCCESS::ok\""
    }));
    assert!(
        resp_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonNumber && &pretty_resp[r.clone()] == "2")
    );
    assert!(
        resp_tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::JsonKeyword && &pretty_resp[r.clone()] == "true")
    );

    let mut hl = HttpSyntaxHighlighter::default();
    hl.parse_str(&pretty_resp);
    let header_body_boundary = pretty_resp.find("\n\n").unwrap();
    let cross_range = (header_body_boundary - 24)..(header_body_boundary + 48);
    let cross_runs = hl.styles_for_mode(&cross_range, false);
    assert_valid_non_overlapping_runs(&pretty_resp, cross_range, &cross_runs);
    assert!(
        cross_runs
            .iter()
            .any(|(_, s)| s.color == Some(rgb(0x030075).into())),
        "cross-line header/body query must include Pretty JSON key color #030075"
    );

    let base64_json_resp = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: application/json; charset=utf-8\r\n\r\n",
        "eyJjb2RlIjowLCJtZXNzYWdlIjoidHJ1ZSIsImNvdW50IjoxMjM0NX0=",
    );
    let b64_body_offset = base64_json_resp.find("\r\n\r\n").unwrap() + 4;
    let b64_tokens = tokenize_http(base64_json_resp);
    assert!(
        b64_tokens.iter().all(|(r, _)| r.end <= b64_body_offset),
        "Base64 body with application/json header must remain plain text without JSON tokens: {b64_tokens:?}"
    );

    for (scalar_body, expected_kind) in [
        ("true", TokenKind::JsonKeyword),
        ("false", TokenKind::JsonKeyword),
        ("null", TokenKind::JsonKeyword),
        ("42", TokenKind::JsonNumber),
        ("\"ok\"", TokenKind::JsonString),
    ] {
        let scalar_resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{scalar_body}"
        );
        let scalar_tokens = tokenize_http(&scalar_resp);
        assert!(
            scalar_tokens
                .iter()
                .any(|(r, k)| *k == expected_kind && &scalar_resp[r.clone()] == scalar_body),
            "valid JSON scalar {scalar_body} must highlight as {expected_kind:?}: {scalar_tokens:?}"
        );
    }
}
#[test]
fn http_response_html_form_and_cap_respect_char_boundaries() {
    let html_resp = concat!(
        "HTTP/2 404 Not Found\r\n",
        "Set-Cookie: sid=abc123; HttpOnly\r\n",
        "Content-Type: text/html\r\n",
        "\r\n",
        "<!-- 注释 --><div class=\"warn\" data-msg='错误'>未找到</div>",
    );
    let tokens = tokenize_http(html_resp);
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::Status4xx && html_resp[r.clone()].starts_with("404"))
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::XmlTag && &html_resp[r.clone()] == "div")
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::XmlAttrName && &html_resp[r.clone()] == "class")
    );
    assert!(
        tokens
            .iter()
            .any(|(r, k)| *k == TokenKind::XmlString && &html_resp[r.clone()] == "\"warn\"")
    );

    let mut hl = HttpSyntaxHighlighter::default();
    hl.parse_str(html_resp);
    let sub = 11..html_resp.len() - 6;
    let runs = hl.styles_for_mode(&sub, false);
    assert_valid_non_overlapping_runs(html_resp, sub, &runs);

    // Multi-byte UTF-8 straddling MAX_HIGHLIGHT_BYTES must never split a character.
    let mut huge = String::from("GET / HTTP/1.1\r\n\r\n");
    while huge.len() < MAX_HIGHLIGHT_BYTES - 1 {
        huge.push('a');
    }
    huge.push_str("中文尾部内容");
    assert!(huge.len() > MAX_HIGHLIGHT_BYTES);
    hl.parse_str(&huge);
    let capped = hl.styles_for_mode(&(0..huge.len()), true);
    assert_valid_non_overlapping_runs(&huge, 0..huge.len(), &capped);
    let _ = gpui_kit::base::input::InputHighlighter::styles(&hl, &(0..huge.len()), &NoResolver);
}

struct SelectionProbe(MessageEditor);

impl Render for SelectionProbe {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        div().size_full()
    }
}

#[gpui_kit::test]
fn selection_seq_and_selected_bytes_track_raw_pretty_and_hex(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, probe) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(640.), px(480.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| SelectionProbe(MessageEditor::new(window, cx))),
        )
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        probe.update(cx, |probe, cx| {
            assert_eq!(probe.0.raw.read(cx).language_name().as_ref(), "http");
            assert_eq!(probe.0.pretty.read(cx).language_name().as_ref(), "http");
            assert_eq!(probe.0.hex.read(cx).language_name().as_ref(), "plaintext");
            assert_eq!(probe.0.selection_seq(), 0);
            assert!(probe.0.selected_bytes(cx).is_none());
            probe
                .0
                .load(b"POST / HTTP/1.1\r\n\r\n{\"a\":1}", window, cx);
            probe
                .0
                .raw
                .update(cx, |s, cx| s.set_selected_range(0..4, cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    let seq1 = cx.read(|cx| {
        let editor = &probe.read(cx).0;
        assert_eq!(editor.selected_bytes(cx), Some((b"POST".to_vec(), true)));
        editor.selection_seq()
    });
    assert!(seq1 > 0);

    cx.update_window(window, |_, window, cx| {
        probe.update(cx, |probe, cx| {
            probe.0.switch(MessageView::Hex, window, cx).unwrap();
            probe
                .0
                .hex
                .update(cx, |s, cx| s.set_selected_range(0..5, cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let editor = &probe.read(cx).0;
        assert!(editor.selection_seq() > seq1);
        assert_eq!(editor.selected_bytes(cx), Some((b"PO".to_vec(), false)));
    });
}

struct LayoutProbe(MessageEditor);

impl Render for LayoutProbe {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        div()
            .id("editor-pane")
            .test_support()
            .size_full()
            .child(message_editor(self.0.state()).size_full())
    }
}

#[gpui_kit::test]
fn message_editor_gutter_and_line_height_match_ext_and_scale_with_settings(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let (window, probe) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(450.), px(497.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut editor = MessageEditor::new(window, cx);
                    editor.load(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n", window, cx);
                    editor.state().update(cx, |state, cx| state.set_selected_range(0..0, cx));
                    LayoutProbe(editor)
                })
            },
        )
        .unwrap()
    });
    let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
    let pane_bounds = vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find("editor-pane").bounds()
    });
    vcx.read(|cx| {
        let state = probe.read(cx).0.state().read(cx);
        let input_bounds = state.input_bounds();
        assert_eq!(input_bounds.origin, gpui_kit::point(pane_bounds.origin.x, pane_bounds.origin.y + px(2.)));
        assert_eq!(input_bounds.size, size(pane_bounds.size.width, pane_bounds.size.height - px(2.)));
        assert_eq!(state.line_number_width(), Some(px(23.)));
        let line_height = state.line_height().expect("editor must lay out");
        assert!((line_height.as_f32() - 17.5).abs() < 0.01, "{line_height:?}");
        let caret = state
            .prepaint_caret_geometry()
            .expect("caret geometry must be stashed");
        assert_eq!(caret.cursor_bounds().left(), pane_bounds.left() + px(29.));
        assert_eq!(
            caret.cursor_bounds().top() - input_bounds.top(),
            (line_height - caret.cursor_bounds().size.height) / 2.,
        );
    });

    vcx.update(|window, cx| {
        let many_lines = "X-Header: v\r\n".repeat(120);
        probe.update(cx, |probe, cx| {
            probe.0.load(many_lines.as_bytes(), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    vcx.read(|cx| {
        let state = probe.read(cx).0.state().read(cx);
        let width_3_digits = state.line_number_width().expect("gutter width");
        assert!(
            width_3_digits > px(23.),
            "3-digit gutter must grow beyond 23px: {width_3_digits:?}"
        );
    });

    vcx.update(|window, cx| {
        let mut display = mio_core::UserDisplaySettings::default();
        display.scale_factor_percent = 125;
        let mut editor_settings = mio_core::UserMessageEditorSettings::default();
        editor_settings.font_size_px = 16;
        crate::theme::apply_user_display(&display, &editor_settings, cx);
        probe.update(cx, |probe, cx| {
            probe.0.load(b"GET / HTTP/1.1\r\n", window, cx);
        });
        window.draw(cx).clear(cx);
    });
    vcx.read(|cx| {
        let state = probe.read(cx).0.state().read(cx);
        let scaled_line_height = state.line_height().expect("scaled line height");
        let expected = 20.0 * crate::theme::EDITOR_LINE_HEIGHT;
        assert!(
            (scaled_line_height.as_f32() - expected).abs() < 0.05,
            "expected {expected}, got {scaled_line_height:?}"
        );
        assert!(
            state.line_number_width().expect("scaled gutter width") > px(23.),
            "larger font and scale must widen the gutter"
        );
    });
    vcx.update(|_, cx| crate::theme::init(cx));
}

#[gpui_kit::test]
fn http_request_start_line_wraps_at_query_ampersand_in_ext_viewport_and_updates_on_edits(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let raw_req = concat!(
        "POST /api/dispatchService?appkey=10000001&v=6.5&appVersion=1.0.27.801&cv=1 HTTP/1.1\r\n",
        "Host: api.example.test\r\n",
        "Accept: */*\r\n",
        "Content-Length: 339\r\n",
        "Content-Type: application/x-www-form-urlencoded\r\n",
        "Accept-Encoding: gzip, deflate, br\r\n",
        "Connection: keep-alive\r\n\r\n",
        "domain=api.example.test cdn.example.test&appName=com.example.demoapp&netType=UNKNOWN",
    );
    let (window, probe) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(450.), px(497.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut editor = MessageEditor::new(window, cx);
                    editor.load(raw_req.as_bytes(), window, cx);
                    LayoutProbe(editor)
                })
            },
        )
        .unwrap()
    });
    let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    let expected_first_visual = "POST /api/dispatchService?appkey=10000001&v=6.5&";
    let expected_second_visual_start = expected_first_visual.len();
    vcx.read(|cx| {
        let state = probe.read(cx).0.state().read(cx);
        let line_height = state.line_height().expect("line height");
        let before_break = state
            .range_to_bounds(&(expected_second_visual_start - 1..expected_second_visual_start))
            .expect("ampersand at end of first visual row must be laid out");
        let after_break = state
            .range_to_bounds(&(expected_second_visual_start..expected_second_visual_start + 1))
            .expect("appVersion at start of second visual row must be laid out");
        let first_row_start = state
            .range_to_bounds(&(0..1))
            .expect("POST at start of first visual row must be laid out");
        assert_eq!(first_row_start.origin.x, px(29.));
        assert!(
            (after_break.origin.x - first_row_start.origin.x).abs() < px(0.5),
            "continuation row must start at the same left edge as row 1 without hanging indent: row1={first_row_start:?}, row2={after_break:?}"
        );
        assert!(
            (after_break.origin.y - before_break.origin.y - line_height).abs() < px(0.5),
            "request start line must wrap right after '&v=6.5&' onto visual row 2: before={before_break:?}, after={after_break:?}"
        );
        let host_offset = state.value().find("Host: api.example.test").unwrap();
        let host_bounds = state
            .range_to_bounds(&(host_offset..host_offset + 4))
            .expect("Host header on buffer line 2 must sit on visual row 3");
        assert!(
            (host_bounds.origin.y - before_break.origin.y - line_height * 2.).abs() < px(0.5),
            "request start line must occupy exactly 2 visual rows before Host header: host={host_bounds:?}, row1={before_break:?}"
        );
    });

    vcx.update(|window, cx| {
        probe.update(cx, |probe, cx| {
            probe.0.toggle_non_printable(window, cx).unwrap();
            probe.0.toggle_hide_headers(window, cx).unwrap();
        });
        window.draw(cx).clear(cx);
    });
    vcx.read(|cx| {
        let state = probe.read(cx).0.state().read(cx);
        let line_height = state.line_height().expect("line height");
        let before_break = state
            .range_to_bounds(&(expected_second_visual_start - 1..expected_second_visual_start))
            .expect("ampersand in non-printable + hide-headers preview");
        let after_break = state
            .range_to_bounds(&(expected_second_visual_start..expected_second_visual_start + 1))
            .expect("appVersion in non-printable + hide-headers preview");
        assert!(
            (after_break.origin.y - before_break.origin.y - line_height).abs() < px(0.5),
            "non-printable + hide-headers preview must wrap at the same '&v=6.5&' boundary"
        );
    });

    vcx.update(|window, cx| {
        probe.update(cx, |probe, cx| {
            probe.0.toggle_hide_headers(window, cx).unwrap();
            probe.0.toggle_non_printable(window, cx).unwrap();
            probe.0.switch(MessageView::Raw, window, cx).unwrap();
            probe.0.raw.update(cx, |state, cx| {
                state.set_highlighter("plaintext", cx);
            });
        });
        window.draw(cx).clear(cx);
    });
    vcx.read(|cx| {
        let state = probe.read(cx).0.state().read(cx);
        let line_height = state.line_height().expect("line height");
        let q_end = "POST /api/dispatchService?".len();
        let q_bounds = state.range_to_bounds(&(q_end - 1..q_end)).unwrap();
        let after_q_bounds = state.range_to_bounds(&(q_end..q_end + 1)).unwrap();
        assert!(
            (after_q_bounds.origin.y - q_bounds.origin.y - line_height).abs() < px(0.5),
            "switching language to plaintext must clear HTTP extra wrap boundaries and revert to standard '?' break"
        );
    });

    vcx.update(|window, cx| {
        let indented_long = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{{\n    \"token\": \"{}\"\n}}",
            "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.".repeat(3)
        );
        probe.update(cx, |probe, cx| {
            probe.0.raw.update(cx, |state, cx| {
                state.set_highlighter_factory(syntax_highlight::http_highlighter_factory(), cx);
                state.set_scroll_beyond_last_line(Some(0), window, cx);
            });
            probe.0.load(indented_long.as_bytes(), window, cx);
        });
        let paint = window.draw(cx);
        let scale = window.scale_factor();
        let state = probe.read(cx).0.state().read(cx);
        let input_bounds = state.input_bounds();
        let gutter_width = state.line_number_width().unwrap();
        let line_height = state.line_height().unwrap();
        let text = state.value();
        let token_line_start = text.find("  \"token\":").unwrap();
        let line_start_bounds = state
            .range_to_bounds(&(token_line_start..token_line_start + 1))
            .unwrap();
        let token_line_end = token_line_start + text[token_line_start..].find('\n').unwrap();
        let (continuation_ix, continuation_first_bounds) = (token_line_start + 1..token_line_end)
            .find_map(|ix| {
                let b = state.range_to_bounds(&(ix..ix + 1))?;
                (b.origin.y > line_start_bounds.origin.y + px(1.)).then_some((ix, b))
            })
            .expect("indented JSON line must soft-wrap onto a continuation row");
        assert!(
            (continuation_first_bounds.origin.x - line_start_bounds.origin.x).abs() < px(0.5),
            "WrappingIndent::None must align continuation row with column 0 instead of hanging indent: start={line_start_bounds:?}, cont={continuation_first_bounds:?}"
        );
        let active_bg: gpui_kit::Background = rgb(0xf0f0f0).into();
        let track_bg: gpui_kit::Background = rgb(0xf7f7f7).into();
        let thumb_bg: gpui_kit::Background = rgb(0xc8c8c8).into();
        let border_bg: gpui_kit::Background = rgb(0xdfdfdf).into();
        let quads: Vec<_> = window
            .painted_quads()
            .into_iter()
            .map(|q| (q.bounds.map(|v| px(v.0 / scale)), q.background))
            .collect();
        assert!(
            quads.iter().all(|(b, bg)| {
                *bg != active_bg
                    || ((b.origin.x - (input_bounds.origin.x + gutter_width + px(2.))).abs() < px(0.5)
                        && (b.size.height - line_height).abs() < px(0.5))
            }),
            "active line background must start at x=25pt (gutter+2pt) and cover only the first visual row height"
        );
        assert!(
            quads.iter().any(|(b, bg)| {
                *bg == border_bg
                    && (b.origin.x - px(22.)).abs() < px(0.5)
                    && (b.size.width - px(1.)).abs() < px(0.5)
            }),
            "gutter right border must sit at x=22..23pt"
        );
        assert!(
            quads.iter().any(|(b, bg)| {
                *bg == track_bg
                    && (b.size.width - px(10.)).abs() < px(0.5)
                    && (b.right() - input_bounds.right()).abs() < px(0.5)
            }),
            "10px vertical scrollbar track must stay visible even when short content does not overflow"
        );
        assert!(
            !quads.iter().any(|(b, bg)| {
                *bg == thumb_bg && (b.size.width - px(6.)).abs() < px(0.5)
            }),
            "short non-overflowing message must not paint a fake scrollbar thumb"
        );
        paint.clear(cx);

        probe.update(cx, |probe, cx| {
            probe
                .0
                .state()
                .update(cx, |state, cx| {
                    state.set_selected_range(continuation_ix..continuation_ix, cx);
                });
        });
        let paint = window.draw(cx);
        let quads: Vec<_> = window
            .painted_quads()
            .into_iter()
            .map(|q| (q.bounds.map(|v| px(v.0 / scale)), q.background))
            .collect();
        let active_quad = quads
            .iter()
            .find_map(|(b, bg)| (*bg == active_bg).then_some(*b))
            .expect("active line quad must be painted on wrapped continuation row");
        assert!(
            (active_quad.origin.y - continuation_first_bounds.origin.y).abs() < px(0.5)
                && (active_quad.size.height - line_height).abs() < px(0.5),
            "moving caret onto wrapped continuation row must move active-line highlight to that visual row: active={active_quad:?}, cont={continuation_first_bounds:?}"
        );
        paint.clear(cx);

        let long_msg = format!(
            "HTTP/1.1 200 OK\r\n{}",
            (0..60)
                .map(|i| format!("X-Header-{i}: value-{i}\r\n"))
                .collect::<String>()
        );
        probe.update(cx, |probe, cx| {
            probe.0.load(long_msg.as_bytes(), window, cx);
        });
        let paint = window.draw(cx);
        let quads: Vec<_> = window
            .painted_quads()
            .into_iter()
            .map(|q| (q.bounds.map(|v| px(v.0 / scale)), q.background))
            .collect();
        assert!(
            quads.iter().any(|(b, bg)| {
                *bg == thumb_bg
                    && (b.size.width - px(6.)).abs() < px(0.5)
                    && (b.right() - (input_bounds.right() - px(2.))).abs() < px(1.)
            }),
            "overflowing message must paint a 6px scrollbar thumb inside the 10px track"
        );
        paint.clear(cx);

        probe.update(cx, |probe, cx| {
            probe.0.state().update(cx, |state, cx| {
                state.set_scroll_offset(gpui_kit::point(px(0.), px(-10_000.)), cx);
            });
        });
        window.draw(cx).clear(cx);
        window.draw(cx).clear(cx);
        let max_scroll_y = probe.read(cx).0.state().read(cx).scroll_offset().y;
        let total_visual_rows = probe.read(cx).0.state().read(cx).display_map().wrap_row_count();
        let expected_max_scroll_y =
            -(total_visual_rows as f32 * line_height.as_f32() - input_bounds.size.height.as_f32());
        assert!(
            (max_scroll_y.as_f32() - expected_max_scroll_y).abs() < 1.0,
            "scroll_beyond_last_line(Some(0)) must clamp max vertical scroll to exact content-viewport difference: got={max_scroll_y:?}, expected={expected_max_scroll_y}"
        );
    });
}

#[test]
fn syntax_highlighter_large_256kib_http_json_under_latency_budget() {
    let mut doc = String::from(
        "POST /api/v2/batch?dry_run=false&limit=500 HTTP/1.1\r\nHost: api.example.com\r\nAuthorization: Bearer secret-token\r\nContent-Type: application/json\r\n\r\n{\"items\":[",
    );
    while doc.len() < 260 * 1024 {
        doc.push_str(
            "{\"id\":12345,\"name\":\"item\",\"active\":true,\"tags\":[\"alpha\",\"beta\"]},",
        );
    }
    doc.push_str("]}");
    let t0 = std::time::Instant::now();
    let mut hl = HttpSyntaxHighlighter::default();
    hl.parse_str(&doc);
    let spans = hl.styles_for_mode(&(0..8192), true);
    let elapsed = t0.elapsed();
    assert!(!spans.is_empty());
    assert!(
        elapsed < std::time::Duration::from_millis(250),
        "256 KiB HTTP+JSON highlight took {elapsed:?}"
    );

    let (np_doc, np_badges) = super::codec::format_non_printable_preview_with_badges(doc.as_bytes());
    let mut np_hl = HttpSyntaxHighlighter::with_badges(np_badges);
    np_hl.parse_str(&np_doc);
    let np_runs = np_hl.styles_for_mode(&(0..np_doc.len()), false);
    assert_valid_non_overlapping_runs(&np_doc, 0..np_doc.len(), &np_runs);
}
