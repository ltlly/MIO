use super::*;

#[gpui_kit::test]
fn ext_picker_callback_receiver_close_releases_token_and_allows_retry(cx: &mut TestAppContext) {
    for destination in [false, true] {
        let mut fixture = fixture(cx, vec![], 64);
        let picker = open_ext_picker(&mut fixture, destination, cx);
        close_ext_picker_receiver(destination, cx);
        assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            let import = &view.ext_import;
            assert_eq!(view.project, Some(fixture.project));
            assert!(import.picker.is_none() && import.job.is_none());
            assert_eq!(import.error.as_deref(), Some("文件选择器已意外关闭。"));
            assert_eq!(import.blocks_transition(), destination);
            assert!(import.created_path().is_none());
        });
        if destination {
            click(&fixture, "ext-convert", cx);
        } else {
            project_menu(&fixture, "导入 外部归档项目…", cx);
        }
        let retry = cx.read(|cx| fixture.view.read(cx).ext_import.picker.unwrap());
        assert_ne!(retry, picker);
        answer_ext_picker(destination, None, cx);
        assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| assert!(fixture.view.read(cx).ext_import.picker.is_none()));
    }
}

#[gpui_kit::test]
fn ext_picker_callback_stale_selection_and_receiver_close_do_not_retarget(
    cx: &mut TestAppContext,
) {
    for destination in [false, true] {
        for disconnected in [false, true] {
            for receiver_closed in [false, true] {
                let mut fixture = fixture(cx, vec![], 64);
                let picker = open_ext_picker(&mut fixture, destination, cx);
                let mut current = fixture.project;
                if disconnected {
                    let (replacement, _) = mpsc::channel(1);
                    drop(std::mem::replace(&mut fixture.events, replacement));
                } else {
                    current.generation += 1;
                    fixture
                        .events
                        .try_send(Event::ProjectOpened {
                            project: current,
                            path: None,
                            workspace: Workspace::default(),
                        })
                        .unwrap();
                }
                cx.run_until_parked();
                let before_callback = cx.read(|cx| {
                    let view = fixture.view.read(cx);
                    assert_eq!(view.project, Some(current));
                    assert_eq!(view.backend_disconnected, disconnected);
                    (
                        view.ext_import.picker,
                        view.error.clone(),
                        view.ext_import.error.clone(),
                    )
                });
                let extension = if destination { "mio" } else { "ext_archive" };
                let selected = std::env::temp_dir()
                    .join(format!("mio-ext-picker-callback-{picker}.{extension}"));
                if receiver_closed {
                    close_ext_picker_receiver(destination, cx);
                } else {
                    answer_ext_picker(destination, Some(selected.clone()), cx);
                }
                assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
                assert!(!selected.exists());
                let commands = drain(&mut fixture.commands);
                if disconnected {
                    assert!(commands.is_empty());
                } else {
                    assert!(matches!(commands.as_slice(),
                        [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == current));
                }
                cx.read(|cx| {
                    let view = fixture.view.read(cx);
                    let import = &view.ext_import;
                    assert_eq!(view.project, Some(current));
                    assert!(import.picker.is_none() && !import.blocks_transition());
                    assert!(import.created_path().is_none());
                    assert_eq!(view.error, before_callback.1);
                    assert_eq!(import.error, before_callback.2);
                    if !disconnected {
                        assert!(import.source.is_none() && import.result.is_none());
                    }
                });
                assert_eq!(before_callback.0, Some(picker));
            }
        }
    }
}

#[gpui_kit::test]
fn ext_picker_shared_gate_direct_quit_after_disconnect_keeps_only_original_panel(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("断开期间保留草稿")], 64);
    let picker = open_ext_picker(&mut fixture, false, cx);
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    pump_native(cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    let second_prompt = cx.has_pending_prompt();
    let recognized = cx.read(|cx| fixture.view.read(cx).file_picker_pending());
    if second_prompt {
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
    }
    assert!(cx.did_prompt_for_paths());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).ext_import.picker, Some(picker)));
    answer_ext_picker(false, None, cx);
    assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "closing the picker must release the Quit gate"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(!cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(view.workspace.tabs[0].title, "断开期间保留草稿");
        assert!(view.backend_disconnected && view.ext_import.picker.is_none());
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
    });
    assert_eq!(
        (second_prompt, recognized),
        (false, true),
        "direct Quit must recognize the disconnected External panel without another confirmation"
    );
}
