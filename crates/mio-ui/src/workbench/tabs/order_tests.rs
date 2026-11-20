use super::*;
use crate::workbench::tests::{Fixture, acknowledge_history_query, drain, fixture, tab};
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};

fn click(fixture: &Fixture, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn settle(fixture: &mut Fixture, cx: &mut TestAppContext) -> Vec<Workspace> {
    let mut saved = Vec::new();
    for _ in 0..8 {
        let commands = drain(&mut fixture.commands);
        let commands = acknowledge_history_query(fixture, commands, cx);
        if commands.is_empty() {
            return saved;
        }
        for command in commands {
            let Command::SaveWorkspace { workspace, .. } = command else {
                panic!("tab ordering may only save workspace or read selected Repeater history")
            };
            workspace.validate().unwrap();
            fixture
                .events
                .try_send(Event::WorkspaceChanged {
                    project: fixture.project,
                    workspace: workspace.clone(),
                })
                .unwrap();
            saved.push(workspace);
        }
        cx.run_until_parked();
    }
    panic!("tab ordering saves did not settle within bounded acknowledgements")
}

fn grouped(cx: &mut TestAppContext, orientation: Orientation) -> (Fixture, [TabId; 3], Uuid) {
    let group = TabGroup {
        id: Uuid::new_v4(),
        title: "Alpha".into(),
        collapsed: false,
        color: Some(7),
    };
    let mut tabs = vec![tab("A"), tab("B"), tab("C")];
    let ids = [tabs[0].id, tabs[1].id, tabs[2].id];
    tabs[0].group = Some(group.id);
    tabs[2].group = Some(group.id);
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace {
                revision: 1,
                orientation,
                groups: vec![group.clone()],
                tabs,
                active: Some(ids[0]),
                ..Default::default()
            },
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    click(&fixture, "tool-repeater", cx);
    settle(&mut fixture, cx);
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let header = window.find(format!("group-{}", group.id));
            assert_eq!(header.role(), Some(gpui_kit::accesskit::Role::Button));
            assert_eq!(header.label(), Some("Alpha"));
            assert_eq!(header.expanded(), Some(true));
        })
        .unwrap();
    assert_order(&fixture, [ids[0], ids[2], ids[1]], orientation, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .workspace
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            ids
        )
    });
    (fixture, ids, group.id)
}

fn assert_order(
    fixture: &Fixture,
    ids: [TabId; 3],
    orientation: Orientation,
    cx: &mut TestAppContext,
) {
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let coordinates = ids.map(|id| {
                let bounds = window.find(format!("work-tab-{id}")).bounds();
                match orientation {
                    Orientation::Horizontal => bounds.origin.x,
                    Orientation::Vertical => bounds.origin.y,
                }
            });
            assert!(
                coordinates[0] < coordinates[1] && coordinates[1] < coordinates[2],
                "tab order must match the rendered positions: {coordinates:?}"
            );
        })
        .unwrap();
}

fn open_menu(fixture: &Fixture, id: TabId, cx: &mut TestAppContext) {
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click(format!("work-tab-{id}"), cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn choose(fixture: &Fixture, label: &str, cx: &mut TestAppContext) {
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let position = window.find_by_label(label).bounds().center();
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn press(fixture: &Fixture, key: &str, cx: &mut TestAppContext) {
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn grouped_directional_close_uses_rendered_neighbors_in_both_orientations(cx: &mut TestAppContext) {
    for orientation in [Orientation::Horizontal, Orientation::Vertical] {
        for collapsed in [false, true] {
            let (mut fixture, [a, b, c], group) = grouped(cx, orientation);
            if collapsed {
                click(&fixture, format!("group-{group}"), cx);
                settle(&mut fixture, cx);
                cx.read(|cx| assert!(fixture.view.read(cx).workspace.groups[0].collapsed));
                fixture
                    .window
                    .update(cx, |_, window, cx| {
                        window.render_frame(cx);
                        let header = window.find(format!("group-{group}"));
                        assert_eq!(header.role(), Some(gpui_kit::accesskit::Role::Button));
                        assert_eq!(header.expanded(), Some(false));
                    })
                    .unwrap();
            }
            open_menu(&fixture, b, cx);
            choose(
                &fixture,
                if orientation == Orientation::Horizontal {
                    "关闭右侧标签"
                } else {
                    "关闭下方标签"
                },
                cx,
            );
            cx.read(|cx| {
                let workspace = &fixture.view.read(cx).workspace;
                assert_eq!(
                    workspace.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
                    [a, b, c],
                    "the last visible tab has no right/below neighbor"
                );
                assert!(workspace.closed.is_empty());
            });
            assert!(settle(&mut fixture, cx).is_empty());
            press(&fixture, "escape", cx);
            open_menu(&fixture, b, cx);
            choose(
                &fixture,
                if orientation == Orientation::Horizontal {
                    "关闭左侧标签"
                } else {
                    "关闭上方标签"
                },
                cx,
            );
            let saved = settle(&mut fixture, cx);
            assert!(!saved.is_empty());
            let workspace = saved.last().unwrap();
            assert_eq!(
                workspace.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
                [b]
            );
            assert_eq!(
                workspace
                    .closed
                    .iter()
                    .map(|tab| tab.id)
                    .collect::<Vec<_>>(),
                [a, c]
            );
            assert!(workspace.closed.iter().all(|tab| !tab.draft.raw.is_empty()));
        }
    }
}

#[gpui_kit::test]
fn grouped_keyboard_navigation_follows_rendered_order_without_rewriting_source_order(
    cx: &mut TestAppContext,
) {
    let (mut fixture, [a, b, c], _) = grouped(cx, Orientation::Horizontal);
    click(&fixture, format!("work-tab-{a}"), cx);
    settle(&mut fixture, cx);
    for (key, expected) in [
        ("ctrl-tab", c),
        ("ctrl-tab", b),
        ("ctrl-tab", a),
        ("ctrl-shift-tab", b),
        ("ctrl-shift-tab", c),
    ] {
        press(&fixture, key, cx);
        let saved = settle(&mut fixture, cx);
        assert!(
            !saved.is_empty(),
            "navigation key must reach workspace persistence"
        );
        assert_eq!(
            saved.last().unwrap().active,
            Some(expected),
            "{key} must follow visible A,C,B order"
        );
        assert_eq!(
            saved
                .last()
                .unwrap()
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            [a, b, c]
        );
    }
}

#[gpui_kit::test]
fn grouped_keyboard_moves_use_visible_neighbor_and_preserve_requested_membership(
    cx: &mut TestAppContext,
) {
    for (selected, key, within_group) in [
        (0, crate::kbd!("alt-down"), true),
        (2, crate::kbd!("alt-up"), true),
        (1, crate::kbd!("alt-up"), false),
    ] {
        let (mut fixture, ids, group) = grouped(cx, Orientation::Horizontal);
        let [a, b, c] = ids;
        click(&fixture, format!("work-tab-{}", ids[selected]), cx);
        settle(&mut fixture, cx);
        // Input reserves Cmd-Alt-Up/Down for multi-cursor editing; focus a workspace control.
        click(&fixture, "tool-repeater", cx);
        settle(&mut fixture, cx);
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(window.find("tool-repeater").focused(), Some(true));
            })
            .unwrap();
        press(&fixture, key, cx);
        let saved = settle(&mut fixture, cx);
        assert!(
            !saved.is_empty(),
            "move key must reach workspace persistence"
        );
        let workspace = saved.last().unwrap();
        assert_eq!(workspace.active, Some(ids[selected]));
        assert_eq!(
            workspace
                .tabs
                .iter()
                .find(|tab| tab.id == ids[selected])
                .unwrap()
                .group,
            Some(group)
        );
        assert_order(
            &fixture,
            if within_group { [c, a, b] } else { [a, b, c] },
            Orientation::Horizontal,
            cx,
        );
        assert!(workspace.closed.is_empty());
    }
}
