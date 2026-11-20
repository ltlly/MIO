use std::fs;

use super::{tests::location, *};

#[test]
fn user_settings_defaults_cas_and_cross_restart_persistence_preserve_legacy_fields() {
    let (_directory, path) = location();
    let mut first = PreferencesStore::new(Some(path.clone()));
    let mut second = PreferencesStore::new(Some(path.clone()));

    let initial = first.query().preferences;
    assert_eq!(
        initial.display,
        UserDisplaySettings {
            theme: UserThemeMode::Light,
            ui_font_size_px: 13,
            scale_factor_percent: 100,
            max_table_cell_preview_chars: 256,
            table_zebra_stripe: true,
            issues_grouped_evidence: true,
            use_system_dpi: false,
        }
    );
    assert_eq!(
        initial.message_editor,
        UserMessageEditorSettings {
            font_size_px: 13,
            soft_wrap: true,
            pretty_print_by_default: true,
            auto_expand_inspector: false,
            highlight_search_case_sensitive_default: false,
            hidden_request_headers: None,
            hidden_response_headers: None,
            hide_request_headers_by_default: false,
            hide_response_headers_by_default: false,
            charset_mode: 0,
            specific_charset: "UTF-8".into(),
            html_render_external_requests: true,
            search_regex_default: false,
            search_autoscroll_default: false,
            url_encode_as_you_type: false,
        }
    );
    assert_eq!(
        initial.misc,
        UserMiscSettings {
            confirm_on_close_tab_with_unsaved_draft: true,
            redact_sensitive_headers_in_copy_curl: false,
            hotkey_send_request: "cmd-enter".into(),
            custom_hotkeys: std::collections::BTreeMap::new(),
            intruder_side_panel_left: false,
            intruder_side_panel_auto_expand: true,
            intruder_auto_payload_append: false,
            intruder_new_tab_config_mode: 0,
            intruder_onboarding_dismissed: false,
            collaborator_onboarding_dismissed: false,
            auto_check_updates: true,
        }
    );

    first
        .set(
            0,
            RunningAttackClose::Continue,
            FinishedAttackClose::KeepMemory,
        )
        .unwrap();

    let display = UserDisplaySettings {
        theme: UserThemeMode::Dark,
        ui_font_size_px: 15,
        scale_factor_percent: 125,
        max_table_cell_preview_chars: 512,
        table_zebra_stripe: false,
        issues_grouped_evidence: false,
        use_system_dpi: true,
    };
    let editor = UserMessageEditorSettings {
        font_size_px: 16,
        soft_wrap: false,
        pretty_print_by_default: false,
        auto_expand_inspector: true,
        highlight_search_case_sensitive_default: true,
        hidden_request_headers: Some(vec!["Accept".into(), "X-Request-Token".into()]),
        hidden_response_headers: Some(vec!["Server".into(), "X-Response-Token".into()]),
        hide_request_headers_by_default: true,
        hide_response_headers_by_default: true,
        charset_mode: 3,
        specific_charset: "ISO-8859-1".into(),
        html_render_external_requests: false,
        search_regex_default: true,
        search_autoscroll_default: true,
        url_encode_as_you_type: true,
    };
    let mut custom_hotkeys = std::collections::BTreeMap::new();
    custom_hotkeys.insert("send_to_repeater".into(), "cmd-r".into());
    let misc = UserMiscSettings {
        confirm_on_close_tab_with_unsaved_draft: false,
        redact_sensitive_headers_in_copy_curl: true,
        hotkey_send_request: "cmd-enter".into(),
        custom_hotkeys,
        intruder_side_panel_left: true,
        intruder_side_panel_auto_expand: false,
        intruder_auto_payload_append: true,
        intruder_new_tab_config_mode: 2,
        intruder_onboarding_dismissed: true,
        collaborator_onboarding_dismissed: true,
        auto_check_updates: false,
    };

    assert!(
        second
            .set_workbench_settings(0, display.clone(), editor.clone(), misc.clone())
            .is_err(),
        "stale CAS revision must reject workbench user settings write"
    );

    let saved = first
        .set_workbench_settings(1, display.clone(), editor.clone(), misc.clone())
        .unwrap();
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.running, RunningAttackClose::Continue);
    assert_eq!(saved.finished, FinishedAttackClose::KeepMemory);
    assert_eq!(saved.display, display);
    assert_eq!(saved.message_editor, editor);
    assert_eq!(saved.misc, misc);

    let mut reopened = PreferencesStore::new(Some(path.clone()));
    let reloaded = reopened.query();
    assert!(reloaded.load_error.is_none());
    assert_eq!(reloaded.preferences, saved);

    let reset = reopened
        .set_workbench_settings(
            2,
            UserDisplaySettings::default(),
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        )
        .unwrap();
    assert_eq!(reset.revision, 3);
    let raw_json = fs::read_to_string(&path).unwrap();
    assert!(
        !raw_json.contains("\"display\"")
            && !raw_json.contains("\"message_editor\"")
            && !raw_json.contains("\"misc\""),
        "default user settings must omit optional keys for backward-clean JSON: {raw_json}"
    );
}

#[test]
fn out_of_range_user_settings_are_rejected_before_and_during_disk_load() {
    let (_directory, path) = location();
    let mut store = PreferencesStore::new(Some(path.clone()));
    let confirmed = store
        .set_workbench_settings(
            0,
            UserDisplaySettings {
                theme: UserThemeMode::Dark,
                ..UserDisplaySettings::default()
            },
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        )
        .unwrap();
    let before = fs::read(&path).unwrap();

    for (display, editor, misc) in [
        (
            UserDisplaySettings {
                ui_font_size_px: 10,
                ..UserDisplaySettings::default()
            },
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings {
                ui_font_size_px: 19,
                ..UserDisplaySettings::default()
            },
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings {
                scale_factor_percent: 79,
                ..UserDisplaySettings::default()
            },
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings {
                scale_factor_percent: 151,
                ..UserDisplaySettings::default()
            },
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings {
                max_table_cell_preview_chars: 63,
                ..UserDisplaySettings::default()
            },
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings {
                max_table_cell_preview_chars: 1025,
                ..UserDisplaySettings::default()
            },
            UserMessageEditorSettings::default(),
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings {
                font_size_px: 9,
                ..UserMessageEditorSettings::default()
            },
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings {
                font_size_px: 23,
                ..UserMessageEditorSettings::default()
            },
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings {
                hidden_request_headers: Some(vec!["".into()]),
                ..UserMessageEditorSettings::default()
            },
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings {
                hidden_response_headers: Some(vec!["Invalid Header".into()]),
                ..UserMessageEditorSettings::default()
            },
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings::default(),
            UserMiscSettings {
                hotkey_send_request: "".into(),
                ..UserMiscSettings::default()
            },
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings {
                charset_mode: 4,
                ..UserMessageEditorSettings::default()
            },
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings {
                specific_charset: "  ".into(),
                ..UserMessageEditorSettings::default()
            },
            UserMiscSettings::default(),
        ),
        (
            UserDisplaySettings::default(),
            UserMessageEditorSettings::default(),
            UserMiscSettings {
                intruder_new_tab_config_mode: 3,
                ..UserMiscSettings::default()
            },
        ),
    ] {
        assert!(
            store
                .set_workbench_settings(1, display, editor, misc)
                .is_err()
        );
        assert_eq!(store.query().preferences, confirmed);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    let corrupted =
        br#"{"version":1,"revision":1,"running":"Ask","finished":"Ask","display":{"ui_font_size_px":99}}"#;
    fs::write(&path, corrupted).unwrap();
    let state = store.query();
    assert!(state.load_error.is_some());
    assert_eq!(state.preferences, UserPreferences::default());
    assert_eq!(fs::read(&path).unwrap(), corrupted);
}

#[test]
fn legacy_user_workbench_json_defaults_new_fields_safely() {
    let (_directory, path) = location();
    let legacy = br#"{"version":1,"revision":2,"running":"Ask","finished":"Ask","display":{"theme":"Dark"},"message_editor":{"soft_wrap":false},"misc":{"redact_sensitive_headers_in_copy_curl":true}}"#;
    fs::write(&path, legacy).unwrap();
    let mut store = PreferencesStore::new(Some(path));
    let state = store.query();
    assert!(state.load_error.is_none());
    assert_eq!(state.preferences.display.theme, UserThemeMode::Dark);
    assert!(state.preferences.display.table_zebra_stripe);
    assert!(state.preferences.display.issues_grouped_evidence);
    assert!(!state.preferences.display.use_system_dpi);
    assert!(!state.preferences.message_editor.soft_wrap);
    assert_eq!(state.preferences.message_editor.charset_mode, 0);
    assert_eq!(state.preferences.message_editor.specific_charset, "UTF-8");
    assert!(state.preferences.message_editor.html_render_external_requests);
    assert!(!state.preferences.message_editor.search_regex_default);
    assert!(!state.preferences.message_editor.search_autoscroll_default);
    assert!(!state.preferences.message_editor.url_encode_as_you_type);
    assert!(state.preferences.misc.redact_sensitive_headers_in_copy_curl);
    assert!(state.preferences.misc.custom_hotkeys.is_empty());
    assert!(!state.preferences.misc.intruder_side_panel_left);
    assert!(state.preferences.misc.intruder_side_panel_auto_expand);
    assert!(!state.preferences.misc.intruder_auto_payload_append);
    assert_eq!(state.preferences.misc.intruder_new_tab_config_mode, 0);
    assert!(!state.preferences.misc.intruder_onboarding_dismissed);
    assert!(!state.preferences.misc.collaborator_onboarding_dismissed);
}
