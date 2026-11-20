use gpui_kit::component::input::Editor;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Bounds, EntityInputHandler, ParentElement, Point, Render, Styled, TestAppContext,
    Window, WindowBounds, WindowOptions, div, point, px, size,
};

use super::*;

struct SearchHarness {
    editor: MessageEditor,
    readonly: bool,
}

impl Render for SearchHarness {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let readonly = self.readonly;
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(self.editor.state())
                        .readonly(readonly)
                        .size_full()
                        .bordered(false)
                        .aria_label("search-test-editor"),
                ),
            )
            .child(self.editor.render_search_bar_editable(
                "test",
                readonly,
                cx,
                move |this, action, window, cx| {
                    let _ = this
                        .editor
                        .handle_search_bar_action_editable(action, readonly, window, cx);
                    cx.notify();
                },
            ))
    }
}

#[gpui_kit::test]
fn message_editor_search_handles_case_regex_navigation_raw_hex_and_ime(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, harness) = cx.update(|cx| {
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
                cx.new(|cx| SearchHarness {
                    editor: MessageEditor::new(window, cx),
                    readonly: true,
                })
            },
        )
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        let raw_bytes = b"GET /items?token=Alpha HTTP/1.1\r\nX-Token: alpha\r\n\r\nTOKEN=ALPHA";
        harness.update(cx, |harness, cx| {
            harness.editor.load(raw_bytes, window, cx);
            harness.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert_eq!(harness.editor.search_status_label(cx), "0 个匹配");

            harness.editor.raw.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(17..22, cx);
            });
            harness.editor.focus_search(window, cx).unwrap();
            assert_eq!(harness.editor.search.read(cx).value().as_ref(), "Alpha");

            harness.editor.search.update(cx, |input, cx| {
                input.set_value("token", window, cx);
            });
            harness.editor.focus_search(window, cx).unwrap();
            assert_eq!(harness.editor.search.read(cx).value().as_ref(), "token");
            harness.editor.search.update(cx, |input, cx| {
                input.set_value("Alpha", window, cx);
            });
            harness.editor.update_search(None, window, cx).unwrap();
            assert_eq!(harness.editor.search_match_count(cx), 3);
            assert_eq!(harness.editor.search_status_label(cx), "1/3 个匹配");

            harness
                .editor
                .update_search(Some(true), window, cx)
                .unwrap();
            assert_eq!(
                harness
                    .editor
                    .state()
                    .read(cx)
                    .search_session()
                    .matcher
                    .current_match_index(),
                1
            );
            assert_eq!(harness.editor.search_status_label(cx), "2/3 个匹配");

            harness
                .editor
                .update_search(Some(false), window, cx)
                .unwrap();
            assert_eq!(harness.editor.search_status_label(cx), "1/3 个匹配");

            harness.editor.toggle_case_sensitive(window, cx).unwrap();
            assert!(harness.editor.case_sensitive);
            assert_eq!(harness.editor.search_match_count(cx), 1);
            assert_eq!(harness.editor.search_status_label(cx), "1/1 个匹配");

            harness.editor.toggle_case_sensitive(window, cx).unwrap();
            assert!(!harness.editor.case_sensitive);
            assert_eq!(harness.editor.search_match_count(cx), 3);

            harness.editor.search.update(cx, |input, cx| {
                input.set_value(r"token=\w+", window, cx);
            });
            harness.editor.update_search(None, window, cx).unwrap();
            assert_eq!(harness.editor.search_match_count(cx), 0);

            harness.editor.toggle_regex(window, cx).unwrap();
            assert!(harness.editor.regex);
            assert_eq!(harness.editor.search_match_count(cx), 2);
            assert_eq!(harness.editor.search_status_label(cx), "1/2 个匹配");

            harness.editor.search.update(cx, |input, cx| {
                input.set_value("[unclosed", window, cx);
            });
            assert!(harness.editor.update_search(None, window, cx).is_err());
            assert_eq!(harness.editor.search_status_label(cx), "正则无效");

            harness.editor.search.update(cx, |input, cx| {
                input.select_all(window, cx);
                input.replace(r"token=\w+", window, cx);
            });
        });
    })
    .unwrap();
    cx.update_window(window, |_, window, cx| {
        let raw_bytes = b"GET /items?token=Alpha HTTP/1.1\r\nX-Token: alpha\r\n\r\nTOKEN=ALPHA";
        harness.update(cx, |harness, cx| {
            assert!(harness.editor.search_error.borrow().is_none());
            assert_eq!(harness.editor.search_status_label(cx), "1/2 个匹配");

            harness.editor.toggle_regex(window, cx).unwrap();
            assert!(!harness.editor.regex);
            assert!(harness.editor.search_error.borrow().is_none());

            harness.editor.search.update(cx, |input, cx| {
                input.set_value("41 6c 70 68 61", window, cx);
            });
            harness.editor.update_search(None, window, cx).unwrap();
            assert_eq!(harness.editor.search_match_count(cx), 0);

            harness.editor.switch(MessageView::Hex, window, cx).unwrap();
            assert_eq!(harness.editor.view, MessageView::Hex);
            assert_eq!(harness.editor.search_match_count(cx), 1);
            assert!(!harness.editor.raw.read(cx).search_session().is_active());
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), raw_bytes);

            harness.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), raw_bytes);

            harness.editor.search.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            });
            assert_eq!(
                harness.editor.update_search(None, window, cx),
                Err(COMPOSITION_PENDING.into())
            );
            assert_eq!(
                harness.editor.toggle_case_sensitive(window, cx),
                Err(COMPOSITION_PENDING.into())
            );
            assert!(!harness.editor.case_sensitive);
            assert_eq!(
                harness.editor.focus_search(window, cx),
                Err(COMPOSITION_PENDING.into())
            );
            assert_eq!(
                harness.editor.switch(MessageView::Hex, window, cx),
                Err(COMPOSITION_PENDING.into())
            );
            assert_eq!(harness.editor.view, MessageView::Raw);

            harness.editor.search.update(cx, |input, cx| {
                input.replace_text_in_range(None, "", window, cx);
                input.set_value("Alpha", window, cx);
            });
            harness.editor.update_search(None, window, cx).unwrap();
            assert_eq!(
                &*harness
                    .editor
                    .raw
                    .read(cx)
                    .search_session()
                    .matcher
                    .matched_ranges(),
                &[17..22, 42..47, 57..62]
            );
            harness.editor.raw.update(cx, |state, cx| {
                state.set_readonly(false, cx);
                state.set_selected_range(0..0, cx);
                state.replace_and_mark_text_in_range(None, "zh", Some(2..2), window, cx);
            });
            assert_eq!(
                &*harness
                    .editor
                    .raw
                    .read(cx)
                    .search_session()
                    .matcher
                    .matched_ranges(),
                &[19..24, 44..49, 59..64]
            );
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn message_editor_pretty_view_formats_json_supports_search_and_preserves_raw_bytes(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let (window, harness) = cx.update(|cx| {
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
                cx.new(|cx| SearchHarness {
                    editor: MessageEditor::new(window, cx),
                    readonly: true,
                })
            },
        )
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        let raw_bytes =
            b"POST /api HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"alpha\":\"item1\",\"beta\":\"item2\"}";
        harness.update(cx, |harness, cx| {
            harness.editor.load(raw_bytes, window, cx);
            if harness.editor.pretty_active {
                harness.editor.switch(MessageView::Raw, window, cx).unwrap();
            }
            assert_eq!(harness.editor.view, MessageView::Raw);
            assert!(!harness.editor.pretty_active);
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), raw_bytes);

            harness.editor.switch_pretty(window, cx).unwrap();
            assert!(harness.editor.pretty_active);
            assert_eq!(harness.editor.view, MessageView::Raw);
            let pretty_text = harness.editor.state().read(cx).value();
            assert!(pretty_text.contains("\n  \"alpha\": \"item1\""));
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), raw_bytes);

            harness.editor.search.update(cx, |input, cx| {
                input.set_value("item", window, cx);
            });
            harness.editor.update_search(None, window, cx).unwrap();
            assert_eq!(harness.editor.search_match_count(cx), 2);
            assert_eq!(harness.editor.search_status_label(cx), "1/2 个匹配");
            harness
                .editor
                .update_search(Some(true), window, cx)
                .unwrap();
            assert_eq!(harness.editor.search_status_label(cx), "2/2 个匹配");
            harness
                .editor
                .update_search(Some(false), window, cx)
                .unwrap();
            assert_eq!(harness.editor.search_status_label(cx), "1/2 个匹配");
            assert!(harness.editor.raw.read(cx).search_session().is_active());
            assert!(!harness.editor.hex.read(cx).search_session().is_active());

            harness.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert!(!harness.editor.pretty_active);
            assert_eq!(harness.editor.view, MessageView::Raw);
            assert!(harness.editor.pretty.read(cx).search_session().is_active());
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), raw_bytes);

            harness.editor.switch_pretty(window, cx).unwrap();
            assert!(harness.editor.pretty_active);
            harness.editor.switch(MessageView::Hex, window, cx).unwrap();
            assert!(!harness.editor.pretty_active);
            assert_eq!(harness.editor.view, MessageView::Hex);
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), raw_bytes);
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn literal_search_streams_across_multi_chunk_rope_boundaries(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, harness) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(720.), px(360.)),
                })),
                ..Default::default()
            },
            cx,
            |win, cx| {
                cx.new(|cx| SearchHarness {
                    editor: MessageEditor::new(win, cx),
                    readonly: true,
                })
            },
        )
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        let mut payload = String::from("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        for idx in 0..80 {
            payload.push_str(&format!("chunk-padding-{idx:04}-中文边界测试-"));
            if idx == 15 || idx == 45 || idx == 78 {
                payload.push_str("NEEDLE_TOKEN");
            }
        }
        harness.update(cx, |harness, cx| {
            harness.editor.load(payload.as_bytes(), window, cx);
            assert!(
                harness.editor.raw.read(cx).text().chunks().count() > 1,
                "multi-KiB payload must span multiple Rope chunks"
            );
            harness.editor.search.update(cx, |input, cx| {
                input.set_value("needle_token", window, cx);
            });
            harness.editor.update_search(None, window, cx).unwrap();
            assert_eq!(harness.editor.search_match_count(cx), 3);
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn message_editor_hide_headers_preview_supports_request_response_pretty_nonprintable_and_search(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let (window, harness) = cx.update(|cx| {
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
                cx.new(|cx| SearchHarness {
                    editor: MessageEditor::new(window, cx),
                    readonly: true,
                })
            },
        )
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        let req_bytes = b"POST /api/items HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\nContent-Length: 18\r\nContent-Type: application/json\r\nConnection: keep-alive\r\nSec-Fetch-Mode: cors\r\nX-Custom-Token:   keep  spaces  \r\nAccept-Encoding: gzip, deflate\r\n\r\n{\"token\":\"Accept\"}";
        harness.update(cx, |harness, cx| {
            harness.editor.load(req_bytes, window, cx);
            harness.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert!(!harness.editor.hide_headers);
            assert!(!harness.editor.is_readonly_preview());

            harness.editor.toggle_hide_headers(window, cx).unwrap();
            assert!(harness.editor.hide_headers);
            assert!(harness.editor.is_readonly_preview());
            let filtered_req = harness.editor.state().read(cx).value().to_string();
            assert_eq!(
                filtered_req,
                "POST /api/items HTTP/1.1\r\nHost: example.com\r\nContent-Length: 18\r\nContent-Type: application/json\r\nConnection: keep-alive\r\nX-Custom-Token:   keep  spaces  \r\n\r\n{\"token\":\"Accept\"}"
            );
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 2, 4, 5, 6, 8, 10, 11][..])
            );
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), req_bytes);
            assert_eq!(harness.editor.bytes_lossless(cx), req_bytes);
        });
        window.render_frame(cx);
        let scale = window.scale_factor();
        let (input_bounds, line_height) = {
            let state = harness.read(cx).editor.state().read(cx);
            (state.input_bounds(), state.line_height().unwrap())
        };
        let dividers: Vec<_> = window
            .painted_quads()
            .into_iter()
            .map(|quad| {
                quad.bounds
                    .intersect(&quad.content_mask.bounds)
                    .map(|v| px(v.0 / scale))
            })
            .filter(|b| {
                (b.size.height - px(1.)).abs() < px(0.5)
                    && b.size.width >= input_bounds.size.width - px(2.)
                    && b.origin.x <= input_bounds.origin.x
                    && b.origin.y > input_bounds.origin.y
                    && b.bottom() < input_bounds.bottom()
            })
            .collect();
        assert_eq!(
            dividers.len(),
            3,
            "hidden headers preview must paint 3 horizontal gap dividers at line jumps 2->4, 6->8 and 8->10"
        );
        assert!((dividers[0].origin.y - (input_bounds.origin.y + line_height * 2.)).abs() < px(1.));
        assert!((dividers[1].origin.y - (input_bounds.origin.y + line_height * 5.)).abs() < px(1.));
        assert!((dividers[2].origin.y - (input_bounds.origin.y + line_height * 6.)).abs() < px(1.));
        let narrow_gutter = harness
            .read(cx)
            .editor
            .state()
            .read(cx)
            .line_number_width()
            .unwrap();
        assert_eq!(narrow_gutter, px(23.));
        harness.update(cx, |harness, cx| {
            harness
                .editor
                .header_preview
                .as_ref()
                .unwrap()
                .update(cx, |state, cx| {
                    state.set_line_number_mapping(Some(vec![1, 2, 4, 5, 6, 8, 10, 120]), cx);
                });
        });
        window.render_frame(cx);
        let wide_gutter = harness
            .read(cx)
            .editor
            .state()
            .read(cx)
            .line_number_width()
            .unwrap();
        assert!(
            wide_gutter > narrow_gutter,
            "mapped 3-digit line number (120) must widen the gutter even when only 5 lines are displayed"
        );
        harness.update(cx, |harness, cx| {
            harness
                .editor
                .header_preview
                .as_ref()
                .unwrap()
                .update(cx, |state, cx| {
                    state.insert("!", window, cx);
                    assert_eq!(
                        state.line_number_mapping(),
                        None,
                        "editing buffer must automatically clear stale line_number_mapping"
                    );
                });
            harness.editor.refresh_header_preview(window, cx).unwrap();
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 2, 4, 5, 6, 8, 10, 11][..])
            );
        });
        harness.update(cx, |harness, cx| {

            harness.editor.search.update(cx, |input, cx| {
                input.set_value("Accept", window, cx);
            });
            harness.editor.update_search(None, window, cx).unwrap();
            assert_eq!(harness.editor.search_match_count(cx), 1);
            assert!(!harness.editor.raw.read(cx).search_session().is_active());

            harness.editor.switch_pretty(window, cx).unwrap();
            assert!(harness.editor.pretty_active);
            assert!(harness.editor.hide_headers);
            let pretty_filtered = harness.editor.state().read(cx).value().to_string();
            assert!(!pretty_filtered.contains("Sec-Fetch-Mode"));
            assert!(pretty_filtered.contains("Host: example.com"));
            assert!(pretty_filtered.contains("\n  \"token\": \"Accept\""));
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 2, 4, 5, 6, 8, 10, 11, 12, 13][..])
            );
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), req_bytes);

            harness.editor.toggle_non_printable(window, cx).unwrap();
            assert!(harness.editor.show_non_printable);
            assert!(harness.editor.hide_headers);
            let np_filtered = harness.editor.state().read(cx).value().to_string();
            assert!(!np_filtered.contains("Sec-Fetch-Mode"));
            assert!(np_filtered.contains("Host: example.com \\r   \\n "));
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 2, 4, 5, 6, 8, 10, 11][..])
            );
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), req_bytes);

            harness.editor.toggle_non_printable(window, cx).unwrap();
            assert!(!harness.editor.show_non_printable);
            assert!(harness.editor.hide_headers);
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 2, 4, 5, 6, 8, 10, 11, 12, 13][..])
            );

            harness
                .editor
                .set_hidden_headers(Some(vec!["X-Custom-Token".into()]), window, cx)
                .unwrap();
            let custom_filtered = harness.editor.state().read(cx).value().to_string();
            assert!(!custom_filtered.contains("X-Custom-Token"));
            assert!(custom_filtered.contains("Accept: */*"));
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13][..])
            );
            assert!(
                harness
                    .editor
                    .set_hidden_headers(Some(vec!["Bad Header".into()]), window, cx)
                    .is_err()
            );
            harness
                .editor
                .set_hidden_headers(None, window, cx)
                .unwrap();
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 2, 4, 5, 6, 8, 10, 11, 12, 13][..])
            );

            harness.editor.switch(MessageView::Hex, window, cx).unwrap();
            assert!(!harness.editor.hide_headers);
            assert_eq!(
                harness
                    .editor
                    .header_preview
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .line_number_mapping(),
                None
            );
            assert_eq!(harness.editor.view, MessageView::Hex);
            assert!(harness.editor.toggle_hide_headers(window, cx).is_err());

            let mut h2_resp = b"HTTP/2 200 OK\r\nDate: Tue, 17 Nov 2026 12:17:54 GMT\r\nServer: MIO-Test\r\nX-Retcode: SUCCESS\r\n\r\n".to_vec();
            h2_resp.extend_from_slice(&[0xff, 0x00, b'O', b'K']);
            harness.editor.load(&h2_resp, window, cx);
            assert_eq!(harness.editor.view, MessageView::Hex);
            harness.editor.toggle_non_printable(window, cx).unwrap();
            harness.editor.toggle_hide_headers(window, cx).unwrap();
            let h2_filtered = harness.editor.state().read(cx).value().to_string();
            assert!(h2_filtered.starts_with("HTTP/2 200 OK \\r   \\n \nX-Retcode: SUCCESS \\r   \\n "));
            assert!(!h2_filtered.contains("Server:"));
            assert!(h2_filtered.ends_with("\\xff\\x00OK"));
            assert_eq!(
                harness.editor.state().read(cx).line_number_mapping(),
                Some(&[1, 4, 5, 6][..])
            );
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), h2_resp);

            let malformed = b"NOT_HTTP_WITHOUT_NEWLINE";
            harness.editor.load(malformed, window, cx);
            assert!(harness.editor.hide_headers);
            assert!(harness.editor.header_preview_error.is_some());
            assert_eq!(harness.editor.state().read(cx).line_number_mapping(), None);
            assert!(harness.editor.state().read(cx).value().contains("不包含可解析的 HTTP 起始行"));
            assert_eq!(harness.editor.bytes(window, cx).unwrap(), malformed);
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn message_editor_search_auto_scroll_reveals_matches_on_query_and_text_changes_and_blocks_ime(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    let (window, harness) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(450.), px(330.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| SearchHarness {
                    editor: MessageEditor::new(window, cx),
                    readonly: true,
                })
            },
        )
        .unwrap()
    });

    let mut body = String::from("GET / HTTP/1.1\r\nHost: example.com\r\n\r\n");
    for row in 0..120 {
        if row == 60 {
            body.push_str("line=060 needle=FIRST_TARGET\r\n");
        } else if row == 95 {
            body.push_str("line=095 needle=FIRST_TARGET second=SECOND_TARGET\r\n");
        } else {
            body.push_str(&format!("line={row:03} padding=plain_text_row\r\n"));
        }
    }

    cx.update_window(window, |_, window, cx| {
        harness.update(cx, |h, cx| {
            h.editor.load(body.as_bytes(), window, cx);
            assert!(!h.editor.auto_scroll_to_match);
        });
        window.render_frame(cx);

        let bar = window.find("test-search-bar").bounds();
        assert_eq!(bar.size, size(px(450.), px(47.)));
        let rel = |id: &'static str| {
            let b = window.find(id).bounds();
            Bounds {
                origin: point(b.origin.x - bar.origin.x, b.origin.y - bar.origin.y),
                size: b.size,
            }
        };
        assert_eq!(
            rel("test-search-help"),
            Bounds {
                origin: point(px(5.), px(12.)),
                size: size(px(22.), px(22.)),
            }
        );
        assert_eq!(
            rel("test-search-options"),
            Bounds {
                origin: point(px(32.), px(12.)),
                size: size(px(22.), px(22.)),
            }
        );
        assert_eq!(
            rel("test-search-prev"),
            Bounds {
                origin: point(px(59.), px(11.)),
                size: size(px(24.), px(24.)),
            }
        );
        assert_eq!(
            rel("test-search-next"),
            Bounds {
                origin: point(px(88.), px(11.)),
                size: size(px(24.), px(24.)),
            }
        );
        assert_eq!(
            rel("test-search-input"),
            Bounds {
                origin: point(px(122.), px(12.)),
                size: size(px(228.), px(23.)),
            }
        );
        assert_eq!(
            rel("test-search-matches"),
            Bounds {
                origin: point(px(365.), px(10.)),
                size: size(px(80.), px(27.)),
            }
        );

        assert!(window.try_find("test-search-selection").is_none());
        harness.update(cx, |h, cx| {
            h.editor.raw.update(cx, |state, cx| {
                state.set_selected_range(0..28, cx);
            });
        });
        window.render_frame(cx);
        let sel_bounds = window.find("test-search-selection").bounds();
        assert!(sel_bounds.size.width > px(0.));
        harness.update(cx, |h, cx| {
            h.editor.raw.update(cx, |state, cx| {
                state.set_selected_range(0..0, cx);
            });
        });
        window.render_frame(cx);
        assert!(window.try_find("test-search-selection").is_none());

        window.click("test-search-options", cx);
        window.render_frame(cx);
        let menu = window.find("test-search-options-menu").bounds();
        assert_eq!(menu.size, size(px(272.), px(91.)));
        let menu_rel = |id: &'static str| {
            let b = window.find(id).bounds();
            Bounds {
                origin: point(
                    b.origin.x - (menu.origin.x + px(1.)),
                    b.origin.y - (menu.origin.y + px(4.)),
                ),
                size: b.size,
            }
        };
        assert_eq!(
            menu_rel("test-search-case"),
            Bounds {
                origin: point(px(5.), px(5.)),
                size: size(px(115.), px(21.)),
            }
        );
        assert_eq!(
            menu_rel("test-search-regex"),
            Bounds {
                origin: point(px(5.), px(31.)),
                size: size(px(66.), px(21.)),
            }
        );
        assert_eq!(
            menu_rel("test-search-autoscroll"),
            Bounds {
                origin: point(px(5.), px(57.)),
                size: size(px(260.), px(21.)),
            }
        );
        assert!(window.try_find("test-search-replace-toggle").is_none());

        harness.update(cx, |h, cx| {
            let visible = h.editor.raw.read(cx).visible_row_range().unwrap();
            assert!(visible.end < 50, "match row 63 must start off-screen: {visible:?}");
            h.editor.search.update(cx, |input, cx| {
                input.set_value("FIRST_TARGET", window, cx);
            });
            h.editor.update_search(None, window, cx).unwrap();
            assert_eq!(h.editor.search_match_count(cx), 2);
            assert_eq!(h.editor.search_status_label(cx), "1/2 个匹配");
        });
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            assert_eq!(h.editor.raw.read(cx).scroll_offset().y, px(0.));
        });

        window.click("test-search-autoscroll", cx);
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            assert!(h.editor.auto_scroll_to_match);
            assert_eq!(
                h.editor
                    .raw
                    .read(cx)
                    .search_session()
                    .matcher
                    .current_match_index(),
                0
            );
            assert!(h.editor.raw.read(cx).scroll_offset().y < px(0.));
            let visible = h.editor.raw.read(cx).visible_row_range().unwrap();
            assert!(visible.contains(&63), "first match row must be revealed: {visible:?}");

            h.editor.update_search(None, window, cx).unwrap();
            assert_eq!(
                h.editor
                    .raw
                    .read(cx)
                    .search_session()
                    .matcher
                    .current_match_index(),
                0
            );

            h.editor.raw.update(cx, |state, cx| {
                state.set_scroll_offset(point(px(0.), px(0.)), cx);
            });
        });
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            assert_eq!(h.editor.raw.read(cx).scroll_offset().y, px(0.));
            let mut updated = body.clone();
            updated.push_str("tail_row=120\r\n");
            h.editor.load(updated.as_bytes(), window, cx);
        });
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            assert!(h.editor.raw.read(cx).scroll_offset().y < px(0.));
            let visible = h.editor.raw.read(cx).visible_row_range().unwrap();
            assert!(
                visible.contains(&63),
                "text change via load must auto-scroll to match when enabled: {visible:?}"
            );

            h.editor.search.update(cx, |input, cx| {
                input.set_value("SECOND_TARGET", window, cx);
            });
            h.editor.update_search(None, window, cx).unwrap();
        });
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            let visible = h.editor.raw.read(cx).visible_row_range().unwrap();
            assert!(
                visible.contains(&98),
                "query change must auto-scroll to new match when enabled: {visible:?}"
            );

            h.editor.raw.update(cx, |state, cx| {
                state.set_readonly(false, cx);
                state.set_scroll_offset(point(px(0.), px(0.)), cx);
            });
            h.readonly = false;
            cx.notify();
        });
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            assert_eq!(h.editor.raw.read(cx).scroll_offset().y, px(0.));
            h.editor.raw.update(cx, |state, cx| {
                state.set_selected_range(0..0, cx);
                state.replace_text_in_range(None, "G", window, cx);
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        harness.update(cx, |h, cx| {
            let visible = h.editor.raw.read(cx).visible_row_range().unwrap();
            assert!(
                visible.contains(&98),
                "editing document text must auto-scroll to match when enabled: {visible:?}"
            );
            h.reads_editable_and_tests_ime(window, cx);
        });
    })
    .unwrap();
}

impl SearchHarness {
    fn reads_editable_and_tests_ime(&mut self, window: &mut Window, cx: &mut App) {
        self.readonly = false;
        self.editor.search.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
        });
        assert!(self.editor.auto_scroll_to_match);
        assert_eq!(
            self.editor.toggle_auto_scroll_to_match(window, cx),
            Err(COMPOSITION_PENDING.into())
        );
        assert!(
            self.editor.auto_scroll_to_match,
            "IME composition must block disabling auto_scroll_to_match"
        );
        self.editor.search.update(cx, |input, cx| {
            input.replace_text_in_range(None, "", window, cx);
        });

        self.editor.toggle_auto_scroll_to_match(window, cx).unwrap();
        assert!(!self.editor.auto_scroll_to_match);

        self.editor.raw.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "hao", Some(3..3), window, cx);
        });
        assert_eq!(
            self.editor.toggle_auto_scroll_to_match(window, cx),
            Err(COMPOSITION_PENDING.into())
        );
        assert!(
            !self.editor.auto_scroll_to_match,
            "IME composition must block enabling auto_scroll_to_match"
        );
        self.editor.raw.update(cx, |state, cx| {
            state.replace_text_in_range(None, "", window, cx);
        });
    }
}

#[gpui_kit::test]
fn message_editor_materializes_lazy_sub_editors_only_on_demand(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, harness) = cx.update(|cx| {
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
                cx.new(|cx| SearchHarness {
                    editor: MessageEditor::new(window, cx),
                    readonly: false,
                })
            },
        )
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        harness.update(cx, |h, cx| {
            assert!(h.editor.header_preview.is_none());
            assert!(h.editor.non_printable.is_none());
            assert!(h.editor.hex_grid.is_none());

            h.editor.load(b"GET / HTTP/1.1\r\nHost: a\r\n\r\nhi", window, cx);
            assert!(h.editor.header_preview.is_none());
            assert!(h.editor.non_printable.is_none());
            assert!(h.editor.hex_grid.is_none());

            h.editor.toggle_non_printable(window, cx).unwrap();
            assert!(h.editor.non_printable.is_some());
            assert!(h.editor.header_preview.is_none());
            assert!(h.editor.hex_grid.is_none());
            h.editor.toggle_non_printable(window, cx).unwrap();

            h.editor.toggle_hide_headers(window, cx).unwrap();
            assert!(h.editor.header_preview.is_some());
            assert!(h.editor.hex_grid.is_none());
            h.editor.toggle_hide_headers(window, cx).unwrap();

            h.editor.switch(MessageView::Hex, window, cx).unwrap();
            assert!(h.editor.hex_grid.is_some());
        });
    })
    .unwrap();
}
