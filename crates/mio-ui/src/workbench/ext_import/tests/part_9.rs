use super::*;

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn ext_picker_shared_gate_native_quit_after_disconnect_keeps_only_original_panel(
    cx: &mut TestAppContext,
) {
    use crate::native_terminate::tests::Probe;
    use objc2_app_kit::NSApplicationTerminateReply as Reply;

    let mut fixture = fixture(cx, vec![tab("原生退出保留草稿")], 64);
    let picker = open_ext_picker(&mut fixture, false, cx);
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    let second_prompt = cx.has_pending_prompt();
    let recognized = cx.read(|cx| fixture.view.read(cx).file_picker_pending());
    if second_prompt {
        cx.simulate_prompt_answer("取消");
        cx.run_until_parked();
    }
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert!(cx.did_prompt_for_paths());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).ext_import.picker, Some(picker)));
    answer_ext_picker(false, None, cx);
    assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "closing the picker must release the native gate"
    );
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    probe.pump_modal_until_replies(2);
    assert_eq!(probe.replies(), [false, false]);
    assert!(!cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(view.workspace.tabs[0].title, "原生退出保留草稿");
        assert!(view.backend_disconnected && view.ext_import.picker.is_none());
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
    });
    assert_eq!(
        (second_prompt, recognized),
        (false, true),
        "the native observer must reject while the disconnected External panel is still pending"
    );
}

#[gpui_kit::test]
fn ext_picker_shared_gate_project_change_blocks_other_pickers(cx: &mut TestAppContext) {
    let mut overlaps = Vec::new();
    for entry in ["打开项目…", "另存为…", "导出 Proxy CA…", "插件目录"] {
        let mut fixture = fixture(cx, vec![], 64);
        let picker = open_ext_picker(&mut fixture, false, cx);
        let mut current = fixture.project;
        current.generation += 1;
        fixture
            .events
            .try_send(Event::ProjectOpened {
                project: current,
                path: None,
                workspace: Workspace::default(),
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            match entry {
                "打开项目…" => window.dispatch_action(Box::new(OpenProject), cx),
                "另存为…" => window.dispatch_action(Box::new(SaveProjectAs), cx),
                "导出 Proxy CA…" => window.dispatch_action(Box::new(ExportCa), cx),
                _ => {
                    window.click("tool-extensions", cx);
                    window.click("plugin-choose", cx);
                }
            }
        })
        .unwrap();
        cx.run_until_parked();
        let ordinary_picker = cx.read(|cx| fixture.view.read(cx).file_picker);
        cx.read(|cx| assert_eq!(fixture.view.read(cx).ext_import.picker, Some(picker)));
        answer_ext_picker(false, None, cx);
        let extra_paths = cx.did_prompt_for_paths();
        let extra_destination = cx.did_prompt_for_new_path();
        if extra_paths {
            answer_ext_picker(false, None, cx);
        }
        if extra_destination {
            answer_ext_picker(true, None, cx);
        }
        if ordinary_picker.is_some() || extra_paths || extra_destination {
            overlaps.push(entry);
        }
        assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
        assert!(!cx.has_pending_prompt());
        assert!(matches!(drain(&mut fixture.commands).as_slice(),
            [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == current));
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.project, Some(current));
            assert!(view.ext_import.picker.is_none() && view.file_picker.is_none());
            assert!(!view.choosing_save_as && !view.ext_import.blocks_transition());
            assert!(view.ext_import.created_path().is_none());
            assert!(!view.shutdown_requested && !view.shutdown.is_cancelled());
        });
    }
    assert!(
        overlaps.is_empty(),
        "a pending External panel must exclude {overlaps:?}"
    );
}
