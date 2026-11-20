use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use mio_core::{
    IntruderAutoThrottle, IntruderPoolUsage, IntruderResourcePool, IntruderResourcePools, Uuid,
};

use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, tab};

#[path = "joined_exit_tests.rs"]
mod joined_exit_tests;
mod lifecycle;
mod retention;

fn click(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
}

fn emit(fixture: &Fixture, event: Event, cx: &mut TestAppContext) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

fn catalog() -> IntruderResourcePools {
    let default = IntruderResourcePool {
        retained_inputs: None,
        id: Uuid::new_v4(),
        name: "默认资源池".into(),
        max_concurrent_requests: Some(10),
        delay: None,
        automatic_throttling: IntruderAutoThrottle {
            enabled: true,
            ..Default::default()
        },
    };
    IntruderResourcePools {
        revision: 1,
        default_pool_id: Some(default.id),
        pools: vec![
            default,
            IntruderResourcePool {
                retained_inputs: None,
                id: Uuid::new_v4(),
                name: "已结束任务池".into(),
                max_concurrent_requests: Some(2),
                delay: None,
                automatic_throttling: IntruderAutoThrottle::default(),
            },
        ],
    }
}

fn answer_reads(
    fixture: &mut Fixture,
    catalog: &IntruderResourcePools,
    used: bool,
    cx: &mut TestAppContext,
) {
    let commands = drain(&mut fixture.commands);
    let mut directory = None;
    let mut usage = None;
    for command in commands {
        match command {
            Command::QueryIntruderResourcePools {
                project,
                query_revision,
            } => {
                assert_eq!(project, fixture.project);
                assert!(directory.replace(query_revision).is_none());
            }
            Command::QueryIntruderPoolUsage {
                project,
                query_revision,
            } => {
                assert_eq!(project, fixture.project);
                assert!(usage.replace(query_revision).is_none());
            }
            Command::QueryIntruderRuns {
                project,
                query_revision,
                ..
            } => emit(
                fixture,
                Event::IntruderRuns {
                    project,
                    query_revision,
                    result: Ok(mio_core::IntruderRunPage {
                        rows: vec![],
                        next_after: None,
                    }),
                },
                cx,
            ),
            _ => panic!("opening Tasks reads metadata only"),
        }
    }
    if let Some(query_revision) = directory {
        emit(
            fixture,
            Event::IntruderResourcePools {
                project: fixture.project,
                query_revision,
                result: Ok(catalog.clone()),
            },
            cx,
        );
    }
    let counts = catalog
        .pools
        .iter()
        .enumerate()
        .map(|(index, pool)| (pool.id, u64::from(index == 1 && used)))
        .collect::<BTreeMap<_, _>>();
    let requires_confirmation = if used {
        [catalog.pools[1].id].into_iter().collect()
    } else {
        BTreeSet::new()
    };
    emit(
        fixture,
        Event::IntruderPoolUsage {
            project: fixture.project,
            query_revision: usage.expect("Tasks must read exact usage"),
            result: Ok(IntruderPoolUsage {
                catalog_revision: catalog.revision,
                task_counts: counts,
                requires_confirmation,
            }),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
}

fn setup(cx: &mut TestAppContext, used: bool) -> (Fixture, AnyWindowHandle, IntruderResourcePools) {
    let mut fixture = fixture(cx, vec![tab("保留请求")], 64);
    drain(&mut fixture.commands);
    click(fixture.window, "tool-dashboard", cx);
    click(fixture.window, "dashboard-task-settings", cx);
    let settings = cx.read(|cx| fixture.view.read(cx).settings_window.unwrap());
    let catalog = catalog();
    answer_reads(&mut fixture, &catalog, used, cx);
    (fixture, settings, catalog)
}

fn dialog(cx: &mut TestAppContext) -> AnyWindowHandle {
    cx.windows()
        .into_iter()
        .find(|handle| {
            cx.update_window(*handle, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("settings-pool-dialog").is_some()
            })
            .unwrap_or(false)
        })
        .expect("pool editor must be visible")
}

fn editor(window: AnyWindowHandle, cx: &mut TestAppContext) -> Entity<PoolEditor> {
    cx.update_window(window, |_, window, cx| {
        window
            .root::<gpui_kit::base::Root>()
            .unwrap()
            .unwrap()
            .read(cx)
            .view()
            .clone()
            .downcast::<PoolEditor>()
            .unwrap()
    })
    .unwrap()
}

fn name(window: AnyWindowHandle, value: &str, cx: &mut TestAppContext) {
    let editor = editor(window, cx);
    let input = cx.read(|cx| editor.read(cx).form.name.clone());
    click(window, ("input", input.entity_id()), cx);
    cx.update_window(window, |_, window, cx| {
        window.press(crate::kbd!("a"), cx);
        window.input(value, cx);
    })
    .unwrap();
    cx.run_until_parked();
}
