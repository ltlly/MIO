use super::*;

#[gpui_kit::test]
fn remember_running_continue_saves_only_running_choice_before_detaching(cx: &mut TestAppContext) {
    mapped_choice(
        false,
        "在后台继续运行",
        Running::Continue,
        Finished::KeepMemory,
        cx,
    );
}

#[gpui_kit::test]
fn remember_running_delete_saves_choice_before_stop_without_premature_discard(
    cx: &mut TestAppContext,
) {
    mapped_choice(false, "丢弃", Running::Delete, Finished::KeepMemory, cx);
}

#[gpui_kit::test]
fn remember_finished_save_starts_project_save_without_waiting_for_preference_ack(
    cx: &mut TestAppContext,
) {
    mapped_choice(
        true,
        "保存到项目文件",
        Running::Continue,
        Finished::SaveProject,
        cx,
    );
}

#[gpui_kit::test]
fn remember_finished_keep_detaches_session_without_waiting_for_preference_ack(
    cx: &mut TestAppContext,
) {
    mapped_choice(
        true,
        "保留在内存",
        Running::Continue,
        Finished::KeepMemory,
        cx,
    );
}

#[gpui_kit::test]
fn remember_finished_delete_submits_only_the_selected_terminal_discard(cx: &mut TestAppContext) {
    mapped_choice(true, "丢弃", Running::Continue, Finished::Delete, cx);
}

#[gpui_kit::test]
fn remember_escape_preserves_task_and_reopens_unchecked(cx: &mut TestAppContext) {
    let (mut fixture, started, confirmed) = setup(false, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    ask_to_close(&started, true, cx);
    let old = remember(cx);
    answer_close_dialog("取消", cx);
    assert!(old.update(cx, |_, _, _| ()).is_err());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    ask_to_close(&started, true, cx);
    let reopened = close_dialog(cx).unwrap();
    assert_ne!(old, reopened);
    cx.update_window(reopened, |_, window, _| {
        assert_eq!(
            window.find("intruder-close-remember").checked(),
            Some(false)
        );
    })
    .unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&confirmed));
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
