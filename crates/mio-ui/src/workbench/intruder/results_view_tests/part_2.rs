use super::*;

pub(in crate::workbench::intruder) fn bootstrap(
    fixture: &mut Fixture,
    started: &mut Started,
    flows: &[FlowRecord],
    cx: &mut TestAppContext,
) {
    started.summary.revision += 1;
    started.summary.dispatched = flows.len() as u64;
    started.summary.completed = flows.len() as u64;
    page(
        fixture,
        started,
        started.probe,
        flows
            .first()
            .map(|flow| vec![row(0, flow)])
            .unwrap_or_default(),
        (flows.len() > 1).then_some(0),
        cx,
    );
    let (revision, after) = query(fixture, started.summary.id);
    assert_eq!(after, None);
    let rows = flows
        .iter()
        .take(PAGE_SIZE)
        .enumerate()
        .map(|(ordinal, flow)| row(ordinal as u64, flow))
        .collect();
    page(
        fixture,
        started,
        revision,
        rows,
        (flows.len() > PAGE_SIZE).then_some((PAGE_SIZE - 1) as u64),
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
}

pub(in crate::workbench::intruder) fn result_settings_fixture(
    named: bool,
    retention: mio_core::IntruderRetention,
    cx: &mut TestAppContext,
) -> (Fixture, Started) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = named.then(|| PathBuf::from("/tmp/mio-result-settings-parity.mio"));
        })
    });
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Completed;
    started.summary.retention = retention;
    let event = result_event(
        &fixture,
        &started.summary,
        revision,
        vec![],
        None,
        retention == mio_core::IntruderRetention::Project,
        cx,
    );
    emit(&fixture, event, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    click(started.window, "intruder-results-settings", cx);
    (fixture, started)
}

pub(in crate::workbench::intruder) fn select(
    fixture: &mut Fixture,
    started: &Started,
    ordinal: u64,
    flow: &FlowRecord,
    cx: &mut TestAppContext,
) -> u64 {
    cx.update_window(started.window, |_, window, cx| {
        window.click(("intruder-result-row", ordinal), cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            project,
            selection_revision,
            flow_id,
        },
    ] = commands.as_slice()
    else {
        panic!("a real result row click must read only that row's body");
    };
    assert_eq!((*project, *flow_id), (fixture.project, flow.summary.id));
    *selection_revision
}

pub(in crate::workbench::intruder) fn detail(
    fixture: &Fixture,
    revision: u64,
    flow: &FlowRecord,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(flow.clone()),
            original: Box::default(),
        },
        cx,
    );
}

pub(in crate::workbench::intruder) fn pause_command(
    fixture: &mut Fixture,
    started: &Started,
    paused: bool,
    cx: &mut TestAppContext,
) -> Uuid {
    click(started.window, "intruder-result-pause", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderPaused {
            project,
            run_id,
            operation_id,
            paused: desired,
        },
    ] = commands.as_slice()
    else {
        panic!("the production pause/resume icon must emit one correlated command");
    };
    assert_eq!(
        (*project, *run_id, *desired),
        (fixture.project, started.summary.id, paused)
    );
    *operation_id
}

pub(in crate::workbench::intruder) fn control_event(
    fixture: &Fixture,
    started: &Started,
    operation_id: Uuid,
) -> Event {
    Event::IntruderRunControl {
        project: fixture.project,
        run_id: started.summary.id,
        operation_id,
        result: Ok(started.summary.clone()),
    }
}

pub(in crate::workbench::intruder) fn close(started: &Started, cx: &mut TestAppContext) {
    cx.update_window(started.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    if has_close_dialog(cx) {
        answer_close_dialog("在后台继续运行", cx);
        cx.run_until_parked();
    }
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
}
