use super::*;

#[gpui_kit::test]
fn result_context_menu_highlight_updates_revision_and_clears_to_none(cx: &mut TestAppContext) {
    use gpui_kit::test::TestEventExt;

    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let mut first = flow(0);
    first.summary.status = Some(200);
    first.summary.state = mio_core::FlowState::Complete;
    let mut second = flow(1);
    second.summary.status = Some(200);
    second.summary.state = mio_core::FlowState::Complete;
    bootstrap(
        &mut fixture,
        &mut started,
        &[first.clone(), second.clone()],
        cx,
    );

    let choose_highlight = |fixture: &mut Fixture, color: usize, cx: &mut TestAppContext| {
        cx.update_window(started.window, |_, window, cx| {
            window.render_frame(cx);
            window.right_click("table-cell-0:0", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let reads = drain(&mut fixture.commands);
        assert!(reads.iter().all(
            |cmd| matches!(cmd, Command::ReadFlow { flow_id, .. } if *flow_id == first.summary.id)
        ));
        if let Some(Command::ReadFlow {
            selection_revision, ..
        }) = reads.first()
        {
            detail(fixture, *selection_revision, &first, cx);
        }
        cx.update_window(started.window, |_, window, cx| {
            window.render_frame(cx);
            let pos = window
                .within("popup-menu")
                .find_by_label("Highlight")
                .bounds()
                .center();
            window.pointer_down(pos, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(pos, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(started.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("intruder-highlight-color", color), cx);
        })
        .unwrap();
        cx.run_until_parked();
    };

    choose_highlight(&mut fixture, 1, cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResultHighlight {
            project,
            run_id,
            entries,
            highlight,
            operation_id,
        },
    ] = commands.as_slice()
    else {
        panic!("expected SetIntruderResultHighlight command for color 1");
    };
    assert_eq!(
        (*project, *run_id, entries.as_slice(), *highlight),
        (fixture.project, started.summary.id, &[(0, 0)][..], Some(1))
    );
    emit(
        &fixture,
        Event::IntruderResultHighlight {
            project: fixture.project,
            operation_id: *operation_id,
            run_id: started.summary.id,
            updated: vec![(0, 1)],
            highlight: Some(1),
            result: Ok(()),
        },
        cx,
    );
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("table-cell-0:7").label(), Some("1"));
        let row = fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .query
            .row_at(0)
            .unwrap();
        assert_eq!((row.highlight, row.highlight_revision), (Some(1), 1));
    })
    .unwrap();

    choose_highlight(&mut fixture, 4, cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResultHighlight {
            entries,
            highlight,
            operation_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("expected SetIntruderResultHighlight command with updated revision");
    };
    assert_eq!((entries.as_slice(), *highlight), (&[(0, 1)][..], Some(4)));
    emit(
        &fixture,
        Event::IntruderResultHighlight {
            project: fixture.project,
            operation_id: *operation_id,
            run_id: started.summary.id,
            updated: vec![(0, 2)],
            highlight: Some(4),
            result: Ok(()),
        },
        cx,
    );
    cx.read(|cx| {
        let row = fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .query
            .row_at(0)
            .unwrap();
        assert_eq!((row.highlight, row.highlight_revision), (Some(4), 2));
    });

    choose_highlight(&mut fixture, 0, cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResultHighlight {
            entries,
            highlight,
            operation_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("expected SetIntruderResultHighlight clear command");
    };
    assert_eq!((entries.as_slice(), *highlight), (&[(0, 2)][..], None));
    emit(
        &fixture,
        Event::IntruderResultHighlight {
            project: fixture.project,
            operation_id: *operation_id,
            run_id: started.summary.id,
            updated: vec![(0, 3)],
            highlight: None,
            result: Ok(()),
        },
        cx,
    );
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("table-cell-0:7").label(), Some("None"));
        let row = fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .query
            .row_at(0)
            .unwrap();
        assert_eq!((row.highlight, row.highlight_revision), (None, 3));
    })
    .unwrap();
}

#[gpui_kit::test]
fn result_notes_dialog_preserves_draft_on_cas_error_and_updates_cell_on_ack(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestEventExt;

    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let first = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&first), cx);

    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("table-cell-0:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let reads = drain(&mut fixture.commands);
    if let [
        Command::ReadFlow {
            selection_revision, ..
        },
    ] = reads.as_slice()
    {
        detail(&fixture, *selection_revision, &first, cx);
    }
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.within("popup-menu").find_by_label("编辑备注…");
        window.pointer_down(item.bounds().center(), gpui_kit::MouseButton::Left, cx);
        window.pointer_up(item.bounds().center(), gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let notes_window = cx.read(|cx| {
        fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .notes_window
            .expect("notes dialog window must open")
    });
    let note_text = "命中中文备注：越权成功";
    cx.update_window(notes_window, |_, window, cx| {
        window.render_frame(cx);
        window.click("intruder-notes-input", cx);
        window.input(note_text, cx);
        window.click("intruder-notes-save", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResultNotes {
            project,
            run_id,
            ordinal,
            expected_notes_revision,
            notes,
            operation_id,
        },
    ] = commands.as_slice()
    else {
        panic!("expected SetIntruderResultNotes command");
    };
    assert_eq!(
        (*project, *run_id, *ordinal, *expected_notes_revision),
        (fixture.project, started.summary.id, 0, 0)
    );
    assert_eq!(notes.as_slice(), note_text.as_bytes());

    let cas_err = "Intruder 结果备注 revision 已改变；未覆盖（当前 1）";
    emit(
        &fixture,
        Event::IntruderResultNotes {
            project: fixture.project,
            operation_id: *operation_id,
            run_id: started.summary.id,
            ordinal: 0,
            notes_revision: 0,
            notes: Vec::new(),
            result: Err(cas_err.into()),
        },
        cx,
    );
    assert!(cx.windows().contains(&notes_window));
    cx.update_window(notes_window, |_, window, cx| {
        window.render_frame(cx);
        let err = window.find("intruder-notes-error");
        assert!(err.visible());
        assert_eq!(err.label(), Some(cas_err));
        assert_eq!(window.find("intruder-notes-input").value(), Some(note_text));
    })
    .unwrap();

    // Retry after CAS error and receive committed receipt
    click(notes_window, "intruder-notes-save", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResultNotes {
            notes: retry_notes,
            operation_id: retry_op,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("expected retry SetIntruderResultNotes command");
    };
    assert_eq!(retry_notes.as_slice(), note_text.as_bytes());
    emit(
        &fixture,
        Event::IntruderResultNotes {
            project: fixture.project,
            operation_id: *retry_op,
            run_id: started.summary.id,
            ordinal: 0,
            notes_revision: 1,
            notes: note_text.as_bytes().to_vec(),
            result: Ok(()),
        },
        cx,
    );
    assert!(
        !cx.windows().contains(&notes_window),
        "notes dialog must close after acknowledged save"
    );
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("table-cell-0:6").label(), Some(note_text));
        let view = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert!(view.notes_window.is_none());
        let row = view.query.row_at(0).unwrap();
        assert_eq!(
            (row.notes_revision, row.notes_preview.as_str()),
            (1, note_text)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn result_filter_dialog_applies_only_with_notes_and_only_highlighted(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let first = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&first), cx);

    click(started.window, "intruder-result-view-filter", cx);
    let filter_window = cx.read(|cx| {
        fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .query
            .filter_window
            .expect("filter dialog must open")
    });
    cx.update_window(filter_window, |_, window, cx| {
        window.render_frame(cx);
        window.click("intruder-result-filter-notes", cx);
        window.click("intruder-result-filter-highlighted", cx);
        window.click("intruder-result-filter-apply", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderResultView { query, cancel }] = commands.as_slice() else {
        panic!("applying annotation filter must emit QueryIntruderResultView");
    };
    assert!(!cancel.is_cancelled());
    assert_eq!(
        query.filter,
        mio_core::IntruderResultFilter {
            status_classes: [true; 4],
            search_term: String::new(),
            regex: false,
            case_sensitive: false,
            negative_search: false,
            only_with_notes: true,
            only_highlighted: true,
            only_grep_matched: false,
        }
    );
}

#[gpui_kit::test]
fn unselected_rows_render_persisted_grep_columns_and_export_all_loaded_rows(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let tab_id = cx.read(|cx| fixture.view.read(cx).workspace.intruder.active.unwrap());
    let grep_cfg = mio_core::IntruderGrepConfig {
        match_expressions: vec!["admin_ok".into()],
        match_case_sensitive: false,
        extract_rules: vec![mio_core::IntruderGrepExtractRule::delimited("uid=", ";")],
        grep_payloads: true,
        grep_payloads_case_sensitive: false,
    };
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.workspace
                .intruder
                .grep_configs
                .insert(tab_id, grep_cfg.clone());
        });
    });
    let mut started = start(&mut fixture, cx);
    started.summary.revision += 1;
    started.summary.total = 2;
    started.summary.dispatched = 2;
    started.summary.completed = 2;
    let mut first = flow(0);
    first.summary.status = Some(200);
    first.summary.state = mio_core::FlowState::Complete;
    let mut second = flow(1);
    second.summary.status = Some(200);
    second.summary.state = mio_core::FlowState::Complete;
    page(
        &fixture,
        &started,
        started.probe,
        vec![row(0, &first)],
        Some(0),
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    let initial_query = view_query(&fixture, started.summary.id, revision, cx);
    assert_eq!(initial_query.grep_config, grep_cfg);
    let mut page = build_view_page(
        &initial_query,
        &started.summary,
        vec![row(0, &first), row(1, &second)],
        None,
        true,
    );
    page.rows[0].payloads = vec![mio_core::IntruderPayloadPreview {
        set_index: 0,
        payload_index: 0,
        raw_bytes: 4,
        prefix: b"p_01".to_vec(),
    }];
    page.rows[0].grep_matches = vec!["admin_ok".into()];
    page.rows[0].grep_extracts = vec![("uid=..;".into(), "42".into())];
    page.rows[0].grep_payload_hits = vec![true];
    page.rows[1].payloads = vec![mio_core::IntruderPayloadPreview {
        set_index: 0,
        payload_index: 1,
        raw_bytes: 4,
        prefix: b"p_02".to_vec(),
    }];
    page.rows[1].grep_matches = Vec::new();
    page.rows[1].grep_extracts = vec![("uid=..;".into(), "99".into())];
    page.rows[1].grep_payload_hits = vec![false];

    emit(
        &fixture,
        Event::IntruderResultView {
            query: initial_query,
            result: Ok(page),
        },
        cx,
    );
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: started.summary.id,
            selection_revision: 1,
            result: Ok(Box::new(started.spec.clone())),
        },
        cx,
    );

    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "table",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(-1200.), gpui_kit::px(0.))),
            cx,
        );
        window.render_frame(cx);
        // Neither row 0 nor row 1 has detail_response_raw loaded; both render from persisted fields
        assert_eq!(window.find("table-cell-0:8").label(), Some("是"));
        assert_eq!(window.find("table-cell-0:9").label(), Some("42"));
        assert_eq!(window.find("table-cell-0:10").label(), Some("是"));
        assert_eq!(window.find("table-cell-1:8").label(), Some("否"));
        assert_eq!(window.find("table-cell-1:9").label(), Some("99"));
        assert_eq!(window.find("table-cell-1:10").label(), Some("否"));

        window.click("intruder-copy-results-tsv", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    let lines: Vec<&str> = copied.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("匹配: admin_ok"));
    assert!(lines[0].contains("提取: uid=..;"));
    assert!(lines[0].contains("回显 (Payloads)"));
    assert!(lines[1].ends_with("是\t42\t是"));
    assert!(lines[2].ends_with("否\t99\t否"));
}
