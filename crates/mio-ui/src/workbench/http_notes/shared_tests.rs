use super::*;

fn save_as(f: &Fixture, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("shift-s"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
}

fn compose(f: &Fixture, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).http_notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            editor.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(Some(0..5), "zhong", Some(5..5), window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn http_project_saved_receipt_autosaves_newer_editor_text(cx: &mut TestAppContext) {
    let (mut f, first, _) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selected = read(&mut f);
    loaded(&f, selected, "first", cx);
    input(&f, "submitted", cx);
    let pending = write(&mut f);
    f.events
        .try_send(Event::ExtHttpNotesChanged {
            project: f.project,
            operation_id: pending.0,
            source_id: first.0,
            item_id: first.1,
            result: Err(ExtHttpNotesError {
                message: "source read only".into(),
                stored: Some(ExtHttpNotes {
                    revision: 0,
                    text: "first".into(),
                }),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    save_as(&f, cx);
    let destination = PathBuf::from("/tmp/mio-http-notes-shared-newer-save.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::SaveProject {
            ext_http_notes,
            websocket_notes,
            path: Some(path),
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must submit the HTTP note once");
    };
    assert_eq!(path, &destination);
    assert!(websocket_notes.is_empty());
    assert_eq!(ext_http_notes, &[pending.1]);

    input(&f, "newer 中文🙂", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "the batch owns the in-flight note revision"
    );
    f.events
        .try_send(Event::ProjectSaved {
            project: f.project,
            path: destination,
            total_bytes: 4096,
            websocket_notes: vec![],
            ext_http_notes: vec![(first.0, first.1, 1)],
            ext_http_highlights: vec![],
        })
        .unwrap();
    cx.run_until_parked();
    let newer = write(&mut f);
    assert_eq!((newer.1.source_id, newer.1.item_id), first);
    assert_eq!(newer.1.expected_notes_revision, 1);
    assert_eq!(newer.1.text, "newer 中文🙂");
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.http_notes.editor.read(cx).value().as_ref(),
            "newer 中文🙂"
        );
        assert!(view.http_notes_unsaved(cx));
    });
    ack(&f, newer.0, &newer.1, 2, cx);
    cx.read(|cx| assert!(!f.view.read(cx).http_notes_unsaved(cx)));
}

#[gpui_kit::test]
fn http_save_as_picker_cancel_resumes_editor_autosave(cx: &mut TestAppContext) {
    let (mut f, first, _) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selected = read(&mut f);
    loaded(&f, selected, "first", cx);
    save_as(&f, cx);
    input(&f, "typed while choosing", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "the open picker must defer source writes"
    );
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, Command::SaveProject { .. }))
    );
    let [
        Command::SetExtHttpNotes {
            operation_id,
            change,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("cancelling the picker must resume the deferred HTTP note write");
    };
    assert_eq!((change.source_id, change.item_id), first);
    assert_eq!(change.expected_notes_revision, 0);
    assert_eq!(change.text, "typed while choosing");
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.choosing_save_as);
        assert!(view.file_picker.is_none());
        assert_eq!(
            view.http_notes.editor.read(cx).value().as_ref(),
            change.text
        );
    });
    ack(&f, *operation_id, change, 1, cx);
    cx.read(|cx| assert!(!f.view.read(cx).http_notes_unsaved(cx)));
}

#[gpui_kit::test]
fn http_notes_ime_rejected_selection_restores_table_row(cx: &mut TestAppContext) {
    let (mut f, first, _) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selected = read(&mut f);
    loaded(&f, selected, "first", cx);
    compose(&f, cx);
    select_row(&f, 1, cx);
    f.window
        .update(cx, |_, window, cx| {
            let view = f.view.read(cx);
            assert_eq!(
                view.selected_history,
                Some(HistoryEntryId::External {
                    source_id: first.0,
                    item_id: first.1
                })
            );
            assert_eq!(
                view.history.read(cx).selected_row(),
                Some(0),
                "a refused switch must keep the visible selection on the note's owner"
            );
            let editor = view.http_notes.editor.clone();
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.value().as_ref(), "zhong");
                assert!(editor.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn http_notes_markdown_toolbar_and_preview_toggle_format_without_ime_or_source_mutation(
    cx: &mut TestAppContext,
) {
    let (mut f, _, _) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selected = read(&mut f);
    loaded(&f, selected, "hello", cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).http_notes.editor.clone();
            editor.update(cx, |state, cx| state.set_selected_range(0..5, cx));
            window.render_frame(cx);
            assert_eq!(window.find("http-notes-preview-toggle").label(), Some("T"));
            assert_eq!(window.find("http-notes-preview-toggle").checked(), Some(false));
            window.click("http-notes-bold", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let bold_write = write(&mut f);
    assert_eq!(bold_write.1.text, "**hello**");
    ack(&f, bold_write.0, &bold_write.1, 1, cx);
    click(&f, "http-notes-preview-toggle", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("http-notes-markdown-preview").is_some());
            assert_eq!(window.find("http-notes-preview-toggle").label(), Some("T"));
            assert_eq!(window.find("http-notes-preview-toggle").checked(), Some(true));
            assert_eq!(
                f.view.read(cx).http_notes.editor.read(cx).value().as_ref(),
                "**hello**"
            );
        })
        .unwrap();
    click(&f, "http-notes-preview-toggle", cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).http_notes.editor.clone();
            editor.update(cx, |state, cx| state.set_selected_range(0..9, cx));
            window.render_frame(cx);
            assert_eq!(window.find("http-notes-preview-toggle").label(), Some("T"));
            assert_eq!(window.find("http-notes-preview-toggle").checked(), Some(false));
            window.click("http-notes-strikethrough", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let strike_write = write(&mut f);
    assert_eq!(strike_write.1.text, "~~**hello**~~");
    ack(&f, strike_write.0, &strike_write.1, 2, cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).http_notes.editor.clone();
            editor.update(cx, |state, cx| {
                state.set_value("hello", window, cx);
                state.set_selected_range(0..5, cx);
            });
            window.render_frame(cx);
            window.click("http-notes-format-link", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let link_write = write(&mut f);
    assert_eq!(link_write.1.text, "[hello](https://)");
    ack(&f, link_write.0, &link_write.1, 3, cx);
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).http_notes.editor.clone();
            let len = editor.read(cx).value().len();
            editor.update(cx, |state, cx| state.set_selected_range(0..len, cx));
            window.render_frame(cx);
            window.click("http-notes-numbered-list", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let num_list_write = write(&mut f);
    assert_eq!(num_list_write.1.text, "1. [hello](https://)");
    ack(&f, num_list_write.0, &num_list_write.1, 4, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("http-notes-markdown-preview").is_none());
            assert!(window.try_find("http-notes-code").is_none());
            let sidebar_bounds = window.find("http-notes-sidebar").bounds();
            let toolbar_bounds = window.find("http-notes-toolbar").bounds();
            let rule_bounds = window.find("http-notes-toolbar-bottom-rule").bounds();
            assert_eq!(sidebar_bounds.size.width, px(343.));
            assert_eq!(toolbar_bounds.size, gpui_kit::size(px(333.), px(38.)));
            assert_eq!(rule_bounds.size, gpui_kit::size(px(343.), px(1.)));
            assert_eq!(rule_bounds.origin.x, sidebar_bounds.origin.x);
            assert_eq!(rule_bounds.bottom(), toolbar_bounds.bottom());
            let mut prev_right = toolbar_bounds.origin.x;
            for id in [
                "http-notes-preview-toggle",
                "http-notes-heading",
                "http-notes-bold",
                "http-notes-italic",
                "http-notes-strikethrough",
                "http-notes-list",
                "http-notes-numbered-list",
                "http-notes-format-link",
                "http-notes-open-window",
            ] {
                let btn = window.find(id);
                assert!(btn.visible(), "{id} must be visible in Notes toolbar");
                let bounds = btn.bounds();
                assert!(bounds.origin.x >= prev_right, "{id} must be ordered left-to-right");
                assert!(
                    bounds.right() <= toolbar_bounds.right(),
                    "{id} must fit inside the 343px Notes toolbar without right clipping"
                );
                prev_right = bounds.right();
            }
            let scale = window.scale_factor();
            let mut dividers: Vec<gpui_kit::Pixels> = window
                .painted_quads()
                .into_iter()
                .map(|quad| {
                    quad.bounds
                        .intersect(&quad.content_mask.bounds)
                        .map(|v| px(v.0 / scale))
                })
                .filter(|b| {
                    b.origin.x >= toolbar_bounds.origin.x
                        && b.right() <= toolbar_bounds.right()
                        && b.origin.y >= toolbar_bounds.origin.y
                        && b.bottom() <= toolbar_bounds.bottom()
                        && (b.size.width - px(1.)).abs() < px(0.5)
                        && (b.size.height - px(20.)).abs() < px(0.5)
                })
                .map(|b| b.origin.x)
                .collect();
            dividers.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(dividers.len(), 4, "Notes toolbar must render 4 vertical 1x20px dividers");
            let groups = [
                ("http-notes-preview-toggle", "http-notes-heading"),
                ("http-notes-strikethrough", "http-notes-list"),
                ("http-notes-numbered-list", "http-notes-format-link"),
                ("http-notes-format-link", "http-notes-open-window"),
            ];
            for (sep_x, (left_id, right_id)) in dividers.into_iter().zip(groups) {
                let left_right = window.find(left_id).bounds().right();
                let right_left = window.find(right_id).bounds().origin.x;
                assert!(
                    sep_x >= left_right && sep_x + px(1.) <= right_left,
                    "vertical divider must sit between {left_id} and {right_id}"
                );
            }
            assert_eq!(window.find("http-notes-preview-toggle").label(), Some("T"));
            assert_eq!(window.find("http-notes-bold").label(), Some("B"));
            assert_eq!(window.find("http-notes-italic").label(), Some("I"));
            assert_eq!(window.find("http-notes-strikethrough").label(), Some("S"));
        })
        .unwrap();
}

#[test]
fn http_notes_heading_levels_preserve_markdown_content_and_plain_text() {
    use super::render::{NoteMarkdownAction, format_markdown_snippet};

    let source = "### **标题🙂** ###";
    for level in 0..=6 {
        let (text, offset, length) = format_markdown_snippet(
            NoteMarkdownAction::HeadingLevel(level),
            source,
            0..source.len(),
        );
        let expected = if level == 0 {
            "**标题🙂**".to_owned()
        } else {
            format!("{} **标题🙂**", "#".repeat(usize::from(level)))
        };
        assert_eq!(text, expected);
        assert_eq!(&text[offset..offset + length], "**标题🙂**");
    }
    let source = "# first\r\n## second";
    let (text, _, _) = format_markdown_snippet(
        NoteMarkdownAction::HeadingLevel(4),
        source,
        0..source.len(),
    );
    assert_eq!(text, "#### first\r\n#### second");
    let (text, _, _) = format_markdown_snippet(
        NoteMarkdownAction::HeadingLevel(0),
        "#literal",
        0..8,
    );
    assert_eq!(text, "#literal");
}

#[gpui_kit::test]
fn http_notes_heading_dropdown_sets_all_levels_and_restores_paragraph(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selected = read(&mut f);
    loaded(&f, selected, "before\n标题🙂\nafter", cx);
    f.window.update(cx, |_, window, cx| {
        let editor = f.view.read(cx).http_notes.editor.clone();
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(9..9, cx);
        });
        window.render_frame(cx);
        assert_eq!(window.find("http-notes-heading").label(), Some("Aa"));
        assert_eq!(window.find("http-notes-heading").bounds().size, gpui_kit::size(px(56.), px(27.)));
        assert_eq!(window.find("http-notes-format-link").bounds().size, gpui_kit::size(px(19.), px(19.)));
        assert_eq!(window.find("http-notes-open-window").bounds().size, gpui_kit::size(px(15.), px(15.)));
    }).unwrap();

    for level in 1..=6 {
        click(&f, "http-notes-heading", cx);
        f.window.update(cx, |_, window, cx| {
            window.render_frame(cx);
            for label in ["Aa", "H1", "H2", "H3", "H4", "H5", "H6"] {
                assert!(window.find(SharedString::from(format!("http-notes-heading-{label}"))).visible());
            }
        }).unwrap();
        click(&f, &format!("http-notes-heading-H{level}"), cx);
        let submitted = write(&mut f);
        assert_eq!((submitted.1.source_id, submitted.1.item_id), first);
        assert_eq!(submitted.1.text, format!("before\n{} 标题🙂\nafter", "#".repeat(level)));
        ack(&f, submitted.0, &submitted.1, level as u64, cx);
        f.window.update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("http-notes-heading").label(), Some(format!("H{level}").as_str()));
            assert_eq!(f.view.read(cx).http_notes.editor.read(cx).selected_value().as_ref(), "标题🙂");
        }).unwrap();
    }
    click(&f, "http-notes-heading", cx);
    click(&f, "http-notes-heading-Aa", cx);
    let submitted = write(&mut f);
    assert_eq!(submitted.1.text, "before\n标题🙂\nafter");
    ack(&f, submitted.0, &submitted.1, 7, cx);
    f.window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("http-notes-heading").label(), Some("Aa"));
        let editor = f.view.read(cx).http_notes.editor.clone();
        editor.update(cx, |editor, cx| {
            editor.focus(window, cx);
            editor.replace_and_mark_text_in_range(None, "候选", Some(2..2), window, cx);
        });
        let before = f.view.read(cx).http_notes.editor.read(cx).value();
        f.view.update(cx, |view, cx| {
            view.apply_http_notes_markdown(super::render::NoteMarkdownAction::HeadingLevel(2), window, cx);
        });
        assert_eq!(f.view.read(cx).http_notes.editor.read(cx).value(), before);
    }).unwrap();
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn http_notes_detached_window_shares_editor_autosave_preview_and_close(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selected = read(&mut f);
    loaded(&f, selected, "original note", cx);
    let editor = cx.read(|cx| f.view.read(cx).http_notes.editor.clone());
    click(&f, "http-notes-open-window", cx);
    let detached = cx.read(|cx| f.view.read(cx).http_notes.detached_window.unwrap());
    click(&f, "http-notes-open-window", cx);
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).http_notes.detached_window, Some(detached));
        assert_eq!(f.view.read(cx).http_notes.editor, editor);
    });
    detached.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("http-notes-detached-window").visible());
        assert!(window.find("http-notes-heading").visible());
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("detached 中文🙂", cx);
    }).unwrap();
    cx.run_until_parked();
    let submitted = write(&mut f);
    assert_eq!((submitted.1.source_id, submitted.1.item_id), first);
    assert_eq!(submitted.1.text, "detached 中文🙂");
    ack(&f, submitted.0, &submitted.1, 1, cx);
    detached.update(cx, |_, window, cx| {
        editor.update(cx, |editor, cx| editor.set_selected_range(0..editor.value().len(), cx));
        window.render_frame(cx);
        window.click("http-notes-bold", cx);
    }).unwrap();
    cx.run_until_parked();
    let submitted = write(&mut f);
    assert_eq!(submitted.1.text, "**detached 中文🙂**");
    ack(&f, submitted.0, &submitted.1, 2, cx);
    detached.update(cx, |_, window, cx| {
        window.render_frame(cx);
        window.click("http-notes-preview-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("http-notes-markdown-preview").visible());
        assert_eq!(f.view.read(cx).http_notes.editor.read(cx).value().as_ref(), "**detached 中文🙂**");
        window.click("http-notes-close", cx);
    }).unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(!cx.windows().contains(&detached)));
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).http_notes.detached_window, None);
        assert_eq!(f.view.read(cx).http_notes.editor, editor);
    });
    f.window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("http-notes-markdown-preview").visible());
        assert_eq!(f.view.read(cx).http_notes.editor.read(cx).value().as_ref(), "**detached 中文🙂**");
    }).unwrap();
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn http_notes_ime_rejected_history_page_clears_table_loading(cx: &mut TestAppContext) {
    let (mut f, first, second) = fixture_notes(cx);
    click(&f, "http-notes-toggle", cx);
    let selected = read(&mut f);
    loaded(&f, selected, "first", cx);
    click(&f, "history-filter", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = commands.as_slice()
    else {
        panic!("refresh must issue one history query");
    };
    cx.read(|cx| assert!(f.view.read(cx).history.read(cx).delegate().loading));
    compose(&f, cx);
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 1,
            rows: vec![row(second.0, second.1, 0)],
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            let view = f.view.read(cx);
            assert!(!view.history_loading);
            assert!(
                !view.history.read(cx).delegate().loading,
                "a rejected page has finished loading even when its old rows are retained"
            );
            assert_eq!(
                view.selected_history,
                Some(HistoryEntryId::External {
                    source_id: first.0,
                    item_id: first.1
                })
            );
            assert_eq!(view.history.read(cx).selected_row(), Some(0));
            assert_eq!(view.history.read(cx).delegate().rows.len(), 2);
            let editor = view.http_notes.editor.clone();
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.value().as_ref(), "zhong");
                assert!(editor.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
}
