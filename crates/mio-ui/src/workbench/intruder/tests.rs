use gpui_kit::{Bounds, ElementInputHandler, InputHandler, TestAppContext};
use mio_core::{
    IntruderRunState, IntruderTab, IntruderWorkspace,
    intruder::{AttackType, Position},
};

use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, project_menu, tab};

mod baseline_setting;
mod part_1;
mod part_2;
mod save_as;
mod save_setting;
mod ui;

pub(super) fn attack() -> IntruderTab {
    let raw = b"POST /intruder HTTP/1.1\r\nHost: fixture.invalid\r\n\r\nx".to_vec();
    IntruderTab {
        id: Uuid::new_v4(),
        title: "1".into(),
        group: None,
        last_run: None,
        spec: IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: Draft {
                revision: 1,
                target: "http://127.0.0.1:1".into(),
                version: HttpVersion::Http11,
                raw: raw.clone(),
            },
            attack_type: AttackType::Sniper,
            positions: vec![Position {
                start: raw.len() - 1,
                end: raw.len(),
            }],
            payload_sets: vec![vec![b"a".to_vec(), vec![0, 255]]],
            max_concurrent_requests: 1,
            request_timeout_ms: 500,
            update_host_header: true,
            save_to_project: true,
        },
    }
}

pub(super) fn intruder_fixture(
    cx: &mut TestAppContext,
    attacks: Vec<IntruderTab>,
    capacity: usize,
) -> Fixture {
    let mut fixture = fixture(cx, vec![tab("Repeater")], capacity);
    drain(&mut fixture.commands);
    let mut workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    workspace.intruder = IntruderWorkspace {
        active: attacks.first().map(|tab| tab.id),
        tabs: attacks,
        ..IntruderWorkspace::default()
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    fixture
}

pub(super) fn default_pool_catalog() -> mio_core::IntruderResourcePools {
    let id = Uuid::from_u128(42);
    mio_core::IntruderResourcePools {
        revision: 1,
        default_pool_id: Some(id),
        pools: vec![mio_core::IntruderResourcePool {
            retained_inputs: None,
            id,
            name: "Default resource pool".into(),
            max_concurrent_requests: Some(10),
            delay: None,
            automatic_throttling: mio_core::IntruderAutoThrottle {
                enabled: true,
                ..Default::default()
            },
        }],
    }
}

pub(super) fn acknowledge_default_pool(fixture: &mut Fixture, cx: &mut TestAppContext) -> Uuid {
    let mut operation = None;
    for command in drain(&mut fixture.commands) {
        match command {
            Command::EnsureIntruderDefaultPool {
                project,
                operation_id,
                cancel,
            } => {
                assert_eq!(project, fixture.project);
                assert!(!cancel.is_cancelled());
                assert!(
                    operation.replace(operation_id).is_none(),
                    "one explicit default-pool admission"
                );
            }
            Command::SaveWorkspace { project, workspace } => {
                assert_eq!(project, fixture.project);
                fixture
                    .events
                    .try_send(Event::WorkspaceChanged { project, workspace })
                    .unwrap();
            }
            Command::SetUserWorkbenchPreferences {
                operation_id,
                expected_revision,
                display,
                message_editor,
                misc,
            } => {
                let mut saved = cx
                    .read(|cx| fixture.view.read(cx).user_preferences.confirmed.clone())
                    .unwrap_or_default();
                saved.revision = expected_revision + 1;
                saved.display = display;
                saved.message_editor = message_editor;
                saved.misc = misc;
                fixture
                    .events
                    .try_send(Event::UserPreferencesSaved {
                        operation_id,
                        result: Ok(saved),
                    })
                    .unwrap();
            }
            _ => panic!(
                "new Intruder tabs may only save preceding edits and ensure the default pool"
            ),
        }
    }
    let catalog = default_pool_catalog();
    let id = catalog.default_pool_id.unwrap();
    fixture
        .events
        .try_send(Event::IntruderDefaultPoolReady {
            project: fixture.project,
            operation_id: operation
                .expect("New / Send to Intruder must explicitly ensure the default pool"),
            result: Ok(catalog),
        })
        .unwrap();
    cx.run_until_parked();
    id
}

pub(super) fn start(fixture: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        fixture
            .view
            .update(cx, |view, cx| view.start_intruder(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn saved(fixture: &mut Fixture) -> Workspace {
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("Start must save the complete workspace before sending anything");
    };
    workspace.clone()
}

pub(super) fn acknowledge(fixture: &Fixture, workspace: Workspace, cx: &mut TestAppContext) {
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn rejected_save_then_repeater_open_keeps_intruder_composition(
    cx: &mut TestAppContext,
    input: usize,
) {
    let mut original = attack();
    if input == 1 {
        original.spec.draft.raw.push(255);
    }
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    start(&fixture, cx);
    let attempted = saved(&mut fixture);
    let mut stored = attempted.clone();
    stored.revision += 1;
    stored.intruder.tabs[0].spec.revision = 5;
    stored.intruder.tabs[0].spec.draft.revision = 3;
    stored.intruder.tabs[0].spec.draft.raw.push(0);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            let editors = &view.intruder.editors[&id];
            let mut handler: Box<dyn InputHandler> = match input {
                0 => Box::new(ElementInputHandler::new(
                    Bounds::default(),
                    editors.request.raw.clone(),
                )),
                1 => Box::new(ElementInputHandler::new(
                    Bounds::default(),
                    editors.request.hex.clone(),
                )),
                _ => Box::new(ElementInputHandler::new(
                    Bounds::default(),
                    editors.target.clone(),
                )),
            };
            // Exercise the input protocol and both event handlers before observer/Change delivery.
            handler.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            let marked = handler.marked_text_range(window, cx).unwrap();
            let contents = (
                editors.request.raw.read(cx).value(),
                editors.request.hex.read(cx).value(),
                editors.target.read(cx).value(),
                editors.request.view,
            );
            assert!(!editors.dirty, "preedit does not emit Change");
            view.on_event(
                Event::WorkspaceRejected {
                    project: fixture.project,
                    revision: attempted.revision,
                    stored,
                    message: "synthetic concurrent draft update".into(),
                },
                window,
                cx,
            );
            assert_eq!(view.workspace.intruder.tabs[0].spec.draft.revision, 4);
            assert!(!view.intruder.editors[&id].dirty);
            view.on_event(
                Event::RepeaterOpened {
                    project: fixture.project,
                    tab: tab("late Repeater result"),
                },
                window,
                cx,
            );
            assert_eq!(
                handler.marked_text_range(window, cx),
                Some(marked),
                "late navigation must not reload an editor with unconfirmed composition"
            );
            let editors = &view.intruder.editors[&id];
            assert_eq!(
                (
                    editors.request.raw.read(cx).value(),
                    editors.request.hex.read(cx).value(),
                    editors.target.read(cx).value(),
                    editors.request.view,
                ),
                contents,
                "all live editor values and the active Raw/Hex view must survive"
            );
            assert_eq!(editors.loaded_revision, original.spec.draft.revision);
            assert_eq!(view.error.as_deref(), Some(editing::COMPOSITION_PENDING));
            assert_eq!(
                view.workspace.intruder.tabs[0].spec.draft.raw,
                original.spec.draft.raw
            );
            assert!(view.intruder.starts.is_empty());
        });
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("the appended Repeater tab may save, but cannot start an attack or retry");
    };
    assert_eq!(
        workspace.intruder.tabs[0].spec.draft.raw,
        original.spec.draft.raw
    );
    assert_eq!(
        workspace.intruder.tabs[0].spec.draft.target,
        original.spec.draft.target
    );
}
