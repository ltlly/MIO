use gpui_kit::TestAppContext;
use gpui_kit::component::ThemeMode;
use gpui_kit::test::TestWindowExt;
use mio_core::{
    Command, Event, UserDisplaySettings, UserMessageEditorSettings, UserMiscSettings,
    UserPreferences, UserThemeMode, Uuid,
};

use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, settings::open_settings, tab};

fn take_workbench_prefs_save(fixture: &mut Fixture) -> (Uuid, UserPreferences) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetUserWorkbenchPreferences {
            operation_id,
            expected_revision,
            display,
            message_editor,
            misc,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "expected exactly one SetUserWorkbenchPreferences command, got {} commands",
            commands.len()
        );
    };
    (
        *operation_id,
        UserPreferences {
            version: 1,
            revision: expected_revision + 1,
            display: display.clone(),
            message_editor: message_editor.clone(),
            misc: misc.clone(),
            ..UserPreferences::default()
        },
    )
}

fn acknowledge_user_prefs(
    fixture: &Fixture,
    operation_id: Uuid,
    saved: UserPreferences,
    cx: &mut TestAppContext,
) {
    fixture
        .events
        .try_send(Event::UserPreferencesSaved {
            operation_id,
            result: Ok(saved),
        })
        .unwrap();
    cx.run_until_parked();
}

fn scroll_editor_top(settings: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.user_pages
                .editor_scroll
                .set_offset(gpui_kit::Point::default());
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
}

fn scroll_editor_bottom(settings: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.user_pages.editor_scroll.scroll_to_bottom();
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn user_settings_display_editor_misc_navigation_and_dark_theme_realtime_sync(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("User 设置实时联动测试")], 64);
    drain(&mut fixture.commands);
    assert!(!theme::is_dark());
    assert_eq!(theme::mode(), ThemeMode::Light);
    assert_eq!(theme::ui_font_size_px(), 13);
    assert_eq!(theme::editor_font_size_px(), 13);
    assert!(theme::editor_soft_wrap());
    assert!(theme::pretty_print_by_default());

    let settings = open_settings(&fixture, cx);

    // 1. Switch to User > Display and verify initial Light / 13pt / 1.0x / Zebra / Grouped state.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-display", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-user-display").selected(), Some(true));
        assert!(window.find("settings-user-display-heading").visible());
        assert!(window.find("settings-user-display-scope").visible());
        assert!(window.find("settings-user-scaling-heading").visible());
        assert!(window.find("settings-user-table-appearance-heading").visible());
        assert!(window.find("settings-user-issues-heading").visible());
        assert!(window.try_find("settings-user-status").is_none());
        assert_eq!(
            window.find("settings-user-theme-light").checked(),
            Some(true)
        );
        assert_eq!(
            window.find("settings-user-theme-dark").checked(),
            Some(false)
        );
        assert_eq!(
            window.find("settings-user-scale-configure").checked(),
            Some(false)
        );
        assert_eq!(
            window.find("settings-user-table-zebra").checked(),
            Some(true)
        );
        assert_eq!(
            window.find("settings-user-issues-grouped").checked(),
            Some(true)
        );
        let entity = settings_entity(window, cx).unwrap();
        assert_eq!(
            entity
                .read(cx)
                .user_pages
                .ui_font_size_select
                .read(cx)
                .selected_value()
                .copied(),
            Some("13")
        );
        assert_eq!(
            entity
                .read(cx)
                .user_pages
                .scale_factor_select
                .read(cx)
                .selected_value()
                .copied(),
            Some("1.0")
        );
    })
    .unwrap();
    cx.run_until_parked();

    // 2. Select Dark theme radio -> emits SetUserWorkbenchPreferences; theme updates only after ACK.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-theme-dark", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        !theme::is_dark(),
        "theme atomics must keep confirmed Light palette until backend ACK arrives"
    );
    let (op1, saved1) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved1.display.theme, UserThemeMode::Dark);
    acknowledge_user_prefs(&fixture, op1, saved1, cx);
    assert!(theme::is_dark());
    assert_eq!(theme::mode(), ThemeMode::Dark);

    // 3. Select UI font size 14pt via dropdown and verify ACK updates theme::ui_font_size_px().
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.set_user_display_font_size(14, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op2, saved2) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved2.display.ui_font_size_px, 14);
    acknowledge_user_prefs(&fixture, op2, saved2, cx);
    assert_eq!(theme::ui_font_size_px(), 14);

    // 4. Switch to User > Message editor, verify External top sections, then update font size to 16pt and soft_wrap.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-message-editor", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-user-message-editor").selected(),
            Some(true)
        );
        assert!(window.find("settings-user-editor-heading").visible());
        assert!(window.find("settings-user-editor-scope").visible());
        assert!(window.find("settings-user-editor-html-heading").visible());
        assert!(window.find("settings-user-editor-search-heading").visible());
    })
    .unwrap();
    scroll_editor_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-editor-font-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("16", cx);
        window.click("settings-user-editor-font-apply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op3, saved3) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved3.message_editor.font_size_px, 16);
    acknowledge_user_prefs(&fixture, op3, saved3, cx);
    assert_eq!(theme::editor_font_size_px(), 16);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-editor-soft-wrap", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op4, saved4) = take_workbench_prefs_save(&mut fixture);
    assert!(!saved4.message_editor.soft_wrap);
    acknowledge_user_prefs(&fixture, op4, saved4, cx);
    assert!(!theme::editor_soft_wrap());

    // 5. Switch to User > Hotkeys (SettingsPage::UserMisc), verify External Hotkeys layout + toggle_user_misc API.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-misc", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-user-misc").selected(), Some(true));
        assert!(window.find("settings-user-misc-heading").visible());
        assert!(window.find("settings-user-misc-scope").visible());
        assert!(window.find("settings-user-hotkey-search").visible());
        assert!(window.find("settings-user-hotkey-table").visible());
        assert!(window.try_find("settings-user-misc-redact-curl").is_none());
        assert!(window.try_find("settings-user-status").is_none());
        let first_row = window.find(("settings-user-hotkey-row", 0usize));
        assert!(first_row.visible());
        assert!(first_row.label().unwrap().contains("⌘Enter"));
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.toggle_user_misc(
                |m| m.redact_sensitive_headers_in_copy_curl = true,
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op5, saved5) = take_workbench_prefs_save(&mut fixture);
    assert!(saved5.misc.redact_sensitive_headers_in_copy_curl);
    acknowledge_user_prefs(&fixture, op5, saved5, cx);
    assert!(theme::redact_sensitive_headers_in_copy_curl());

    // 5b. Switch send hotkey to ctrl-enter and verify table row 0 + runtime getter update + search filter.
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.toggle_user_misc(|m| m.hotkey_send_request = "ctrl-enter".into(), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op5b, saved5b) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved5b.misc.hotkey_send_request, "ctrl-enter");
    acknowledge_user_prefs(&fixture, op5b, saved5b, cx);
    assert_eq!(theme::hotkey_send_request(), "ctrl-enter");
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let first_row = window.find(("settings-user-hotkey-row", 0usize));
        assert!(first_row.label().unwrap().contains("⌃Enter"));
        window.click("settings-user-hotkey-search", cx);
        window.input("Repeater", cx);
        window.render_frame(cx);
        assert!(window.try_find(("settings-user-hotkey-row", 0usize)).is_none());
        assert!(window.find(("settings-user-hotkey-row", 2usize)).visible());
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.reset_user_misc_defaults(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op5c, saved5c) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved5c.misc, UserMiscSettings::default());
    acknowledge_user_prefs(&fixture, op5c, saved5c, cx);
    assert_eq!(theme::hotkey_send_request(), "cmd-enter");
    assert!(!theme::redact_sensitive_headers_in_copy_curl());
    assert!(theme::confirm_on_close_tab_with_unsaved_draft());

    // Restore Light defaults so global atomics remain clean for other tests.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-display", cx);
        window.render_frame(cx);
        window.click("settings-user-display-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op6, saved6) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved6.display, UserDisplaySettings::default());
    acknowledge_user_prefs(&fixture, op6, saved6, cx);

    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-message-editor", cx);
        window.render_frame(cx);
    })
    .unwrap();
    scroll_editor_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-editor-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op7, saved7) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved7.message_editor, UserMessageEditorSettings::default());
    acknowledge_user_prefs(&fixture, op7, saved7, cx);
    assert!(!theme::is_dark());
    assert_eq!(theme::ui_font_size_px(), 13);
    assert_eq!(theme::editor_font_size_px(), 13);
    assert!(theme::editor_soft_wrap());
    assert!(theme::pretty_print_by_default());
}

#[gpui_kit::test]
fn user_settings_reject_out_of_range_font_size_and_guard_active_ime_composition(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("User 设置越界与 IME 守卫")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    // 1. Out-of-range editor font size (99) is rejected with an explicit Alert while preserving input.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-message-editor", cx);
        window.render_frame(cx);
    })
    .unwrap();
    scroll_editor_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-editor-font-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("99", cx);
        window.click("settings-user-editor-font-apply", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-user-editor-font-input").value(),
            Some("99"),
            "rejected input must stay intact for user correction"
        );
    })
    .unwrap();
    scroll_editor_top(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let alert = window.find("settings-user-error");
        assert!(alert.visible());
        assert!(alert.label().unwrap().contains("10 到 22 pt"));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "out-of-range font size must not enqueue any preference save command"
    );

    // 2. Active IME composition in editor_font_size_input blocks apply and checkbox toggles.
    scroll_editor_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let entity = settings_entity(window, cx).unwrap();
        let input = entity.read(cx).user_pages.editor_font_size_input.clone();
        input.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "15", Some(2..2), window, cx)
        });
        window.render_frame(cx);
        window.click("settings-user-editor-font-apply", cx);
        window.render_frame(cx);
        assert!(
            input.update(cx, |state, cx| state
                .marked_text_range(window, cx)
                .is_some()),
            "IME marked text must remain untouched"
        );
    })
    .unwrap();
    scroll_editor_top(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let alert = window.find("settings-user-error");
        assert!(alert.visible());
        assert!(alert.label().unwrap().contains("输入法仍在组词"));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "active IME composition must block SetUserWorkbenchPreferences"
    );
}

#[gpui_kit::test]
fn user_settings_pretty_default_auto_inspector_search_case_and_preview_truncation_sync(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("User 编辑器与预览实时联动")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    // 1. Update table cell preview limit to 64 and verify truncate_cell_preview.
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| {
                let mut display = view.user_preferences.display.clone();
                display.max_table_cell_preview_chars = 64;
                let editor = view.user_preferences.message_editor.clone();
                let misc = view.user_preferences.misc.clone();
                view.set_user_workbench_preferences(display, editor, misc, cx)
            })
            .unwrap();
    });
    cx.run_until_parked();
    let (op1, saved1) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved1.display.max_table_cell_preview_chars, 64);
    acknowledge_user_prefs(&fixture, op1, saved1, cx);
    assert_eq!(theme::table_cell_preview_chars(), 64);
    let long_cell = "a".repeat(80);
    let truncated = theme::truncate_cell_preview(&long_cell);
    assert_eq!(truncated.chars().count(), 65);
    assert!(truncated.ends_with('…'));

    // 2. Toggle pretty_print_by_default off and back on, and enable auto_expand_inspector and highlight_search_case_sensitive_default.
    assert!(theme::pretty_print_by_default());
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-message-editor", cx);
        window.render_frame(cx);
    })
    .unwrap();
    scroll_editor_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-editor-pretty-default", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op_pretty_off, saved_pretty_off) = take_workbench_prefs_save(&mut fixture);
    assert!(!saved_pretty_off.message_editor.pretty_print_by_default);
    acknowledge_user_prefs(&fixture, op_pretty_off, saved_pretty_off, cx);
    assert!(!theme::pretty_print_by_default());

    for (scroll_top, id) in [
        (false, "settings-user-editor-pretty-default"),
        (false, "settings-user-editor-auto-inspector"),
        (true, "settings-user-editor-search-case"),
    ] {
        if scroll_top {
            scroll_editor_top(settings, cx);
        } else {
            scroll_editor_bottom(settings, cx);
        }
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
        let (op, saved) = take_workbench_prefs_save(&mut fixture);
        acknowledge_user_prefs(&fixture, op, saved, cx);
    }
    assert!(theme::pretty_print_by_default());
    assert!(theme::auto_expand_inspector());
    assert!(theme::highlight_search_case_sensitive_default());

    // Verify existing workbench inspector auto-expanded, existing editor updated case_sensitive,
    // and loading JSON message auto-enables Pretty view while preserving raw bytes.
    cx.update_window(fixture.window, |_, window, cx| {
        let raw_json =
            b"POST /api HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Type: application/json\r\n\r\n{\"k\":1}";
        fixture.view.update(cx, |view, cx| {
            assert!(view.inspector.open);
            let tab_id = view.workspace.active.unwrap();
            let editors = view.tab_editors.get_mut(&tab_id).unwrap();
            assert!(editors.request.case_sensitive);
            editors.request.load(raw_json, window, cx);
            assert!(editors.request.pretty_active);
            assert_eq!(editors.request.bytes(window, cx).unwrap(), raw_json);
        });
    })
    .unwrap();

    // Reset display & editor defaults for clean thread_local state.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-display", cx);
        window.render_frame(cx);
        window.click("settings-user-display-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op, saved, cx);

    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-message-editor", cx);
        window.render_frame(cx);
    })
    .unwrap();
    scroll_editor_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-editor-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert_eq!(theme::table_cell_preview_chars(), 256);
    assert!(theme::pretty_print_by_default());
    assert!(!theme::auto_expand_inspector());
    assert!(!theme::highlight_search_case_sensitive_default());
}

#[gpui_kit::test]
fn user_settings_downscale_80_percent_shrinks_metrics_and_restores_100_percent(
    cx: &mut TestAppContext,
) {
    use gpui_kit::component::table::TableDelegate as _;
    let mut fixture = fixture(cx, vec![tab("User 80% 缩小比例")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    assert_eq!(theme::row_height(), px(theme::ROW_HEIGHT));
    assert_eq!(theme::toolbar_height(), px(theme::TOOLBAR_HEIGHT));
    assert_eq!(theme::tool_tab_height(), px(42.));
    assert_eq!(theme::work_tab_bar_height(), px(47.));
    let base_hist_w = cx.read(|cx| {
        let delegate = fixture.view.read(cx).history.read(cx).delegate();
        assert_eq!(delegate.columns_count(cx), 18);
        delegate.column(0, cx).width
    });
    let base_ws_w = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .websocket
            .table
            .read(cx)
            .delegate()
            .column(0, cx)
            .width
    });

    // Upscale to 150% first and render WebSocket table to ensure in-place columns upscale cleanly.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-display", cx);
        window.render_frame(cx);
        window.click("settings-user-scale-configure", cx);
        window.render_frame(cx);
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.set_user_display_scale_percent(150, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op_up, saved_up) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op_up, saved_up, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.tool = crate::workbench::Tool::Proxy;
            view.proxy_page = crate::workbench::ProxyPage::WebSocketHistory;
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
    drain(&mut fixture.commands);
    let up_ws_w = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .websocket
            .table
            .read(cx)
            .delegate()
            .column(0, cx)
            .width
    });
    assert!(up_ws_w > base_ws_w);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.set_user_display_scale_percent(80, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved.display.scale_factor_percent, 80);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    drain(&mut fixture.commands);

    assert!((theme::ui_scale() - 0.8).abs() < 0.01);
    assert!(theme::row_height() < px(theme::ROW_HEIGHT));
    assert!(theme::toolbar_height() < px(theme::TOOLBAR_HEIGHT));
    assert!(theme::tool_tab_height() < px(42.));
    assert!(theme::work_tab_bar_height() < px(47.));
    let down_hist_w = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .history
            .read(cx)
            .delegate()
            .column(0, cx)
            .width
    });
    let down_ws_w = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .websocket
            .table
            .read(cx)
            .delegate()
            .column(0, cx)
            .width
    });
    assert!(down_hist_w < base_hist_w);
    assert!(down_ws_w < base_ws_w);

    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-display", cx);
        window.render_frame(cx);
        window.click("settings-user-display-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op_reset, saved_reset) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op_reset, saved_reset, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(theme::ui_scale(), 1.0);
    assert_eq!(theme::row_height(), px(theme::ROW_HEIGHT));
    assert_eq!(theme::toolbar_height(), px(theme::TOOLBAR_HEIGHT));
    assert_eq!(theme::tool_tab_height(), px(42.));
    assert_eq!(theme::work_tab_bar_height(), px(47.));
    let restored_ws_w = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .websocket
            .table
            .read(cx)
            .delegate()
            .column(0, cx)
            .width
    });
    assert_eq!(restored_ws_w, base_ws_w);
}

#[gpui_kit::test]
fn user_settings_hide_specified_headers_layout_validation_and_editor_sync(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestQueryExt;
    use gpui_kit::{px, size};
    let mut fixture = fixture(cx, vec![tab("User 隐藏指定标头设置")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-message-editor", cx);
        window.render_frame(cx);
        assert!(
            window.find("settings-hidden-request-section").bounds().size.width > px(800.),
            "top-level Hide request headers section must span full content width so scope pill aligns flush right"
        );
        assert!(
            window.find("settings-hidden-response-section").bounds().size.width > px(800.),
            "top-level Hide response headers section must span full content width so scope pill aligns flush right"
        );
        assert_eq!(
            window.find("settings-hidden-request-remove").bounds().size,
            size(px(92.), px(24.))
        );
        assert_eq!(
            window.find("settings-hidden-request-clear").bounds().size,
            size(px(92.), px(24.))
        );
        assert_eq!(
            window.find("settings-hidden-request-list").bounds().size,
            size(px(452.), px(135.))
        );
        assert_eq!(
            window.find("settings-hidden-request-add").bounds().size,
            size(px(92.), px(24.))
        );
        assert!(window.find(("settings-hidden-request-row", 0usize)).visible());
        assert!(window.find(("settings-hidden-request-row", 4usize)).visible());
        assert!(window.find_by_label("accept").visible());
    })
    .unwrap();
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.user_pages
                .editor_scroll
                .set_offset(gpui_kit::point(px(0.), px(-140.)));
            cx.notify();
        });
        window.render_frame(cx);
        window.click("settings-hidden-request-input", cx);
        window.input("Bad Header", cx);
        window.click("settings-hidden-request-add", cx);
        window.render_frame(cx);
        assert!(window.find("settings-user-error").label().unwrap().contains("HTTP 标头 token"));

        window.click("settings-hidden-request-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("x-secret-trace", cx);
        window.click("settings-hidden-request-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op_add, saved_add) = take_workbench_prefs_save(&mut fixture);
    assert!(saved_add.message_editor.hidden_request_headers.as_ref().unwrap().contains(&"x-secret-trace".into()));
    acknowledge_user_prefs(&fixture, op_add, saved_add, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-hidden-request-default", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op_def, saved_def) = take_workbench_prefs_save(&mut fixture);
    assert!(saved_def.message_editor.hide_request_headers_by_default);
    acknowledge_user_prefs(&fixture, op_def, saved_def, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        let raw = b"POST / HTTP/1.1\r\nHost: example.com\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nConnection: keep-alive\r\nX-Secret-Trace: 42\r\n\r\nok";
        fixture.view.update(cx, |view, cx| {
            let tab_id = view.workspace.active.unwrap();
            let editors = view.tab_editors.get_mut(&tab_id).unwrap();
            editors.request.load(raw, window, cx);
            assert!(editors.request.hide_headers);
            let text = editors.request.state().read(cx).value().to_string();
            assert!(!text.contains("X-Secret-Trace"));
            assert!(text.contains("Content-Length: 2"));
            assert!(text.contains("Content-Type: text/plain"));
            assert!(text.contains("Connection: keep-alive"));
            assert_eq!(editors.request.bytes(window, cx).unwrap(), raw);
        });
    })
    .unwrap();

    for custom_header in ["content-length", "content-type", "connection"] {
        cx.update_window(settings, |_, window, cx| {
            window.click("settings-hidden-request-input", cx);
            window.input(custom_header, cx);
            window.click("settings-hidden-request-add", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let (op_custom, saved_custom) = take_workbench_prefs_save(&mut fixture);
        assert!(
            saved_custom
                .message_editor
                .hidden_request_headers
                .as_ref()
                .unwrap()
                .contains(&custom_header.into()),
            "global custom hidden_request_headers must persist {custom_header}"
        );
        acknowledge_user_prefs(&fixture, op_custom, saved_custom, cx);
    }

    cx.update_window(fixture.window, |_, window, cx| {
        let raw = b"POST / HTTP/1.1\r\nHost: example.com\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nConnection: keep-alive\r\nX-Secret-Trace: 42\r\n\r\nok";
        fixture.view.update(cx, |view, cx| {
            let tab_id = view.workspace.active.unwrap();
            let editors = view.tab_editors.get_mut(&tab_id).unwrap();
            let text = editors.request.state().read(cx).value().to_string();
            assert!(editors.request.hide_headers);
            assert!(!text.contains("X-Secret-Trace"));
            assert!(!text.contains("Content-Length:"));
            assert!(!text.contains("Content-Type:"));
            assert!(!text.contains("Connection:"));
            assert!(text.contains("Host: example.com"));
            assert_eq!(editors.request.bytes(window, cx).unwrap(), raw);
        });
    })
    .unwrap();

    scroll_editor_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-editor-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op_reset, saved_reset) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op_reset, saved_reset, cx);
    assert_eq!(theme::hidden_request_headers(), None);
    assert!(!theme::hide_headers_by_default(false));

    cx.update_window(fixture.window, |_, window, cx| {
        let raw = b"POST / HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nConnection: keep-alive\r\n\r\nok";
        fixture.view.update(cx, |view, cx| {
            let tab_id = view.workspace.active.unwrap();
            let editors = view.tab_editors.get_mut(&tab_id).unwrap();
            editors.request.toggle_hide_headers(window, cx).unwrap();
            editors.request.load(raw, window, cx);
            assert!(editors.request.hide_headers);
            let text = editors.request.state().read(cx).value().to_string();
            assert!(!text.contains("Accept: */*"));
            assert!(text.contains("Content-Length: 2"));
            assert!(text.contains("Content-Type: text/plain"));
            assert!(text.contains("Connection: keep-alive"));
            assert_eq!(editors.request.bytes(window, cx).unwrap(), raw);
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn user_settings_intruder_display_editor_charset_and_hotkeys_persistence(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("Intruder Display Editor Hotkeys 持久化")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    // 1. Intruder user preferences persist via SetUserWorkbenchPreferences.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-intruder", cx);
        window.render_frame(cx);
        window.click("intruder-side-panel-left", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert!(saved.misc.intruder_side_panel_left);
    acknowledge_user_prefs(&fixture, op, saved, cx);

    for id in [
        "intruder-side-panel-collapsed",
        "intruder-payload-append",
        "intruder-new-tab-last",
    ] {
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
        let (op, saved) = take_workbench_prefs_save(&mut fixture);
        acknowledge_user_prefs(&fixture, op, saved, cx);
    }
    cx.read(|cx| {
        let misc = &fixture.view.read(cx).user_preferences.misc;
        assert!(misc.intruder_side_panel_left);
        assert!(!misc.intruder_side_panel_auto_expand);
        assert!(misc.intruder_auto_payload_append);
        assert_eq!(misc.intruder_new_tab_config_mode, 2);
    });

    // 2. Display zebra stripe, issues grouped evidence, and use_system_dpi persist & wire to theme.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-display", cx);
        window.render_frame(cx);
        window.click("settings-user-table-no-zebra", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert!(!saved.display.table_zebra_stripe);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert!(!theme::table_zebra_stripe());

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-issues-ungrouped", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert!(!saved.display.issues_grouped_evidence);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert!(!theme::issues_grouped_evidence());

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-scale-configure", cx);
        window.render_frame(cx);
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| this.set_user_display_scale_percent(150, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert_eq!(theme::scale_factor_percent(), 150);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-user-scale-system-dpi", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert!(saved.display.use_system_dpi);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert!(theme::use_system_dpi());
    assert_eq!(theme::scale_factor_percent(), 100);

    // 3. Message editor charset, html_render_external_requests, search_regex/autoscroll persist & wire to MessageEditor.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-message-editor", cx);
        window.render_frame(cx);
        window.click("settings-user-editor-html-requests", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert!(!saved.message_editor.html_render_external_requests);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert!(!theme::html_render_external_requests());

    for id in [
        "settings-user-editor-search-regex",
        "settings-user-editor-search-autoscroll",
        "settings-user-editor-charset-specific",
    ] {
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
        let (op, saved) = take_workbench_prefs_save(&mut fixture);
        acknowledge_user_prefs(&fixture, op, saved, cx);
    }
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.set_user_editor_specific_charset("ISO-8859-1".into(), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(saved.message_editor.charset_mode, 3);
    assert_eq!(saved.message_editor.specific_charset, "ISO-8859-1");
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert!(theme::message_editor_search_regex_default());
    assert!(theme::message_editor_search_autoscroll_default());
    assert_eq!(theme::message_editor_charset(), (3, "ISO-8859-1".to_owned()));

    cx.update_window(fixture.window, |_, window, cx| {
        let latin1_raw = b"GET / HTTP/1.1\r\nHost: a.example\r\n\r\ncaf\xe9";
        fixture.view.update(cx, |view, cx| {
            let tab_id = view.workspace.active.unwrap();
            let editors = view.tab_editors.get_mut(&tab_id).unwrap();
            assert!(editors.request.regex);
            assert!(editors.request.auto_scroll_to_match);

            let mut fresh = crate::editing::MessageEditor::new(window, cx);
            assert!(fresh.regex);
            assert!(fresh.auto_scroll_to_match);
            fresh.load(latin1_raw, window, cx);
            assert_eq!(fresh.view, crate::editing::MessageView::Raw);
            assert!(fresh.state().read(cx).value().contains("café"));
            assert_eq!(fresh.bytes(window, cx).unwrap(), latin1_raw);
        });
    })
    .unwrap();

    // 4. Hotkeys interactive selection, inline rebinding, custom_hotkeys persistence & single-row reset.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-misc", cx);
        window.render_frame(cx);
        window.click(("settings-user-hotkey-row", 2usize), cx);
        window.render_frame(cx);
        window.click("settings-user-hotkey-edit", cx);
        window.render_frame(cx);
        assert!(window.find("settings-user-hotkey-capture-bar").visible());
        window.click("settings-user-hotkey-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("cmd-shift-r", cx);
        window.click("settings-user-hotkey-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert_eq!(
        saved.misc.custom_hotkeys.get("send_to_repeater").map(String::as_str),
        Some("cmd-shift-r")
    );
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert_eq!(
        theme::custom_hotkeys().get("send_to_repeater").map(String::as_str),
        Some("cmd-shift-r")
    );
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let row2 = window.find(("settings-user-hotkey-row", 2usize));
        assert!(row2.label().unwrap().contains("⌘⇧R"));
        window.click("settings-user-hotkey-reset-one", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert!(!saved.misc.custom_hotkeys.contains_key("send_to_repeater"));
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert!(!theme::custom_hotkeys().contains_key("send_to_repeater"));

    // Restore defaults so global thread_local theme state stays clean for other tests.
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.reset_user_display_defaults(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op, saved, cx);

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.reset_user_editor_defaults(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op, saved, cx);

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.reset_user_misc_defaults(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    acknowledge_user_prefs(&fixture, op, saved, cx);
    assert!(theme::table_zebra_stripe());
    assert!(!theme::message_editor_search_regex_default());
    assert!(!theme::message_editor_search_autoscroll_default());
    assert_eq!(theme::message_editor_charset(), (0, "UTF-8".to_owned()));
}

#[gpui_kit::test]
fn software_update_settings_check_install_and_top_badge_flow(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("更新设置测试")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    cx.update_window(settings, |_, window, cx| {
        window.click("settings-user-misc", cx);
        window.render_frame(cx);
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.user_pages.misc_scroll.scroll_to_bottom();
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.find("settings-user-updates-heading").visible());
        assert_eq!(
            window.find("settings-user-auto-check-updates").checked(),
            Some(true)
        );
        window.click("settings-user-auto-check-updates", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (op, saved) = take_workbench_prefs_save(&mut fixture);
    assert!(!saved.misc.auto_check_updates);
    acknowledge_user_prefs(&fixture, op, saved, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-user-auto-check-updates").checked(),
            Some(false)
        );
        window.click("settings-user-check-updates", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let check_cmds = drain(&mut fixture.commands);
    assert!(matches!(
        check_cmds.as_slice(),
        [Command::CheckForUpdates { manual: true }]
    ));

    let available = mio_core::UpdateInfo {
        current_version: "0.2.0".into(),
        latest_version: "0.3.0".into(),
        tag_name: "v0.3.0".into(),
        release_notes: "Release 0.3.0".into(),
        html_url: "https://github.com/ltlly/MIO/releases/tag/v0.3.0".into(),
        asset_name: "MIO-v0.3.0-macos-arm64.zip".into(),
        asset_url: "https://example.invalid/MIO-v0.3.0-macos-arm64.zip".into(),
        checksums_url: Some("https://example.invalid/SHA256SUMS.txt".into()),
        asset_sha256: Some("0".repeat(64)),
        asset_size: 42_000_000,
    };
    fixture
        .events
        .try_send(Event::UpdateStatus(mio_core::UpdateStatus::Available(
            available.clone(),
        )))
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("suiteUpdateBadge").visible());
    })
    .unwrap();

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.user_pages.misc_scroll.scroll_to_bottom();
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.find("settings-user-install-update").visible());
        window.click("settings-user-install-update", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let install_cmds = drain(&mut fixture.commands);
    assert_eq!(install_cmds.len(), 1);
    assert!(matches!(
        &install_cmds[0],
        Command::InstallUpdate { info } if info == &available
    ));
}
