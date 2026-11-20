use super::*;

#[gpui_kit::test]
fn close_policy_finished_ask_and_load_error_never_apply_automatic_delete(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    completed(&mut fixture, &mut started, cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Delete,
            FinishedAttackClose::Ask,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    ask_to_close(&started, true, cx);
    let dialog = close_dialog(cx).unwrap();
    cx.update_window(dialog, |_, window, _| {
        assert!(window.try_find_by_label("保存到项目文件").is_none());
        assert!(window.find_by_label("保留在内存").visible());
        assert!(window.find_by_label("丢弃").visible());
    })
    .unwrap();
    answer_close_dialog("取消", cx);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    assert!(!has_close_dialog(cx));
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
        assert!(view.intruder_result_close.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Delete,
            FinishedAttackClose::Delete,
            2,
            Some("synthetic load failure".into()),
            cx,
        )
        .is_empty()
    );
    ask_to_close(&started, false, cx);
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    answer_close_dialog("保留在内存", cx);
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SetIntruderRetention {
            retention: mio_core::IntruderRetention::Session,
            ..
        }]
    ));
}

#[gpui_kit::test]
fn close_policy_late_automatic_discard_ack_updates_data_but_keeps_window(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    completed(&mut fixture, &mut started, cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Continue,
            FinishedAttackClose::Delete,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    close_without_prompt(&started, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.intruder.results.views[&started.summary.id].discarded);
    });
}

#[gpui_kit::test]
fn close_policy_never_overrides_explicit_dashboard_delete_or_save(cx: &mut TestAppContext) {
    let (mut fixture, first, _) = dashboard_tasks(cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Continue,
            FinishedAttackClose::SaveProject,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    dashboard_task_menu(&fixture, first.id, "删除", cx);
    assert!(cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_prompt_answer("否");
    cx.run_until_parked();
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Delete,
            FinishedAttackClose::Delete,
            2,
            None,
            cx,
        )
        .is_empty()
    );
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-policy-dashboard.mio"))
        })
    });
    dashboard_task_menu(&fixture, first.id, "保存到项目文件", cx);
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::SetIntruderRetention { retention: mio_core::IntruderRetention::Project, run_id, .. }] if *run_id == first.id)
    );
}
