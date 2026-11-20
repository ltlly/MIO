use super::*;

#[gpui_kit::test]
fn intruder_tab_menu_matches_order_separators_and_disabled_actions(cx: &mut TestAppContext) {
    let first = attack();
    let id = first.id;
    let mut fixture = intruder_fixture(cx, vec![first], 64);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    open_menu(&fixture, id, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let expected = [
            ("重命名", false),
            ("关闭标签", false),
            ("关闭其它标签", true),
            ("关闭左侧标签", true),
            ("关闭右侧标签", true),
            ("重新打开已关闭标签", true),
            ("标签视图设置", false),
        ];
        let mut previous = px(0.);
        for (label, disabled) in expected {
            let item = window.find_by_label(label);
            assert!(item.bounds().origin.y >= previous, "{label}");
            previous = item.bounds().bottom();
            if disabled {
                // PopupMenu 0.9 omits aria-disabled, so verify its pointer behavior directly.
                window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
                window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
                window.render_frame(cx);
                assert!(
                    window.find_by_label(label).visible(),
                    "{label} dismissed the menu"
                );
                assert_eq!(fixture.view.read(cx).workspace, original);
                assert!(fixture.view.read(cx).intruder.rename.is_none());
            }
        }
        for index in [2usize, 6, 8] {
            let item = window.within("popup-menu").find(index);
            assert_eq!(item.label(), None);
            assert_eq!(item.role(), Some(gpui_kit::accesskit::Role::MenuItem));
        }
        assert!(window.try_find_by_label("添加到组").is_none());
        let rename = window.find_by_label("重命名").bounds().center();
        window.pointer_down(rename, MouseButton::Left, cx);
        window.pointer_up(rename, MouseButton::Left, cx);
        assert!(fixture.view.read(cx).intruder.rename.is_some());
        window.press("escape", cx);
        fixture.view.update(cx, |view, cx| {
            view.backend_disconnected = true;
            cx.notify();
        });
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    open_menu(&fixture, id, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        for label in [
            "重命名",
            "关闭标签",
            "关闭其它标签",
            "关闭左侧标签",
            "关闭右侧标签",
            "重新打开已关闭标签",
        ] {
            let item = window.find_by_label(label).bounds().center();
            window.pointer_down(item, MouseButton::Left, cx);
            window.pointer_up(item, MouseButton::Left, cx);
            window.render_frame(cx);
            assert!(
                window.find_by_label(label).visible(),
                "{label} dismissed the menu"
            );
            assert_eq!(fixture.view.read(cx).workspace, original);
            assert!(fixture.view.read(cx).intruder.rename.is_none());
        }
        let submenu = window.find_by_label("标签视图设置").bounds().center();
        window.pointer_move(submenu, None, cx);
        for label in ["横向标签", "垂直标签"] {
            let item = window.find_by_label(label).bounds().center();
            window.pointer_down(item, MouseButton::Left, cx);
            window.pointer_up(item, MouseButton::Left, cx);
            window.render_frame(cx);
            assert!(
                window.find_by_label(label).visible(),
                "{label} dismissed the submenu"
            );
            assert_eq!(fixture.view.read(cx).workspace, original);
        }
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn intruder_rename_orientation_and_resize_keep_and_persist_current_drafts(cx: &mut TestAppContext) {
    let first = attack();
    let id = first.id;
    let mut fixture = intruder_fixture(cx, vec![first], 64);
    let input = cx.read(|cx| {
        fixture.view.read(cx).intruder.editors[&id]
            .request
            .state()
            .clone()
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("保留草稿", cx);
        window.double_click(SharedString::from(format!("intruder-tab-{id}")), cx);
        let rename = fixture
            .view
            .read(cx)
            .intruder
            .rename
            .as_ref()
            .unwrap()
            .input
            .clone();
        window.click(("input", rename.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("中文标签", cx);
        window.render_frame(cx);
        let bounds = window.find(("input", rename.entity_id())).bounds();
        let focused = rename.focus_handle(cx).is_focused(window);
        let value = rename.read(cx).value();
        assert!(bounds.size.width > px(0.) && focused && value.as_ref() == "中文标签",
            "inline rename before Enter: bounds={bounds:?}, focused={focused}, value={value:?}, error={:?}", fixture.view.read(cx).error);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.intruder.rename.is_none(),
            "after Enter: rename={:?}, title={:?}, error={:?}",
            view.intruder
                .rename
                .as_ref()
                .map(|rename| rename.input.read(cx).value()),
            view.workspace.intruder.tabs[0].title,
            view.error
        );
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0].title,
            "中文标签"
        );
    });
    choose_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find("intruder-workspace").bounds();
        let handle = window.find("intruder-work-tabs-resize").bounds().center();
        let end = point(bounds.origin.x + px(260.), handle.y);
        window.pointer_down(handle, MouseButton::Left, cx);
        window.pointer_move(handle + point(px(12.), px(0.)), Some(MouseButton::Left), cx);
        window.pointer_move(end, Some(MouseButton::Left), cx);
        window.pointer_up(end, MouseButton::Left, cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tab_sidebar_width,
            260
        );
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-work-tabs").bounds().size.width,
            px(260.)
        );
        let handle = window.find("intruder-work-tabs-resize").bounds().center();
        window.pointer_down(handle, MouseButton::Left, cx);
        window.pointer_move(handle + point(px(80.), px(0.)), Some(MouseButton::Left), cx);
        window.press("escape", cx);
        window.pointer_up(handle, MouseButton::Left, cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tab_sidebar_width,
            260
        );
        assert_eq!(
            fixture.view.read(cx).intruder.editors[&id]
                .request
                .state()
                .entity_id(),
            input.entity_id()
        );
    })
    .unwrap();
    choose_orientation(&fixture, Orientation::Horizontal, cx);
    let persisted = acknowledge_saves(&mut fixture, cx);
    assert_eq!(persisted.intruder.orientation, Orientation::Horizontal);
    assert_eq!(persisted.intruder.tab_sidebar_width, 260);
    assert_eq!(persisted.intruder.tabs[0].title, "中文标签");
    assert!(
        persisted.intruder.tabs[0]
            .spec
            .draft
            .raw
            .ends_with("保留草稿".as_bytes())
    );
    assert_eq!(persisted.tabs[0].title, "Repeater");
}

#[gpui_kit::test]
fn intruder_rename_composition_survives_close_and_replacement_attempts(cx: &mut TestAppContext) {
    for replace in [false, true] {
        let attacks = vec![attack(), attack()];
        let id = attacks[0].id;
        let other = attacks[1].id;
        let mut fixture = intruder_fixture(cx, attacks, 64);
        open_menu(&fixture, id, cx);
        choose_item(&fixture, "重命名", cx);
        let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
        let input = cx.read(|cx| {
            fixture
                .view
                .read(cx)
                .intruder
                .rename
                .as_ref()
                .unwrap()
                .input
                .clone()
        });
        assert!(drain(&mut fixture.commands).is_empty());
        cx.update_window(fixture.window, |_, window, cx| {
            window.click(("input", input.entity_id()), cx);
            input.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
                assert!(state.marked_text_range(window, cx).is_some());
            });
            if replace {
                window.double_click(SharedString::from(format!("intruder-tab-{other}")), cx);
            } else {
                window.click(SharedString::from(format!("intruder-close-{id}")), cx);
            }
            let view = fixture.view.read(cx);
            let rename = view.intruder.rename.as_ref().expect(
                "closing or renaming another tab must preserve the unconfirmed title composition",
            );
            assert_eq!(rename.id, id);
            assert_eq!(rename.input.entity_id(), input.entity_id());
            assert_eq!(view.workspace, original);
            assert_eq!(view.error.as_deref(), Some(editing::COMPOSITION_PENDING));
            assert!(input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }));
        })
        .unwrap();
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
    }
}

#[gpui_kit::test]
fn intruder_resize_escape_persists_original_width_after_interleaved_workspace_ack(
    cx: &mut TestAppContext,
) {
    let first = attack();
    let id = first.id;
    let mut fixture = intruder_fixture(cx, vec![first], 64);
    choose_orientation(&fixture, Orientation::Vertical, cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("orientation must have one pending workspace snapshot")
    };
    let original_width = workspace.intruder.tab_sidebar_width;
    open_menu(&fixture, id, cx);
    choose_item(&fixture, "重命名", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture
            .view
            .read(cx)
            .intruder
            .rename
            .as_ref()
            .unwrap()
            .input
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("交错保存的名称", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let handle = cx
        .update_window(fixture.window, |_, window, cx| {
            assert!(fixture.view.read(cx).workspace_dirty);
            assert!(fixture.view.read(cx).intruder.rename.is_none());
            window.render_frame(cx);
            let bounds = window.find("intruder-workspace").bounds();
            let handle = window.find("intruder-work-tabs-resize").bounds().center();
            window.pointer_down(handle, MouseButton::Left, cx);
            window.pointer_move(handle + point(px(12.), px(0.)), Some(MouseButton::Left), cx);
            window.pointer_move(
                point(bounds.origin.x + px(260.), handle.y),
                Some(MouseButton::Left),
                cx,
            );
            assert_eq!(
                fixture.view.read(cx).workspace.intruder.tab_sidebar_width,
                260
            );
            handle
        })
        .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).pending_workspace,
            Some(workspace.revision + 1)
        );
    });
    let preview_commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            workspace: preview, ..
        },
    ] = preview_commands.as_slice()
    else {
        panic!("the interleaved ACK must persist the in-progress drag preview")
    };
    assert_eq!(preview.intruder.tab_sidebar_width, 260);
    assert_eq!(preview.intruder.tabs[0].title, "交错保存的名称");
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: preview.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.press("escape", cx);
        window.pointer_up(handle, MouseButton::Left, cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tab_sidebar_width,
            original_width
        );
    })
    .unwrap();
    let persisted = acknowledge_saves(&mut fixture, cx);
    assert_eq!(
        persisted.intruder.tab_sidebar_width, original_width,
        "Escape must restore the persisted width even when an earlier ACK saved the drag preview"
    );
    assert_eq!(persisted.intruder.tabs[0].title, "交错保存的名称");
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_workspace.is_none());
        assert!(!view.workspace_dirty);
        assert_eq!(view.workspace.intruder.tab_sidebar_width, original_width);
    });
}

#[gpui_kit::test]
fn intruder_batch_close_and_reopen_preserve_every_spec_and_edited_body(cx: &mut TestAppContext) {
    let mut attacks = vec![attack(), attack(), attack()];
    for (index, tab) in attacks.iter_mut().enumerate() {
        tab.title = (index + 1).to_string();
    }
    let ids: Vec<_> = attacks.iter().map(|tab| tab.id).collect();
    let mut fixture = intruder_fixture(cx, attacks.clone(), 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&ids[0]]
            .request
            .state()
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("批量保留", cx);
    })
    .unwrap();
    choose_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.drag_to(
            SharedString::from(format!("intruder-tab-{}", ids[1])),
            SharedString::from(format!("intruder-tab-{}", ids[0])),
            cx,
        );
        assert_eq!(fixture.view.read(cx).workspace.intruder.tabs[0].id, ids[1]);
        window.drag_to(
            SharedString::from(format!("intruder-tab-{}", ids[0])),
            SharedString::from(format!("intruder-tab-{}", ids[1])),
            cx,
        );
        assert_eq!(fixture.view.read(cx).workspace.intruder.tabs[0].id, ids[0]);
    })
    .unwrap();
    open_menu(&fixture, ids[1], cx);
    choose_item(&fixture, "关闭上方标签", cx);
    cx.read(|cx| {
        let workspace = &fixture.view.read(cx).workspace.intruder;
        assert_eq!(
            workspace.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            ids[1..]
        );
        assert_eq!(workspace.closed[0].id, ids[0]);
        assert!(
            workspace.closed[0]
                .spec
                .draft
                .raw
                .ends_with("批量保留".as_bytes())
        );
    });
    open_menu(&fixture, ids[1], cx);
    choose_item(&fixture, "关闭下方标签", cx);
    open_menu(&fixture, ids[1], cx);
    choose_item(&fixture, "重新打开已关闭标签", cx);
    open_menu(&fixture, ids[2], cx);
    choose_item(&fixture, "重新打开已关闭标签", cx);
    open_menu(&fixture, ids[1], cx);
    choose_item(&fixture, "关闭其它标签", cx);
    cx.read(|cx| {
        let workspace = &fixture.view.read(cx).workspace.intruder;
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.tabs[0].id, ids[1]);
        assert_eq!(workspace.closed.len(), 2);
    });
    open_menu(&fixture, ids[1], cx);
    choose_item(&fixture, "重新打开已关闭标签", cx);
    open_menu(&fixture, ids[0], cx);
    choose_item(&fixture, "重新打开已关闭标签", cx);
    let persisted = acknowledge_saves(&mut fixture, cx);
    assert!(persisted.intruder.closed.is_empty());
    assert_eq!(persisted.intruder.tabs.len(), 3);
    for original in attacks.iter().skip(1) {
        assert_eq!(
            persisted
                .intruder
                .tabs
                .iter()
                .find(|tab| tab.id == original.id),
            Some(original)
        );
    }
    let edited = persisted
        .intruder
        .tabs
        .iter()
        .find(|tab| tab.id == ids[0])
        .unwrap();
    assert!(edited.spec.draft.raw.ends_with("批量保留".as_bytes()));
    assert_eq!(edited.spec.payload_sets, attacks[0].spec.payload_sets);
    assert_eq!(edited.spec.positions, attacks[0].spec.positions);
}
