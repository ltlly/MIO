use gpui_kit::test::TestWindowExt;
use mio_core::IntruderRetention;

use super::*;

fn click(fixture: &Fixture, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
}

fn started_summary(command: &Command, retention: IntruderRetention) -> IntruderRunSummary {
    let Command::StartIntruder {
        run_id,
        tab_id,
        spec,
        ..
    } = command
    else {
        panic!("Start command required")
    };
    IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: *run_id,
        tab_id: *tab_id,
        revision: 1,
        spec_revision: spec.revision,
        total: spec.validate().unwrap(),
        dispatched: 0,
        completed: 0,
        state: IntruderRunState::Running,
        started_unix_ms: 1,
        retention,
        title: String::new(),
        title_revision: 0,
    }
}

#[gpui_kit::test]
fn new_intruder_tab_defaults_to_session_and_temporary_save_setting_is_disabled(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    click(&fixture, "tool-intruder", cx);
    click(&fixture, "intruder-new", cx);
    acknowledge_default_pool(&mut fixture, cx);
    let snapshot = saved(&mut fixture);
    assert!(!snapshot.intruder.tabs[0].spec.save_to_project);
    acknowledge(&fixture, snapshot.clone(), cx);
    click(&fixture, "intruder-sidebar-settings", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("intruder-config-settings").visible());
        let checkbox = window.find("intruder-config-save-to-project");
        assert_eq!(checkbox.checked(), Some(false));
        assert_eq!(checkbox.disabled(), Some(true));
        window.click("intruder-config-save-to-project", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs,
            snapshot.intruder.tabs
        )
    });
}

#[gpui_kit::test]
fn named_save_setting_advances_only_spec_and_revokes_pending_start(cx: &mut TestAppContext) {
    let mut original = attack();
    original.spec.save_to_project = false;
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-save-setting.mio"))
        })
    });
    click(&fixture, "tool-intruder", cx);
    click(&fixture, "intruder-sidebar-settings", cx);
    click(&fixture, "intruder-start", cx);
    let before = saved(&mut fixture);
    click(&fixture, "intruder-config-save-to-project", cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.starts.is_empty());
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft,
            original.spec.draft
        );
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.revision,
            original.spec.revision + 1
        );
        assert!(view.workspace.intruder.tabs[0].spec.save_to_project);
        assert_eq!(
            view.intruder.editors[&id].loaded_revision,
            original.spec.draft.revision
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, before, cx);
    let changed = saved(&mut fixture);
    assert!(changed.intruder.tabs[0].spec.save_to_project);
    acknowledge(&fixture, changed, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "a changed setting never automatically replays Start"
    );
    click(&fixture, "intruder-start", cx);
    let activation = saved(&mut fixture);
    acknowledge(&fixture, activation, cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::StartIntruder { spec, .. }] if spec.save_to_project)
    );
}

#[gpui_kit::test]
fn save_setting_click_preserves_ime_composition_and_does_not_partially_capture(
    cx: &mut TestAppContext,
) {
    let original = attack();
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-save-setting-ime.mio"))
        })
    });
    click(&fixture, "tool-intruder", cx);
    click(&fixture, "intruder-sidebar-settings", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture.view.read(cx).intruder.editors[&id].target.clone();
        window.click(("input", input.entity_id()), cx);
        input.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
        });
        window.click("intruder-config-save-to-project", cx);
        assert!(input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.tabs[0], original);
        assert_eq!(view.error.as_deref(), Some(editing::COMPOSITION_PENDING));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn session_started_ack_immediately_guards_exit_and_never_persists_run_reference(
    cx: &mut TestAppContext,
) {
    let mut original = attack();
    original.spec.save_to_project = false;
    original.last_run = Some(Uuid::new_v4());
    let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
    click(&fixture, "tool-intruder", cx);
    click(&fixture, "intruder-start", cx);
    let workspace = saved(&mut fixture);
    acknowledge(&fixture, workspace, cx);
    let commands = drain(&mut fixture.commands);
    let [command @ Command::StartIntruder { spec, .. }] = commands.as_slice() else {
        panic!("one Session activation expected")
    };
    assert!(!spec.save_to_project);
    let summary = started_summary(command, IntruderRetention::Session);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::IntruderStarted {
                    project: fixture.project,
                    run_id: summary.id,
                    tab_id: summary.tab_id,
                    spec_revision: summary.spec_revision,
                    result: Ok(summary.clone()),
                },
                window,
                cx,
            );
            assert!(
                view.intruder.session_runs.contains(&summary.id),
                "Session is known before any result page"
            );
            assert_eq!(
                view.intruder.runs[&summary.id].summary.retention,
                IntruderRetention::Session
            );
            assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
            assert_eq!(
                view.workspace.intruder.tabs[0].spec.draft,
                original.spec.draft
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let persisted: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .collect();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].intruder.tabs[0].last_run, None);
    assert_eq!(
        persisted[0].intruder.tabs[0].spec.draft,
        original.spec.draft
    );
}

#[gpui_kit::test]
fn started_retention_must_match_the_frozen_save_setting(cx: &mut TestAppContext) {
    for save_to_project in [false, true] {
        let mut original = attack();
        original.spec.save_to_project = save_to_project;
        let mut fixture = intruder_fixture(cx, vec![original.clone()], 64);
        click(&fixture, "tool-intruder", cx);
        click(&fixture, "intruder-start", cx);
        let workspace = saved(&mut fixture);
        acknowledge(&fixture, workspace, cx);
        let commands = drain(&mut fixture.commands);
        let [command @ Command::StartIntruder { cancel, .. }] = commands.as_slice() else {
            panic!("one activation expected")
        };
        let wrong = started_summary(
            command,
            if save_to_project {
                IntruderRetention::Session
            } else {
                IntruderRetention::Project
            },
        );
        fixture
            .events
            .try_send(Event::IntruderStarted {
                project: fixture.project,
                run_id: wrong.id,
                tab_id: wrong.tab_id,
                spec_revision: wrong.spec_revision,
                result: Ok(wrong.clone()),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(cancel.is_cancelled());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(!view.intruder.runs.contains_key(&wrong.id));
            assert!(!view.intruder.session_runs.contains(&wrong.id));
            assert_eq!(view.workspace.intruder.tabs[0], original);
            assert!(view.error.as_ref().unwrap().contains("不一致"));
        });
        assert!(drain(&mut fixture.commands).is_empty());
    }
}
