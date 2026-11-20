use super::*;

#[gpui_kit::test]
fn failed_filter_keeps_offpage_selection_and_success_prunes_only_excluded_ids(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = setup(cx);
    select_and_load(&mut f, 0, &rows[0], false, cx);
    select_and_load(&mut f, 1, &rows[1], true, cx);
    let ids = [rows[0].id(), rows[1].id()];
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(2, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let next = query(&mut f);
    reply(
        &f,
        &next,
        rows[2..].to_vec(),
        4,
        ids.map(|id| (id, true)).to_vec(),
        cx,
    );
    click(f.window, "http-history-filter", cx);
    let dialog = cx.read(|cx| f.view.read(cx).http_filter.window.unwrap());
    click(dialog, "http-filter-highlight", cx);
    click(dialog, "http-filter-apply", cx);
    let failed = query(&mut f);
    assert!(failed.filter.only_highlighted);
    f.events
        .try_send(Event::HttpHistoryPage {
            query: failed.clone(),
            result: Err("controlled query failure".into()),
        })
        .unwrap();
    cx.run_until_parked();
    assert_selection(&f, &ids, rows[1].id(), cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.http_filter.applied.only_highlighted);
        assert!(!view.history_loading);
        assert_eq!(view.history.read(cx).delegate().rows, rows[2..]);
        assert_eq!(
            view.history.read(cx).delegate().selection_anchor,
            Some(rows[1].id())
        );
        assert!(view.history_request.view == MessageView::Hex);
        assert_eq!(
            view.history_request.hex.read(cx).value().as_ref(),
            editing::hex_encode(REQUEST)
        );
    });

    click(dialog, "http-filter-apply-close", cx);
    let success = query(&mut f);
    let mut remaining = rows[1].clone();
    if let HistoryRow::ExtProxy(row) = &mut remaining {
        row.highlight = Some(ExtHttpHighlight {
            revision: 1,
            color: 3,
        });
    }
    reply(
        &f,
        &success,
        vec![remaining],
        1,
        vec![(ids[1], true), (ids[0], false)],
        cx,
    );
    assert_selection(&f, &[ids[1]], ids[1], cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.http_filter.applied.only_highlighted);
        assert_eq!(view.history.read(cx).selected_row(), Some(0));
        assert_eq!(
            view.history.read(cx).delegate().selected_order,
            vec![ids[1]]
        );
        assert_eq!(
            view.history.read(cx).delegate().selection_anchor,
            Some(ids[1])
        );
        assert!(view.history_request.view == MessageView::Hex);
        assert_eq!(
            view.history_request.hex.read(cx).value().as_ref(),
            editing::hex_encode(REQUEST)
        );
        assert!(!cx.windows().contains(&dialog));
    });
    assert!(
        drain(&mut f.commands).is_empty(),
        "a metadata-only filter must not reread or resend the selected request"
    );

    // Filtering out the remaining row (empty table result) clears table selection but retains the displayed message editor and Inspector content.
    let url_input = cx.read(|cx| f.view.read(cx).history_filter.clone());
    f.window
        .update(cx, |_, window, cx| {
            url_input.update(cx, |input, cx| {
                input.set_value("__no_match_in_history__", window, cx);
            });
            window.click("history-filter", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let empty_query = query(&mut f);
    assert_eq!(empty_query.url_filter, "__no_match_in_history__");
    reply(
        &f,
        &empty_query,
        vec![],
        0,
        vec![(ids[1], false)],
        cx,
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.selected_history, None);
        assert!(view.history.read(cx).selected_row().is_none());
        assert!(view.history.read(cx).delegate().rows.is_empty());
        assert_eq!(
            view.history_request.hex.read(cx).value().as_ref(),
            editing::hex_encode(REQUEST)
        );
        assert!(view.imported_history.row.is_some());
        let HistoryEntryId::External { source_id, item_id } = ids[1] else {
            unreachable!()
        };
        assert!(!source_id.is_nil());
        assert_eq!(view.selected_http_notes(), Some((source_id, item_id)));
        assert_eq!(view.http_filter.applied_url_filter, "__no_match_in_history__");
    });
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("http-notes-toggle").disabled(), None);
            assert_eq!(window.find("history-filter").label(), Some("清除筛选"));
            let h_handle = &f.view.read(cx).history.read(cx).horizontal_scroll_handle;
            assert!(
                h_handle.max_offset().x > px(0.),
                "empty HTTP history table must retain positive horizontal scroll range from header columns"
            );
        })
        .unwrap();
    click(f.window, "http-notes-toggle", cx);
    let note_cmd = drain(&mut f.commands);
    let HistoryEntryId::External {
        source_id: expected_source,
        item_id: expected_item,
    } = ids[1]
    else {
        unreachable!()
    };
    assert!(
        matches!(
            note_cmd.as_slice(),
            [Command::ReadExtHttpNotes { source_id, item_id, .. }]
                if *source_id == expected_source && *item_id == expected_item
        ),
        "retained imported row must read notes with its real source_id/item_id (len={})",
        note_cmd.len()
    );
    // Clicking the clear button on history-filter clears the search input and dispatches an unfiltered query.
    click(f.window, "history-filter", cx);
    let cleared_query = query(&mut f);
    assert_eq!(cleared_query.url_filter, "");
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).history_filter.read(cx).value().as_ref(), "");
    });
}

fn live_offpage(
    cx: &mut TestAppContext,
) -> (Fixture, HistoryRow, HistoryRow, mio_core::FlowRecord, u64) {
    let mut f = fixture(cx, vec![], 64);
    let source = Uuid::new_v4();
    let archive = row(source, 0);
    let second = row(source, 1);
    let live = record("http://127.0.0.1:9002/live", "initial");
    let live_id = HistoryEntryId::Live(live.summary.id);
    let initial = query(&mut f);
    reply(
        &f,
        &initial,
        vec![archive.clone(), live.summary.clone().into()],
        3,
        vec![],
        cx,
    );
    click(f.window, "proxy-history", cx);
    assert!(drain(&mut f.commands).is_empty());
    select_and_load(&mut f, 0, &archive, false, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click_with_modifiers("table-cell-1:0", crate::secondary_modifiers(true), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            flow_id,
            selection_revision,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("selecting Live requests its exact flow once")
    };
    assert_eq!(*flow_id, live.summary.id);
    let selection_revision = *selection_revision;
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(2, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let next = query(&mut f);
    reply(
        &f,
        &next,
        vec![second.clone()],
        3,
        vec![(archive.id(), true), (live_id, true)],
        cx,
    );
    assert_selection(&f, &[archive.id(), live_id], live_id, cx);
    (f, archive, second, live, selection_revision)
}

fn transfer_after_live_refresh(
    f: &mut Fixture,
    second: &HistoryRow,
    live_id: Uuid,
    cx: &mut TestAppContext,
) {
    select_and_load(f, 0, second, true, cx);
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
            let at = window
                .within("popup-menu")
                .find_by_label("发送到 Repeater")
                .bounds()
                .center();
            window.pointer_down(at, MouseButton::Left, cx);
            window.pointer_up(at, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::ReadHistoryTransfer { entries, .. }] = commands.as_slice() else {
        panic!("one read-only transfer batch")
    };
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().any(|entry| matches!(entry, mio_core::HistoryTransferEntry::Live { flow_id, revision: 2, .. } if *flow_id == live_id)));
}

#[gpui_kit::test]
fn late_live_detail_refreshes_offpage_selected_revision_before_transfer(cx: &mut TestAppContext) {
    let (mut f, _, second, mut live, selection_revision) = live_offpage(cx);
    let live_id = HistoryEntryId::Live(live.summary.id);
    live.summary.revision = 2;
    live.summary.status = Some(204);
    live.summary.state = mio_core::FlowState::Complete;
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision,
            record: Box::new(live.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.detail.as_ref().unwrap().summary.revision, 2);
        let cached = view
            .history
            .read(cx)
            .delegate()
            .row_by_id(live_id)
            .unwrap()
            .live()
            .unwrap();
        assert_eq!(
            cached.revision, 2,
            "a late detail receipt must advance the off-page selection revision"
        );
    });
    assert!(drain(&mut f.commands).is_empty());
    transfer_after_live_refresh(&mut f, &second, live.summary.id, cx);
}

#[gpui_kit::test]
fn history_refresh_updates_offpage_live_revision_without_reading_its_body(cx: &mut TestAppContext) {
    let (mut f, archive, second, mut live, _) = live_offpage(cx);
    let live_id = HistoryEntryId::Live(live.summary.id);
    live.summary.revision = 2;
    live.summary.status = Some(204);
    live.summary.state = mio_core::FlowState::Complete;
    f.events
        .try_send(Event::HistoryChanged { project: f.project })
        .unwrap();
    cx.run_until_parked();
    let refresh = query(&mut f);
    assert_eq!(refresh.offset, 2);
    f.events
        .try_send(Event::HttpHistoryPage {
            query: refresh,
            result: Ok(HttpHistoryResult {
                total: 3,
                rows: vec![second.clone()],
                selected_matches: vec![(archive.id(), true), (live_id, true)],
                selected_ranks: vec![(archive.id(), 0), (live_id, 1)],
                selected_rows: vec![archive.clone(), HistoryRow::Live(live.summary.clone())],
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert_selection(&f, &[archive.id(), live_id], live_id, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        let cached = view
            .history
            .read(cx)
            .delegate()
            .row_by_id(live_id)
            .unwrap()
            .live()
            .unwrap();
        assert_eq!(
            (cached.revision, cached.status),
            (2, Some(204)),
            "a page reply must refresh selected rows outside that page"
        );
        assert!(
            view.detail.is_none(),
            "summary refresh must not fabricate or request a body"
        );
    });
    assert!(
        drain(&mut f.commands).is_empty(),
        "off-page summary refresh must not fetch message bodies"
    );
    transfer_after_live_refresh(&mut f, &second, live.summary.id, cx);
}

#[gpui_kit::test]
fn deleting_filtered_out_live_detail_clears_editors_and_disables_notes_while_preserving_retained_imported_row(
    cx: &mut TestAppContext,
) {
    let (mut f, archive, _second, live, selection_revision) = live_offpage(cx);
    let live_flow_id = live.summary.id;
    let HistoryEntryId::External {
        source_id: archive_source_id,
        item_id: archive_item_id,
    } = archive.id()
    else {
        unreachable!()
    };

    // Deliver the live FlowDetail so the bottom request/response editors and Inspector hold the live flow.
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision,
            record: Box::new(live.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();

    // Filter out all rows so selected_history becomes None while live detail remains retained in the bottom pane.
    let url_input = cx.read(|cx| f.view.read(cx).history_filter.clone());
    f.window
        .update(cx, |_, window, cx| {
            url_input.update(cx, |input, cx| {
                input.set_value("__no_match_live__", window, cx);
            });
            window.click("history-filter", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let empty_live_query = query(&mut f);
    reply(
        &f,
        &empty_live_query,
        vec![],
        0,
        vec![(archive.id(), false), (HistoryEntryId::Live(live_flow_id), false)],
        cx,
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.selected_history, None);
        assert!(view.detail.is_some());
        assert_eq!(view.selected_http_notes(), Some((Uuid::nil(), live_flow_id)));
        assert!(!view.history_request.raw.read(cx).value().is_empty());
    });

    // Open Notes for the retained live detail and load its note text.
    click(f.window, "http-notes-toggle", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotes {
            selection_revision: note_rev,
            source_id,
            item_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("expected one ReadExtHttpNotes for retained live detail");
    };
    assert!(source_id.is_nil());
    assert_eq!(*item_id, live_flow_id);
    f.events
        .try_send(Event::ExtHttpNotes {
            project: f.project,
            selection_revision: *note_rev,
            source_id: *source_id,
            item_id: *item_id,
            result: Ok(mio_core::ExtHttpNotes {
                revision: 1,
                text: "live retained note".into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.http_notes.editor.read(cx).value().as_ref(),
            "live retained note"
        );
    });

    // Trigger ClearLiveProxyHistory and deliver its HttpHistoryDeleted receipt; editors, detail, and Notes must clear and disable.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.clear_live_proxy_history(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let clear_cmds_1 = drain(&mut f.commands);
    let [Command::ClearLiveProxyHistory { operation_id: clear_op_1, .. }] = clear_cmds_1.as_slice() else {
        panic!("expected ClearLiveProxyHistory command");
    };
    let clear_op_1 = *clear_op_1;
    f.events
        .try_send(Event::HttpHistoryDeleted {
            project: f.project,
            operation_id: clear_op_1,
            deleted_ids: vec![live_flow_id],
            result: Ok(1),
        })
        .unwrap();
    cx.run_until_parked();
    let post_delete_query = query(&mut f);
    reply(&f, &post_delete_query, vec![], 0, vec![], cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.detail.is_none());
        assert_eq!(view.selected_http_notes(), None);
        assert_eq!(view.http_notes.active, None);
        assert_eq!(view.http_notes.editor.read(cx).value().as_ref(), "");
        assert_eq!(view.history_request.raw.read(cx).value().as_ref(), "");
    });
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("http-notes-toggle").disabled(), Some(true));
        })
        .unwrap();

    // Clear search to bring back the imported archive row, select it, filter it out, and verify deleting unrelated live flows keeps imported detail & Notes intact.
    click(f.window, "history-filter", cx);
    let restore_query = query(&mut f);
    reply(&f, &restore_query, vec![archive.clone()], 1, vec![], cx);
    select_and_load(&mut f, 0, &archive, false, cx);
    f.window
        .update(cx, |_, window, cx| {
            url_input.update(cx, |input, cx| {
                input.set_value("__hide_imported__", window, cx);
            });
            window.click("history-filter", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let hide_imported_query = query(&mut f);
    reply(
        &f,
        &hide_imported_query,
        vec![],
        0,
        vec![(archive.id(), false)],
        cx,
    );
    let unrelated_live_id = Uuid::new_v4();
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.clear_live_proxy_history(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let clear_cmds_2 = drain(&mut f.commands);
    let [Command::ClearLiveProxyHistory { operation_id: clear_op_2, .. }] = clear_cmds_2.as_slice() else {
        panic!("expected ClearLiveProxyHistory command");
    };
    let clear_op_2 = *clear_op_2;
    f.events
        .try_send(Event::HttpHistoryDeleted {
            project: f.project,
            operation_id: clear_op_2,
            deleted_ids: vec![unrelated_live_id],
            result: Ok(1),
        })
        .unwrap();
    cx.run_until_parked();
    let after_unrelated_delete = query(&mut f);
    reply(&f, &after_unrelated_delete, vec![], 0, vec![], cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.imported_history.row.is_some());
        assert_eq!(
            view.selected_http_notes(),
            Some((archive_source_id, archive_item_id))
        );
        assert_eq!(
            view.history_request.hex.read(cx).value().as_ref(),
            editing::hex_encode(REQUEST)
        );
    });
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_ne!(window.find("http-notes-toggle").disabled(), Some(true));
        })
        .unwrap();
}
