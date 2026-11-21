use super::*;
use crate::workbench::tests::{Fixture, acknowledge_history_query, drain, fixture, tab};
use gpui_kit::InputEvent;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};

fn click(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}
fn new_group(
    f: &mut Fixture,
    tab: TabId,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<GroupEditor>) {
    click(f.window, "tool-repeater", cx);
    let commands = drain(&mut f.commands);
    assert!(acknowledge_history_query(f, commands, cx).is_empty());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click(format!("work-tab-{tab}"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let menu = window.find_by_label("添加到组").bounds().center();
            window.pointer_move(menu, None, cx);
            let create = window.find_by_label("新建组").bounds().center();
            window.pointer_down(create, MouseButton::Left, cx);
            window.pointer_up(create, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    dialog(f, cx)
}
fn dialog(f: &Fixture, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<GroupEditor>) {
    let handle = cx.read(|cx| f.view.read(cx).tab_group_dialog.unwrap());
    let entity = handle
        .update(cx, |_, window, cx| group_editor(window, cx).unwrap())
        .unwrap();
    (handle, entity)
}
fn saved(f: &mut Fixture) -> Workspace {
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("group editing must save exactly one workspace")
    };
    workspace.clone()
}
fn ack(f: &Fixture, workspace: Workspace, cx: &mut TestAppContext) {
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
}
fn rename(
    window: AnyWindowHandle,
    editor: &Entity<GroupEditor>,
    text: &str,
    cx: &mut TestAppContext,
) {
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let input = editor.read(cx).name.clone();
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(text, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn group_create_cancel_then_save_color_membership_and_order_only_after_ack(
    cx: &mut TestAppContext,
) {
    let first = tab("first");
    let second = tab("second");
    let mut f = fixture(cx, vec![first.clone(), second.clone()], 64);
    drain(&mut f.commands);
    let original = cx.read(|cx| f.view.read(cx).workspace.clone());
    let (window, editor) = new_group(&mut f, first.id, cx);
    assert!(drain(&mut f.commands).is_empty());
    rename(window, &editor, "取消的组", cx);
    click(window, ("group-color", 12usize), cx);
    click(window, "group-cancel", cx);
    cx.read(|cx| assert_eq!(f.view.read(cx).workspace, original));
    assert!(drain(&mut f.commands).is_empty());
    drop(editor);
    let (window, editor) = new_group(&mut f, first.id, cx);
    rename(window, &editor, "Alpha组", cx);
    click(window, ("group-color", 12usize), cx);
    click(window, "group-submit", cx);
    let workspace = saved(&mut f);
    assert_eq!(workspace.groups.len(), 1);
    let group = &workspace.groups[0];
    assert_eq!(group.title, "Alpha组");
    assert_eq!(group.color, Some(12));
    assert_eq!(workspace.tabs[0].group, Some(group.id));
    assert_eq!(workspace.tabs[1].group, None);
    assert!(window.update(cx, |_, _, _| ()).is_ok());
    ack(&f, workspace.clone(), cx);
    assert!(window.update(cx, |_, _, _| ()).is_err());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let group_bounds = window.find(format!("group-{}", group.id)).bounds();
            let ungrouped = window.find(format!("work-tab-{}", second.id)).bounds();
            assert!(group_bounds.origin.x < ungrouped.origin.x);
            assert_eq!(f.view.read(cx).workspace.groups, workspace.groups);
        })
        .unwrap();
}

#[gpui_kit::test]
fn editing_group_lists_only_own_and_ungrouped_tabs_and_save_failure_keeps_draft(
    cx: &mut TestAppContext,
) {
    let mut first = tab("first");
    let mut second = tab("second");
    let third = tab("ungrouped");
    let alpha = TabGroup {
        id: Uuid::new_v4(),
        title: "Alpha".into(),
        collapsed: false,
        color: Some(7),
    };
    let beta = TabGroup {
        id: Uuid::new_v4(),
        title: "Beta".into(),
        collapsed: false,
        color: Some(1),
    };
    first.group = Some(alpha.id);
    second.group = Some(beta.id);
    let mut f = fixture(cx, vec![], 64);
    drain(&mut f.commands);
    let workspace = Workspace {
        revision: 1,
        tabs: vec![first.clone(), second.clone(), third.clone()],
        groups: vec![alpha.clone(), beta.clone()],
        active: Some(first.id),
        ..Default::default()
    };
    f.events
        .try_send(Event::ProjectOpened {
            project: f.project,
            path: None,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);
    click(f.window, "tool-repeater", cx);
    let commands = drain(&mut f.commands);
    acknowledge_history_query(&f, commands, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click(format!("group-{}", alpha.id), cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let edit = window.find_by_label("编辑组").bounds().center();
            window.pointer_down(edit, MouseButton::Left, cx);
            window.pointer_up(edit, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let (window, editor) = dialog(&f, cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find(format!("group-member-{}", first.id)).visible());
            assert!(
                window
                    .try_find(format!("group-member-{}", second.id))
                    .is_none()
            );
            assert!(window.find(format!("group-member-{}", third.id)).visible());
        })
        .unwrap();
    click(window, format!("group-member-{}", first.id), cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("group-submit").disabled(), Some(true));
            window.click("group-submit", cx);
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    click(window, "group-select-all", cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("group-select-all").label(), Some("取消全选"));
        })
        .unwrap();
    click(window, "group-select-all", cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("group-select-all").label(), Some("全选"));
            assert_eq!(window.find("group-submit").disabled(), Some(true));
        })
        .unwrap();
    click(window, "group-select-all", cx);
    rename(window, &editor, "Alpha更新", cx);
    click(window, ("group-color", 1usize), cx);
    click(window, "group-submit", cx);
    let attempted = saved(&mut f);
    assert_eq!(attempted.tabs[0].group, Some(alpha.id));
    assert_eq!(attempted.tabs[1].group, Some(beta.id));
    assert_eq!(attempted.tabs[2].group, Some(alpha.id));
    f.events
        .try_send(Event::WorkspaceRejected {
            project: f.project,
            revision: attempted.revision,
            stored: workspace,
            message: "disk rejected".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(editor.read(cx).name.read(cx).value().as_ref(), "Alpha更新");
        assert!(editor.read(cx).error.is_some());
        assert_eq!(editor.read(cx).group.color, Some(1));
    });
    assert!(window.update(cx, |_, _, _| ()).is_ok());
    rename(window, &editor, "Alpha再次更新🙂", cx);
    click(window, ("group-color", 12usize), cx);
    cx.read(|cx| {
        assert_eq!(
            editor.read(cx).name.read(cx).value().as_ref(),
            "Alpha再次更新🙂"
        );
        assert_eq!(editor.read(cx).group.color, Some(12));
        assert_eq!(f.view.read(cx).workspace.groups[0].title, "Alpha更新");
        assert_eq!(f.view.read(cx).workspace.groups[0].color, Some(1));
    });
    click(window, "group-submit", cx);
    let retry = saved(&mut f);
    assert_eq!(retry.groups[0].id, alpha.id);
    assert_eq!(retry.groups[0].title, "Alpha再次更新🙂");
    assert_eq!(retry.groups[0].color, Some(12));
    assert_eq!(retry.groups[1], beta);
    assert_eq!(retry.tabs, attempted.tabs);
    ack(&f, retry, cx);
    assert!(window.update(cx, |_, _, _| ()).is_err());
}

#[gpui_kit::test]
fn group_name_ime_and_late_native_commit_are_preserved_across_save_ack(cx: &mut TestAppContext) {
    let first = tab("first");
    let mut f = fixture(cx, vec![first.clone()], 64);
    drain(&mut f.commands);
    let (window, editor) = new_group(&mut f, first.id, cx);
    window
        .update(cx, |_, window, cx| {
            let name = editor.read(cx).name.clone();
            window.click(("input", name.entity_id()), cx);
            name.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
            window.click("group-submit", cx);
            assert!(name.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }));
            name.update(cx, |input, cx| {
                input.unmark_text(window, cx);
                input.set_value("已提交组名", window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let name = editor.read(cx).name.clone();
            let revision = name.read(cx).document_revision();
            let submit = window.find("group-submit").bounds().center();
            window.pointer_move(submit, None, cx);
            window.pointer_down(submit, MouseButton::Left, cx);
            // Deliver the queued text commit before the next frame disables the input.
            window.dispatch_event(
                gpui_kit::MouseUpEvent {
                    button: MouseButton::Left,
                    position: submit,
                    modifiers: gpui_kit::Modifiers::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            assert!(editor.read(cx).submitting);
            assert_eq!(editor.read(cx).group.title, "已提交组名");
            name.update(cx, |input, cx| {
                input.replace_text_in_range(
                    Some(0.."已提交组名".encode_utf16().count()),
                    "稍后到达的中文🙂",
                    window,
                    cx,
                )
            });
            assert_eq!(name.read(cx).value().as_ref(), "稍后到达的中文🙂");
            assert!(name.read(cx).document_revision() > revision);
        })
        .unwrap();
    cx.run_until_parked();
    let submitted = saved(&mut f);
    assert_eq!(submitted.groups[0].title, "已提交组名");
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let name = editor.read(cx).name.clone();
            let revision = name.read(cx).document_revision();
            name.update(cx, |input, cx| {
                input.replace_text_in_range(None, "禁用后不应插入", window, cx);
            });
            assert_eq!(name.read(cx).value().as_ref(), "稍后到达的中文🙂");
            assert_eq!(name.read(cx).document_revision(), revision);
        })
        .unwrap();
    ack(&f, submitted.clone(), cx);
    assert!(
        window.update(cx, |_, _, _| ()).is_ok(),
        "a save ACK must not close over a newer queued native text commit"
    );
    cx.read(|cx| {
        assert_eq!(
            editor.read(cx).name.read(cx).value().as_ref(),
            "稍后到达的中文🙂"
        );
        assert_eq!(f.view.read(cx).workspace.groups[0].title, "已提交组名");
    });
    click(window, "group-submit", cx);
    let latest = saved(&mut f);
    assert_eq!(latest.groups[0].id, submitted.groups[0].id);
    assert_eq!(latest.groups[0].title, "稍后到达的中文🙂");
    assert_eq!(latest.tabs, submitted.tabs);
    ack(&f, latest, cx);
    assert!(window.update(cx, |_, _, _| ()).is_err());
}

#[gpui_kit::test]
fn cancelled_or_old_project_group_dialog_never_commits_membership(cx: &mut TestAppContext) {
    let first = tab("first");
    let mut f = fixture(cx, vec![first.clone()], 64);
    drain(&mut f.commands);
    let original = cx.read(|cx| f.view.read(cx).workspace.clone());
    let (window, editor) = new_group(&mut f, first.id, cx);
    rename(window, &editor, "不提交", cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("group-submit", cx);
            window.click("group-cancel", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| assert_eq!(f.view.read(cx).workspace, original));
    drop(editor);
    let (window, editor) = new_group(&mut f, first.id, cx);
    rename(window, &editor, "旧项目组", cx);
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: f.project.generation + 1,
    };
    let next = Workspace {
        revision: 9,
        ..Default::default()
    };
    f.events
        .try_send(Event::ProjectOpened {
            project,
            path: None,
            workspace: next.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(window.update(cx, |_, _, _| ()).is_err());
    let commands = drain(&mut f.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project: actual, .. } }] if *actual == project)
    );
    cx.read(|cx| assert_eq!(*f.view.read(cx).workspace, next));
}

#[gpui_kit::test]
fn open_group_draft_blocks_native_quit_and_project_open_until_cancelled(cx: &mut TestAppContext) {
    let first = tab("first");
    let mut f = fixture(cx, vec![first.clone()], 64);
    drain(&mut f.commands);
    let (window, editor) = new_group(&mut f, first.id, cx);
    rename(window, &editor, "未提交的分组🙂", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("q"), cx);
        })
        .unwrap();
    crate::workbench::tests::pump_native(cx);
    assert!(
        !cx.has_pending_prompt(),
        "native Quit must reject while the group draft dialog owns the operation"
    );
    assert!(drain(&mut f.commands).is_empty());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let error = window.find("workspace-error");
            assert!(error.visible());
            assert!(
                error
                    .label()
                    .is_some_and(|label| label.contains("当前操作尚未结束"))
            );
            window.press(crate::kbd!("o"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        !cx.did_prompt_for_paths(),
        "project Open cannot hide an unapplied modal group draft"
    );
    assert!(window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.quitting);
        assert!(view.transition.is_none());
        assert!(view.workspace.groups.is_empty());
        assert_eq!(
            editor.read(cx).name.read(cx).value().as_ref(),
            "未提交的分组🙂"
        );
    });
    drop(editor);
    click(window, "group-cancel", cx);
    cx.run_until_parked();
    cx.read(|cx| assert!(f.view.read(cx).tab_group_dialog.is_none()));
    click(f.window, "tool-repeater", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("o"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        cx.did_prompt_for_paths(),
        "cancelling the group dialog must release the project-operation gate"
    );
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
}
