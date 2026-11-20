use gpui_kit::test::TestWindowExt;
use gpui_kit::{ScrollDelta, TestAppContext, point};

use super::*;
use crate::workbench::intruder::tests::{attack, default_pool_catalog, intruder_fixture};
use crate::workbench::tests::{Fixture, drain};

mod part_1;
mod part_2;

pub(super) fn click(
    fixture: &Fixture,
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut TestAppContext,
) {
    let id = id.into();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        if let Some(panel) = window.try_find("intruder-resource-pools-panel") {
            let panel = panel.bounds();
            let target = window.find(id.clone()).bounds();
            if target.origin.x >= panel.origin.x {
                let delta = if target.origin.y < panel.origin.y {
                    panel.origin.y - target.origin.y + px(20.)
                } else if target.bottom() > panel.bottom() {
                    panel.bottom() - target.bottom() - px(20.)
                } else {
                    px(0.)
                };
                if delta != px(0.) {
                    window.scroll(
                        "intruder-resource-pools-panel",
                        ScrollDelta::Pixels(point(px(0.), delta)),
                        cx,
                    );
                    window.render_frame(cx);
                }
            }
        }
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn emit(fixture: &Fixture, event: Event, cx: &mut TestAppContext) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

pub(super) fn query(fixture: &mut Fixture) -> u64 {
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResourcePools {
            project,
            query_revision,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "opening Resource pool must issue one bounded catalog query, not a mutation or Start"
        )
    };
    assert_eq!(*project, fixture.project);
    *query_revision
}

pub(super) fn reply(
    fixture: &Fixture,
    revision: u64,
    catalog: IntruderResourcePools,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::IntruderResourcePools {
            project: fixture.project,
            query_revision: revision,
            result: Ok(catalog),
        },
        cx,
    );
}

pub(super) fn setup(cx: &mut TestAppContext) -> (Fixture, TabId, IntruderResourcePools) {
    let config = attack();
    let id = config.id;
    let mut fixture = intruder_fixture(cx, vec![config], 64);
    click(&fixture, "tool-intruder", cx);
    click(&fixture, "intruder-sidebar-resource-pool", cx);
    let revision = query(&mut fixture);
    let catalog = default_pool_catalog();
    reply(&fixture, revision, catalog.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    (fixture, id, catalog)
}

pub(super) fn saves(fixture: &mut Fixture, cx: &mut TestAppContext) {
    for _ in 0..4 {
        let commands = drain(&mut fixture.commands);
        if commands.is_empty() {
            return;
        }
        for command in commands {
            let Command::SaveWorkspace { project, workspace } = command else {
                panic!(
                    "editing a Create form saves its draft only; no catalog creation, binding or network operation"
                )
            };
            assert_eq!(project, fixture.project);
            emit(fixture, Event::WorkspaceChanged { project, workspace }, cx);
        }
    }
    panic!("form saves must settle within bounded acknowledgements")
}

pub(super) fn text(
    fixture: &Fixture,
    id: TabId,
    field: usize,
    value: &str,
    cx: &mut TestAppContext,
) {
    let input = cx.read(|cx| {
        fixture.view.read(cx).intruder.editors[&id]
            .pool_form
            .inputs()[field]
            .clone()
    });
    click(fixture, ("input", input.entity_id()), cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("a"), cx);
        window.input(value, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn created(fixture: &Fixture, id: TabId, cx: &TestAppContext) -> IntruderResourcePool {
    cx.read(|cx| {
        match &fixture
            .view
            .read(cx)
            .workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .unwrap()
            .spec
            .resource_pool
        {
            Some(IntruderPoolSelection::Create(pool)) => pool.clone(),
            _ => panic!("Create configuration expected"),
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StartEdit {
    Request,
    PoolName,
    InvalidCap,
    NameComposition,
}

pub(super) fn create_start_ack_with_new_input(edit: StartEdit, cx: &mut TestAppContext) {
    let (mut fixture, id, mut catalog) = setup(cx);
    let project_path = PathBuf::from("/tmp/mio-pool-start-ack.mio");
    fixture
        .view
        .update(cx, |view, _| view.project_path = Some(project_path.clone()));
    click(&fixture, "intruder-pool-create", cx);
    text(&fixture, id, 0, "Pool A", cx);
    saves(&mut fixture, cx);
    click(&fixture, "intruder-pool-cap-enabled", cx);
    text(&fixture, id, 1, "2", cx);
    saves(&mut fixture, cx);
    let original_pool = created(&fixture, id, cx);
    click(&fixture, "intruder-start", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("Start saves its exact Create snapshot before admission")
    };
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: *project,
            workspace: workspace.clone(),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [Command::StartIntruder { run_id, spec, .. }] = commands.as_slice() else {
        panic!("one frozen Start must be in flight before later user input")
    };
    let frozen = *spec.clone();
    let run_id = *run_id;
    assert_eq!(
        frozen.resource_pool,
        Some(IntruderPoolSelection::Create(original_pool.clone()))
    );
    let new_request = "POST /after-start HTTP/1.1\r\nHost: fixture.invalid\r\n\r\nx";
    match edit {
        StartEdit::Request => {
            cx.update_window(fixture.window, |_, window, cx| {
                let input = fixture.view.read(cx).intruder.editors[&id]
                    .request
                    .state()
                    .clone();
                window.click(("input", input.entity_id()), cx);
                window.press(crate::kbd!("a"), cx);
                window.press("left", cx);
                for _ in 0.."POST ".len() {
                    window.press("right", cx);
                }
                for _ in 0.."/intruder".len() {
                    window.press("shift-right", cx);
                }
                assert_eq!(input.read(cx).selected_range(), 5..14);
                window.input("/after-start", cx);
                window.press(crate::kbd!("s"), cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
        StartEdit::PoolName => text(&fixture, id, 0, "Pool B", cx),
        StartEdit::InvalidCap => text(&fixture, id, 1, "not-a-number", cx),
        StartEdit::NameComposition => {
            cx.update_window(fixture.window, |_, window, cx| {
                let input = fixture.view.read(cx).intruder.editors[&id]
                    .pool_form
                    .name
                    .clone();
                window.click(("input", input.entity_id()), cx);
                window.press(crate::kbd!("a"), cx);
                input.update(cx, |input, cx| {
                    input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
                });
            })
            .unwrap();
            cx.run_until_parked();
        }
    }
    let mut pending_commands = drain(&mut fixture.commands);
    if matches!(edit, StartEdit::Request | StartEdit::PoolName) {
        assert!(
            matches!(pending_commands.as_slice(), [Command::SaveWorkspace { .. }]),
            "only the later draft is saved while Start waits"
        );
    } else {
        assert!(
            pending_commands.is_empty(),
            "invalid or composing pool input must not be harvested"
        );
    }
    let run = IntruderRunSummary {
        id: run_id,
        tab_id: id,
        revision: 1,
        spec_revision: frozen.revision,
        total: frozen.validate().unwrap(),
        dispatched: 0,
        completed: 0,
        state: mio_core::IntruderRunState::Running,
        started_unix_ms: 1,
        retention: mio_core::IntruderRetention::Project,
        title: String::new(),
        title_revision: 0,
        baseline: None,
        resource_pool: Some(original_pool.id),
        resource_pool_revision: 1,
    };
    emit(
        &fixture,
        Event::IntruderStarted {
            project: fixture.project,
            run_id,
            tab_id: id,
            spec_revision: frozen.revision,
            result: Ok(run.clone()),
        },
        cx,
    );
    let next_pool_id = cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        let current = view.workspace.intruder.tabs[0].spec.clone();
        let form_id = view.intruder.editors[&id].pool_form.id;
        let cap_input = view.intruder.editors[&id].pool_form.cap.clone();
        let name_input = view.intruder.editors[&id].pool_form.name.clone();
        assert_eq!(view.intruder.runs[&run_id].summary, run, "the admitted run remains frozen on Pool A");
        if edit == StartEdit::Request {
            assert_eq!(current.draft.raw, new_request.as_bytes());
            assert_eq!(current.resource_pool, Some(IntruderPoolSelection::Existing(original_pool.id)),
                "request-only edits must not leave an already-created pool as Create");
            original_pool.id
        } else {
            let Some(IntruderPoolSelection::Create(pool)) = &current.resource_pool else {
                panic!("new or unfinished pool input must remain editable Create after the old Start ACK")
            };
            assert_ne!(pool.id, original_pool.id, "the next Create must not reuse the pool ID already committed by Start");
            assert_eq!(form_id, pool.id);
            match edit {
                StartEdit::PoolName => assert_eq!(pool.name, "Pool B"),
                StartEdit::InvalidCap => {
                    let input = &cap_input;
                    assert_eq!(input.read(cx).value().as_ref(), "not-a-number");
                    assert!(window.find(("input", input.entity_id())).visible(), "the unfinished cap field remains present for the real correction below");
                    assert_eq!(pool.max_concurrent_requests, Some(2), "the malformed text is retained in its editor, not coerced into the saved model");
                }
                StartEdit::NameComposition => {
                    let input = &name_input;
                    assert!(input.update(cx, |input, cx| input.marked_text_range(window, cx).is_some()));
                    assert!(window.find(("input", input.entity_id())).visible(), "the composing field remains present for the real commit below");
                    assert_eq!(pool.name, original_pool.name, "the ACK must not harvest an IME preedit");
                }
                StartEdit::Request => unreachable!(),
            }
            pool.id
        }
    }).unwrap();
    pending_commands.extend(drain(&mut fixture.commands));
    catalog.revision += 1;
    catalog.pools.push(original_pool.clone());
    let mut catalog_reads = 0;
    for _ in 0..4 {
        if pending_commands.is_empty() {
            break;
        }
        for command in pending_commands {
            match command {
                Command::SaveWorkspace { project, workspace } => {
                    emit(&fixture, Event::WorkspaceChanged { project, workspace }, cx)
                }
                Command::SaveProject { project, .. } => emit(
                    &fixture,
                    Event::ProjectSaved {
                        websocket_notes: vec![],
                        ext_http_notes: Vec::new(),
                        ext_http_highlights: Vec::new(),
                        project,
                        path: project_path.clone(),
                        total_bytes: 128,
                    },
                    cx,
                ),
                Command::QueryIntruderResourcePools { query_revision, .. } => {
                    catalog_reads += 1;
                    reply(&fixture, query_revision, catalog.clone(), cx);
                }
                Command::QueryIntruderResults { .. } => {}
                _ => panic!("a late Start ACK must not issue a second attack or pool mutation"),
            }
        }
        pending_commands = drain(&mut fixture.commands);
    }
    assert!(pending_commands.is_empty());
    assert_eq!(catalog_reads, 1);
    let owner = cx.read(|cx| fixture.view.read(cx).intruder.results.views[&run_id].owner);
    cx.update_window(owner, |_, window, cx| {
        window.click("intruder-results-positions", cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            selection_revision, ..
        },
    ] = commands.as_slice()
    else {
        panic!("Positions must read the original admitted specification")
    };
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id,
            selection_revision: *selection_revision,
            result: Ok(Box::new(frozen.clone())),
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&run_id]
                .spec
                .as_deref(),
            Some(&frozen)
        )
    });
    assert!(drain(&mut fixture.commands).is_empty());
    if edit == StartEdit::InvalidCap {
        text(&fixture, id, 1, "3", cx);
        saves(&mut fixture, cx);
    } else if edit == StartEdit::NameComposition {
        cx.update_window(fixture.window, |_, window, cx| window.input("完成池名", cx))
            .unwrap();
        cx.run_until_parked();
        saves(&mut fixture, cx);
        cx.update_window(fixture.window, |_, window, cx| {
            let input = fixture.view.read(cx).intruder.editors[&id]
                .pool_form
                .name
                .clone();
            assert!(input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_none()
            }));
        })
        .unwrap();
    }
    click(&fixture, "intruder-start", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("correctable pending input must permit another explicit Start save")
    };
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: *project,
            workspace: workspace.clone(),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [Command::StartIntruder { spec, .. }] = commands.as_slice() else {
        panic!("one second Start after the exact save ACK")
    };
    if edit == StartEdit::Request {
        assert_eq!(
            spec.resource_pool,
            Some(IntruderPoolSelection::Existing(original_pool.id))
        );
        assert_eq!(spec.draft.raw, new_request.as_bytes());
    } else {
        let Some(IntruderPoolSelection::Create(pool)) = &spec.resource_pool else {
            panic!("the corrected later form must create its own pool")
        };
        assert_eq!(pool.id, next_pool_id);
        assert_ne!(pool.id, original_pool.id);
        match edit {
            StartEdit::PoolName => assert_eq!(pool.name, "Pool B"),
            StartEdit::InvalidCap => assert_eq!(pool.max_concurrent_requests, Some(3)),
            StartEdit::NameComposition => assert_eq!(pool.name, "完成池名"),
            StartEdit::Request => unreachable!(),
        }
    }
    assert_eq!(
        cx.read(|cx| fixture.view.read(cx).intruder.results.views[&run_id]
            .spec
            .as_deref()
            .cloned()),
        Some(frozen)
    );
}
