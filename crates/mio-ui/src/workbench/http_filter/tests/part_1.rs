use super::*;

#[gpui_kit::test]
fn http_filter_search_term_toggles_presets_revert_and_ime_guard(cx: &mut TestAppContext) {
    let (mut f, rows) = boot(cx);
    let (handle, dialog) = open(&f, cx);
    let search_term = cx.read(|cx| dialog.read(cx).search_term.clone());
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(search_term.read(cx).presentation().placeholder().as_ref(), "");
            assert_eq!(search_term.read(cx).value().as_ref(), "");
            window.click("http-filter-search-term", cx);
            window.input("secret-token", cx);
            assert_eq!(search_term.read(cx).value().as_ref(), "secret-token");
        })
        .unwrap();
    cx.run_until_parked();
    for id in [
        "http-filter-regex",
        "http-filter-case-sensitive",
        "http-filter-negative",
    ] {
        click(handle, id, cx);
    }
    click(handle, "http-filter-apply", cx);
    let applied = query(&mut f);
    assert_eq!(
        applied.filter,
        HttpHistoryFilter {
            mime_types: [true, true, true, false, true, false, true, true],
            extensions: HttpExtensionFilter::Hide(HIDDEN_EXTENSIONS.into()),
            search_term: "secret-token".into(),
            regex: true,
            case_sensitive: true,
            negative_search: true,
            ..Default::default()
        }
    );
    reply(&f, &applied, rows.clone(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("http-history-filter").label(),
                Some("筛选设置：隐藏 CSS 与图像内容；隐藏指定扩展名；排除表达式 secret-token")
            );
        })
        .unwrap();

    // Hide all preserves search_term input and the three search flags.
    click(handle, "http-filter-hide-all", cx);
    cx.read(|cx| {
        let view = dialog.read(cx);
        assert!(view.draft.hide_without_response);
        assert!(view.draft.parameterized_only);
        assert_eq!(view.draft.status_classes, [false; 4]);
        assert_eq!(view.draft.extensions, HttpExtensionFilter::Any);
        assert_eq!(view.draft.search_term, "secret-token");
        assert_eq!(view.search_term.read(cx).value().as_ref(), "secret-token");
        assert_eq!(view.listener_port.read(cx).value().as_ref(), "");
        assert!(view.draft.regex && view.draft.case_sensitive && view.draft.negative_search);
    });

    // Show all clears search_term and resets the three search flags to false.
    click(handle, "http-filter-all", cx);
    cx.read(|cx| {
        let view = dialog.read(cx);
        assert_eq!(view.draft, HttpHistoryFilter::default());
        assert_eq!(view.search_term.read(cx).value().as_ref(), "");
        assert!(!view.draft.regex && !view.draft.case_sensitive && !view.draft.negative_search);
    });

    // Revert restores applied filter and syncs the search_term input.
    click(handle, "http-filter-revert", cx);
    cx.read(|cx| {
        let view = dialog.read(cx);
        assert_eq!(view.draft, applied.filter);
        assert_eq!(view.search_term.read(cx).value().as_ref(), "secret-token");
        assert_eq!(view.listener_port.read(cx).value().as_ref(), "");
    });

    // IME composition in search_term blocks Apply, Show all, Hide all, Revert, and Cancel.
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("http-filter-search-term", cx);
            search_term.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
            for button_id in [
                "http-filter-apply",
                "http-filter-all",
                "http-filter-hide-all",
                "http-filter-revert",
                "http-filter-cancel",
            ] {
                window.click(button_id, cx);
                window.render_frame(cx);
                assert!(
                    search_term.update(cx, |input, cx| input
                        .marked_text_range(window, cx)
                        .is_some()),
                    "{button_id} must preserve active IME composition"
                );
            }
            search_term.update(cx, |input, cx| {
                input.unmark_text(window, cx);
                input.set_value("中文搜索词", window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.read(|cx| cx.windows().contains(&handle)));
    assert!(drain(&mut f.commands).is_empty());

    click(handle, "http-filter-negative", cx);
    click(handle, "http-filter-apply-close", cx);
    let updated = query(&mut f);
    assert_eq!(updated.filter.search_term, "中文搜索词");
    assert!(!updated.filter.negative_search);
    reply(&f, &updated, rows, cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("http-history-filter").label(),
                Some("筛选设置：隐藏 CSS 与图像内容；隐藏指定扩展名；匹配表达式 中文搜索词")
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn http_filter_mime_types_listener_port_validation_presets_revert_and_viewport_bounds(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = boot(cx);
    let (handle, dialog) = open(&f, cx);
    let mime_ids = [
        "http-filter-mime-html",
        "http-filter-mime-script",
        "http-filter-mime-xml",
        "http-filter-mime-css",
        "http-filter-mime-other-text",
        "http-filter-mime-images",
        "http-filter-mime-flash",
        "http-filter-mime-other-binary",
    ];
    let button_ids = [
        "http-filter-all",
        "http-filter-hide-all",
        "http-filter-revert",
        "http-filter-cancel",
        "http-filter-apply",
        "http-filter-apply-close",
    ];
    let groups = [
        ("http-filter-request-group", 328., 123.),
        ("http-filter-mime-group", 299., 123.),
        ("http-filter-status-group", 235., 123.),
        ("http-filter-search-group", 253., 99.),
        ("http-filter-extension-group", 287., 99.),
        ("http-filter-annotation-group", 202., 99.),
        ("http-filter-listener-group", 112., 99.),
    ];
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let dialog_bounds = window.find("http-filter-dialog").bounds();
            assert_eq!(dialog_bounds.size.width, px(910.));
            assert_eq!(dialog_bounds.size.height, px(348.));
            for (id, w, h) in groups {
                let bounds = window.find(id).bounds();
                assert_eq!(bounds.size, size(px(w), px(h)), "{id} bounds mismatch");
            }
            assert_eq!(window.find("http-filter-settings-first-row").bounds().size, size(px(884.), px(123.)));
            assert_eq!(window.find("http-filter-settings-second-row").bounds().size, size(px(884.), px(99.)));
            assert_eq!(window.find("http-filter-search-term").bounds().size.height, px(23.));
            assert_eq!(window.find("http-filter-listener-port").bounds().size.height, px(23.));
            assert_eq!(window.find("http-filter-mode-settings").bounds().size, size(px(98.), px(21.)));
            assert_eq!(window.find("http-filter-mode-script").bounds().size, size(px(85.), px(21.)));
            let scale = window.scale_factor();
            for id in ["http-filter-mime-html", "http-filter-mime-script", "http-filter-2xx"] {
                let check_bounds = window.find(id).bounds();
                assert_eq!(window.find(id).checked(), Some(true));
                assert!(window.painted_quads().iter().any(|quad| {
                    let bounds = quad.bounds.map(|value| px(value.0 / scale));
                    quad.background == rgb(0x26649d).into()
                        && (bounds.size.width - px(14.)).abs() <= px(1.)
                        && (bounds.size.height - px(14.)).abs() <= px(1.)
                        && bounds.origin.x >= check_bounds.origin.x
                        && bounds.right() <= check_bounds.right()
                        && bounds.origin.y >= check_bounds.origin.y
                        && bounds.bottom() <= check_bounds.bottom()
                }), "{id} must paint a 14px blue checked indicator inside its bounds");
            }
            for id in mime_ids
                .into_iter()
                .chain(button_ids)
                .chain(["http-filter-listener-port"])
            {
                let target = window.find(id);
                assert!(target.visible(), "{id} must be visible in viewport");
                assert!(
                    target.bounds().bottom() <= dialog_bounds.bottom(),
                    "{id} must fit inside the filter dialog without scrolling"
                );
            }
        })
        .unwrap();

    let listener_port = cx.read(|cx| dialog.read(cx).listener_port.clone());
    for invalid in ["0", "70000", "abc"] {
        type_in_named(
            handle,
            "http-filter-listener-port",
            &listener_port,
            invalid,
            cx,
        );
        click(handle, "http-filter-apply", cx);
        assert!(
            drain(&mut f.commands).is_empty(),
            "invalid listener port {invalid} must not send a query"
        );
        cx.read(|cx| {
            let view = dialog.read(cx);
            assert_eq!(view.listener_port.read(cx).value().as_ref(), invalid);
            assert!(
                view.error
                    .as_ref()
                    .is_some_and(|error| error.contains("1–65535"))
            );
        });
        handle
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let dialog_bounds = window.find("http-filter-dialog").bounds();
                assert_eq!(
                    window.find("http-filter-error").role(),
                    Some(gpui_kit::accesskit::Role::Alert)
                );
                for id in button_ids {
                    let target = window.find(id);
                    assert!(target.visible());
                    assert!(target.bounds().bottom() <= dialog_bounds.bottom());
                }
            })
            .unwrap();
    }

    for (index, id) in mime_ids.into_iter().enumerate() {
        if index == 1 || index == 7 {
            click(handle, id, cx);
        }
    }
    type_in_named(
        handle,
        "http-filter-listener-port",
        &listener_port,
        "8080",
        cx,
    );
    click(handle, "http-filter-apply", cx);
    let applied = query(&mut f);
    assert_eq!(
        applied.filter.mime_types,
        [true, false, true, false, true, false, true, false]
    );
    assert_eq!(applied.filter.listener_port, Some(8080));
    reply(&f, &applied, rows.clone(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("http-history-filter").label(),
                Some("筛选设置：按 MIME 类型筛选；隐藏指定扩展名；Listener 端口 8080")
            );
        })
        .unwrap();

    click(handle, "http-filter-hide-all", cx);
    cx.read(|cx| {
        let view = dialog.read(cx);
        assert_eq!(view.draft.mime_types, [false; 8]);
        assert_eq!(view.draft.listener_port, None);
        assert_eq!(view.listener_port.read(cx).value().as_ref(), "");
    });

    type_in_named(
        handle,
        "http-filter-listener-port",
        &listener_port,
        "65535",
        cx,
    );
    click(handle, "http-filter-all", cx);
    cx.read(|cx| {
        let view = dialog.read(cx);
        assert_eq!(view.draft.mime_types, [true; 8]);
        assert_eq!(view.draft.listener_port, None);
        assert_eq!(view.listener_port.read(cx).value().as_ref(), "");
    });

    click(handle, "http-filter-revert", cx);
    cx.read(|cx| {
        let view = dialog.read(cx);
        assert_eq!(view.draft, applied.filter);
        assert_eq!(view.listener_port.read(cx).value().as_ref(), "8080");
    });

    type_in_named(handle, "http-filter-listener-port", &listener_port, "1", cx);
    click(handle, "http-filter-apply-close", cx);
    let min_port = query(&mut f);
    assert_eq!(min_port.filter.listener_port, Some(1));
    reply(&f, &min_port, rows, cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
}

#[gpui_kit::test]
fn http_filter_convert_to_script_validation_and_script_mode_apply(cx: &mut TestAppContext) {
    let (mut f, rows) = boot(cx);
    let (handle, dialog) = open(&f, cx);

    click(handle, "http-filter-in-scope-only", cx);
    click(handle, "http-filter-no-response", cx);
    click(handle, "http-filter-convert-to-script", cx);

    cx.read(|cx| {
        let view = dialog.read(cx);
        assert!(view.script_mode);
        let script = view.filter_script_editor.read(cx).value();
        assert!(script.contains("in_scope"));
        assert!(script.contains("has_response"));
    });

    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("http-filter-script-editor").visible());
            dialog.update(cx, |this, cx| {
                this.filter_script_editor
                    .update(cx, |editor, cx| editor.set_value("if (", window, cx));
            });
        })
        .unwrap();
    cx.run_until_parked();

    click(handle, "http-filter-apply", cx);
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        assert!(dialog.read(cx).error.is_some());
        assert_eq!(dialog.read(cx).filter_script_editor.read(cx).value().as_ref(), "if (");
    });

    click(handle, "http-filter-script-new", cx);
    click(handle, "http-filter-script-template", cx);
    cx.read(|cx| {
        assert_eq!(
            dialog.read(cx).filter_script_editor.read(cx).value().as_ref(),
            mio_core::DEFAULT_HTTP_FILTER_SCRIPT_TEMPLATE
        );
    });

    click(handle, "http-filter-apply-close", cx);
    let applied = query(&mut f);
    assert_eq!(
        applied.filter.filter_script,
        mio_core::DEFAULT_HTTP_FILTER_SCRIPT_TEMPLATE
    );
    assert!(!applied.filter.in_scope_only);
    assert!(!applied.filter.hide_without_response);
    reply(&f, &applied, rows, cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("http-history-filter").label(),
                Some("筛选设置：Script mode (脚本已启用)")
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn http_filter_script_library_save_load_overwrite_and_corrupted_file_error(
    cx: &mut TestAppContext,
) {
    let (f, _) = boot(cx);
    cx.read(|cx| {
        assert!(f.view.read(cx).http_filter.search_placeholder_initialized.get());
    });
    let (handle, dialog) = open(&f, cx);
    let qa_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local/qa-macos");
    std::fs::create_dir_all(&qa_root).unwrap();
    let qa_root = qa_root.canonicalize().unwrap();
    let dir = tempfile::TempDir::new_in(&qa_root).unwrap();
    let path = dir.path().join("http-filter-scripts.json");
    dialog.update(cx, |this, _| this.library_path = Some(path.clone()));

    handle.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("http-filter-github-scripts").is_none());
    }).unwrap();

    click(handle, "http-filter-mode-script", cx);
    click(handle, "http-filter-script-reference", cx);
    handle.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("http-filter-github-scripts").visible());
        assert!(window.find("http-filter-script-capabilities").visible());
        assert!(window.find("http-filter-script-signature").visible());
        assert_eq!(window.find("http-filter-script-save-library").bounds().size, size(px(160.), px(24.)));
        let editor = dialog.read(cx).filter_script_editor.read(cx);
        assert_eq!(editor.line_number_width(), Some(px(23.)));
        let first_char = editor.range_to_bounds(&(0..1)).expect("first char bounds");
        assert_eq!(first_char.origin.x - editor.input_bounds().origin.x, px(29.));
    }).unwrap();
    assert!(!path.exists());

    click(handle, "http-filter-script-load", cx);
    assert!(!path.exists());
    click(handle, "http-filter-library-cancel", cx);

    handle.update(cx, |_, window, cx| {
        dialog.update(cx, |this, cx| {
            this.filter_script_editor.update(cx, |editor, cx| editor.set_value("request.method == \"GET\"", window, cx));
        });
    }).unwrap();
    click(handle, "http-filter-script-save-library", cx);
    assert!(!path.exists());
    let name_input = cx.read(|cx| dialog.read(cx).library_name_input.clone());
    type_in_named(handle, "http-filter-library-name", &name_input, "GET only", cx);
    click(handle, "http-filter-library-save", cx);
    assert!(path.exists());
    cx.read(|cx| {
        assert!(dialog.read(cx).library_overlay.is_none());
        assert_eq!(dialog.read(cx).script_title, "GET only");
    });

    // Duplicate name requires explicit overwrite confirmation.
    handle.update(cx, |_, window, cx| {
        dialog.update(cx, |this, cx| {
            this.filter_script_editor.update(cx, |editor, cx| editor.set_value("response.status == 201", window, cx));
        });
    }).unwrap();
    click(handle, "http-filter-script-save-library", cx);
    click(handle, "http-filter-library-save", cx);
    handle.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("http-filter-library-overwrite-warning").visible());
    }).unwrap();
    click(handle, "http-filter-library-save", cx);
    cx.read(|cx| assert!(dialog.read(cx).library_overlay.is_none()));

    click(handle, "http-filter-script-new", cx);
    handle.update(cx, |_, window, cx| {
        dialog.update(cx, |this, cx| this.clear_script(window, cx));
    }).unwrap();
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).script_title, "Untitled");
        assert_eq!(dialog.read(cx).filter_script_editor.read(cx).value().as_ref(), "return true;");
    });

    click(handle, "http-filter-script-load", cx);
    click(handle, ("http-filter-library-entry", 0usize), cx);
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).script_title, "GET only");
        assert_eq!(dialog.read(cx).filter_script_editor.read(cx).value().as_ref(), "response.status == 201");
    });

    std::fs::write(&path, b"{corrupted").unwrap();
    click(handle, "http-filter-script-load", cx);
    cx.read(|cx| {
        assert!(dialog.read(cx).library_overlay.is_none());
        assert!(dialog.read(cx).error.is_some());
    });

    // Verify table layout column toggle preserves custom width & order and restores defaults.
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("history-options-menu", cx);
            window.render_frame(cx);
            assert!(window.within("popup-menu").find_by_label("表格列布局 (Table layout)").visible());
            assert!(window.within("popup-menu").find_by_label("连续自动滚屏 (Continuous scroll)").visible());
            assert!(window.within("popup-menu").find_by_label("清空历史").visible());
            f.view.update(cx, |view, cx| {
                assert_eq!(view.http_filter.all_columns.len(), 18);
                assert_eq!(
                    view.http_filter.all_columns[17].key.as_ref(),
                    "start_response_timer"
                );
                view.history.update(cx, |table, cx| {
                    table.delegate_mut().columns[1].width = px(240.);
                    table.delegate_mut().columns.swap(1, 2);
                    table.refresh(cx);
                });
                view.toggle_history_column_visibility("host", cx);
            });
            window.render_frame(cx);
            let cols: Vec<(String, f32)> = f
                .view
                .read(cx)
                .history
                .read(cx)
                .delegate()
                .columns
                .iter()
                .map(|c| (c.key.to_string(), c.width.as_f32()))
                .collect();
            assert_eq!(cols.len(), 17);
            assert!(!cols.iter().any(|(k, _)| k == "host"));
            assert_eq!(cols[1].0, "method");

            f.view.update(cx, |view, cx| {
                view.toggle_history_column_visibility("host", cx);
            });
            window.render_frame(cx);
            let cols_readded: Vec<(String, f32)> = f
                .view
                .read(cx)
                .history
                .read(cx)
                .delegate()
                .columns
                .iter()
                .map(|c| (c.key.to_string(), c.width.as_f32()))
                .collect();
            assert_eq!(cols_readded.len(), 18);
            assert_eq!(cols_readded[1].0, "method");
            assert_eq!(cols_readded[2], ("host".to_owned(), 240.));

            f.view.update(cx, |view, cx| {
                view.restore_default_history_columns(cx);
            });
            window.render_frame(cx);
            let cols_restored: Vec<(String, f32)> = f
                .view
                .read(cx)
                .history
                .read(cx)
                .delegate()
                .columns
                .iter()
                .map(|c| (c.key.to_string(), c.width.as_f32()))
                .collect();
            assert_eq!(cols_restored.len(), 18);
            assert_eq!(cols_restored[1], ("host".to_owned(), 187.));
            assert_eq!(cols_restored[2], ("method".to_owned(), 60.));
        })
        .unwrap();
}

#[gpui_kit::test]
fn http_history_continuous_scroll_row_menu_issues_mio_at_detached_window_and_docs(
    cx: &mut TestAppContext,
) {
    let (mut f, _) = boot(cx);
    f.window
        .update(cx, |_, window, _| {
            window.resize(gpui_kit::size(px(1280.), px(854.)));
        })
        .unwrap();
    let rows: Vec<HistoryRow> = (0..40)
        .rev()
        .map(|idx| {
            let mut rec = record(
                &format!("http://127.0.0.1:9001/item/{idx}?p={idx}"),
                "body",
            );
            rec.summary.source = TrafficSource::Proxy;
            rec.summary.started_unix_ms = 1_700_000_000_000 + idx as i64 * 10;
            HistoryRow::Live(rec.summary)
        })
        .collect();

    click(f.window, "history-filter", cx);
    let initial = query(&mut f);
    reply(&f, &initial, rows.clone(), cx);

    // Scroll down to row 30 while continuous_scroll is off.
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            f.view.update(cx, |view, cx| {
                view.history.update(cx, |table, cx| table.scroll_to_row(30, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let scrolled_y = cx.read(|cx| {
        f.view
            .read(cx)
            .history
            .read(cx)
            .vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .offset()
            .y
    });
    assert!(scrolled_y < px(0.), "scrolling to row 30 must move vertical offset negative");

    // Enable continuous scroll via history-options-menu and verify immediate scroll to newest row (0 in descending).
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("history-options-menu", cx);
            window.render_frame(cx);
            let item = window
                .within("popup-menu")
                .find_by_label("连续自动滚屏 (Continuous scroll)")
                .bounds()
                .center();
            window.pointer_down(item, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(item, gpui_kit::MouseButton::Left, cx);
            window.render_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(f.view.read(cx).http_filter.continuous_scroll);
        let y = f
            .view
            .read(cx)
            .history
            .read(cx)
            .vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .offset()
            .y;
        assert_eq!(y, px(0.), "enabling continuous scroll in descending sort must scroll to row 0");
    });

    // Scroll away again, then simulate HistoryChanged -> automatic refresh scrolls back to row 0.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.history.update(cx, |table, cx| table.scroll_to_row(30, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    f.events
        .try_send(Event::HistoryChanged { project: f.project })
        .unwrap();
    cx.run_until_parked();
    let auto_query = query(&mut f);
    let mut rows_with_new = rows.clone();
    let mut newest = record("http://127.0.0.1:9001/item/40?p=40", "newest");
    newest.summary.source = TrafficSource::Proxy;
    rows_with_new.insert(0, HistoryRow::Live(newest.summary));
    reply(&f, &auto_query, rows_with_new.clone(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
    cx.read(|cx| {
        let y = f
            .view
            .read(cx)
            .history
            .read(cx)
            .vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .offset()
            .y;
        assert_eq!(y, px(0.), "HistoryChanged with continuous scroll in descending sort must scroll to row 0");
    });

    // Switch to ascending sort on # column, then trigger HistoryChanged -> scrolls to bottom (last row).
    click(f.window, ("col-header", 0usize), cx);
    let asc_query = query(&mut f);
    assert!(asc_query.sort.ascending);
    let mut asc_rows = rows_with_new.clone();
    asc_rows.reverse();
    reply(&f, &asc_query, asc_rows.clone(), cx);
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.history.update(cx, |table, cx| table.scroll_to_row(0, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    f.events
        .try_send(Event::HistoryChanged { project: f.project })
        .unwrap();
    cx.run_until_parked();
    let asc_auto = query(&mut f);
    reply(&f, &asc_auto, asc_rows.clone(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let y = f
                .view
                .read(cx)
                .history
                .read(cx)
                .vertical_scroll_handle
                .0
                .borrow()
                .base_handle
                .offset()
                .y;
            assert!(
                y < px(0.),
                "HistoryChanged with continuous scroll in ascending # sort must scroll to bottom row"
            );
            f.view.update(cx, |view, cx| {
                view.history.update(cx, |table, cx| table.scroll_to_row(0, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();

    // Select row 0 and verify row context menu geometry and items: URL header, Add to scope, Send to MioAT, Record an issue, Show new history window, Copy URL, Save item, Add to site map, Documentation.
    click(f.window, "table-cell-0:0", cx);
    let _ = drain(&mut f.commands);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let menu_bounds = window.find("popup-menu").bounds();
            assert!(
                menu_bounds.size.height <= px(620.),
                "row context menu height {:?} must fit within 620pt without clipping bottom items",
                menu_bounds.size.height
            );
            let header = window.find(("history-row-url-header", 0usize));
            assert!(header.visible());
            assert_eq!(header.label(), Some("http://127.0.0.1:9001/item/0?p=0"));
            assert_eq!(header.bounds().size.height, px(21.));
            let org = window.find(("history-row-to-organiser", 0usize));
            assert_eq!(
                header.bounds().left(),
                org.bounds().left(),
                "URL header text must start at the same x column as regular menu item text"
            );
            assert!(window.within("popup-menu").find_by_label("添加到范围").visible());
            assert!(window.within("popup-menu").find_by_label("显示新历史记录窗口 (Show new history window)").visible());
            assert!(window.within("popup-menu").find_by_label("复制 URL").visible());
            assert!(window.within("popup-menu").find_by_label("导出选中项到文件 (Save item)").visible());
            assert!(window.within("popup-menu").find_by_label("添加到站点地图 (Add to site map)").visible());
            assert!(window.within("popup-menu").find_by_label("Proxy 历史记录文档 (Proxy history documentation)").visible());
            window.press("up", cx);
            window.render_frame(cx);
            assert!(window.within("popup-menu").find_by_label("Proxy 历史记录文档 (Proxy history documentation)").visible());
            window.press("down", cx);
            window.render_frame(cx);
            let ai_center = window.find(("history-row-to-mio-at", 0usize)).bounds().center();
            window.pointer_down(ai_center, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(ai_center, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let clip = cx
        .read(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .expect("Send to MioAT copies IPC CLI command");
    assert!(clip.contains("mio-cli --socket"));
    assert!(clip.contains("flows get"));
    assert!(clip.ends_with("--json"));

    // Test RecordManualFinding and AttachFlowToFinding actions + detached history window.
    let finding_id = mio_core::Uuid::new_v4();
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.record_manual_issue_from_history(Some(asc_rows[0].clone()), cx);
                view.attach_selected_history_to_issue(finding_id, Some(asc_rows[0].clone()), cx);
                view.open_detached_history_window(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(mio_core::AuditCommand::RecordManualFinding { finding, .. })
            if finding.url == "http://127.0.0.1:9001/item/0?p=0"
    )));
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(mio_core::AuditCommand::AttachFlowToFinding { finding_id: id, url, .. })
            if *id == finding_id && url == "http://127.0.0.1:9001/item/0?p=0"
    )));
    let detached = cx.read(|cx| {
        f.view
            .read(cx)
            .http_filter
            .detached_history_window
            .expect("detached history window opened")
    });
    detached
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("http-history-detached-window").visible());
            window.remove_window();
        })
        .unwrap();
    cx.run_until_parked();
}
