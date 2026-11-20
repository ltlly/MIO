use super::*;

#[gpui_kit::test]
fn production_scroll_reads_one_bounded_page_and_rejects_old_body_and_page(cx: &mut TestAppContext) {
    let mut attack = attack();
    attack.spec.payload_sets[0] = (0..500)
        .map(|index| index.to_string().into_bytes())
        .collect();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let mut started = start(&mut fixture, cx);
    let flows: Vec<_> = (0..500).map(flow).collect();
    bootstrap(&mut fixture, &mut started, &flows, cx);
    let old_body = select(&mut fixture, &started, 0, &flows[0], cx);
    cx.update_window(started.window, |_, window, cx| {
        window.scroll(
            "intruder-results-table",
            ScrollDelta::Pixels(point(px(0.), px(-8000.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let (forward, after) = query(&mut fixture, started.summary.id);
    let offset = after.unwrap() as usize + 1;
    let forward_query = view_query(&fixture, started.summary.id, forward, cx);
    assert!((PAGE_SIZE..500).contains(&offset));
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.query.pending.as_ref().unwrap().query.offset, offset);
        assert_eq!(result.query.page.as_ref().unwrap().offset, 0);
        assert!(result.detail.is_none());
        assert_eq!(
            result.selected,
            Some(0),
            "paging retains the same selected request"
        );
    });
    detail(&fixture, old_body, &flows[0], cx);
    let end = (offset + PAGE_SIZE).min(flows.len());
    let mut forward_page = build_view_page(
        &forward_query,
        &started.summary,
        (offset..end)
            .map(|ordinal| row(ordinal as u64, &flows[ordinal]))
            .collect(),
        (end < flows.len()).then_some((end - 1) as u64),
        true,
    );
    forward_page.total = flows.len();
    forward_page.next_offset = (end < flows.len()).then_some(end);
    forward_page.selected_rank = Some(0);
    let selected = row(0, &flows[0]);
    forward_page.selected_row = Some(IntruderResultViewRow {
        ordinal: 0,
        result: Some(selected.clone()),
        payloads: vec![],
        response_length: match &selected.outcome {
            IntruderResultOutcome::Flow(flow) if flow.status.is_some() => Some(flow.response_bytes),
            _ => None,
        },
        notes_revision: 0,
        notes_preview: String::new(),
        highlight: None,
        highlight_revision: 0,
        grep_matches: Vec::new(),
        grep_extracts: Vec::new(),
        grep_payload_hits: Vec::new(),
    });
    emit(
        &fixture,
        Event::IntruderResultView {
            query: forward_query.clone(),
            result: Ok(forward_page),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(started.window, |_, window, cx| {
        window.scroll(
            "intruder-results-table",
            ScrollDelta::Pixels(point(px(0.), px(20000.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let (back, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, Some(0), "the selected rank zero is already retained");
    let back_query = view_query(&fixture, started.summary.id, back, cx);
    assert_eq!(back_query.offset, 1);
    assert_eq!(back_query.selected_ordinal, Some(0));
    assert!(back > forward);
    let late = build_view_page(
        &forward_query,
        &started.summary,
        vec![row(offset as u64, &flows[offset])],
        None,
        true,
    );
    emit(
        &fixture,
        Event::IntruderResultView {
            query: forward_query,
            result: Ok(late),
        },
        cx,
    );
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_ne!(result.query.page.as_ref().unwrap().offset, 0);
        assert_eq!(
            result
                .query
                .pending
                .as_ref()
                .map(|pending| pending.query.query_revision),
            Some(back)
        );
        assert_eq!(result.selected, Some(0));
        assert_eq!(result.detail.as_deref(), Some(&flows[0]));
    });
    let back_end = back_query.offset + PAGE_SIZE;
    let mut back_page = build_view_page(
        &back_query,
        &started.summary,
        (back_query.offset..back_end)
            .map(|ordinal| row(ordinal as u64, &flows[ordinal]))
            .collect(),
        Some((back_end - 1) as u64),
        true,
    );
    back_page.total = flows.len();
    back_page.next_offset = Some(back_end);
    back_page.selected_rank = Some(0);
    let selected = row(0, &flows[0]);
    back_page.selected_row = Some(IntruderResultViewRow {
        ordinal: 0,
        result: Some(selected.clone()),
        payloads: vec![],
        response_length: match &selected.outcome {
            IntruderResultOutcome::Flow(flow) if flow.status.is_some() => Some(flow.response_bytes),
            _ => None,
        },
        notes_revision: 0,
        notes_preview: String::new(),
        highlight: None,
        highlight_revision: 0,
        grep_matches: Vec::new(),
        grep_extracts: Vec::new(),
        grep_payload_hits: Vec::new(),
    });
    emit(
        &fixture,
        Event::IntruderResultView {
            query: back_query,
            result: Ok(back_page),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(started.window, |_, window, cx| {
        window.click(("intruder-result-row", 0u64), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.query.page.as_ref().unwrap().total, flows.len());
        assert!(result.query.pending.is_none());
        assert_eq!(result.selected, Some(0));
        assert_eq!(result.detail.as_deref(), Some(&flows[0]));
        assert_eq!(
            result.query.row_at(0).unwrap().result,
            Some(row(0, &flows[0]))
        );
        assert_eq!(
            result.query.row_at(1).unwrap().result,
            Some(row(1, &flows[1]))
        );
    });
    let revision = select(&mut fixture, &started, 1, &flows[1], cx);
    assert!(revision > old_body);
    detail(&fixture, old_body, &flows[0], cx);
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.selected, Some(1));
        assert!(result.detail.is_none());
    });
    detail(&fixture, revision, &flows[1], cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail
                .as_deref(),
            Some(&flows[1])
        )
    });
}

#[gpui_kit::test]
fn running_result_close_requires_a_choice_before_detaching_or_cancelling(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(started.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        has_close_dialog(cx),
        "running result close must ask whether to discard or continue in background"
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        assert!(
            view.intruder
                .results
                .views
                .contains_key(&started.summary.id)
        );
    });
    answer_close_dialog("在后台继续运行", cx);
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        assert!(
            !view
                .intruder
                .results
                .views
                .contains_key(&started.summary.id)
        );
        assert_eq!(view.workspace, workspace);
    });
    assert!(drain(&mut fixture.commands).iter().all(|command| matches!(command, Command::QueryIntruderResults { run_id, .. } if *run_id == started.summary.id)));
}

#[gpui_kit::test]
fn real_pause_resume_controls_correlate_acks_and_stop_invalidates_late_resume(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let pause = pause_command(&mut fixture, &started, true, cx);
    click(started.window, "intruder-result-pause", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-result-pause").disabled(), Some(true));
    })
    .unwrap();
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    emit(
        &fixture,
        control_event(&fixture, &started, Uuid::new_v4()),
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary.state,
            IntruderRunState::Running
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .control_pending
                .is_some()
        );
    });
    emit(&fixture, control_event(&fixture, &started, pause), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert!(
            result.control_pending.is_none(),
            "the event pump must deliver the control ACK"
        );
        assert_eq!(result.summary.as_ref(), Some(&started.summary));
        assert!(result.control_error.is_none());
    });
    let (refresh, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    page(&fixture, &started, refresh, vec![], None, cx);
    assert!(!started.cancel.is_cancelled());
    let resumed = pause_command(&mut fixture, &started, false, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Running;
    emit(&fixture, control_event(&fixture, &started, resumed), cx);
    let (refresh, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, refresh, vec![], None, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.runs[&started.summary.id]
                .summary
                .state,
            IntruderRunState::Running
        );
    });
    assert!(!started.cancel.is_cancelled());
    let paused_again = pause_command(&mut fixture, &started, true, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    emit(
        &fixture,
        control_event(&fixture, &started, paused_again),
        cx,
    );
    let (refresh, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, refresh, vec![], None, cx);
    let resume = pause_command(&mut fixture, &started, false, cx);
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, cx| {
            view.stop_intruder_run(started.summary.id, cx)
        });
    })
    .unwrap();
    assert!(started.cancel.is_cancelled());
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::StopIntruder { project, run_id }] if *project == fixture.project && *run_id == started.summary.id)
    );
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Running;
    let late_resume = control_event(&fixture, &started, resume);
    emit(&fixture, late_resume.clone(), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        assert!(view.intruder.results.views[&started.summary.id].stop_requested);
        assert!(
            view.intruder.results.views[&started.summary.id]
                .control_pending
                .is_none()
        );
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref()
                .unwrap()
                .state,
            IntruderRunState::Paused
        );
    });
    let (terminal, _) = query(&mut fixture, started.summary.id);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Stopped;
    page(&fixture, &started, terminal, vec![], None, cx);
    let (display, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    assert!(display > terminal);
    let display_query = view_query(&fixture, started.summary.id, display, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-result-pause").disabled(), Some(true));
        assert!(
            !fixture
                .view
                .read(cx)
                .intruder
                .runs
                .contains_key(&started.summary.id)
        );
    })
    .unwrap();
    emit(&fixture, late_resume, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert_eq!(
        view_query(&fixture, started.summary.id, display, cx),
        display_query,
        "a late Resume cannot replace the terminal display query"
    );
    cx.read(|cx| {
        assert!(
            !fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .query
                .pending
                .as_ref()
                .unwrap()
                .cancel
                .is_cancelled()
        );
    });
    page(&fixture, &started, display, vec![], None, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert!(!view.intruder.runs.contains_key(&started.summary.id));
        assert_eq!(result.summary.as_ref(), Some(&started.summary));
        assert_eq!(result.query.page.as_ref().unwrap().run, started.summary);
        assert!(result.query.pending.is_none());
        assert!(result.control_pending.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn real_pause_queue_failure_and_close_preserve_task_and_drop_pending_ack(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 4);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, _| {
            for revision in 0..4 {
                view.commands
                    .try_send(Command::QueryIntruderRuns {
                        project: fixture.project,
                        query_revision: revision,
                        after: None,
                        limit: 1,
                    })
                    .unwrap();
            }
        });
    })
    .unwrap();
    click(started.window, "intruder-result-pause", cx);
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 4);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryIntruderRuns { .. }))
    );
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert!(result.control_pending.is_none());
        assert!(result.control_error.as_ref().unwrap().contains("未入队"));
        assert_eq!(
            result.summary.as_ref().unwrap().state,
            IntruderRunState::Running
        );
    });
    let pause = pause_command(&mut fixture, &started, true, cx);
    emit(
        &fixture,
        Event::IntruderRunControl {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: pause,
            result: Err("pause persistence failed".into()),
        },
        cx,
    );
    let (refresh, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, refresh, vec![], None, cx);
    cx.read(|cx| {
        assert!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .control_error
                .as_ref()
                .unwrap()
                .contains("persistence failed")
        )
    });
    let retry = pause_command(&mut fixture, &started, true, cx);
    assert_ne!(retry, pause);
    close(&started, cx);
    assert!(!started.cancel.is_cancelled());
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryIntruderResults { run_id, limit: 1, .. }] if *run_id == started.summary.id)
    );
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    emit(&fixture, control_event(&fixture, &started, retry), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.results.views.is_empty());
        assert!(view.intruder.runs.contains_key(&started.summary.id));
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
