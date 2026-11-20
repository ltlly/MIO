use super::*;

#[gpui_kit::test]
fn intruder_rename_escape_reverts_a_title_saved_while_editing(cx: &mut TestAppContext) {
    let first = attack();
    let id = first.id;
    let original_title = first.title.clone();
    let mut fixture = intruder_fixture(cx, vec![first], 64);
    let path = PathBuf::from("/tmp/mio-intruder-rename-preview.mio");
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, _| view.project_path = Some(path.clone()))
    });
    open_menu(&fixture, id, cx);
    choose_item(&fixture, "重命名", cx);
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
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("尚在编辑的名称", cx);
        window.render_frame(cx);
        let bounds = window.find(("input", input.entity_id())).bounds();
        let focused = input.focus_handle(cx).is_focused(window);
        let value = input.read(cx).value();
        assert!(bounds.size.width > px(0.) && focused && value.as_ref() == "尚在编辑的名称",
            "inline rename before Save: bounds={bounds:?}, focused={focused}, value={value:?}, error={:?}", fixture.view.read(cx).error);
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            workspace: preview, ..
        },
    ] = commands.as_slice()
    else {
        let kinds: Vec<_> = commands
            .iter()
            .map(|command| match command {
                Command::SaveWorkspace { workspace, .. } => format!(
                    "SaveWorkspace revision={} title={:?}",
                    workspace.revision, workspace.intruder.tabs[0].title
                ),
                Command::SaveProject { path, .. } => format!("SaveProject path={path:?}"),
                other => format!("{:?}", std::mem::discriminant(other)),
            })
            .collect();
        panic!("saving from the rename input must first capture its workspace; got {kinds:?}")
    };
    assert_eq!(preview.intruder.tabs[0].title, "尚在编辑的名称");
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: preview.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::SaveProject { project, path: None, .. }] if *project == fixture.project)
    );
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .intruder
                .rename
                .as_ref()
                .unwrap()
                .input
                .entity_id(),
            input.entity_id()
        );
        assert!(input.focus_handle(cx).is_focused(window));
        window.press("escape", cx);
        assert!(fixture.view.read(cx).intruder.rename.is_none());
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0].title,
            original_title,
            "Escape must cancel the title even when an intervening save captured its preview"
        );
    })
    .unwrap();
    cx.run_until_parked();
    let persisted = acknowledge_saves(&mut fixture, cx);
    assert_eq!(persisted.intruder.tabs[0].title, original_title);
}

#[gpui_kit::test]
fn intruder_grouped_close_below_uses_display_order(cx: &mut TestAppContext) {
    let mut grouped = attack();
    grouped.title = "组内标签".into();
    let mut ungrouped = attack();
    ungrouped.title = "顶部标签".into();
    let grouped_id = grouped.id;
    let ungrouped_id = ungrouped.id;
    let mut fixture = intruder_fixture(cx, vec![grouped, ungrouped], 64);
    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            let group = Uuid::new_v4();
            view.workspace.intruder.groups.push(mio_core::TabGroup {
                id: group,
                title: "已保存的组".into(),
                collapsed: false,
                color: None,
            });
            view.workspace.intruder.tabs[0].group = Some(group);
            view.workspace.validate().unwrap();
            cx.notify();
        })
    });
    choose_orientation(&fixture, Orientation::Vertical, cx);
    acknowledge_saves(&mut fixture, cx);
    open_menu(&fixture, ungrouped_id, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let upper = window
            .find(SharedString::from(format!("intruder-tab-{ungrouped_id}")))
            .bounds();
        let lower = window
            .find(SharedString::from(format!("intruder-tab-{grouped_id}")))
            .bounds();
        assert!(
            lower.origin.y >= upper.bottom(),
            "the grouped tab is visually below the ungrouped tab"
        );
    })
    .unwrap();
    choose_item(&fixture, "关闭下方标签", cx);
    cx.read(|cx| {
        let workspace = &fixture.view.read(cx).workspace.intruder;
        assert_eq!(
            workspace.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![ungrouped_id]
        );
        assert_eq!(
            workspace
                .closed
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            vec![grouped_id]
        );
    });
    let persisted = acknowledge_saves(&mut fixture, cx);
    assert_eq!(persisted.intruder.closed[0].id, grouped_id);
}

#[gpui_kit::test]
fn intruder_inline_rename_enter_blur_and_late_old_blur_keep_input_identity(
    cx: &mut TestAppContext,
) {
    let attacks = vec![attack(), attack()];
    let id = attacks[0].id;
    let other = attacks[1].id;
    let mut fixture = intruder_fixture(cx, attacks, 64);
    // GPUI deliberately omits focus-change notifications for inactive TestPlatform windows.
    cx.update_window(fixture.window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    for (title, blur) in [("Enter 提交", false), ("失焦提交", true)] {
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
            window.input(title, cx);
            window.render_frame(cx);
            let bounds = window.find(("input", input.entity_id())).bounds();
            let focused = input.focus_handle(cx).is_focused(window);
            assert!(focused && input.read(cx).value().as_ref() == title,
                "before commit: title={title:?}, blur={blur}, bounds={bounds:?}, focused={focused}, value={:?}, error={:?}",
                input.read(cx).value(), fixture.view.read(cx).error);
            assert!(window.try_find("intruder-rename-apply").is_none());
            assert!(window.try_find("intruder-rename-cancel").is_none());
            if blur {
                let request = fixture.view.read(cx).intruder.editors[&id]
                    .request
                    .state()
                    .clone();
                window.click(("input", request.entity_id()), cx);
                assert!(request.focus_handle(cx).is_focused(window));
            } else {
                window.press("enter", cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            let view = fixture.view.read(cx);
            assert!(view.intruder.rename.is_none(),
                "after commit: title={title:?}, blur={blur}, rename={:?}, request_focused={}, error={:?}",
                view.intruder.rename.as_ref().map(|rename| (rename.input.read(cx).value(), rename.input.focus_handle(cx).is_focused(window))),
                view.intruder.editors[&id].request.state().focus_handle(cx).is_focused(window), view.error);
            assert_eq!(view.workspace.intruder.tabs[0].title, title);
        }).unwrap();
        assert_eq!(
            acknowledge_saves(&mut fixture, cx).intruder.tabs[0].title,
            title
        );
    }

    open_menu(&fixture, id, cx);
    choose_item(&fixture, "重命名", cx);
    let successor = cx
        .update_window(fixture.window, |_, window, cx| {
            let old = fixture
                .view
                .read(cx)
                .intruder
                .rename
                .as_ref()
                .unwrap()
                .input
                .clone();
            // Deliver this old input event only after real double-click creates its successor.
            old.update(cx, |_, cx| cx.emit(InputEvent::Blur));
            window.double_click(SharedString::from(format!("intruder-tab-{other}")), cx);
            let successor = fixture
                .view
                .read(cx)
                .intruder
                .rename
                .as_ref()
                .unwrap()
                .input
                .clone();
            assert_ne!(successor.entity_id(), old.entity_id());
            window.click(("input", successor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("新输入仍在编辑", cx);
            successor
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        let rename = view
            .intruder
            .rename
            .as_ref()
            .expect("an old Blur must not commit the replacement input");
        assert_eq!(rename.id, other);
        assert_eq!(rename.input.entity_id(), successor.entity_id());
        assert_eq!(successor.read(cx).value().as_ref(), "新输入仍在编辑");
        assert_eq!(view.workspace.intruder.tabs[1].title, "1");
        assert!(successor.focus_handle(cx).is_focused(window));
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.rename.is_none()));
    let persisted = acknowledge_saves(&mut fixture, cx);
    assert_eq!(persisted.intruder.tabs[0].title, "失焦提交");
    assert_eq!(persisted.intruder.tabs[1].title, "新输入仍在编辑");
}

#[gpui_kit::test]
fn intruder_native_rename_entry_survives_split_pointer_dispatch_and_frames(
    cx: &mut TestAppContext,
) {
    for menu_entry in [true, false] {
        let tab = attack();
        let id = tab.id;
        let mut fixture = intruder_fixture(cx, vec![tab], 64);
        cx.update_window(fixture.window, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        choose_orientation(&fixture, Orientation::Vertical, cx);
        acknowledge_saves(&mut fixture, cx);
        cx.update_window(fixture.window, |_, window, cx| {
            window.click(SharedString::from(format!("intruder-tab-{id}")), cx);
        })
        .unwrap();
        cx.run_until_parked();
        acknowledge_saves(&mut fixture, cx);
        if menu_entry {
            open_menu(&fixture, id, cx);
        }
        let position = cx
            .update_window(fixture.window, |_, window, cx| {
                window.render_frame(cx);
                if menu_entry {
                    window.find_by_label("重命名").bounds().center()
                } else {
                    let bounds = window
                        .find(SharedString::from(format!("intruder-tab-{id}")))
                        .bounds();
                    point(bounds.origin.x + px(20.), bounds.center().y)
                }
            })
            .unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
        for click_count in 1..=if menu_entry { 1 } else { 2 } {
            native.simulate_event(gpui_kit::MouseDownEvent {
                position,
                button: MouseButton::Left,
                click_count,
                modifiers: Default::default(),
                first_mouse: false,
            });
            cx.run_until_parked();
            native.simulate_event(gpui_kit::MouseUpEvent {
                position,
                button: MouseButton::Left,
                click_count,
                modifiers: Default::default(),
            });
            cx.run_until_parked();
        }
        for frame in 0..3 {
            cx.update_window(fixture.window, |_, window, cx| {
                window.render_frame(cx);
                window.simulate_next_frame(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(fixture.window, |_, window, cx| {
                let view = fixture.view.read(cx);
                let rename = view.intruder.rename.as_ref().unwrap_or_else(|| panic!(
                    "native-style rename disappeared: menu_entry={menu_entry}, frame={frame}, error={:?}, active={}",
                    view.error, window.is_window_active()));
                let input = window.find(("input", rename.input.entity_id()));
                assert!(input.visible(), "menu_entry={menu_entry}, frame={frame}");
                assert!(rename.input.focus_handle(cx).is_focused(window),
                    "native-style rename lost focus: menu_entry={menu_entry}, frame={frame}");
            }).unwrap();
        }
    }
}
