use mio_core::IntruderResourcePools;

use super::*;
use crate::workbench::intruder::tests::default_pool_catalog;

#[path = "results_pool_tests/part_1.rs"]
mod part_1;
#[path = "results_pool_tests/part_2.rs"]
mod part_2;

pub(super) fn pool_radio(id: Uuid) -> SharedString {
    format!("intruder-result-pool-{id}").into()
}

pub(super) fn open_pool(
    fixture: &mut Fixture,
    started: &Started,
    catalog: &IntruderResourcePools,
    cx: &mut TestAppContext,
) {
    click(started.window, "intruder-results-resource-pool", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResourcePools {
            project,
            query_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("opening the result sidebar must only query the catalog")
    };
    assert_eq!(*project, fixture.project);
    emit(
        fixture,
        Event::IntruderResourcePools {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(catalog.clone()),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("intruder-result-pool-panel").visible());
    })
    .unwrap();
}

pub(super) fn setup(
    cx: &mut TestAppContext,
    capacity: usize,
) -> (Fixture, Started, IntruderResourcePools) {
    let mut fixture = intruder_fixture(cx, vec![attack()], capacity);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[flow(0)], cx);
    let mut catalog = default_pool_catalog();
    let mut second = catalog.pools[0].clone();
    second.id = Uuid::from_u128(43);
    second.name = "Second resource pool".into();
    catalog.pools.push(second);
    open_pool(&mut fixture, &started, &catalog, cx);
    (fixture, started, catalog)
}

pub(super) fn select_pool(started: &Started, pool: Uuid, cx: &mut TestAppContext) {
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(pool_radio(pool), cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn request(
    fixture: &mut Fixture,
    started: &Started,
    pool: Uuid,
    cx: &mut TestAppContext,
) -> (Uuid, CancellationToken) {
    select_pool(started, pool, cx);
    select_pool(started, pool, cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResourcePool {
            project,
            run_id,
            operation_id,
            expected_pool_revision,
            resource_pool,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("the radio must submit one binding change, without Save, Start or replay")
    };
    assert_eq!(
        (*project, *run_id, *expected_pool_revision, *resource_pool),
        (
            fixture.project,
            started.summary.id,
            started.summary.resource_pool_revision,
            pool
        )
    );
    assert!(!cancel.is_cancelled());
    (*operation_id, cancel.clone())
}

pub(super) fn accepted(started: &Started, pool: Uuid) -> IntruderRunSummary {
    let mut run = started.summary.clone();
    run.resource_pool = Some(pool);
    run.resource_pool_revision += 1;
    run
}

pub(super) fn reply(
    fixture: &Fixture,
    started: &Started,
    operation: Uuid,
    result: Result<IntruderRunSummary, String>,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::IntruderResourcePoolChanged {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: operation,
            result,
        },
        cx,
    );
}
