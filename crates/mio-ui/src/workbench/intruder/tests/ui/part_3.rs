use super::*;

#[gpui_kit::test]
fn payload_file_picker_project_change_keeps_token_until_stale_callback(cx: &mut TestAppContext) {
    let attack = attack();
    let id = attack.id;
    let expected = attack.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let (picker, old_editor) = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("tool-intruder", cx);
            window.click("intruder-payload-load", cx);
            let view = fixture.view.read(cx);
            (
                view.file_picker,
                view.intruder.editors[&id].payload_input.entity_id(),
            )
        })
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    let mut next = fixture.project;
    next.generation += 1;
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: next,
            path: None,
            workspace,
        })
        .unwrap();
    pump_native(cx);
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == next));
    let (editor, busy_before_callback) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        let editor = view.intruder.editors[&id].payload_input.entity_id();
        assert_ne!(editor, old_editor);
        assert!(!view.intruder.editors[&id].loading_payloads);
        (editor, (view.file_picker, view.file_picker_pending()))
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-payload-load", cx);
    })
    .unwrap();
    let selected = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    assert!(selected.is_file());
    cx.simulate_path_prompt_response(|_| Some(vec![selected]));
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths() && !cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert_eq!(view.intruder.editors[&id].payload_input.entity_id(), editor);
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        assert!(view.error.is_none());
    });
    assert!(
        picker.is_some(),
        "the payload picker must own the shared token"
    );
    assert_eq!(busy_before_callback, (picker, true));
}

#[gpui_kit::test]
fn payload_file_picker_backend_disconnect_discards_path_and_blocks_reload(cx: &mut TestAppContext) {
    let attack = attack();
    let id = attack.id;
    let expected = attack.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-payload-load", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    let picker = cx.read(|cx| fixture.view.read(cx).file_picker);
    drop(fixture.events);
    pump_native(cx);
    let (busy_before_callback, error) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected && !view.backend_available());
        assert!(!view.shutdown.is_cancelled());
        assert!(view.intruder.editors[&id].loading_payloads);
        (
            (view.file_picker, view.file_picker_pending()),
            view.error.clone(),
        )
    });
    assert!(drain(&mut fixture.commands).is_empty());
    let selected = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    assert!(selected.is_file());
    cx.simulate_path_prompt_response(|_| Some(vec![selected]));
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        assert_eq!(view.error, error);
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-payload-load", cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths() && !cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        assert_eq!(view.error, error);
    });
    assert!(
        picker.is_some(),
        "the payload picker must own the shared token"
    );
    assert_eq!(busy_before_callback, (picker, true));
}

#[gpui_kit::test]
fn collapsing_payloads_keeps_the_entry_and_routes_the_next_keyboard_action(
    cx: &mut TestAppContext,
) {
    let attack = attack();
    let id = attack.id;
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let input = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("tool-intruder", cx);
            let input = fixture.view.read(cx).intruder.editors[&id]
                .payload_input
                .clone();
            window.click(("input", input.entity_id()), cx);
            window.input("尚未添加", cx);
            assert!(input.focus_handle(cx).is_focused(window));
            window.click("intruder-sidebar-payloads", cx);
            assert!(!fixture.view.read(cx).intruder.editors[&id].payloads_open);
            assert_eq!(input.read(cx).value().as_ref(), "尚未添加");
            assert!(!input.focus_handle(cx).is_focused(window));
            window.press(crate::kbd!("t"), cx);
            assert_eq!(fixture.view.read(cx).workspace.intruder.tabs.len(), 1);
            assert_eq!(input.read(cx).value().as_ref(), "尚未添加");
            input
        })
        .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    cx.update_window(fixture.window, |_, _, cx| {
        assert_eq!(fixture.view.read(cx).workspace.intruder.tabs.len(), 2);
        assert_eq!(input.read(cx).value().as_ref(), "尚未添加");
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::SaveWorkspace { .. })),
        "the keyboard action only creates a tab after its Ensure ACK, never Start"
    );
}

#[gpui_kit::test]
fn restore_expands_its_group_and_moves_focus_from_the_previous_request(cx: &mut TestAppContext) {
    let first = attack();
    let first_id = first.id;
    let second = attack();
    let second_id = second.id;
    let group_id = Uuid::new_v4();
    let mut fixture = intruder_fixture(cx, vec![first, second], 64);
    fixture.view.update(cx, |view, cx| {
        view.workspace.intruder.tabs[0].group = Some(group_id);
        view.workspace.intruder.groups.push(mio_core::TabGroup {
            id: group_id,
            title: "恢复组".into(),
            collapsed: false,
            color: None,
        });
        cx.notify();
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click(SharedString::from(format!("intruder-close-{first_id}")), cx);
        window.click(SharedString::from(format!("intruder-group-{group_id}")), cx);
        assert!(fixture.view.read(cx).workspace.intruder.groups[0].collapsed);
        let input = fixture.view.read(cx).intruder.editors[&second_id]
            .request
            .state()
            .clone();
        window.click(("input", input.entity_id()), cx);
        assert!(input.focus_handle(cx).is_focused(window));
        window.press(crate::kbd!("shift-t"), cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.active, Some(first_id));
        assert!(!view.workspace.intruder.groups[0].collapsed);
        assert!(
            view.intruder.editors[&first_id]
                .request
                .state()
                .focus_handle(cx)
                .is_focused(window)
        );
        assert!(
            window
                .find(SharedString::from(format!("intruder-tab-{first_id}")))
                .visible()
        );
        window.press(crate::kbd!("t"), cx);
        assert_eq!(fixture.view.read(cx).workspace.intruder.tabs.len(), 2);
    })
    .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.tabs.len(), 3);
        assert!(!view.workspace.intruder.groups[0].collapsed);
        assert!(
            window
                .find(SharedString::from(format!("intruder-tab-{first_id}")))
                .visible()
        );
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::SaveWorkspace { .. })),
        "restored focus routes Cmd-T through Ensure without starting an attack"
    );
}

#[gpui_kit::test]
fn a_mixed_all_positions_view_cannot_remove_invisible_stale_selections(cx: &mut TestAppContext) {
    let mut attack = attack();
    let id = attack.id;
    attack
        .spec
        .positions
        .insert(0, Position { start: 0, end: 0 });
    attack.spec.payload_sets = vec![vec![b"A".to_vec()]];
    let fixture = intruder_fixture(cx, vec![attack], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click(("intruder-payload", 0_usize), cx);
        assert!(
            fixture.view.read(cx).intruder.editors[&id]
                .payload_selection
                .contains(&0)
        );
        window.click("intruder-attack-type", cx);
        let choice = window.find_by_label("Pitchfork attack").bounds().center();
        window.pointer_down(choice, MouseButton::Left, cx);
        window.pointer_up(choice, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture.view.read(cx).intruder.editors[&id]
            .payload_input
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.input("B", cx);
        window.click("intruder-payload-add", cx);
        let expected = vec![vec![b"A".to_vec(), b"B".to_vec()], vec![b"B".to_vec()]];
        let view = fixture.view.read(cx);
        assert!(!view.intruder.editors[&id].payloads_identical);
        assert!(view.intruder.editors[&id].payload_selection.is_empty());
        assert_eq!(view.workspace.intruder.tabs[0].spec.payload_sets, expected);
        assert_eq!(
            window.find("intruder-payload-remove").disabled(),
            Some(true)
        );
        window.click("intruder-payload-remove", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets,
            expected
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn unsupported_set_count_rejects_add_without_clearing_the_entry(cx: &mut TestAppContext) {
    let mut attack = attack();
    let id = attack.id;
    attack.spec.attack_type = AttackType::Pitchfork;
    attack.spec.positions = vec![Position { start: 0, end: 0 }; 256];
    let expected = attack.spec.payload_sets.clone();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let input = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("tool-intruder", cx);
            let input = fixture.view.read(cx).intruder.editors[&id]
                .payload_input
                .clone();
            window.click(("input", input.entity_id()), cx);
            window.input("保留此项", cx);
            window.press("enter", cx);
            input
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(input.read(cx).value().as_ref(), "保留此项");
        let view = fixture.view.read(cx);
        assert!(view.error.as_ref().unwrap().contains("20 个"));
        assert_eq!(view.workspace.intruder.tabs[0].spec.payload_sets, expected);
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn intruder_shortcut_does_not_cancel_a_hidden_repeater_request(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-repeater", cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    let cancel = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::Send { cancel, .. } => Some(cancel),
            _ => None,
        })
        .expect("the real Repeater Send click creates its owned cancellation token");
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.press(crate::kbd!("period"), cx);
    })
    .unwrap();
    assert!(!cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).sending.len(), 1));
}

#[test]
fn cached_hex_length_agrees_with_the_existing_lossless_decoder() {
    for text in [
        "",
        "00 fF\r\n",
        "0",
        "0g",
        "ＦＦ",
        "00\u{a0}ff",
        "\t00 FF \n",
    ] {
        assert_eq!(
            render::hex_length(text),
            editing::hex_decode(text).ok().map(|bytes| bytes.len())
        );
    }
}

#[test]
fn intruder_onboarding_rocket_svg_is_valid() {
    let image = render::onboarding::intruder_rocket_image();
    assert_eq!(image.format(), gpui_kit::ImageFormat::Svg);
    let text = std::str::from_utf8(image.bytes()).expect("rocket SVG must be valid UTF-8");
    assert!(text.contains("<svg") && text.contains("</svg>"));
}

#[gpui_kit::test]
fn empty_intruder_shows_ext_onboarding_dismiss_toggle_and_populated_layout(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.render_frame(cx);
        assert!(window.find("intruder-onboarding").visible());
        let learn_more = window.find("intruder-onboarding-learn-more");
        assert!(learn_more.visible());
        assert_eq!(learn_more.bounds().size.width, px(114.));
        assert_eq!(learn_more.bounds().size.height, px(28.));
        let dont_show = window.find("intruder-new");
        assert!(dont_show.visible());
        assert_eq!(dont_show.label(), Some("不再显示此提示"));
        assert!(window.try_find("intruder-work-tabs").is_none());

        window.click("intruder-onboarding-dismiss", cx);
        window.render_frame(cx);
        assert!(window.try_find("intruder-onboarding").is_none());
        assert!(window.find("intruder-work-tabs").visible());
        assert!(window.find("intruder-new").visible());
        assert!(window.find("intruder-toggle-orientation").visible());

        window.click("intruder-show-onboarding", cx);
        window.render_frame(cx);
        assert!(window.find("intruder-onboarding").visible());
        window.click("intruder-new", cx);
        assert!(fixture.view.read(cx).intruder.onboarding_dismissed);
    })
    .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("intruder-onboarding").is_none());
        assert!(window.find("intruder-work-tabs").visible());
        let id = fixture.view.read(cx).workspace.intruder.active.unwrap();
        let editor = fixture.view.read(cx).intruder.editors[&id]
            .request
            .state()
            .clone();
        let editor_bounds = window.find(("input", editor.entity_id())).bounds();
        let add_bounds = window.find("intruder-position-add").bounds();
        let clear_bounds = window.find("intruder-position-clear").bounds();
        let auto_bounds = window.find("intruder-position-auto").bounds();
        assert!(
            add_bounds.origin.x >= editor_bounds.right() - px(1.),
            "Add § button must sit to the right of the request editor"
        );
        assert!(add_bounds.origin.y < clear_bounds.origin.y);
        assert!(clear_bounds.origin.y < auto_bounds.origin.y);

        assert_eq!(
            fixture.view.read(cx).workspace.intruder.orientation,
            Orientation::Vertical
        );
        window.click("intruder-toggle-orientation", cx);
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.orientation,
            Orientation::Horizontal
        );
        window.click("intruder-header-settings", cx);
        window.render_frame(cx);
        assert!(fixture.view.read(cx).intruder.editors[&id].settings_open);
        assert!(window.find("intruder-config-settings").visible());
    })
    .unwrap();
}
