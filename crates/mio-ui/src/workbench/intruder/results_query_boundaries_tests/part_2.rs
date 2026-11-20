use super::*;

#[gpui_kit::test]
fn pure_stop_terminal_probe_refreshes_display_and_reads_the_last_completed_flow(
    cx: &mut TestAppContext,
) {
    let mut summary = run(2, None);
    summary.dispatched = 1;
    summary.completed = 1;
    summary.state = IntruderRunState::Running;
    let first = flow(0, 200);
    let last = flow(1, 201);
    let first_row = row(0, &first, Some(b"first"));
    let last_row = row(1, &last, Some(b"last"));
    let (mut f, window, initial, _) = open(&summary, cx);
    let original = page(&summary, &initial, vec![first_row.clone()], 1);
    answer(&f, initial, original.clone(), cx);
    let attack_cancel = CancellationToken::new();
    f.view.update(cx, |view, _| {
        view.intruder.runs.insert(
            summary.id,
            IntruderRun {
                summary: summary.clone(),
                cancel: attack_cancel.clone(),
            },
        );
    });
    assert!(drain(&mut f.commands).is_empty());

    f.view
        .update(cx, |view, cx| view.stop_intruder_run(summary.id, cx));
    let commands = drain(&mut f.commands);
    let [Command::StopIntruder { project, run_id }] = commands.as_slice() else {
        panic!("pure Stop must cancel the active attack without closing its result view")
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    assert!(attack_cancel.is_cancelled());
    emit(&f, Event::IntruderChanged { project: f.project }, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("Stop must first confirm its terminal state with only one unfiltered probe")
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    let mut stopped = summary.clone();
    stopped.revision += 1;
    stopped.dispatched = 2;
    stopped.completed = 2;
    stopped.state = IntruderRunState::Stopped;
    emit(
        &f,
        Event::IntruderResults {
            project: f.project,
            run_id: summary.id,
            query_revision: *query_revision,
            result: Ok(mio_core::IntruderResultPage {
                run: stopped.clone(),
                rows: vec![first_row.result.clone().unwrap()],
                next_after: Some(0),
                persisted: true,
            }),
        },
        cx,
    );

    let (display, display_cancel) = query(&mut f);
    assert_eq!(
        (display.run_id, display.view_id, display.offset),
        (summary.id, summary.id, 0)
    );
    assert!(!display_cancel.is_cancelled());
    cx.read(|cx| {
        let workbench = f.view.read(cx);
        let view = &workbench.intruder.results.views[&summary.id];
        assert_eq!(view.summary.as_ref(), Some(&stopped));
        assert!(!view.stop_requested);
        assert!(view.pending.is_none());
        assert_eq!(view.query.page.as_ref(), Some(&original));
        assert!(!workbench.intruder.runs.contains_key(&summary.id));
        assert!(workbench.intruder_result_close.is_none());
    });
    answer(
        &f,
        display.clone(),
        page(&stopped, &display, vec![first_row, last_row], 2),
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("table-cell-1:1").label(), Some("last"));
        assert_eq!(window.find("table-cell-1:2").label(), Some("201"));
    })
    .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    click(window, "table-cell-1:0", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            project,
            flow_id,
            selection_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("the refreshed last result must read its own stable flow identity")
    };
    assert_eq!((*project, *flow_id), (f.project, last.summary.id));
    emit(
        &f,
        Event::FlowDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(last.clone()),
            original: Box::default(),
        },
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.selected, Some(1));
        assert_eq!(view.detail.as_deref(), Some(&last));
        assert_eq!(view.query.page.as_ref().unwrap().total, 2);
    });
    assert!(drain(&mut f.commands).is_empty());
}
