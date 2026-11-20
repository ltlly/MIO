use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

use super::*;
use crate::workbench::intruder::tests::{attack, default_pool_catalog, intruder_fixture};
#[cfg(target_os = "macos")]
use crate::workbench::tests::pump_native;
use crate::workbench::tests::{Fixture, drain};

#[path = "default_pool_tests/part_1.rs"]
mod part_1;

pub(super) fn emit(fixture: &Fixture, event: Event, cx: &mut TestAppContext) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

pub(super) fn new_tab(fixture: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-new", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn ensure(fixture: &mut Fixture, cx: &mut TestAppContext) -> (Uuid, CancellationToken) {
    let mut attempt = None;
    for command in drain(&mut fixture.commands) {
        match command {
            Command::EnsureIntruderDefaultPool {
                project,
                operation_id,
                cancel,
            } => {
                assert_eq!(project, fixture.project);
                assert!(!cancel.is_cancelled());
                assert!(attempt.replace((operation_id, cancel)).is_none());
            }
            Command::SaveWorkspace { project, workspace } => {
                assert_eq!(project, fixture.project);
                emit(fixture, Event::WorkspaceChanged { project, workspace }, cx);
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
                emit(
                    fixture,
                    Event::UserPreferencesSaved {
                        operation_id,
                        result: Ok(saved),
                    },
                    cx,
                );
            }
            _ => panic!("default-pool admission must not issue an attack or unrelated command"),
        }
    }
    attempt.expect("one explicit EnsureIntruderDefaultPool")
}

pub(super) fn ready(fixture: &Fixture, operation_id: Uuid, cx: &mut TestAppContext) {
    emit(
        fixture,
        Event::IntruderDefaultPoolReady {
            project: fixture.project,
            operation_id,
            result: Ok(default_pool_catalog()),
        },
        cx,
    );
}

pub(super) fn saved_tab(fixture: &mut Fixture) -> Workspace {
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("the confirmed default pool creates and saves exactly one tab, without Start")
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(workspace.intruder.tabs.len(), 1);
    assert_eq!(
        workspace.intruder.tabs[0].spec.resource_pool,
        Some(mio_core::IntruderPoolSelection::Existing(
            default_pool_catalog().default_pool_id.unwrap()
        ))
    );
    workspace.clone()
}
