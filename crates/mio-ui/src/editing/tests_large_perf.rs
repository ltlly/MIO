use std::time::Instant;

use gpui_kit::component::input::Editor;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Bounds, ClipboardItem, Point, Render, TestAppContext, WindowBounds, WindowOptions, size,
};

use super::*;

const STEP_BUDGET_MS: u128 = 1500;

struct LargePerfHarness {
    editor: MessageEditor,
}

impl Render for LargePerfHarness {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let selected = !self.editor.state().read(cx).selected_range().is_empty();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.editor.state())
                        .readonly(true)
                        .size_full()
                        .bordered(false)
                        .context_menu(move |menu, _, _| {
                            menu.submenu("转换选区", copy_converted_selection_submenu(!selected))
                        })
                        .aria_label("large-perf-editor"),
                ),
            )
            .child(
                self.editor
                    .render_search_bar("perf", cx, |this, action, window, cx| {
                        let _ = this.editor.handle_search_bar_action(action, window, cx);
                        cx.notify();
                    }),
            )
    }
}

fn open_perf_window(
    cx: &mut TestAppContext,
) -> (gpui_kit::AnyWindowHandle, Entity<LargePerfHarness>) {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(960.), px(640.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| LargePerfHarness {
                    editor: MessageEditor::new(window, cx),
                })
            },
        )
        .unwrap()
    })
}

fn build_512k_multiline_response() -> Vec<u8> {
    let target_len = 512 * 1024;
    let header =
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nX-Bench: 512k\r\n\r\n";
    let mut out = String::with_capacity(target_len);
    out.push_str(header);
    out.push_str("line=00000 token=PERF_ANCHOR_512K status=head\r\n");
    let mut line_no = 1usize;
    let mut mid_inserted = false;
    let tail_line = "line=tail token=PERF_ANCHOR_512K status=tail\r\n";
    while out.len() + tail_line.len() + 128 < target_len {
        if !mid_inserted && out.len() >= target_len / 2 {
            out.push_str("line=middle token=PERF_ANCHOR_512K status=mid\r\n");
            mid_inserted = true;
        } else {
            out.push_str(&format!(
                "line={line_no:05} payload=0123456789abcdef0123456789abcdef0123456789abcdef\r\n"
            ));
        }
        line_no += 1;
    }
    out.push_str(tail_line);
    if out.len() < target_len {
        let pad_len = target_len - out.len();
        out.push_str(&"x".repeat(pad_len));
    }
    assert_eq!(out.len(), target_len);
    out.into_bytes()
}

fn build_64k_single_line_json_response() -> Vec<u8> {
    let target_len = 64 * 1024;
    let header = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Bench: 64k-json\r\n\r\n";
    let mut body = String::with_capacity(target_len);
    body.push_str("{\"head\":\"JSON_NEEDLE_64K\",\"items\":[");
    let mut idx = 0usize;
    let mut mid_inserted = false;
    while header.len() + body.len() + 256 < target_len {
        if idx > 0 {
            body.push(',');
        }
        if !mid_inserted && header.len() + body.len() >= target_len / 2 {
            body.push_str("{\"id\":999999,\"tag\":\"JSON_NEEDLE_64K\"}");
            mid_inserted = true;
        } else {
            body.push_str(&format!("{{\"id\":{idx},\"v\":\"abcdef0123456789\"}}"));
        }
        idx += 1;
    }
    let suffix_prefix = "],\"tail\":\"JSON_NEEDLE_64K\",\"pad\":\"";
    let suffix_end = "\"}";
    let used = header.len() + body.len() + suffix_prefix.len() + suffix_end.len();
    assert!(used <= target_len);
    let pad = "p".repeat(target_len - used);
    body.push_str(suffix_prefix);
    body.push_str(&pad);
    body.push_str(suffix_end);
    assert!(!body.contains(['\r', '\n']));
    let mut full = String::with_capacity(target_len);
    full.push_str(header);
    full.push_str(&body);
    assert_eq!(full.len(), target_len);
    full.into_bytes()
}

#[gpui_kit::test]
fn large_512k_multiline_and_64k_single_line_json_render_switch_and_search_stay_bounded(
    cx: &mut TestAppContext,
) {
    let (window, harness) = open_perf_window(cx);
    let multiline_512k = build_512k_multiline_response();
    let single_line_64k = build_64k_single_line_json_response();

    cx.update_window(window, |_, window, cx| {
        let load_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.load(&multiline_512k, window, cx);
            if h.editor.pretty_active() {
                h.editor.switch(MessageView::Raw, window, cx).unwrap();
            }
        });
        window.render_frame(cx);
        let load_ms = load_start.elapsed().as_millis();
        assert!(
            load_ms < STEP_BUDGET_MS,
            "512 KiB Raw load+render exceeded budget: {load_ms}ms"
        );

        let search_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.search.update(cx, |input, cx| {
                input.set_value("PERF_ANCHOR_512K", window, cx)
            });
            h.editor.update_search(None, window, cx).unwrap();
            assert_eq!(h.editor.search_match_count(cx), 3);
            assert_eq!(h.editor.search_status_label(cx), "1/3 个匹配");
        });
        window.render_frame(cx);
        window.click("perf-search-next", cx);
        window.render_frame(cx);
        let search_ms = search_start.elapsed().as_millis();
        assert_eq!(
            harness.read(cx).editor.search_status_label(cx),
            "2/3 个匹配"
        );
        assert!(
            search_ms < STEP_BUDGET_MS,
            "512 KiB search+next+render exceeded budget: {search_ms}ms"
        );

        harness.update(cx, |h, cx| {
            h.editor
                .search
                .update(cx, |input, cx| input.set_value("", window, cx));
            h.editor.update_search(None, window, cx).unwrap();
        });

        let pretty_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.switch_pretty(window, cx).unwrap();
            assert!(h.editor.pretty_active());
        });
        window.render_frame(cx);
        let pretty_ms = pretty_start.elapsed().as_millis();
        assert!(
            pretty_ms < STEP_BUDGET_MS,
            "512 KiB Pretty switch+render exceeded budget: {pretty_ms}ms"
        );

        let hex_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.switch(MessageView::Hex, window, cx).unwrap();
            assert!(!h.editor.pretty_active());
            assert_eq!(h.editor.view(), MessageView::Hex);
        });
        window.render_frame(cx);
        let hex_ms = hex_start.elapsed().as_millis();
        assert!(
            hex_ms < STEP_BUDGET_MS,
            "512 KiB Hex switch+render exceeded budget: {hex_ms}ms"
        );

        let raw_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert_eq!(h.editor.view(), MessageView::Raw);
            assert_eq!(h.editor.bytes(window, cx).unwrap(), multiline_512k);
        });
        window.render_frame(cx);
        let raw_ms = raw_start.elapsed().as_millis();
        assert!(
            raw_ms < STEP_BUDGET_MS,
            "512 KiB Raw restore+render exceeded budget: {raw_ms}ms"
        );
    })
    .unwrap();

    cx.update_window(window, |_, window, cx| {
        let load_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.load(&single_line_64k, window, cx);
            if h.editor.pretty_active() {
                h.editor.switch(MessageView::Raw, window, cx).unwrap();
            }
        });
        window.render_frame(cx);
        let load_ms = load_start.elapsed().as_millis();
        assert!(
            load_ms < STEP_BUDGET_MS,
            "64 KiB single-line JSON load+render exceeded budget: {load_ms}ms"
        );

        let search_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.search.update(cx, |input, cx| {
                input.set_value("JSON_NEEDLE_64K", window, cx)
            });
            h.editor.update_search(None, window, cx).unwrap();
            assert_eq!(h.editor.search_match_count(cx), 3);
            assert_eq!(h.editor.search_status_label(cx), "1/3 个匹配");
        });
        window.render_frame(cx);
        window.click("perf-search-next", cx);
        window.render_frame(cx);
        let search_ms = search_start.elapsed().as_millis();
        assert_eq!(
            harness.read(cx).editor.search_status_label(cx),
            "2/3 个匹配"
        );
        assert!(
            search_ms < STEP_BUDGET_MS,
            "64 KiB single-line JSON search+next+render exceeded budget: {search_ms}ms"
        );

        let pretty_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.switch_pretty(window, cx).unwrap();
            assert!(h.editor.pretty_active());
            assert!(
                h.editor
                    .state()
                    .read(cx)
                    .value()
                    .contains("\n  \"head\": \"JSON_NEEDLE_64K\"")
            );
            assert_eq!(h.editor.search_match_count(cx), 3);
        });
        window.render_frame(cx);
        window.click("perf-search-next", cx);
        window.render_frame(cx);
        let pretty_ms = pretty_start.elapsed().as_millis();
        assert_eq!(
            harness.read(cx).editor.search_status_label(cx),
            "2/3 个匹配"
        );
        assert!(
            pretty_ms < STEP_BUDGET_MS,
            "64 KiB JSON Pretty switch+search+render exceeded budget: {pretty_ms}ms"
        );

        harness.update(cx, |h, cx| {
            h.editor
                .search
                .update(cx, |input, cx| input.set_value("", window, cx));
            h.editor.update_search(None, window, cx).unwrap();
        });

        let hex_raw_start = Instant::now();
        harness.update(cx, |h, cx| {
            h.editor.switch(MessageView::Hex, window, cx).unwrap();
        });
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            h.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert!(!h.editor.pretty_active());
            assert_eq!(h.editor.view(), MessageView::Raw);
            assert_eq!(h.editor.bytes(window, cx).unwrap(), single_line_64k);
        });
        window.render_frame(cx);
        let hex_raw_ms = hex_raw_start.elapsed().as_millis();
        assert!(
            hex_raw_ms < STEP_BUDGET_MS,
            "64 KiB JSON Hex->Raw roundtrip exceeded budget: {hex_raw_ms}ms"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn readonly_editor_converts_selection_to_clipboard_without_mutating_source(
    cx: &mut TestAppContext,
) {
    let (window, harness) = open_perf_window(cx);
    let raw_response =
        "HTTP/1.1 200 OK\r\n\r\nq=%E4%B8%AD%E6%96%87&b64=5Lit5paH&html=&lt;tag&gt;&hex=41e4b8ad";

    cx.update_window(window, |_, window, cx| {
        let state = harness.update(cx, |h, cx| {
            h.editor.load(raw_response.as_bytes(), window, cx);
            if h.editor.pretty_active() {
                h.editor.switch(MessageView::Raw, window, cx).unwrap();
            }
            h.editor.state()
        });
        window.render_frame(cx);
        let rev_before = state.read(cx).document_revision();
        assert!(!copy_converted_selection_submenu(false).is_empty());

        assert_eq!(
            convert_selected_editor_text(&state, ConvertKind::UrlDecode, window, cx),
            Err("请先在消息编辑器中选中要转换的文本。".into())
        );

        let url_span = "%E4%B8%AD%E6%96%87";
        let start = raw_response.find(url_span).unwrap();
        state.update(cx, |s, cx| {
            s.set_selected_range(start..start + url_span.len(), cx)
        });
        let decoded_url =
            convert_selected_editor_text(&state, ConvertKind::UrlDecode, window, cx).unwrap();
        assert_eq!(decoded_url, "中文");
        cx.write_to_clipboard(ClipboardItem::new_string(decoded_url));
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("中文".into())
        );

        let html_span = "&lt;tag&gt;";
        let html_start = raw_response.find(html_span).unwrap();
        state.update(cx, |s, cx| {
            s.set_selected_range(html_start..html_start + html_span.len(), cx);
        });
        let decoded_html =
            convert_selected_editor_text(&state, ConvertKind::HtmlDecode, window, cx).unwrap();
        assert_eq!(decoded_html, "<tag>");

        let hex_span = "41e4b8ad";
        let hex_start = raw_response.find(hex_span).unwrap();
        state.update(cx, |s, cx| {
            s.set_selected_range(hex_start..hex_start + hex_span.len(), cx);
        });
        let decoded_hex =
            convert_selected_editor_text(&state, ConvertKind::AsciiHexDecode, window, cx).unwrap();
        assert_eq!(decoded_hex, "A中");
        let encoded_b64 =
            convert_selected_editor_text(&state, ConvertKind::Base64Encode, window, cx).unwrap();
        assert_eq!(encoded_b64, "NDFlNGI4YWQ=");

        let b64_span = "5Lit5paH";
        let b64_start = raw_response.find(b64_span).unwrap();
        state.update(cx, |s, cx| {
            s.set_selected_range(b64_start..b64_start + b64_span.len(), cx);
        });
        let decoded_b64 =
            convert_selected_editor_text(&state, ConvertKind::Base64Decode, window, cx).unwrap();
        assert_eq!(decoded_b64, "中文");

        assert_eq!(state.read(cx).value().as_ref(), raw_response);
        assert_eq!(state.read(cx).document_revision(), rev_before);
    })
    .unwrap();
}
