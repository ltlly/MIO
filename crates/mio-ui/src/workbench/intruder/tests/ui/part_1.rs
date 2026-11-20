use super::*;

#[gpui_kit::test]
fn new_tabs_type_close_restore_and_navigation_use_real_intruder_events(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    let request = "GET /draft?literal=§ HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-new", cx);
        assert!(fixture.view.read(cx).workspace.intruder.tabs.is_empty());
    })
    .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    let first = cx
        .update_window(fixture.window, |_, window, cx| {
            let first = fixture.view.read(cx).workspace.intruder.active.unwrap();
            let view = fixture.view.read(cx);
            assert_eq!(view.workspace.intruder.orientation, Orientation::Vertical);
            assert!(view.workspace.intruder.tabs[0].spec.draft.raw.is_empty());
            assert!(view.workspace.intruder.tabs[0].spec.draft.target.is_empty());
            let input = view.intruder.editors[&first].request.state().clone();
            let target = view.intruder.editors[&first].target.clone();
            assert!(input.focus_handle(cx).is_focused(window));
            window.input(request, cx);
            window.click(("input", target.entity_id()), cx);
            window.input("http://127.0.0.1:8123", cx);
            window.press(crate::kbd!("t"), cx);
            assert_eq!(fixture.view.read(cx).workspace.intruder.tabs.len(), 1);
            first
        })
        .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        let second = view.workspace.intruder.active.unwrap();
        assert_ne!(first, second);
        assert_eq!(view.workspace.intruder.tabs.len(), 2);
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft.raw,
            request.as_bytes()
        );
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft.target,
            "http://127.0.0.1:8123"
        );
        assert!(view.workspace.intruder.tabs[0].spec.positions.is_empty());
        assert!(
            view.intruder.editors[&second]
                .request
                .state()
                .focus_handle(cx)
                .is_focused(window)
        );
        window.press("ctrl-shift-tab", cx);
        assert_eq!(fixture.view.read(cx).workspace.intruder.active, Some(first));
        window.press(crate::kbd!("w"), cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.active,
            Some(second)
        );
        assert_eq!(fixture.view.read(cx).workspace.intruder.closed[0].id, first);
        window.press(crate::kbd!("shift-t"), cx);
        assert_eq!(fixture.view.read(cx).workspace.intruder.active, Some(first));
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.intruder.tabs.last().unwrap().spec.draft.raw,
            request.as_bytes()
        );
        assert!(view.workspace.intruder.closed.is_empty());
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    // Cmd-R created a new tab with no stored history.
    let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::SaveWorkspace { .. }))
    );
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.editors.contains_key(&first)));
}

#[gpui_kit::test]
fn send_to_intruder_and_repeater_copy_current_input_with_independent_ids(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    let request = "POST /edited HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n中文";
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    let source = cx
        .update_window(fixture.window, |_, window, cx| {
            let source = fixture.view.read(cx).workspace.active.unwrap();
            let input = fixture.view.read(cx).tab_editors[&source]
                .request
                .state()
                .clone();
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(request, cx);
            window.press(crate::kbd!("i"), cx);
            assert!(fixture.view.read(cx).workspace.intruder.tabs.is_empty());
            source
        })
        .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Intruder);
        let intruder = view.workspace.intruder.active.unwrap();
        assert_ne!(source, intruder);
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft.raw,
            request.as_bytes()
        );
        assert_eq!(view.workspace.intruder.tabs[0].spec.draft.revision, 1);
        assert!(
            view.intruder.editors[&intruder]
                .request
                .state()
                .focus_handle(cx)
                .is_focused(window)
        );
        window.press(crate::kbd!("r"), cx);
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Repeater);
        let duplicate = view.workspace.active.unwrap();
        assert_ne!(duplicate, source);
        assert_ne!(duplicate, intruder);
        assert_eq!(view.workspace.tabs.len(), 2);
        assert_eq!(view.workspace.tabs[1].draft.raw, request.as_bytes());
        assert_eq!(view.workspace.tabs[1].draft.revision, 1);
        assert!(
            view.tab_editors[&duplicate]
                .request
                .state()
                .focus_handle(cx)
                .is_focused(window)
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::SaveWorkspace { .. }))
    );
}

#[gpui_kit::test]
fn payload_actions_preserve_binary_empty_and_duplicates_until_explicit_edits(
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
            window.input("新增", cx);
            window.press("enter", cx);
            input
        })
        .unwrap();
    // PressEnter subscriptions run after the outer update has delivered its effects.
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(input.read(cx).value().is_empty());
        cx.write_to_clipboard(ClipboardItem::new_string("a\r\n\r\n重复\n重复\n".into()));
        window.click("intruder-payload-paste", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![
                b"a".to_vec(),
                vec![0, 255],
                "新增".as_bytes().to_vec(),
                b"a".to_vec(),
                vec![],
                "重复".as_bytes().to_vec(),
                "重复".as_bytes().to_vec(),
            ]
        );
        window.click("intruder-payload-deduplicate", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![
                b"a".to_vec(),
                vec![0, 255],
                "新增".as_bytes().to_vec(),
                vec![],
                "重复".as_bytes().to_vec(),
            ]
        );
        window.click(("intruder-payload", 1_usize), cx);
        window.click("intruder-payload-remove", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![
                b"a".to_vec(),
                "新增".as_bytes().to_vec(),
                vec![],
                "重复".as_bytes().to_vec(),
            ]
        );
        window.click("intruder-payload-clear", cx);
        assert!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0]
                .is_empty()
        );
        window.click("intruder-payload-add", cx);
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![Vec::<u8>::new()]
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::SaveWorkspace { .. }))
    );
}

#[gpui_kit::test]
fn changing_attack_types_retains_every_payload_set_and_only_changes_configuration(
    cx: &mut TestAppContext,
) {
    let mut attack = attack();
    attack.spec.attack_type = AttackType::Pitchfork;
    attack
        .spec
        .positions
        .insert(0, Position { start: 6, end: 14 });
    attack.spec.payload_sets.push(vec![b"second".to_vec()]);
    let raw = attack.spec.draft.clone();
    let sets = attack.spec.payload_sets.clone();
    let fixture = intruder_fixture(cx, vec![attack], 64);
    for (kind, label) in [
        (AttackType::ClusterBomb, "Cluster bomb attack"),
        (AttackType::Sniper, "Sniper attack"),
        (AttackType::BatteringRam, "Battering ram attack"),
        (AttackType::Pitchfork, "Pitchfork attack"),
    ] {
        cx.update_window(fixture.window, |_, window, cx| {
            window.click("tool-intruder", cx);
            window.click("intruder-attack-type", cx);
            let choice = window.find_by_label(label).bounds().center();
            window.pointer_down(choice, MouseButton::Left, cx);
            window.pointer_up(choice, MouseButton::Left, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.read(|cx| {
            let spec = &fixture.view.read(cx).workspace.intruder.tabs[0].spec;
            assert_eq!(spec.attack_type, kind);
            assert_eq!(spec.payload_sets, sets);
            assert_eq!(spec.draft, raw);
        });
    }
}

#[gpui_kit::test]
fn start_button_saves_exact_configuration_before_admission(cx: &mut TestAppContext) {
    let attack = attack();
    let id = attack.id;
    let original = attack.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-update-host", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let changed = saved(&mut fixture);
    acknowledge(&fixture, changed, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-start", cx)
    })
    .unwrap();
    cx.run_until_parked();
    let pending = saved(&mut fixture);
    let stored = &pending.intruder.tabs[0].spec;
    assert!(!stored.update_host_header);
    assert_eq!(stored.revision, original.revision + 1);
    assert_eq!(stored.draft, original.draft);
    acknowledge(&fixture, pending, cx);
    let commands = drain(&mut fixture.commands);
    let [Command::StartIntruder { tab_id, spec, .. }] = commands.as_slice() else {
        panic!("acknowledging the button's saved configuration admits one attack");
    };
    assert_eq!(*tab_id, id);
    assert!(!spec.update_host_header);
    assert_eq!(spec.draft, original.draft);
}

#[gpui_kit::test]
fn history_transfer_captures_original_view_before_a_late_selection_change(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    let flow = record("http://127.0.0.1:9001/history", "最终正文");
    let original = OriginalMessages {
        request: Some(record("http://127.0.0.1:9001/history", "原始正文").request),
        ..OriginalMessages::default()
    };
    let detail_revision = select_history(&mut fixture, &flow, cx);
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: detail_revision,
            record: Box::new(flow.clone()),
            original: Box::new(original.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-request-version", cx);
        window.render_frame(cx);
        let original_item = window.find_by_label("原始请求").bounds().center();
        window.pointer_down(original_item, MouseButton::Left, cx);
        window.pointer_up(original_item, MouseButton::Left, cx);
        let input = fixture.view.read(cx).history_request.state().clone();
        window.click(("input", input.entity_id()), cx);
    })
    .unwrap();
    let revision = queue_history_transfer(&mut fixture, cx);
    assert_ne!(detail_revision, revision);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-request-version", cx);
        window.render_frame(cx);
        let final_item = window.find_by_label("最终请求").bounds().center();
        window.pointer_down(final_item, MouseButton::Left, cx);
        window.pointer_up(final_item, MouseButton::Left, cx);
    })
    .unwrap();
    let raw = request_to_raw(original.request.as_ref().unwrap());
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(flow),
            original: Box::new(original),
        })
        .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Intruder);
        assert_eq!(view.workspace.intruder.tabs.len(), 1);
        assert_eq!(view.workspace.intruder.tabs[0].spec.draft.raw, raw);
        let id = view.workspace.intruder.active.unwrap();
        assert!(
            view.intruder.editors[&id]
                .request
                .state()
                .focus_handle(cx)
                .is_focused(window)
        );
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::SaveWorkspace { .. }))
    );
}

#[gpui_kit::test]
fn wrong_flow_replies_do_not_consume_a_pending_history_transfer(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    let flow = record("http://127.0.0.1:9001/history", "待传入的正文");
    let other = record("http://127.0.0.1:9001/other", "不属于此读取的正文");
    select_history(&mut fixture, &flow, cx);
    let revision = queue_history_transfer(&mut fixture, cx);
    for event in [
        Event::FlowDetailError {
            project: fixture.project,
            selection_revision: revision,
            flow_id: other.summary.id,
            message: "unrelated read failed".into(),
        },
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(other),
            original: Box::default(),
        },
    ] {
        fixture.events.try_send(event).unwrap();
        cx.run_until_parked();
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.has_intruder_transfer(revision));
            assert!(view.workspace.intruder.tabs.is_empty());
            assert!(view.error.is_none());
        });
        assert!(drain(&mut fixture.commands).is_empty());
    }
    let raw = request_to_raw(&flow.request);
    for repetition in 0..2 {
        fixture
            .events
            .try_send(Event::FlowDetail {
                project: fixture.project,
                selection_revision: revision,
                record: Box::new(flow.clone()),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();
        if repetition == 0 {
            acknowledge_default_pool(&mut fixture, cx);
        }
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(!view.has_intruder_transfer(revision));
            assert_eq!(view.workspace.intruder.tabs.len(), 1);
            assert_eq!(view.workspace.intruder.tabs[0].spec.draft.raw, raw);
        });
    }
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveWorkspace { .. }]
    ));
}
