use super::*;

#[gpui_kit::test]
fn failed_history_transfer_cannot_create_a_tab_from_a_late_reply(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    let flow = record("http://127.0.0.1:9001/history", "保留");
    select_history(&mut fixture, &flow, cx);
    let revision = queue_history_transfer(&mut fixture, cx);
    fixture
        .events
        .try_send(Event::FlowDetailError {
            project: fixture.project,
            selection_revision: revision,
            flow_id: flow.summary.id,
            message: "fixture read failure".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert!(!fixture.view.read(cx).has_intruder_transfer(revision)));
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(flow),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert!(fixture.view.read(cx).workspace.intruder.tabs.is_empty()));
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn disconnected_history_transfer_is_removed_without_claiming_a_new_tab(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    let flow = record("http://127.0.0.1:9001/history", "保留");
    select_history(&mut fixture, &flow, cx);
    let revision = queue_history_transfer(&mut fixture, cx);
    drop(fixture.events);
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        assert!(!view.has_intruder_transfer(revision));
        assert!(view.workspace.intruder.tabs.is_empty());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
async fn payload_file_load_appends_binary_lines_saves_once_and_releases_picker(
    cx: &mut TestAppContext,
) {
    use std::io::Write;

    struct PayloadFile(PathBuf);

    impl Drop for PayloadFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    let path = std::env::temp_dir().join(format!("mio-intruder-load-{}.txt", Uuid::new_v4()));
    let mut file = std::fs::File::create_new(&path).unwrap();
    let source = PayloadFile(path);
    let bytes = b"new\r\n\r\n\xff\0\nnew\nlast";
    file.write_all(bytes).unwrap();
    drop(file);

    let attack = attack();
    let id = attack.id;
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let mut expected = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let latest = "http://127.0.0.1:8123/unsaved-load-draft";
    let pending = "尚未添加";
    let (target, input, picker) = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("tool-intruder", cx);
            let editors = &fixture.view.read(cx).intruder.editors[&id];
            let target = editors.target.clone();
            let input = editors.payload_input.clone();
            window.click(("input", target.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(latest, cx);
            window.click(("input", input.entity_id()), cx);
            window.input(pending, cx);
            window.click("intruder-payload-load", cx);
            let view = fixture.view.read(cx);
            assert!(view.intruder.editors[&id].loading_payloads);
            assert!(view.file_picker_pending());
            assert_eq!(view.workspace, expected);
            (target, input, view.file_picker.unwrap())
        })
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        Some(vec![source.0.clone()])
    });
    cx.condition(&fixture.view, move |view, _| {
        !view.intruder.editors[&id].loading_payloads
    })
    .await;
    cx.run_until_parked();

    // Saving also records the default sidebar width.
    expected.tab_sidebar_width = 180;
    let spec = &mut expected.intruder.tabs[0].spec;
    spec.draft.target = latest.into();
    spec.draft.revision += 1;
    // Draft capture and payload append advance the spec separately.
    spec.revision += 2;
    spec.payload_sets[0].extend([
        b"new".to_vec(),
        vec![],
        vec![255, 0],
        b"new".to_vec(),
        b"last".to_vec(),
    ]);
    assert!(!cx.did_prompt_for_paths() && !cx.has_pending_prompt());
    assert_eq!(std::fs::read(&source.0).unwrap(), bytes);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, expected);
        assert_eq!(target.read(cx).value().as_ref(), latest);
        assert_eq!(input.read(cx).value().as_ref(), pending);
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert!(view.error.is_none());
    });
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("Load must queue exactly one complete workspace save and no attack");
    };
    assert_eq!(*project, fixture.project);
    expected.revision += 1;
    assert_eq!(*workspace, expected);
    acknowledge(&fixture, workspace.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());

    let retry = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("intruder-payload-load", cx);
            let view = fixture.view.read(cx);
            assert!(view.intruder.editors[&id].loading_payloads);
            assert!(view.file_picker_pending());
            view.file_picker.unwrap()
        })
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    assert_ne!(retry, picker);
    cx.simulate_path_prompt_response(|_| None);
    cx.condition(&fixture.view, move |view, _| {
        !view.intruder.editors[&id].loading_payloads
    })
    .await;
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths() && !cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, expected);
        assert_eq!(target.read(cx).value().as_ref(), latest);
        assert_eq!(input.read(cx).value().as_ref(), pending);
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert!(view.error.is_none());
    });
}

#[gpui_kit::test]
fn payload_file_picker_cancel_keeps_every_item_and_reenables_the_list(cx: &mut TestAppContext) {
    let attack = attack();
    let id = attack.id;
    let expected = attack.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let latest = "http://127.0.0.1:8123/unsaved-picker-draft";
    let target = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("tool-intruder", cx);
            let target = fixture.view.read(cx).intruder.editors[&id].target.clone();
            window.click(("input", target.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(latest, cx);
            window.click("intruder-payload-load", cx);
            assert!(fixture.view.read(cx).intruder.editors[&id].loading_payloads);
            target
        })
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    let picker = cx.read(|cx| fixture.view.read(cx).file_picker);
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    let overlapping_quit = cx.has_pending_prompt();
    if overlapping_quit {
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
    }
    let busy_before_cancel = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        assert_eq!(target.read(cx).value().as_ref(), latest);
        (view.file_picker, view.file_picker_pending())
    });
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths() && !cx.has_pending_prompt());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-payload-load", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    let retry = cx.read(|cx| fixture.view.read(cx).file_picker);
    cx.simulate_path_prompt_response(|_| None);
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths());
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        assert_eq!(target.read(cx).value().as_ref(), latest);
        assert!(view.transition.is_none() && view.quit_attempt.is_none());
        assert!(!view.quitting && !view.shutdown_requested && !view.shutdown.is_cancelled());
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(
        !overlapping_quit,
        "Quit must wait for the payload picker callback"
    );
    assert!(
        picker.is_some(),
        "the payload picker must own the shared token"
    );
    assert_eq!(busy_before_cancel, (picker, true));
    assert!(retry.is_some());
    assert_ne!(retry, picker);
}

#[gpui_kit::test]
fn file_picker_completion_does_not_attach_to_a_restored_editor_instance(cx: &mut TestAppContext) {
    let attack = attack();
    let id = attack.id;
    let expected = attack.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![attack.clone()], 64);
    let (before, picker) = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click("tool-intruder", cx);
            let input = fixture.view.read(cx).intruder.editors[&id]
                .request
                .state()
                .clone();
            window.click(("input", input.entity_id()), cx);
            window.click("intruder-payload-load", cx);
            let before = fixture.view.read(cx).intruder.editors[&id]
                .payload_input
                .entity_id();
            let picker = fixture.view.read(cx).file_picker;
            window.press(crate::kbd!("w"), cx);
            window.press(crate::kbd!("shift-t"), cx);
            let view = fixture.view.read(cx);
            assert_eq!(view.workspace.intruder.active, Some(id));
            assert_ne!(view.intruder.editors[&id].payload_input.entity_id(), before);
            (before, picker)
        })
        .unwrap();
    pump_native(cx);
    let busy_after_restore = cx.read(|cx| {
        let view = fixture.view.read(cx);
        (view.file_picker, view.file_picker_pending())
    });
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("close/restore must queue only the closed-tab snapshot before its ACK");
    };
    assert_eq!(*project, fixture.project);
    assert!(workspace.intruder.tabs.is_empty() && workspace.intruder.active.is_none());
    assert_eq!(workspace.intruder.closed, vec![attack]);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/mio-missing-payload-fixture")]));
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert_ne!(view.intruder.editors[&id].payload_input.entity_id(), before);
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        assert!(
            view.error.is_none(),
            "a completion belonging to the old editor must not report into its restored instance"
        );
    });
    assert!(
        picker.is_some(),
        "the payload picker must own the shared token"
    );
    assert_eq!(busy_after_restore, (picker, true));
}

#[gpui_kit::test]
fn payload_load_respects_an_existing_picker_and_cancelled_backend(cx: &mut TestAppContext) {
    let attack = attack();
    let id = attack.id;
    let expected = attack.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
    })
    .unwrap();
    project_menu(&fixture, "打开项目…", cx);
    assert!(cx.did_prompt_for_paths());
    let picker = cx.read(|cx| fixture.view.read(cx).file_picker.unwrap());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-payload-load", cx);
    })
    .unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.file_picker, Some(picker));
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
    });
    cx.simulate_path_prompt_response(|options| {
        assert_eq!(options.prompt.as_deref(), Some("打开 MIO 项目"));
        None
    });
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths() && !cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    let shutdown = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        view.shutdown.clone()
    });
    shutdown.cancel();
    pump_native(cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-payload-load", cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(!cx.did_prompt_for_paths() && !cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.backend_disconnected && !view.backend_available());
        assert!(view.shutdown.is_cancelled());
        assert!(view.file_picker.is_none() && !view.file_picker_pending());
        assert!(!view.intruder.editors[&id].loading_payloads);
        assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
    });
}
