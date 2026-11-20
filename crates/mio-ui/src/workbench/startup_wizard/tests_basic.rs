use mio_core::{UserPreferences, UserPreferencesState};

use gpui_kit::{AnyWindowHandle, TestAppContext, test::TestWindowExt};
use mio_core::StartupPreferences;

use super::*;
use crate::workbench::tests::{drain, fixture, tab};

pub(super) fn click(
    window: AnyWindowHandle,
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn startup_wizard_modes_name_file_sync_and_start_project(cx: &mut TestAppContext) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
                assert!(wb.startup_wizard_active());
                assert_eq!(
                    wb.startup_wizard.as_ref().map(|w| (w.step, w.mode)),
                    Some((WizardStep::Step1, ProjectMode::Temporary))
                );

                wb.select_wizard_project_mode(ProjectMode::NewOnDisk, window, cx);
                let wizard = wb.startup_wizard.as_ref().expect("wizard initialized");
                assert_eq!(wizard.mode, ProjectMode::NewOnDisk);
                let today = format_today_iso();
                assert_eq!(wizard.new_name_input.read(cx).value().as_ref(), today);
                assert_eq!(
                    wizard.new_file_input.read(cx).value().as_ref(),
                    format!("{today}.mio")
                );

                let name_input = wizard.new_name_input.clone();
                name_input.update(cx, |state, cx| {
                    state.set_value("demo-proj", window, cx);
                });
                wb.on_wizard_new_name_changed(window, cx);
                let wizard = wb.startup_wizard.as_ref().expect("wizard initialized");
                assert_eq!(
                    wizard.new_file_input.read(cx).value().as_ref(),
                    "demo-proj.mio"
                );

                wb.select_wizard_project_mode(ProjectMode::OpenExisting, window, cx);
                assert_eq!(
                    wb.startup_wizard.as_ref().map(|w| w.config_choice),
                    Some(ConfigChoice::Project)
                );
                let open_input = wb
                    .startup_wizard
                    .as_ref()
                    .expect("wizard initialized")
                    .open_file_input
                    .clone();
                open_input.update(cx, |state, cx| {
                    state.set_value("/tmp/existing-test.mio", window, cx);
                });

                wb.wizard_next(window, cx);
                assert_eq!(
                    wb.startup_wizard.as_ref().map(|w| w.step),
                    Some(WizardStep::Step2)
                );
                wb.wizard_back(cx);
                assert_eq!(
                    wb.startup_wizard.as_ref().map(|w| w.step),
                    Some(WizardStep::Step1)
                );
                wb.wizard_next(window, cx);
                wb.wizard_start(window, cx);
                assert_eq!(
                    wb.startup_wizard
                        .as_ref()
                        .map(|w| (w.step, w.awaiting_start)),
                    Some((WizardStep::Starting, true))
                );
            });
        })
        .unwrap();

    let cmd = drain(&mut fx.commands)
        .into_iter()
        .find(|cmd| matches!(cmd, Command::StartProject { .. }))
        .expect("StartProject command sent");
    match cmd {
        Command::StartProject {
            path,
            create_new,
            options,
        } => {
            #[cfg(not(windows))]
            assert_eq!(path, Some(PathBuf::from("/tmp/existing-test.mio")));
            #[cfg(windows)]
            assert!(
                path.as_deref().and_then(|p| p.to_str()).is_some_and(|s| s
                    == "/tmp/existing-test.mio"
                    || s.ends_with(":/tmp/existing-test.mio")),
                "unexpected Windows StartProject path: {path:?}"
            );
            assert!(!create_new);
            assert_eq!(options.config_mode, StartupDefaultConfig::Project);
        }
        _ => panic!("unexpected command variant"),
    }
}

#[gpui_kit::test]
async fn startup_wizard_cancel_confirm_no_and_yes(cx: &mut TestAppContext) {
    let fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
                wb.open_wizard_cancel_confirm(cx);
                assert!(wb.startup_wizard_picker_pending());

                wb.answer_wizard_cancel(false, window, cx);
                assert!(!wb.startup_wizard_picker_pending());
                assert!(!wb.shutdown_requested);
                assert!(!wb.shutdown.is_cancelled());

                wb.open_wizard_cancel_confirm(cx);
                wb.answer_wizard_cancel(true, window, cx);
                assert!(wb.shutdown_requested);
                assert!(wb.shutdown.is_cancelled());
            });
        })
        .unwrap();
}

#[gpui_kit::test]
async fn startup_wizard_real_clicks_distinguish_unremembered_mru_from_remembered_defaults(
    cx: &mut TestAppContext,
) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    drain(&mut fx.commands);

    // Simulate MRU advancing revision (>0) without the user ever remembering a default config.
    let unremembered = UserPreferences {
        revision: 3,
        recent_projects: vec![RecentProject {
            name: "alpha".into(),
            path: PathBuf::from("/tmp/alpha.mio"),
            last_accessed_unix_ms: 1_700_000_000_000,
        }],
        ..UserPreferences::default()
    };
    fx.events
        .try_send(Event::UserPreferences {
            operation_id: Uuid::new_v4(),
            state: UserPreferencesState {
                preferences: unremembered,
                load_error: None,
            },
        })
        .unwrap();
    cx.run_until_parked();

    click(window, "wizard-mode-open", cx);
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.mode, ProjectMode::OpenExisting);
        assert_eq!(wizard.config_choice, ConfigChoice::Project);
    });

    // User explicitly remembers "Use MIO defaults" via real UI clicks.
    click(window, ("wizard-recent-project-row", 0_usize), cx);
    click(window, "wizard-next", cx);
    click(window, "wizard-cfg-defaults", cx);
    click(window, "wizard-default-future", cx);
    click(window, "wizard-start", cx);

    let commands = drain(&mut fx.commands);
    let Some(Command::SetUserStartupPreferences {
        operation_id,
        expected_revision,
        default_config,
        disable_extensions,
    }) = commands
        .into_iter()
        .find(|cmd| matches!(cmd, Command::SetUserStartupPreferences { .. }))
    else {
        panic!("expected SetUserStartupPreferences command");
    };
    assert_eq!(expected_revision, 3);
    assert_eq!(default_config, StartupDefaultConfig::Defaults);
    assert!(!disable_extensions);

    let remembered_defaults = UserPreferences {
        revision: 4,
        startup: StartupPreferences {
            default_config: Some(StartupDefaultConfig::Defaults),
            disable_extensions: false,
        },
        ..UserPreferences::default()
    };
    assert!(
        !remembered_defaults.startup.is_default(),
        "explicitly remembered Defaults must not be treated as unremembered factory default"
    );

    // Fresh wizard instance synced with explicitly remembered Defaults must keep Defaults even in OpenExisting mode.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.user_preferences.confirmed = Some(remembered_defaults.clone());
                wb.init_startup_wizard(None, window, cx);
                wb.sync_startup_wizard_preferences(cx);
            });
            window.render_frame(cx);
        })
        .unwrap();

    click(window, "wizard-mode-open", cx);
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.mode, ProjectMode::OpenExisting);
        assert_eq!(
            wizard.config_choice,
            ConfigChoice::Defaults,
            "explicitly remembered Defaults must be respected when switching to OpenExisting"
        );
    });
    let _ = operation_id;
}
