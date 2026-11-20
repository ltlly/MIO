use super::*;

#[gpui_kit::test]
fn same_lifecycle_page_cannot_replace_a_newer_task_title(cx: &mut TestAppContext) {
    let (fixture, expected, query) = named_revision_fixture(cx);
    let mut late = expected.clone();
    late.title.clear();
    late.title_revision = 0;
    probe_page(&fixture, &late, query, vec![], cx);
    assert_summary_caches(&fixture, &expected, cx);
}

#[gpui_kit::test]
fn result_pages_merge_lifecycle_and_task_title_revisions_independently(cx: &mut TestAppContext) {
    for newer_title in [true, false] {
        let (fixture, mut expected, query) = named_revision_fixture(cx);
        let mut incoming = expected.clone();
        if newer_title {
            incoming.revision = 4;
            incoming.state = IntruderRunState::Running;
            incoming.completed = 0;
            incoming.title = "较晚确认的任务名".into();
            incoming.title_revision = 2;
            expected.title = incoming.title.clone();
            expected.title_revision = 2;
        } else {
            incoming.revision = 6;
            incoming.state = IntruderRunState::Running;
            incoming.completed = 2;
            incoming.title.clear();
            incoming.title_revision = 0;
            expected.revision = 6;
            expected.state = IntruderRunState::Running;
            expected.completed = 2;
        }
        let rows = if newer_title {
            vec![row(0, &flow())]
        } else {
            vec![]
        };
        probe_page(&fixture, &incoming, query, rows, cx);
        assert_summary_caches(&fixture, &expected, cx);
        cx.read(|cx| {
            assert!(
                displayed_rows(&fixture.view.read(cx).intruder.results.views[&expected.id])
                    .is_empty(),
                "a newer title does not make an old lifecycle page's result rows current"
            )
        });
    }
}

#[gpui_kit::test]
fn result_pages_merge_pool_revision_without_rolling_back_binding_or_lifecycle(
    cx: &mut TestAppContext,
) {
    for incoming_pool_revision in [1, 2, 3] {
        let (mut fixture, mut expected, query) = named_revision_fixture(cx);
        expected.resource_pool = Some(Uuid::new_v4());
        expected.resource_pool_revision = 2;
        update(&fixture, cx, |view, _, _| {
            view.update_intruder_run_summary(&expected);
        });
        assert_summary_caches(&fixture, &expected, cx);
        let mut incoming = expected.clone();
        incoming.resource_pool = Some(Uuid::new_v4());
        incoming.resource_pool_revision = incoming_pool_revision;
        incoming.state = IntruderRunState::Running;
        let rows = if incoming_pool_revision == 3 {
            incoming.revision = 4;
            incoming.completed = 0;
            expected.resource_pool = incoming.resource_pool;
            expected.resource_pool_revision = 3;
            vec![row(0, &flow())]
        } else {
            incoming.revision = 6;
            incoming.completed = 2;
            expected.revision = 6;
            expected.state = IntruderRunState::Running;
            expected.completed = 2;
            vec![]
        };
        probe_page(&fixture, &incoming, query, rows, cx);
        assert_summary_caches(&fixture, &expected, cx);
        cx.read(|cx| {
            assert!(displayed_rows(&fixture.view.read(cx).intruder.results.views[&expected.id]).is_empty(),
                "a newer pool binding cannot authorize stale lifecycle rows; equal-revision conflicting bindings must keep the confirmed pool");
        });
        let refreshed = next_query(&mut fixture, expected.id, 0);
        assert_eq!(refreshed.filter, Default::default());
        assert_eq!(refreshed.sort, Default::default());
        assert!(
            refreshed.selected_ordinal.is_none(),
            "merging pool metadata only refreshes the existing result view without sending or stopping an attack"
        );
    }
}

#[test]
fn pool_summary_merge_checks_binding_identity_and_revision_bounds() {
    let mut confirmed = run();
    assert_eq!(
        (confirmed.resource_pool, confirmed.resource_pool_revision),
        (None, 0)
    );
    for revision in [1, i64::MAX as u64] {
        let mut incoming = confirmed.clone();
        incoming.resource_pool = Some(Uuid::new_v4());
        incoming.resource_pool_revision = revision;
        results::merge_run_summary(&mut confirmed, &incoming);
        assert_eq!(confirmed, incoming);
    }
    let confirmed = IntruderRunSummary {
        resource_pool: Some(Uuid::new_v4()),
        resource_pool_revision: 2,
        ..run()
    };
    for (pool, revision) in [
        (None, 0),
        (None, 3),
        (Some(Uuid::nil()), 3),
        (Some(Uuid::new_v4()), 0),
        (Some(Uuid::new_v4()), i64::MAX as u64 + 1),
    ] {
        let mut incoming = confirmed.clone();
        incoming.revision += 1;
        incoming.resource_pool = pool;
        incoming.resource_pool_revision = revision;
        let mut merged = confirmed.clone();
        results::merge_run_summary(&mut merged, &incoming);
        assert_eq!(merged.revision, incoming.revision);
        assert_eq!(
            (merged.resource_pool, merged.resource_pool_revision),
            (confirmed.resource_pool, confirmed.resource_pool_revision)
        );
    }
}

#[gpui_kit::test]
fn task_title_updates_never_merge_across_run_tab_or_spec_identity(cx: &mut TestAppContext) {
    for identity in 0..3 {
        let (fixture, expected, query) = named_revision_fixture(cx);
        let mut wrong = expected.clone();
        wrong.revision = 6;
        wrong.title_revision = 2;
        wrong.title = "不属于该运行".into();
        match identity {
            0 => wrong.id = Uuid::new_v4(),
            1 => wrong.tab_id = Uuid::new_v4(),
            _ => wrong.spec_revision += 1,
        }
        emit(
            &fixture,
            Event::IntruderResults {
                project: fixture.project,
                run_id: expected.id,
                query_revision: query,
                result: Ok(IntruderResultPage {
                    run: wrong.clone(),
                    rows: vec![],
                    next_after: None,
                    persisted: true,
                }),
            },
            cx,
        );
        assert_summary_caches(&fixture, &expected, cx);
        update(&fixture, cx, |view, _, _| {
            view.update_intruder_run_summary(&wrong)
        });
        assert_summary_caches(&fixture, &expected, cx);
    }
}
