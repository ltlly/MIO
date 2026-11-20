use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext, point};

use super::*;
use crate::workbench::intruder::tests::{attack, intruder_fixture};
use crate::workbench::tests::{Fixture, drain};

#[path = "tabs_tests/part_1.rs"]
mod part_1;
#[path = "tabs_tests/part_2.rs"]
mod part_2;

pub(super) fn open_menu(fixture: &Fixture, id: TabId, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.right_click(SharedString::from(format!("intruder-tab-{id}")), cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn choose_item(fixture: &Fixture, label: &str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label(label);
        assert_ne!(item.disabled(), Some(true));
        window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
        window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn choose_orientation(fixture: &Fixture, desired: Orientation, cx: &mut TestAppContext) {
    let id = cx.read(|cx| fixture.view.read(cx).workspace.intruder.active.unwrap());
    open_menu(fixture, id, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let submenu = window.find_by_label("标签视图设置").bounds().center();
        window.pointer_move(submenu, None, cx);
        let label = match desired {
            Orientation::Horizontal => "横向标签",
            Orientation::Vertical => "垂直标签",
        };
        let item = window.find_by_label(label).bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.orientation,
            desired
        )
    });
}

pub(super) fn acknowledge_saves(fixture: &mut Fixture, cx: &mut TestAppContext) -> Workspace {
    let mut latest = None;
    for _ in 0..8 {
        let commands = drain(&mut fixture.commands);
        if commands.is_empty() {
            return latest.expect("workspace changes must reach the persistence command");
        }
        for command in commands {
            let Command::SaveWorkspace { workspace, .. } = command else {
                panic!("tab actions must only save the workspace");
            };
            workspace.validate().unwrap();
            fixture
                .events
                .try_send(Event::WorkspaceChanged {
                    project: fixture.project,
                    workspace: workspace.clone(),
                })
                .unwrap();
            latest = Some(workspace);
        }
        cx.run_until_parked();
    }
    panic!("workspace saves did not settle within the bounded acknowledgements");
}
