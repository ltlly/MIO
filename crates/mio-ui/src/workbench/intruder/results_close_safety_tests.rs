use super::*;

#[path = "results_close_safety_tests/part_1.rs"]
mod part_1;
#[path = "results_close_safety_tests/part_2.rs"]
mod part_2;

pub(super) fn discarding_with_another_live_run(
    cx: &mut TestAppContext,
) -> (Fixture, Started, Started, Discard) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut selected = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut selected, &[], cx);
    let mut other = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut other, &[], cx);
    choose_discard(&mut fixture, &selected, cx);
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let queries = drain(&mut fixture.commands);
    assert_eq!(queries.len(), 2);
    let mut revisions = HashMap::new();
    for command in queries {
        match command {
            Command::QueryIntruderResults {
                project,
                run_id,
                query_revision,
                after: None,
                limit: 1,
            } => {
                assert_eq!(project, fixture.project);
                assert_eq!(run_id, selected.summary.id);
                assert!(revisions.insert(run_id, query_revision).is_none());
            }
            Command::QueryIntruderResultView { query, cancel } => {
                assert_eq!(query.project, fixture.project);
                assert_eq!(query.run_id, other.summary.id);
                assert_eq!(query.view_id, other.summary.id);
                assert_eq!(query.offset, 0);
                assert_eq!(query.limit, PAGE_SIZE);
                assert_eq!(query.filter, mio_core::IntruderResultFilter::default());
                assert_eq!(query.sort, mio_core::IntruderResultSort::default());
                assert!(!cancel.is_cancelled());
                assert!(
                    revisions
                        .insert(query.run_id, query.query_revision)
                        .is_none()
                );
            }
            _ => panic!("Stop uses its status probe; the other view only refreshes its display"),
        }
    }
    page(
        &fixture,
        &other,
        revisions[&other.summary.id],
        vec![],
        None,
        cx,
    );
    let revision = revisions[&selected.summary.id];
    stopped(&fixture, &mut selected, revision, cx);
    let discard = take_discard(&mut fixture, &selected, cx);
    assert!(selected.cancel.is_cancelled());
    assert!(!other.cancel.is_cancelled());
    assert_eq!(
        discard.workspace.intruder.tabs[0].last_run,
        Some(other.summary.id)
    );
    (fixture, selected, other, discard)
}

pub(super) fn save_workspace_without_reviving_requests(
    fixture: &mut Fixture,
    revision: u64,
    cx: &mut TestAppContext,
) -> Workspace {
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.capture_drafts(window, cx).unwrap();
            assert!(view.save_workspace(cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("an explicit workspace save must not issue a send or retry the failed close");
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(workspace.revision, revision);
    let saved = workspace.clone();
    emit(
        fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved.clone(),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    // No Send was admitted for the main draft; the saved draft's history is empty.
    let commands = crate::workbench::tests::acknowledge_history_query(fixture, commands, cx);
    assert!(
        commands.is_empty(),
        "a later save ACK must not revive cancelled Send or Start intents"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.intruder.starts.is_empty());
    });
    saved
}
