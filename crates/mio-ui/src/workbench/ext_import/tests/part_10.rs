use super::*;

#[gpui_kit::test]
fn ext_picker_shared_gate_ordinary_export_blocks_ext_buttons(cx: &mut TestAppContext) {
    let mut overlaps = Vec::new();
    for destination in [false, true] {
        let mut fixture = fixture(cx, vec![], 64);
        open_ext_picker(&mut fixture, destination, cx);
        answer_ext_picker(destination, None, cx);
        project_menu(&fixture, "导出 Proxy CA…", cx);
        let ordinary_picker = cx.read(|cx| fixture.view.read(cx).file_picker.unwrap());
        let button = if destination {
            "ext-convert"
        } else {
            "ext-choose-source"
        };
        click(&fixture, button, cx);
        cx.run_until_parked();
        let ext_picker = cx.read(|cx| fixture.view.read(cx).ext_import.picker);
        cx.read(|cx| assert_eq!(fixture.view.read(cx).file_picker, Some(ordinary_picker)));
        answer_ext_picker(true, None, cx);
        let extra_paths = cx.did_prompt_for_paths();
        let extra_destination = cx.did_prompt_for_new_path();
        if extra_paths {
            answer_ext_picker(false, None, cx);
        }
        if extra_destination {
            answer_ext_picker(true, None, cx);
        }
        if ext_picker.is_some() || extra_paths || extra_destination {
            overlaps.push(button);
        }
        assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
        click(&fixture, button, cx);
        cx.read(|cx| assert!(fixture.view.read(cx).ext_import.picker.is_some()));
        answer_ext_picker(destination, None, cx);
        assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.project, Some(fixture.project));
            assert!(view.ext_import.picker.is_none() && view.file_picker.is_none());
            assert!(
                !view.choosing_save_as && !view.shutdown_requested && !view.shutdown.is_cancelled()
            );
            assert!(view.ext_import.created_path().is_none());
        });
    }
    assert!(
        overlaps.is_empty(),
        "the ordinary picker must exclude {overlaps:?}"
    );
}

#[gpui_kit::test]
fn ext_picker_disconnected_backend_cannot_reopen_source(cx: &mut TestAppContext) {
    let first = tab("断连后保留源选择前的编辑");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let raw = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /source HTTP/1.1\r\nHost: localhost\r\n\r\n未保存源选择输入🙂";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let picker = open_ext_picker(&mut fixture, false, cx);
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    let error = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected && !view.backend_available());
        assert!(!view.shutdown.is_cancelled());
        assert_eq!(view.ext_import.picker, Some(picker));
        assert!(view.file_picker_pending());
        assert_eq!(raw.read(cx).value().as_str(), latest);
        view.error.clone()
    });
    assert!(cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
    let selected = std::env::temp_dir().join(format!("mio-disconnected-source-{picker}.ext_archive"));
    answer_ext_picker(false, Some(selected.clone()), cx);
    assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.ext_import.picker.is_none() && !view.file_picker_pending());
        assert!(view.ext_import.source.is_none() && view.ext_import.result.is_none());
    });

    click(&fixture, "ext-choose-source", cx);
    cx.run_until_parked();
    let reopened = (
        cx.did_prompt_for_paths(),
        cx.did_prompt_for_new_path(),
        cx.read(|cx| fixture.view.read(cx).ext_import.picker),
    );
    if reopened.0 {
        answer_ext_picker(false, None, cx);
    }
    if reopened.1 {
        answer_ext_picker(true, None, cx);
    }
    assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
    assert!(!cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(!selected.exists());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(view.workspace, original);
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert_eq!(view.error, error);
        assert!(view.backend_disconnected && !view.backend_available());
        assert!(view.ext_import.picker.is_none() && !view.file_picker_pending());
        assert!(!view.ext_import.blocks_transition());
        assert!(view.ext_import.source.is_none() && view.ext_import.result.is_none());
        assert!(view.ext_import.error.is_none() && view.ext_import.created_path().is_none());
        assert!(view.transition.is_none() && view.quit_attempt.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
    });
    assert_eq!(
        reopened,
        (false, false, None),
        "a disconnected backend must not open another source picker after the old callback"
    );
}

#[gpui_kit::test]
fn ext_picker_cancelled_backend_cannot_reopen_destination(cx: &mut TestAppContext) {
    let first = tab("取消后保留目标选择前的编辑");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let raw = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /destination HTTP/1.1\r\nHost: localhost\r\n\r\n未保存目标选择输入🙂";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let picker = open_ext_picker(&mut fixture, true, cx);
    let shutdown = cx.read(|cx| fixture.view.read(cx).shutdown.clone());
    // Cancellation can precede channel closure while the inspection is still visible.
    shutdown.cancel();
    cx.run_until_parked();
    let inspection_id = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.backend_disconnected && !view.backend_available());
        assert!(view.shutdown.is_cancelled());
        assert_eq!(view.ext_import.picker, Some(picker));
        assert!(view.file_picker_pending());
        assert_eq!(raw.read(cx).value().as_str(), latest);
        let Some(ExtJobOutcome::Inspected(inspection)) = &view.ext_import.result else {
            panic!("the destination picker must still own its inspection");
        };
        inspection.id
    });
    assert!(!cx.did_prompt_for_paths() && cx.did_prompt_for_new_path());
    let selected = std::env::temp_dir().join(format!("mio-cancelled-destination-{picker}.mio"));
    answer_ext_picker(true, Some(selected.clone()), cx);
    assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.ext_import.picker.is_none() && !view.file_picker_pending());
        assert!(matches!(&view.ext_import.result,
            Some(ExtJobOutcome::Inspected(inspection)) if inspection.id == inspection_id));
    });

    click(&fixture, "ext-convert", cx);
    cx.run_until_parked();
    let reopened = (
        cx.did_prompt_for_paths(),
        cx.did_prompt_for_new_path(),
        cx.read(|cx| fixture.view.read(cx).ext_import.picker),
    );
    if reopened.0 {
        answer_ext_picker(false, None, cx);
    }
    if reopened.1 {
        answer_ext_picker(true, None, cx);
    }
    assert!(!cx.did_prompt_for_paths() && !cx.did_prompt_for_new_path());
    assert!(!cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(!selected.exists());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(view.workspace, original);
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert!(!view.backend_disconnected && !view.backend_available());
        let import = &view.ext_import;
        assert!(import.picker.is_none() && !view.file_picker_pending());
        assert!(import.job.is_none() && import.pending_release.is_none());
        assert!(import.blocks_transition());
        assert!(matches!(&import.result,
            Some(ExtJobOutcome::Inspected(inspection)) if inspection.id == inspection_id));
        assert_eq!(
            import.source.as_ref(),
            Some(&PathBuf::from("/tmp/mio-ui-ext-synthetic.archive"))
        );
        assert!(view.error.is_none() && import.error.is_none() && import.created_path().is_none());
        assert!(view.transition.is_none() && view.quit_attempt.is_none());
        assert!(!view.shutdown_requested && view.shutdown.is_cancelled() && !view.quitting);
    });
    assert_eq!(
        reopened,
        (false, false, None),
        "a cancelled backend must not open another destination picker after the old callback"
    );
}
