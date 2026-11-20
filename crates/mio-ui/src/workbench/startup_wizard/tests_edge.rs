use mio_core::UserPreferences;
use std::ffi::OsString;

use gpui_kit::{TestAppContext, test::TestWindowExt};
use mio_core::StartupPreferences;

use super::tests_basic::click;
use super::*;
use crate::workbench::tests::{drain, fixture, tab};

#[gpui_kit::test]
async fn startup_wizard_full_queue_preserves_pending_preference_save_across_uncheck_retry(
    cx: &mut TestAppContext,
) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
                wb.sync_startup_wizard_preferences(cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    drain(&mut fx.commands);

    click(window, "wizard-next", cx);
    click(window, "wizard-default-future", cx);

    // Leave exactly 1 slot in the command channel so SetUserStartupPreferences succeeds and StartProject fails with Full.
    cx.update(|cx| {
        let wb = view.read(cx);
        for _ in 0..15 {
            wb.commands
                .try_send(Command::QueryUserPreferences {
                    operation_id: Uuid::new_v4(),
                })
                .unwrap();
        }
    });

    click(window, "wizard-start", cx);

    let mut drained = drain(&mut fx.commands);
    let pref_cmd = drained.pop().expect("last queued command");
    let Command::SetUserStartupPreferences {
        operation_id: first_op,
        expected_revision: 0,
        default_config: StartupDefaultConfig::Defaults,
        disable_extensions: false,
    } = pref_cmd
    else {
        panic!("expected SetUserStartupPreferences in last slot");
    };
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::Step2);
        assert!(!wizard.awaiting_start);
        assert_eq!(
            wizard.pending_preference_save.as_ref().map(|p| p.0),
            Some(first_op)
        );
        assert!(wizard.error.is_some());
    });

    // User unchecks "Default to the above in future", edits NewOnDisk draft, and clicks Start again before ACK arrives.
    click(window, "wizard-default-future", cx);
    click(window, "wizard-back", cx);
    click(window, "wizard-mode-new", cx);
    let name_input = cx.read(|cx| {
        view.read(cx)
            .startup_wizard
            .as_ref()
            .unwrap()
            .new_name_input
            .clone()
    });
    cx.update_window(window, |_, window, cx| {
        window.click(("input", name_input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("retained-draft", cx);
    })
    .unwrap();
    cx.run_until_parked();
    click(window, "wizard-next", cx);
    click(window, "wizard-start", cx);

    // Because pending_preference_save is still in flight, Start must be gated globally with a visible error and no command sent.
    assert!(
        drain(&mut fx.commands).is_empty(),
        "must not send StartProject while preference save is still unacknowledged"
    );
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::Step2);
        assert!(!wizard.awaiting_start);
        assert_eq!(
            wizard.pending_preference_save.as_ref().map(|p| p.0),
            Some(first_op)
        );
        assert!(
            wizard
                .error
                .as_deref()
                .is_some_and(|err| err.contains("保存中"))
        );
        assert_eq!(
            wizard.new_name_input.read(cx).value().as_ref(),
            "retained-draft"
        );
    });

    // Now deliver the matching ACK for first_op; it must be accepted, clear pending state, and NOT auto-start the project.
    let saved = UserPreferences {
        revision: 1,
        startup: StartupPreferences {
            default_config: Some(StartupDefaultConfig::Defaults),
            disable_extensions: false,
        },
        ..UserPreferences::default()
    };
    fx.events
        .try_send(Event::UserPreferencesSaved {
            operation_id: first_op,
            result: Ok(saved.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fx.commands).is_empty());
    cx.read(|cx| {
        let wb = view.read(cx);
        let wizard = wb.startup_wizard.as_ref().unwrap();
        assert!(wizard.pending_preference_save.is_none());
        assert_eq!(wb.user_preferences.confirmed.as_ref(), Some(&saved));
        assert_eq!(wizard.step, WizardStep::Step2);
        assert!(!wizard.awaiting_start);
        assert_eq!(
            wizard.new_name_input.read(cx).value().as_ref(),
            "retained-draft"
        );
    });
}

#[gpui_kit::test]
async fn startup_wizard_cli_existing_mio_preserves_project_config_and_recovers_on_error(
    cx: &mut TestAppContext,
) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();
    drain(&mut fx.commands);

    let cli_mio = PathBuf::from("/tmp/cli-existing-project.mio");
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(Some(cli_mio.clone()), window, cx);
                assert!(!wb.startup_wizard_active());
            });
        })
        .unwrap();

    // Initial temporary ProjectOpened triggers CLI project open.
    fx.events
        .try_send(Event::ProjectOpened {
            project: fx.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();

    let commands = drain(&mut fx.commands);
    let Some(Command::StartProject {
        path,
        create_new,
        options,
    }) = commands
        .into_iter()
        .find(|cmd| matches!(cmd, Command::StartProject { .. }))
    else {
        panic!("CLI .mio must enqueue StartProject");
    };
    assert_eq!(path, Some(cli_mio));
    assert!(!create_new);
    assert_eq!(
        options.config_mode,
        StartupDefaultConfig::Project,
        "CLI existing .mio must use Project config_mode so saved Repeater settings are not reset"
    );
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert!(
            wizard.awaiting_start,
            "CLI StartProject must mark awaiting_start so failure can recover into wizard UI"
        );
    });

    // If StartProject fails, wizard becomes active with visible error banner for recovery.
    fx.events
        .try_send(Event::Error {
            project: Some(fx.project),
            operation: "start_project",
            message: "项目文件损坏".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let wb = view.read(cx);
        assert!(wb.startup_wizard_active());
        let wizard = wb.startup_wizard.as_ref().unwrap();
        assert!(!wizard.awaiting_start);
        assert_eq!(wizard.step, WizardStep::Step1);
        assert_eq!(wizard.error.as_deref(), Some("项目文件损坏"));
    });
}

#[gpui_kit::test]
async fn startup_wizard_cli_and_open_existing_archive_send_start_project_directly(
    cx: &mut TestAppContext,
) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();
    drain(&mut fx.commands);

    let cli_archive = std::env::current_dir()
        .unwrap_or_default()
        .join(".local/cli-existing.archive");
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(Some(cli_archive.clone()), window, cx);
                assert!(!wb.startup_wizard_active());
            });
        })
        .unwrap();

    fx.events
        .try_send(Event::ProjectOpened {
            project: fx.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();

    let commands = drain(&mut fx.commands);
    assert!(
        !commands
            .iter()
            .any(|cmd| matches!(cmd, Command::InspectExternal { .. }))
    );
    let Some(Command::StartProject {
        path,
        create_new,
        options,
    }) = commands
        .into_iter()
        .find(|cmd| matches!(cmd, Command::StartProject { .. }))
    else {
        panic!("CLI .archive must enqueue StartProject directly");
    };
    assert_eq!(path, Some(cli_archive));
    assert!(!create_new);
    assert_eq!(options.config_mode, StartupDefaultConfig::Project);
    cx.read(|cx| {
        let wb = view.read(cx);
        assert!(!wb.ext_import.visible);
        assert!(wb.startup_wizard.as_ref().unwrap().awaiting_start);
    });

    let open_archive = std::env::current_dir()
        .unwrap_or_default()
        .join(".local/wizard-open.archive");
    let open_archive_text = open_archive.to_string_lossy().into_owned();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
                wb.select_wizard_project_mode(ProjectMode::OpenExisting, window, cx);
                let open_input = wb
                    .startup_wizard
                    .as_ref()
                    .unwrap()
                    .open_file_input
                    .clone();
                open_input.update(cx, |state, cx| {
                    state.set_value(open_archive_text, window, cx);
                });
                wb.wizard_next(window, cx);
                wb.wizard_start(window, cx);
            });
        })
        .unwrap();

    let commands = drain(&mut fx.commands);
    let Some(Command::StartProject {
        path,
        create_new,
        ..
    }) = commands
        .into_iter()
        .find(|cmd| matches!(cmd, Command::StartProject { .. }))
    else {
        panic!("wizard OpenExisting .archive must enqueue StartProject directly");
    };
    assert_eq!(path, Some(open_archive));
    assert!(!create_new);
}

#[gpui_kit::test]
async fn startup_wizard_rejects_non_utf8_picker_paths_and_blocks_ime_composition(
    cx: &mut TestAppContext,
) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();
    drain(&mut fx.commands);

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();

    click(window, "wizard-mode-open", cx);
    let open_input = cx.read(|cx| {
        view.read(cx)
            .startup_wizard
            .as_ref()
            .unwrap()
            .open_file_input
            .clone()
    });
    cx.update_window(window, |_, window, cx| {
        window.click(("input", open_input.entity_id()), cx);
        window.input("/tmp/keep-valid.mio", cx);
    })
    .unwrap();
    cx.run_until_parked();

    click(window, "wizard-choose-open-file", cx);
    assert!(cx.did_prompt_for_paths());
    #[cfg(unix)]
    let non_utf8 = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(b"/tmp/bad-\xff.mio".to_vec()))
    };
    #[cfg(windows)]
    let non_utf8 = {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xD800,
        ]))
    };
    cx.simulate_path_prompt_response(|_| Some(vec![non_utf8.clone()]));
    cx.run_until_parked();

    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            wizard.open_file_input.read(cx).value().as_ref(),
            "/tmp/keep-valid.mio",
            "non-UTF8 picker path must not overwrite input with lossy replacement characters"
        );
        assert!(
            wizard
                .error
                .as_deref()
                .is_some_and(|err| err.contains("UTF-8")),
            "expected explicit non-UTF8 path error, got {:?}",
            wizard.error
        );
    });

    // Active IME composition in open_file_input must block wizard-next without losing marked text.
    cx.update_window(window, |_, window, cx| {
        window.click(("input", open_input.entity_id()), cx);
        open_input.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "nihao", Some(5..5), window, cx);
        });
        assert!(open_input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
    })
    .unwrap();

    click(window, "wizard-next", cx);
    cx.update_window(window, |_, window, cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::Step1);
        assert_eq!(wizard.error.as_deref(), Some(editing::COMPOSITION_PENDING));
        assert!(open_input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
    })
    .unwrap();
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
async fn startup_wizard_runtime_exit_resets_starting_and_allows_exit(cx: &mut TestAppContext) {
    let fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();

    let (finished, joined) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();
    cx.update(|cx| {
        let joined = cx.background_spawn(async move { joined.await? });
        Workbench::observe_runtime(&view, window, joined, cx);
    });
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |wb, cx| wb.init_startup_wizard(None, window, cx));
        window.render_frame(cx);
    })
    .unwrap();

    click(window, "wizard-next", cx);
    click(window, "wizard-start", cx);
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            (wizard.step, wizard.awaiting_start),
            (WizardStep::Starting, true)
        );
    });

    finished
        .send(Err(anyhow::anyhow!("synthetic runtime failure")))
        .unwrap();
    crate::workbench::tests::pump_native(cx);

    cx.read(|cx| {
        let wb = view.read(cx);
        assert!(wb.runtime_result.is_some());
        let wizard = wb.startup_wizard.as_ref().unwrap();
        assert_eq!(
            (wizard.step, wizard.awaiting_start),
            (WizardStep::Step2, false)
        );
        assert!(wizard.error.is_some());
    });

    let probe = crate::native_terminate::tests::Probe::installed();
    click(window, "wizard-cancel", cx);
    crate::workbench::tests::pump_native(cx);
    assert!(cx.read(|cx| view.read(cx).startup_wizard_picker_pending()));
    click(window, "wizard-cancel-yes", cx);
    crate::workbench::tests::pump_native(cx);
    assert!(cx.read(|cx| view.read(cx).quitting));
    assert_eq!(probe.replies(), [true]);
}

#[gpui_kit::test]
async fn startup_wizard_error_retry_dismiss_and_cancel_during_starting(cx: &mut TestAppContext) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();
    drain(&mut fx.commands);

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();

    click(window, "wizard-mode-open", cx);
    let open_input = cx.read(|cx| {
        view.read(cx)
            .startup_wizard
            .as_ref()
            .unwrap()
            .open_file_input
            .clone()
    });
    window
        .update(cx, |_, window, cx| {
            open_input.update(cx, |state, cx| {
                state.set_value("/tmp/retry-project.mio", window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();

    click(window, "wizard-next", cx);
    click(window, "wizard-start", cx);
    drain(&mut fx.commands);
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            (wizard.step, wizard.awaiting_start),
            (WizardStep::Starting, true)
        );
    });

    // Cancel remains interactive while starting and returns cleanly when declined.
    click(window, "wizard-cancel", cx);
    cx.read(|cx| {
        assert!(view.read(cx).startup_wizard_picker_pending());
    });
    click(window, "wizard-cancel-no", cx);
    cx.read(|cx| {
        let wb = view.read(cx);
        assert!(!wb.startup_wizard_picker_pending());
        let wizard = wb.startup_wizard.as_ref().unwrap();
        assert_eq!(
            (wizard.step, wizard.awaiting_start),
            (WizardStep::Starting, true)
        );
    });

    // Start failure surfaces error banner with Retry action that re-enqueues StartProject.
    fx.events
        .try_send(Event::Error {
            project: Some(fx.project),
            operation: "start_project",
            message: "磁盘读取失败".into(),
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::Step1);
        assert!(wizard.last_start_failed);
        assert_eq!(wizard.error.as_deref(), Some("磁盘读取失败"));
    });

    click(window, "wizard-retry", cx);
    let retried = drain(&mut fx.commands)
        .into_iter()
        .any(|cmd| matches!(cmd, Command::StartProject { .. }));
    assert!(retried, "clicking Retry must re-enqueue StartProject");
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            (wizard.step, wizard.awaiting_start, wizard.last_start_failed),
            (WizardStep::Starting, true, false)
        );
        assert!(wizard.error.is_none());
    });

    // Dismissing an error banner clears both the error message and retry state.
    fx.events
        .try_send(Event::Error {
            project: Some(fx.project),
            operation: "start_project",
            message: "权限不足".into(),
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| window.render_frame(cx))
        .unwrap();
    click(window, "wizard-error-dismiss", cx);
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert!(wizard.error.is_none());
        assert!(!wizard.last_start_failed);
    });
}

#[gpui_kit::test]
async fn startup_wizard_choose_file_auto_selects_option_on_step1_and_step2(
    cx: &mut TestAppContext,
) {
    let mut fx = fixture(cx, vec![tab("1")], 16);
    let window = fx.window;
    let view = fx.view.clone();
    drain(&mut fx.commands);

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |wb, cx| {
                wb.init_startup_wizard(None, window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();

    // Clicking Choose file on NewOnDisk while Temporary is active auto-selects NewOnDisk.
    click(window, "wizard-choose-new-file", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.mode, ProjectMode::NewOnDisk);
    });
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();

    // Clicking Choose file on OpenExisting while NewOnDisk is active auto-selects OpenExisting.
    click(window, "wizard-choose-open-file", cx);
    assert!(cx.did_prompt_for_paths());
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.mode, ProjectMode::OpenExisting);
    });
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/auto-selected.mio")]));
    cx.run_until_parked();
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            wizard.open_file_input.read(cx).value().as_ref(),
            "/tmp/auto-selected.mio"
        );
    });

    // On Step 2, clicking Choose file on ConfigFile auto-selects ConfigFile.
    click(window, "wizard-next", cx);
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::Step2);
        assert_eq!(wizard.config_choice, ConfigChoice::Project);
    });
    click(window, "wizard-choose-config-file", cx);
    assert!(cx.did_prompt_for_paths());
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(wizard.config_choice, ConfigChoice::ConfigFile);
    });
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/auto-config.json")]));
    cx.run_until_parked();
    cx.read(|cx| {
        let wizard = view.read(cx).startup_wizard.as_ref().unwrap();
        assert_eq!(
            wizard.config_file_input.read(cx).value().as_ref(),
            "/tmp/auto-config.json"
        );
    });
}
