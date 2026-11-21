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

#[gpui_kit::test]
async fn startup_wizard_remove_selected_and_prune_missing_recent_projects(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let live_path = dir.path().join("live.mio");
    std::fs::write(&live_path, b"live").unwrap();
    let missing_path = dir.path().join("missing.mio");

    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
                let prefs = UserPreferences {
                    revision: 2,
                    recent_projects: vec![
                        mio_core::RecentProject {
                            name: "live".into(),
                            path: live_path.clone(),
                            last_accessed_unix_ms: 2_000,
                        },
                        mio_core::RecentProject {
                            name: "missing".into(),
                            path: missing_path.clone(),
                            last_accessed_unix_ms: 1_000,
                        },
                    ],
                    ..UserPreferences::default()
                };
                wb.user_preferences.confirmed = Some(prefs);
                wb.sync_startup_wizard_preferences(cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    drain(&mut fx.commands);

    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        let delegate = wizard.recent_table.read(cx).delegate();
        assert_eq!(delegate.missing_count(), 1);
        assert!(delegate.is_missing(&missing_path));
        assert!(!delegate.is_missing(&live_path));
    });

    // Select row 0 (live) and click "移除选中".
    click(window, ("wizard-recent-project-row", 0_usize), cx);
    click(window, "wizard-remove-recent", cx);

    let remove_cmd = drain(&mut fx.commands)
        .into_iter()
        .find(|c| matches!(c, Command::RemoveRecentProject { .. }))
        .expect("expected RemoveRecentProject command");
    let Command::RemoveRecentProject {
        operation_id: rem_op,
        expected_revision: 2,
        path: rem_path,
    } = remove_cmd
    else {
        unreachable!()
    };
    assert_eq!(rem_path, live_path);

    fx.events
        .try_send(Event::UserPreferencesSaved {
            operation_id: rem_op,
            result: Ok(UserPreferences {
                revision: 3,
                recent_projects: vec![mio_core::RecentProject {
                    name: "missing".into(),
                    path: missing_path.clone(),
                    last_accessed_unix_ms: 1_000,
                }],
                ..UserPreferences::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();

    // Now select the missing row into open_file_input and click "清理失效项目".
    click(window, ("wizard-recent-project-row", 0_usize), cx);
    click(window, "wizard-prune-recent", cx);

    let prune_cmd = drain(&mut fx.commands)
        .into_iter()
        .find(|c| matches!(c, Command::PruneMissingRecentProjects { .. }))
        .expect("expected PruneMissingRecentProjects command");
    let Command::PruneMissingRecentProjects {
        operation_id: prune_op,
        expected_revision: 3,
    } = prune_cmd
    else {
        unreachable!()
    };

    fx.events
        .try_send(Event::UserPreferencesSaved {
            operation_id: prune_op,
            result: Ok(UserPreferences {
                revision: 4,
                recent_projects: Vec::new(),
                ..UserPreferences::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let wb = view.read(cx);
        let wizard = wb.startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.recent_table.read(cx).delegate().rows.len(), 0);
        assert_eq!(wizard.recent_table.read(cx).delegate().missing_count(), 0);
        assert_eq!(wizard.open_file_input.read(cx).value().as_ref(), "");
        assert_eq!(wb.user_preferences.confirmed.as_ref().unwrap().revision, 4);
    });
}

#[gpui_kit::test]
fn startup_wizard_failed_remove_preserves_input_and_sync_preserves_selection_by_path(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path_a = dir.path().join("a.mio");
    let path_b = dir.path().join("b.mio");
    std::fs::write(&path_a, b"a").unwrap();
    std::fs::write(&path_b, b"b").unwrap();

    let mut fx = fixture(cx, vec![tab("1")], 16);
    let (window, view) = (fx.window, fx.view.clone());

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
                wb.user_preferences.confirmed = Some(UserPreferences {
                    revision: 5,
                    recent_projects: vec![
                        mio_core::RecentProject {
                            name: "A".into(),
                            path: path_a.clone(),
                            last_accessed_unix_ms: 2_000,
                        },
                        mio_core::RecentProject {
                            name: "B".into(),
                            path: path_b.clone(),
                            last_accessed_unix_ms: 1_000,
                        },
                    ],
                    ..UserPreferences::default()
                });
                wb.sync_startup_wizard_preferences(cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    drain(&mut fx.commands);

    // Select row 0 ("A"), verify open_file_input has path_a, then trigger remove that FAILS.
    click(window, ("wizard-recent-project-row", 0_usize), cx);
    click(window, "wizard-remove-recent", cx);
    let rem_op = drain(&mut fx.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::RemoveRecentProject { operation_id, .. } => Some(operation_id),
            _ => None,
        })
        .expect("expected RemoveRecentProject");

    // While waiting or upon save failure, open_file_input MUST NOT be cleared.
    cx.read(|cx| {
        let wiz = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            wiz.open_file_input.read(cx).value().as_ref(),
            path_a.to_string_lossy().as_ref()
        );
    });

    fx.events
        .try_send(Event::UserPreferencesSaved {
            operation_id: rem_op,
            result: Err("写入失败".into()),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let wiz = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            wiz.open_file_input.read(cx).value().as_ref(),
            path_a.to_string_lossy().as_ref(),
            "open_file_input must stay preserved when RemoveRecentProject fails"
        );
    });

    // Now select row 1 ("B"), type a custom path into open_file_input, and remove "A" ->
    // "B" shifts to row 0 silently WITHOUT overwriting open_file_input.
    let custom_path = dir.path().join("custom.mio").to_string_lossy().into_owned();
    click(window, ("wizard-recent-project-row", 1_usize), cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                let wiz = wb.startup_wizard.as_mut().unwrap();
                wiz.open_file_input
                    .update(cx, |s, cx| s.set_value(custom_path.clone(), window, cx));
                wb.remove_wizard_recent_path(path_a.clone(), window, cx);
            });
        })
        .unwrap();
    let rem_a_op = drain(&mut fx.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::RemoveRecentProject { operation_id, .. } => Some(operation_id),
            _ => None,
        })
        .unwrap();
    fx.events
        .try_send(Event::UserPreferencesSaved {
            operation_id: rem_a_op,
            result: Ok(UserPreferences {
                revision: 6,
                recent_projects: vec![
                    mio_core::RecentProject {
                        name: "A".into(),
                        path: path_a.clone(),
                        last_accessed_unix_ms: 2_000,
                    },
                    mio_core::RecentProject {
                        name: "B".into(),
                        path: path_b.clone(),
                        last_accessed_unix_ms: 1_000,
                    },
                ],
                ..UserPreferences::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();

    // Verify sorting by Name descending keeps "B" selected at row 0 without overwriting open_file_input.
    window
        .update(cx, |_, window, cx| {
            let table = view
                .read(cx)
                .startup_wizard
                .as_ref()
                .unwrap()
                .recent_table
                .clone();
            table.update(cx, |t, cx| {
                t.sort_column(
                    0,
                    gpui_kit::component::table::ColumnSort::Descending,
                    window,
                    cx,
                );
            });
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let wiz = view.read(cx).startup_wizard.as_ref().unwrap();
        let table = wiz.recent_table.read(cx);
        let sel = table.selected_row().expect("B should stay selected across sort");
        assert_eq!(table.delegate().rows[sel].path, path_b);
        assert_eq!(wiz.open_file_input.read(cx).value().as_ref(), custom_path);
    });

    // Finally remove "A" for real -> "B" moves to row 0 and custom open_file_input remains intact.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.remove_wizard_recent_path(path_a.clone(), window, cx)
            });
        })
        .unwrap();
    let rem_a2_op = drain(&mut fx.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::RemoveRecentProject { operation_id, .. } => Some(operation_id),
            _ => None,
        })
        .unwrap();
    fx.events
        .try_send(Event::UserPreferencesSaved {
            operation_id: rem_a2_op,
            result: Ok(UserPreferences {
                revision: 7,
                recent_projects: vec![mio_core::RecentProject {
                    name: "B".into(),
                    path: path_b.clone(),
                    last_accessed_unix_ms: 1_000,
                }],
                ..UserPreferences::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let wiz = view.read(cx).startup_wizard.as_ref().unwrap();
        let table = wiz.recent_table.read(cx);
        let sel = table.selected_row().expect("B should remain selected");
        assert_eq!(table.delegate().rows[sel].path, path_b);
        assert_eq!(wiz.open_file_input.read(cx).value().as_ref(), custom_path);
    });
}
