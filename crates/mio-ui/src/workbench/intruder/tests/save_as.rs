use gpui_kit::test::TestWindowExt;

use super::*;

fn click_start(fixture: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-start", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn save_as_blocks_waiting_start(choose_path_before_ack: bool, cx: &mut TestAppContext) {
    let original = attack();
    let tab_id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    click_start(&fixture, cx);
    let persisted = saved(&mut fixture);
    let (run_id, cancel) = cx.read(|cx| {
        let pending = &fixture.view.read(cx).intruder.starts[&tab_id];
        assert!(pending.phase == StartPhase::Saving);
        (pending.run_id, pending.cancel.clone())
    });

    project_menu(&fixture, "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.read(|cx| assert!(fixture.view.read(cx).choosing_save_as));
    assert!(drain(&mut fixture.commands).is_empty());
    let destination = PathBuf::from("/intruder-save-as-event-fixture.mio");
    if choose_path_before_ack {
        cx.simulate_new_path_selection(|_| Some(destination.clone()));
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(!view.choosing_save_as);
            assert_eq!(view.pending_project_save, Some(Some(destination.clone())));
        });
    }

    acknowledge(&fixture, persisted, cx);
    let late_commands = drain(&mut fixture.commands);
    if choose_path_before_ack {
        fixture
            .events
            .try_send(Event::ProjectSaved {
                websocket_notes: vec![],
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project: fixture.project,
                path: destination.clone(),
                total_bytes: 4096,
            })
            .unwrap();
    } else {
        cx.simulate_new_path_selection(|_| None);
    }
    cx.run_until_parked();
    let after_save = drain(&mut fixture.commands);

    assert!(
        !late_commands
            .iter()
            .chain(&after_save)
            .any(|command| matches!(command, Command::StartIntruder { .. })),
        "an old workspace ACK must not start an attack while Save As owns the project"
    );
    if choose_path_before_ack {
        assert!(matches!(late_commands.as_slice(),
            [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: Some(path), workspace: None }]
            if *project == fixture.project && path == &destination));
    } else {
        assert!(late_commands.is_empty());
    }
    assert!(after_save.is_empty());
    assert!(cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.starts.is_empty());
        assert!(view.intruder.runs.is_empty());
        assert_eq!(view.workspace.intruder.tabs, vec![original.clone()]);
        assert!(!view.choosing_save_as);
    });

    click_start(&fixture, cx);
    let retry = saved(&mut fixture);
    acknowledge(&fixture, retry, cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::StartIntruder {
            project,
            run_id: retry_run,
            tab_id: retry_tab,
            spec,
            cancel: retry_cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("only a new explicit Start click may dispatch the preserved attack");
    };
    assert_eq!((*project, *retry_tab), (fixture.project, tab_id));
    assert_ne!(*retry_run, run_id);
    assert_eq!(**spec, original.spec);
    assert!(!retry_cancel.is_cancelled());
}

#[gpui_kit::test]
fn choosing_save_as_cancels_start_waiting_for_workspace_ack(cx: &mut TestAppContext) {
    save_as_blocks_waiting_start(false, cx);
}

#[gpui_kit::test]
fn selected_save_as_destination_cancels_start_waiting_for_workspace_ack(cx: &mut TestAppContext) {
    save_as_blocks_waiting_start(true, cx);
}
